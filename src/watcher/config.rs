use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use regex::Regex;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use crate::watcher::core::ActiveWindowEventV1;

#[derive(Debug)]
pub struct WindowEventMatcher {
    pub window_title_regex: Option<Regex>,
    pub process_path_regex: Option<Regex>
}

#[derive(Debug)]
pub struct ConfigTag {
    pub tag: String,
    pub matcher: WindowEventMatcher
}

impl WindowEventMatcher {
    pub fn matches(&self, e: &ActiveWindowEventV1) -> bool {
        if let Some(tmp) = &self.window_title_regex {
            if !tmp.is_match(e.window_title.as_str()) {
                return false;
            }
        }

        if let Some(tmp) = &self.process_path_regex {
            // An event whose path is unknown (or not representable as UTF-8) is tested
            // against the empty string, so it does not match. Skipping the test instead
            // would make such an event match every process_path rule - an "ignore" rule
            // would then quietly discard it.
            let path = e.process_path.as_deref().and_then(|path| path.to_str()).unwrap_or("");
            if !tmp.is_match(path) {
                return false;
            }
        }

        true
    }

    pub fn from_json_single(val: &Value) -> Result<WindowEventMatcher> {
        if !val.is_object() {
            bail!("WindowEventMatcher definition should be an object, not {:?}", val);
        }

        let window_title = &val["window_title"];
        let process_path = &val["process_path"];

        let window_title_regex = if let Some(tmp) = window_title.as_str() {
            Some(Regex::new(tmp)?)
        } else { None };

        let process_path_regex = if let Some(tmp) = process_path.as_str() {
            Some(Regex::new(tmp)?)
        } else { None };

        if ![&window_title_regex, &process_path_regex].iter().any(|tmp| tmp.is_some()) {
            bail!("WindowEventMatcher must define at least one of window_title, process_path");
        }

        Ok(WindowEventMatcher {
            window_title_regex: window_title_regex,
            process_path_regex: process_path_regex,
        })
    }

    pub fn from_json(val: &Value) -> Result<Vec<WindowEventMatcher>> {
        if let Some(arr) = val.as_array() {
            let mut matchers = Vec::<WindowEventMatcher>::new();

            for v in arr {
                matchers.push(WindowEventMatcher::from_json_single(v)?);
            }

            Ok(matchers)
        } else if val.is_object() {
            Ok(vec![WindowEventMatcher::from_json_single(val)?])
        } else if val.is_null() {
            Ok(vec![])
        } else {
            bail!("WindowEventMatcher(s) JSON definition must be object, array, or null")
        }
    }
}

#[derive(Debug)]
pub struct Config {
    pub output_dir: PathBuf,
    pub sample_every: Duration,
    pub write_every: Duration,
    pub tags: Vec<ConfigTag>,
    pub ignore: Vec<WindowEventMatcher>,
    pub anonymize: Vec<WindowEventMatcher>,
}

#[derive(Debug)]
pub struct BaseConfig {
    pub tags: Vec<ConfigTag>,
    pub ignore: Vec<WindowEventMatcher>,
    pub anonymize: Vec<WindowEventMatcher>,
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Config> {
        let data = fs::read_to_string(path)?;
        let d = serde_json::from_str::<Value>(&data)?;

        let mut tags = Config::read_tags(&d["tags"])?;
        let mut ignore = WindowEventMatcher::from_json(&d["ignore"])?;
        let mut anonymize = WindowEventMatcher::from_json(&d["anonymize"])?;

        // import tags, ignore, anonymize from base config
        if let Some(path_to_base_config_str) = d["main"]["path_to_base_config"].as_str() {
            let relative_path_to_base_config = PathBuf::from(path_to_base_config_str);
            let path_to_base_config = path.parent().unwrap().join(relative_path_to_base_config);
            match BaseConfig::from_file(path_to_base_config.as_path()) {
                Ok(base_config) => {
                    tags.extend(base_config.tags);
                    ignore.extend(base_config.ignore);
                    anonymize.extend(base_config.anonymize);
                }
                Err(e) => {
                    eprintln!("failed to read base_config {:?}: {:?}", path_to_base_config, e);
                }
            }
        }

        let relative_output_dir = PathBuf::from(d["main"]["output_dir"].as_str().ok_or(anyhow!("cannot read output_dir"))?);
        let output_dir = path.parent().unwrap().join(relative_output_dir);
        let sample_every = Duration::from_secs_f32(d["main"]["sample_every_sec"].as_f64().ok_or(anyhow!("cannot read sample_every_sec"))? as f32);
        let write_every = Duration::from_secs_f32(d["main"]["write_every_sec"].as_f64().ok_or(anyhow!("cannot read write_every_sec"))? as f32);

        Ok(Config {
            output_dir,
            sample_every,
            write_every,
            tags,
            ignore,
            anonymize,
        })
    }

