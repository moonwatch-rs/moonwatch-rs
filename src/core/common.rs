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
