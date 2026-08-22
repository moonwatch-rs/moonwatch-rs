use std::path::{Path, PathBuf};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serializer};

pub fn deserialize_regex<'de, D>(de: D) -> Result<Regex, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(de)?;
    Regex::new(&s).map_err(serde::de::Error::custom)
}

pub fn serialize_regex<S>(re: &Regex, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(re.as_str())
}

/// Directory holding a Moonwatch configuration file, and therefore also the paths that
/// file resolves relatively.
///
/// A bare file name (as passed by a launcher that relies on the working directory) has
/// `Some("")` as its parent, which would join into a relative path with a leading
/// separator, so that case falls back to `.`.
pub fn config_dir(config_path: &Path) -> PathBuf {
    config_path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}
