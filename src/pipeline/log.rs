//! Reading of Moonwatch logs (JSONL files written by the recorder) into Polars dataframes.
//!
//! A log file is a stream of newline-delimited `Event` objects (see `crate::core::model::event`).
//! Since the ETL pipeline works on dataframes rather than on individual events, the files are read
//! with the Polars NDJSON reader and converted per event type into dataframes with a fixed schema.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use glob::glob;
use polars::prelude::*;
use indicatif::ProgressIterator;

/// Convert a Polars error into an `anyhow::Error`.
///
/// This is needed to disambiguate `context()`/`with_context()`, since `polars::prelude`
/// brings its own `PolarsContext` trait with the same methods into scope.
fn polars_err(e: PolarsError) -> anyhow::Error {
    anyhow::Error::new(e)
}

/// Value of the `type` attribute of the legacy `ActiveWindowEvent` log line.
const ACTIVE_WINDOW_EVENT: &str = "ActiveWindowEvent";

/// Value of the `type` attribute of the `ActiveWindowEventV1` log line.
const ACTIVE_WINDOW_EVENT_V1: &str = "ActiveWindowEventV1";

/// Value of the `type` attribute of the `ActiveActivityEventV1` log line.
const ACTIVE_ACTIVITY_EVENT_V1: &str = "ActiveActivityEventV1";

/// Value of the `type` attribute of the `DeviceUnlockEventV1` log line.
const DEVICE_UNLOCK_EVENT_V1: &str = "DeviceUnlockEventV1";

/// Event types that this module knows how to convert; anything else is ignored.
const KNOWN_EVENT_TYPES: [&str; 4] = [
    ACTIVE_WINDOW_EVENT,
    ACTIVE_WINDOW_EVENT_V1,
    ACTIVE_ACTIVITY_EVENT_V1,
    DEVICE_UNLOCK_EVENT_V1,
];

/// Polars datatype used for `time` columns (naive UTC).
pub fn time_dtype() -> DataType {
    DataType::Datetime(TimeUnit::Microseconds, None)
}

/// Polars datatype used for `duration` and `idleFor` columns.
///
/// Note that the time unit must stay in sync with `crate::pipeline::transform`,
/// which compares `idleFor` against a literal cast to this datatype.
pub fn duration_dtype() -> DataType {
    DataType::Duration(TimeUnit::Milliseconds)
}

/// Polars datatype used for the `tags` column.
pub fn tags_dtype() -> DataType {
    DataType::List(Box::new(DataType::String))
}

/// Schema of the dataframe returned by `MoonwatchLogParser::active_window_event_df`.
pub fn active_window_event_schema() -> Schema {
    Schema::from_iter([
        Field::new("time".into(), time_dtype()),
        Field::new("duration".into(), duration_dtype()),
        Field::new("hostname".into(), DataType::String),
        Field::new("username".into(), DataType::String),
        Field::new("idleFor".into(), duration_dtype()),
        Field::new("processPath".into(), DataType::String),
        Field::new("processName".into(), DataType::String),
        Field::new("tags".into(), tags_dtype()),
    ])
}

/// Schema of the dataframe returned by `MoonwatchLogParser::active_activity_event_df`.
pub fn active_activity_event_schema() -> Schema {
    Schema::from_iter([
        Field::new("time".into(), time_dtype()),
        Field::new("duration".into(), duration_dtype()),
        Field::new("hostname".into(), DataType::String),
        Field::new("applicationLabel".into(), DataType::String),
        Field::new("applicationId".into(), DataType::String),
    ])
}

/// Schema of the dataframe returned by `MoonwatchLogParser::device_unlock_event_df`.
pub fn device_unlock_event_schema() -> Schema {
    Schema::from_iter([
        Field::new("time".into(), time_dtype()),
        Field::new("hostname".into(), DataType::String),
    ])
}

/// Schema of the dataframe returned by `MoonwatchLogParser::unified_active_event_df`.
///
/// This is the dataframe equivalent of `crate::pipeline::model::event::ActiveEvent`
/// and the main input of the ETL pipeline.
pub fn unified_active_event_schema() -> Schema {
    Schema::from_iter([
        Field::new("time".into(), time_dtype()),
        Field::new("duration".into(), duration_dtype()),
        Field::new("hostname".into(), DataType::String),
        Field::new("username".into(), DataType::String),
        Field::new("idleFor".into(), duration_dtype()),
        Field::new("name".into(), DataType::String),
        Field::new("category".into(), DataType::String),
        Field::new("processPath".into(), DataType::String),
        Field::new("processName".into(), DataType::String),
        Field::new("applicationLabel".into(), DataType::String),
        Field::new("applicationId".into(), DataType::String),
        Field::new("ignore".into(), DataType::Boolean),
        Field::new("isMobile".into(), DataType::Boolean),
        Field::new("tags".into(), tags_dtype()),
    ])
}

