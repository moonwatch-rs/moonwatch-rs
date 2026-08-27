use std::path::{Path, PathBuf};
use anyhow::{bail, Result};
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

/// The user's home directory.
pub fn home_dir() -> Result<PathBuf> {
    // Un-deprecated in Rust 1.87 together with a fix for the Windows behaviour that got it
    // deprecated in the first place, so this is again the right way to ask.
    match std::env::home_dir() {
        Some(home) if !home.as_os_str().is_empty() => Ok(home),
        _ => bail!("could not determine your home directory"),
    }
}

/// The directory Moonwatch.rs installs itself into, `~/.moonwatch-rs`.
///
/// This is what `moonwatch_rs install` writes to and what the autostart entry it creates
/// points at; nothing else in the program depends on it, as an installation is located by
/// the path of the executable instead (see `default_config_path`).
pub fn moonwatch_dir_in_home() -> Result<PathBuf> {
    Ok(home_dir()?.join(".moonwatch-rs"))
}