    pub fn read_tags(obj: &Value) -> Result<Vec<ConfigTag>> {
        if obj.is_null() {
            return Ok(vec![]);
        }

        let Some(entries) = obj.as_object() else {
            bail!("JSON value of 'tags' key must be JSON object or null");
        };

        let mut tags = Vec::<ConfigTag>::new();

        for (key, val) in entries {
            let matchers = WindowEventMatcher::from_json(val)?;

            for matcher in matchers {
                tags.push( ConfigTag {
                    tag: key.to_string(),
                    matcher
                });
            }
        }

        Ok(tags)
    }
}

impl BaseConfig {
    pub fn from_file(path: &Path) -> Result<BaseConfig> {
        let data = fs::read_to_string(path)?;
        let d = serde_json::from_str::<Value>(&data)?;

        let tags = Config::read_tags(&d["tags"])?;
        let ignore = WindowEventMatcher::from_json(&d["ignore"])?;
        let anonymize = WindowEventMatcher::from_json(&d["anonymize"])?;

        Ok(BaseConfig { tags, ignore, anonymize })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(process_path: Option<&str>) -> ActiveWindowEventV1 {
        ActiveWindowEventV1::new(
            Duration::from_secs(0),
            "Some Window".to_string(),
            process_path.map(PathBuf::from),
            Duration::from_secs(15),
        )
    }

    fn matcher(json: &str) -> WindowEventMatcher {
        WindowEventMatcher::from_json_single(&serde_json::from_str(json).expect("valid JSON"))
            .expect("valid matcher")
    }

    #[test]
    fn process_path_matcher_matches_on_the_path() {
        let firefox = matcher(r#"{"process_path": "firefox(\\.exe)?$"}"#);

        assert!(firefox.matches(&event(Some("/usr/bin/firefox"))));
        assert!(!firefox.matches(&event(Some("/usr/bin/chrome"))));
    }

    /// An event whose path could not be determined (an elevated process, say) is tested
    /// against the empty string, so a rule naming a program does not match it. Skipping the
    /// test instead would make it match *every* process_path rule, and a single "ignore"
    /// entry would then quietly discard all such events.
    ///
    /// A pattern that matches the empty string still matches, which is the right reading of
    /// a deliberate catch-all like `".*"`.
    #[test]
    fn process_path_matcher_does_not_match_an_unknown_path() {
        assert!(!matcher(r#"{"process_path": "firefox"}"#).matches(&event(None)));
        assert!(!matcher(r#"{"process_path": "^/usr/bin/"}"#).matches(&event(None)));
        assert!(matcher(r#"{"process_path": ".*"}"#).matches(&event(None)),
                "a catch-all is meant to catch everything");
    }

    /// AND semantics across the predicates of one matcher are unaffected by the path being
    /// unknown: the title still has to match as well.
    #[test]
    fn a_title_only_matcher_still_works_without_a_path() {
        let by_title = matcher(r#"{"window_title": "^Some"}"#);

        assert!(by_title.matches(&event(None)));
        assert!(!matcher(r#"{"window_title": "^Other"}"#).matches(&event(None)));
    }
}