/// A single Moonwatch log file (`.jsonl` or `.jsonl.gz`).
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct MoonwatchLog {
    pub path: PathBuf,
}

impl MoonwatchLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        MoonwatchLog { path: path.into() }
    }

    /// Return Moonwatch logs recursively found in given directory, sorted by filename.
    ///
    /// The recorder names log files after a UUIDv7, so sorting by filename also sorts
    /// them by the time they were written.
    pub fn find_in_directory(directory: &Path) -> Result<Vec<MoonwatchLog>> {
        let pattern = directory.join("**").join("*.jsonl*");

        let mut logs = Vec::new();
        for entry in glob(pattern.to_str().unwrap()).expect("Failed to read glob pattern") {
            let path = entry
                .with_context(|| format!("Failed to read directory {directory:?}"))?;
            if path.is_file() && is_moonwatch_log_filename(&path) {
                logs.push(MoonwatchLog::new(path));
            }
        }
        logs.sort();
        Ok(logs)
    }

    /// Read the whole log file into memory, decompressing it if needed.
    fn read_bytes(&self) -> Result<Vec<u8>> {
        let name = filename(&self.path);
        let gzipped = if name.ends_with(".jsonl.gz") {
            true
        } else if name.ends_with(".jsonl") {
            false
        } else {
            bail!("Unsupported file extension: {:?}", self.path);
        };

        let file = File::open(&self.path)
            .with_context(|| format!("Failed to open {:?}", self.path))?;
        let mut buffer = Vec::new();

        if gzipped {
            GzDecoder::new(file).read_to_end(&mut buffer)
        } else {
            (&file).read_to_end(&mut buffer)
        }
        .with_context(|| format!("Failed to read {:?}", self.path))?;

        Ok(buffer)
    }

    /// Return the log file as a dataframe of raw log lines, with `data` still nested.
    ///
    /// The schema depends on which event types the file happens to contain; use
    /// `MoonwatchLogParser` to get dataframes with a stable schema.
    pub fn read_raw_df(&self) -> Result<DataFrame> {
        let bytes = self.read_bytes()?;

        // The NDJSON reader cannot infer a schema from an empty file.
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            log::warn!("Moonwatch log is empty: {:?}", self.path);
            return Ok(DataFrame::empty());
        }

        let df = JsonReader::new(Cursor::new(bytes))
            .with_json_format(JsonFormat::JsonLines)
            .infer_schema_len(None)
            .finish()
            .map_err(polars_err)
            .with_context(|| format!("Failed to parse {:?}", self.path))?;

        warn_about_unknown_event_types(&self.path, &df);
        Ok(df)
    }
}

impl PartialOrd for MoonwatchLog {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MoonwatchLog {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path.cmp(&other.path)
    }
}

/// Parses a set of Moonwatch logs into Polars dataframes.
///
/// All files are read up front by `MoonwatchLogParser::read`; the accessors then convert
/// the raw dataframes without touching the filesystem again.
pub struct MoonwatchLogParser {
    raw: Vec<(PathBuf, DataFrame)>,
}

/// Converts raw log lines of a single event type into a dataframe chunk.
type EventConverter = fn(DataFrame) -> PolarsResult<DataFrame>;

impl MoonwatchLogParser {
    pub fn read(logs: &[MoonwatchLog]) -> Result<Self> {
        let mut raw = Vec::with_capacity(logs.len());
        for log in logs.iter().progress() {
            log::debug!("Reading Moonwatch log {:?}", log.path);
            raw.push((log.path.clone(), log.read_raw_df()?));
        }
        log::info!("Read {} Moonwatch log file(s)", raw.len());
        Ok(MoonwatchLogParser { raw })
    }

    /// Return dataframe of `ActiveWindowEventV1` events (including migrated legacy events).
    pub fn active_window_event_df(&self) -> Result<DataFrame> {
        self.collect_event_df(
            &active_window_event_schema(),
            &[
                (ACTIVE_WINDOW_EVENT_V1, convert_active_window_event_v1),
                (ACTIVE_WINDOW_EVENT, convert_active_window_event),
            ],
        )
    }

    /// Return dataframe of `ActiveActivityEventV1` events.
    pub fn active_activity_event_df(&self) -> Result<DataFrame> {
        self.collect_event_df(
            &active_activity_event_schema(),
            &[(ACTIVE_ACTIVITY_EVENT_V1, convert_active_activity_event_v1)],
        )
    }

