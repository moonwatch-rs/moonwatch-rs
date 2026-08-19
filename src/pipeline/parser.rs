use std::path::PathBuf;
use anyhow::{Result};
use polars::datatypes::{DataType, Field};
use polars::prelude::*;

fn polars_err(e: PolarsError) -> anyhow::Error {
    anyhow::Error::new(e)
}

const ACTIVE_WINDOW_EVENT: &str = "ActiveWindowEvent";
const ACTIVE_WINDOW_EVENT_V1: &str = "ActiveWindowEventV1";
const ACTIVE_ACTIVITY_EVENT_V1: &str = "ActiveActivityEventV1";
const DEVICE_UNLOCK_EVENT_V1: &str = "DeviceUnlockEventV1";

fn input_super_schema() -> Schema {
    Schema::from_iter([
        Field::new(
            "type".into(),
            DataType::from_frozen_categories(
                FrozenCategories::new([
                    ACTIVE_WINDOW_EVENT,
                    ACTIVE_WINDOW_EVENT_V1,
                    ACTIVE_ACTIVITY_EVENT_V1,
                    DEVICE_UNLOCK_EVENT_V1,
                ]).unwrap()
            )
        ),
        Field::new("time".into(), DataType::Datetime(TimeUnit::Microseconds, None)),
        Field::new("duration".into(), DataType::Float64),
        Field::new("hostname".into(), DataType::String),
        Field::new("username".into(), DataType::String),
        Field::new("idle_for".into(), DataType::Float64),
        Field::new("process_path".into(), DataType::String),
        Field::new("tags".into(), DataType::List(Box::new(DataType::String))),
        Field::new(
            "data".into(),
            DataType::Struct(vec![
                Field::new("duration".into(), DataType::Float64),
                Field::new("hostname".into(), DataType::String),
                Field::new("applicationLabel".into(), DataType::String),
                Field::new("applicationId".into(), DataType::String),
                Field::new("username".into(), DataType::String),
                Field::new("idleFor".into(), DataType::Float64),
                Field::new("processPath".into(), DataType::String),
                Field::new("tags".into(), DataType::List(Box::new(DataType::String))),
            ]),
        ),
    ])
}

fn duration_dtype() -> DataType {
    DataType::Duration(TimeUnit::Milliseconds)
}

fn seconds_to_duration(seconds: Expr) -> Expr {
    (seconds * lit(1000.0))
        .cast(DataType::Int64)
        .cast(duration_dtype())
}

pub fn output_active_event_schema() -> Schema {
    Schema::from_iter([
        Field::new("time".into(), DataType::Datetime(TimeUnit::Microseconds, None)),
        Field::new("duration".into(), duration_dtype()),
        Field::new("hostname".into(), DataType::String),
        Field::new("username".into(), DataType::String),
        Field::new("idleFor".into(), duration_dtype()),
        Field::new("processName".into(), DataType::String),
        Field::new("processPath".into(), DataType::String),
        Field::new("applicationLabel".into(), DataType::String),
        Field::new("applicationId".into(), DataType::String),
        Field::new("name".into(), DataType::String),
        Field::new("category".into(), DataType::String),
        Field::new("ignore".into(), DataType::Boolean),
        Field::new("isMobile".into(), DataType::Boolean),
        Field::new("tags".into(), DataType::List(Box::new(DataType::String))),
    ])
}