    /// Return dataframe of `DeviceUnlockEventV1` events.
    pub fn device_unlock_event_df(&self) -> Result<DataFrame> {
        self.collect_event_df(
            &device_unlock_event_schema(),
            &[(DEVICE_UNLOCK_EVENT_V1, convert_device_unlock_event_v1)],
        )
    }

    /// Return dataframe unifying desktop and mobile active events, ie. the pipeline input.
    ///
    /// Attributes that only exist on one of the two platforms are null for the other one.
    pub fn unified_active_event_df(&self) -> Result<DataFrame> {
        let schema = unified_active_event_schema();

        let desktop = self.active_window_event_df()?.lazy().select([
            col("time"),
            col("duration"),
            col("hostname"),
            col("username"),
            col("idleFor"),
            col("processName").alias("name"),
            lit(NULL).cast(DataType::String).alias("category"),
            col("processPath"),
            col("processName"),
            lit(NULL).cast(DataType::String).alias("applicationLabel"),
            lit(NULL).cast(DataType::String).alias("applicationId"),
            lit(false).alias("ignore"),
            lit(false).alias("isMobile"),
            col("tags"),
        ]);

        let mobile = self.active_activity_event_df()?.lazy().select([
            col("time"),
            col("duration"),
            col("hostname"),
            lit(NULL).cast(DataType::String).alias("username"),
            lit(NULL).cast(duration_dtype()).alias("idleFor"),
            col("applicationLabel").str().to_lowercase().alias("name"),
            lit(NULL).cast(DataType::String).alias("category"),
            lit(NULL).cast(DataType::String).alias("processPath"),
            lit(NULL).cast(DataType::String).alias("processName"),
            col("applicationLabel"),
            col("applicationId"),
            lit(false).alias("ignore"),
            lit(true).alias("isMobile"),
            empty_tags(),
        ]);

        let df = concat(
            [desktop, mobile],
            UnionArgs {
                rechunk: true,
                ..Default::default()
            },
        )
        .and_then(|lf| lf.collect())
        .map_err(polars_err)
        .context("Failed to concatenate desktop and mobile active events")?;

        cast_to_schema(df, &schema)
    }

    /// Convert every raw dataframe with every given converter and stack the results.
    ///
    /// Event types that a given log file does not contain are skipped, so that files
    /// written by different Moonwatch versions can be read together.
    fn collect_event_df(
        &self,
        schema: &Schema,
        converters: &[(&str, EventConverter)],
    ) -> Result<DataFrame> {
        let mut chunks: Vec<LazyFrame> = Vec::new();

        for (path, raw_df) in &self.raw {
            for (event_type, convert) in converters {
                let Some(df) = filter_by_event_type(raw_df, event_type)
                    .map_err(polars_err)
                    .with_context(|| format!("Failed to read {event_type} events from {path:?}"))?
                else {
                    continue;
                };
                let chunk = convert(df)
                    .map_err(polars_err)
                    .with_context(|| format!("Failed to read {event_type} events from {path:?}"))?;
                chunks.push(chunk.lazy());
            }
        }

        if chunks.is_empty() {
            return Ok(DataFrame::empty_with_schema(schema));
        }

        let df = concat(
            chunks,
            UnionArgs {
                rechunk: true,
                ..Default::default()
            },
        )
        .and_then(|lf| lf.collect())
        .map_err(polars_err)
        .context("Failed to concatenate log file chunks")?;

        cast_to_schema(df, schema)
    }
}

/// Warn about event types that this module does not know how to convert.
///
/// Such events are silently dropped from every dataframe, which would otherwise be
/// an easy way to lose data when reading logs written by a newer Moonwatch version.
fn warn_about_unknown_event_types(path: &Path, df: &DataFrame) {
    let Ok(column) = df.column("type") else {
        return;
    };
    let Ok(unique) = column.as_materialized_series().unique() else {
        return;
    };
    let Ok(unique) = unique.str() else {
        return;
    };

    for event_type in unique.iter().flatten() {
        if !KNOWN_EVENT_TYPES.contains(&event_type) {
            log::warn!("Ignoring events of unknown type {event_type:?} in {path:?}");
        }
    }
}

/// Return rows of given event type, or `None` if the dataframe contains none.
fn filter_by_event_type(df: &DataFrame, event_type: &str) -> PolarsResult<Option<DataFrame>> {
    if !df.schema().contains("type") {
        return Ok(None);
    }

    let out = df
        .clone()
        .lazy()
        .filter(col("type").eq(lit(event_type)))
        .collect()?;

    Ok(if out.height() == 0 { None } else { Some(out) })
}

/// Cast the dataframe to given schema and return its columns in schema order.
fn cast_to_schema(df: DataFrame, schema: &Schema) -> Result<DataFrame> {
    let exprs = schema
        .iter()
        .map(|(name, dtype)| col(name.clone()).cast(dtype.clone()))
        .collect::<Vec<_>>();

    df.lazy()
        .select(exprs)
        .collect()
        .map_err(polars_err)
        .context("Failed to cast dataframe to target schema")
}

/// Return Polars expression parsing the RFC 3339 `time` attribute into a naive UTC datetime.
fn parse_time() -> Expr {
    col("time")
        .cast(DataType::String)
        .str()
        .to_datetime(
            Some(TimeUnit::Microseconds),
            Some(TimeZone::UTC),
            StrptimeOptions::default(),
            lit("raise"),
        )
        .dt()
        .replace_time_zone(None, lit("raise"), NonExistent::Raise)
        .alias("time")
}

/// Return Polars expression converting a number of seconds into a duration.
///
/// The column is read as `Float64` because the legacy watcher writes whole seconds
/// as floats, whereas the recorder writes them as integers.
fn parse_duration_seconds(name: &str) -> Expr {
    (col(name).cast(DataType::Float64) * lit(1000.0))
        .cast(DataType::Int64)
        .cast(duration_dtype())
        .alias(name)
}

/// Return Polars expression deriving `processName` from given process path column.
///
/// This takes the last path segment (for both Unix and Windows separators, since logs may be
/// processed on a different platform than the one that wrote them), strips the executable
/// file extension and converts the result to lowercase.
fn process_path_to_name(name: &str) -> Expr {
    col(name)
        .cast(DataType::String)
        .str()
        .extract(lit(r"([^\\/]+)$"), 1)
        .str()
        .replace(lit(r"(?i)\.(exe|bat|ps1|com|sh|bin)$"), lit(""), false)
        .str()
        .to_lowercase()
        .alias("processName")
}

/// Return Polars expression for an empty `tags` list.
fn empty_tags() -> Expr {
    let empty = Series::new_empty(PlSmallStr::EMPTY, &DataType::String);
    let list = empty
        .implode()
        .expect("imploding an empty series cannot fail")
        .into_series();
    // `first()` turns the one-element literal into a scalar, which Polars broadcasts
    lit(list).first().alias("tags")
}

/// Convert `ActiveWindowEventV1` log lines.
fn convert_active_window_event_v1(df: DataFrame) -> PolarsResult<DataFrame> {
    // Project before unnesting: a file that also contains legacy `ActiveWindowEvent` lines has
    // top-level `duration`/`hostname`/... columns that would collide with the nested ones.
    df.select(["time", "data"])?
        .unnest(["data"], None)?
        .lazy()
        .select([
            parse_time(),
            parse_duration_seconds("duration"),
            col("hostname").cast(DataType::String),
            col("username").cast(DataType::String),
            parse_duration_seconds("idleFor"),
            col("processPath").cast(DataType::String),
            process_path_to_name("processPath"),
            col("tags").cast(tags_dtype()),
        ])
        .collect()
}

/// Convert legacy `ActiveWindowEvent` log lines, which are flat and use snake_case.
fn convert_active_window_event(df: DataFrame) -> PolarsResult<DataFrame> {
    df.lazy()
        .select([
            parse_time(),
            parse_duration_seconds("duration"),
            col("hostname").cast(DataType::String),
            col("username").cast(DataType::String),
            parse_duration_seconds("idle_for").alias("idleFor"),
            col("process_path").cast(DataType::String).alias("processPath"),
            process_path_to_name("process_path"),
            col("tags").cast(tags_dtype()),
        ])
        .collect()
}

/// Convert `ActiveActivityEventV1` log lines.
fn convert_active_activity_event_v1(df: DataFrame) -> PolarsResult<DataFrame> {
    df.select(["time", "data"])?
        .unnest(["data"], None)?
        .lazy()
        .select([
            parse_time(),
            parse_duration_seconds("duration"),
            col("hostname").cast(DataType::String),
            col("applicationLabel").cast(DataType::String),
            col("applicationId").cast(DataType::String),
        ])
        .collect()
}

/// Convert `DeviceUnlockEventV1` log lines.
fn convert_device_unlock_event_v1(df: DataFrame) -> PolarsResult<DataFrame> {
    df.select(["time", "data"])?
        .unnest(["data"], None)?
        .lazy()
        .select([parse_time(), col("hostname").cast(DataType::String)])
        .collect()
}

/// Return the file name of given path, or an empty string if it has none.
fn filename(path: &Path) -> &str {
    path.file_name().and_then(|s| s.to_str()).unwrap_or_default()
}

/// Return whether given path looks like a Moonwatch log file.
fn is_moonwatch_log_filename(path: &Path) -> bool {
    let name = filename(path);
    name.ends_with(".jsonl") || name.ends_with(".jsonl.gz")
}