pub fn output_unlock_event_schema() -> Schema {
    Schema::from_iter([
        Field::new("time".into(), DataType::Datetime(TimeUnit::Microseconds, None)),
        Field::new("hostname".into(), DataType::String),
    ])
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct MoonwatchLogParser {
    pub input_paths: Vec<PathBuf>,
}

impl MoonwatchLogParser {
    pub fn new(paths: Vec<impl Into<PathBuf>>) -> Self {
        MoonwatchLogParser {
            input_paths: paths.into_iter().map(Into::into).collect(),
        }
    }

    pub fn get_input_lazy_df(&self) -> Result<LazyFrame> {
        let paths = self
            .input_paths
            .iter()
            .map(|path| PlRefPath::try_from_path(path))
            .collect::<PolarsResult<Vec<_>>>()
            .map_err(polars_err)?;

        LazyJsonLineReader::new_paths(paths.into_iter().collect())
            .with_schema(Some(Arc::new(input_super_schema())))
            .with_ignore_errors(true)
            .low_memory(true)
            .finish()
            .map_err(polars_err)
    }

    pub fn extract_active_event_df(lf: LazyFrame) -> LazyFrame {
        let schema = output_active_event_schema();
        let active_event_types = Series::new(
            "type".into(),
            [Series::new(
                "".into(),
                [
                    ACTIVE_WINDOW_EVENT,
                    ACTIVE_WINDOW_EVENT_V1,
                    ACTIVE_ACTIVITY_EVENT_V1,
                ],
            )],
        );
        let no_tags = Series::new("tags".into(), [Series::new("".into(), Vec::<&str>::new())]);

        lf.filter(col("type").is_in(lit(active_event_types), false))
            .select([
                col("time"),
                seconds_to_duration(coalesce(&[
                    col("data").struct_().field_by_name("duration"),
                    col("duration"),
                ]))
                    .alias("duration"),
                coalesce(&[
                    col("data").struct_().field_by_name("hostname"),
                    col("hostname"),
                ])
                    .alias("hostname"),
                coalesce(&[
                    col("data").struct_().field_by_name("username"),
                    col("username"),
                ])
                    .alias("username"),
                seconds_to_duration(coalesce(&[
                    col("data").struct_().field_by_name("idleFor"),
                    col("idle_for"),
                ]))
                    .alias("idleFor"),
                lit(NULL).cast(DataType::String).alias("category"),
                coalesce(&[
                    col("data").struct_().field_by_name("processPath"),
                    col("process_path"),
                ])
                    .alias("processPath"),
                col("data")
                    .struct_()
                    .field_by_name("applicationLabel")
                    .alias("applicationLabel"),
                col("data")
                    .struct_()
                    .field_by_name("applicationId")
                    .alias("applicationId"),
                lit(false).alias("ignore"),
                col("type")
                    .eq(lit(ACTIVE_ACTIVITY_EVENT_V1))
                    .alias("isMobile"),
                coalesce(&[
                    col("data").struct_().field_by_name("tags"),
                    col("tags"),
                    lit(no_tags),
                ])
                .alias("tags"),
            ])
            .with_columns([col("processPath")
                .str()
                .extract(lit(r"([^\\/]+)$"), 1)
                .str()
                .replace(lit(r"(?i)\.(exe|bat|ps1|com|sh|bin)$"), lit(""), false)
                .alias("processName")])
            .with_columns([
                coalesce(&[col("processName"), col("applicationLabel")])
                    .str()
                    .to_lowercase()
                    .alias("name")])
            .cast(
                schema
                    .iter()
                    .map(|(name, dtype)| (name.as_str(), dtype.clone()))
                    .collect(),
                true,
            )
            .select(
                schema
                    .iter_names()
                    .map(|name| col(name.clone()))
                    .collect::<Vec<_>>(),
            )
    }

    pub fn extract_unlock_event_df(lf: LazyFrame) -> LazyFrame {
        let schema = output_unlock_event_schema();

        lf.filter(col("type").eq(lit(DEVICE_UNLOCK_EVENT_V1)))
            .select([
                col("time"),
                col("data").struct_().field_by_name("hostname")
                    .alias("hostname")
                ])
            .cast(
                schema
                    .iter()
                    .map(|(name, dtype)| (name.as_str(), dtype.clone()))
                    .collect(),
                true,
            )
            .select(
                schema
                    .iter_names()
                    .map(|name| col(name.clone()))
                    .collect::<Vec<_>>(),
            )
    }
}
