//! Minimal [superjson](https://github.com/blitz-js/superjson) envelope support.
//!
//! Screen Studio persists every JSON document as
//! `{ "json": <plain json>, "meta": { "values": { "<dot.path>": ["Date"] }, "v": 1 } }`.
//! `meta` is omitted entirely when no value needs a type annotation
//! (e.g. `recording-markers.json` is just `{ "json": [] }`).
//!
//! Only the `Date` annotation is used by Screen Studio; `undefined`, `bigint`, `Map`, `Set`
//! and `regexp` annotations are decoded leniently (kept as plain JSON) and never emitted.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

use crate::{Error, Result};

/// Raw envelope as stored on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    pub json: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    /// dot-separated path → list of type annotations (only `"Date"` is used by Screen Studio)
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub values: Map<String, Value>,
    #[serde(default = "one")]
    pub v: u32,
}

fn one() -> u32 {
    1
}

/// Decode an envelope string into `T`. Date annotations are left as ISO-8601 strings, which
/// is what `chrono::DateTime<Utc>` deserializes from, so no transformation is needed.
pub fn from_str<T: DeserializeOwned>(s: &str) -> std::result::Result<T, serde_json::Error> {
    let env: Envelope = serde_json::from_str(s)?;
    serde_json::from_value(env.json)
}

/// Decode an envelope into a raw [`Value`] (the `json` member), also returning the meta.
pub fn to_value_with_meta(s: &str) -> std::result::Result<(Value, Option<Meta>), serde_json::Error> {
    let env: Envelope = serde_json::from_str(s)?;
    Ok((env.json, env.meta))
}

/// Encode `value` as a superjson envelope, annotating every field named in `date_paths`
/// (dot-separated paths relative to the root, e.g. `"createdAt"` or `"scenes.0.createdAt"`)
/// as a `Date`.
pub fn to_string_pretty<T: Serialize>(value: &T, date_paths: &[&str]) -> Result<String> {
    let mut json = serde_json::to_value(value).map_err(|e| Error::SuperJson(e.to_string()))?;
    normalize_numbers(&mut json);
    let meta = if date_paths.is_empty() {
        None
    } else {
        let mut values = Map::new();
        for p in date_paths {
            // Only annotate paths that exist and are non-null, like superjson does.
            if path_exists(&json, p) {
                values.insert((*p).to_string(), Value::Array(vec![Value::String("Date".into())]));
            }
        }
        if values.is_empty() {
            None
        } else {
            Some(Meta { values, v: 1 })
        }
    };
    let env = Envelope { json, meta };
    serde_json::to_string_pretty(&env).map_err(|e| Error::SuperJson(e.to_string()))
}

/// JavaScript's `JSON.stringify` prints integral doubles without a fraction (`1`, not `1.0`).
/// Rewrite integral floats as integers so our output is indistinguishable from Screen Studio's
/// and so that `Value` comparisons in tests are meaningful.
pub fn normalize_numbers(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
                    *v = Value::from(f as i64);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize_numbers),
        Value::Object(m) => m.values_mut().for_each(normalize_numbers),
        _ => {}
    }
}

fn path_exists(root: &Value, path: &str) -> bool {
    let mut cur = root;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(m) => match m.get(seg) {
                Some(v) => v,
                None => return false,
            },
            Value::Array(a) => match seg.parse::<usize>().ok().and_then(|i| a.get(i)) {
                Some(v) => v,
                None => return false,
            },
            _ => return false,
        };
    }
    !cur.is_null()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_dates() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct T {
            #[serde(rename = "createdAt")]
            created_at: chrono::DateTime<chrono::Utc>,
            name: String,
        }
        let t = T { created_at: "2026-10-07T18:56:12.921Z".parse().unwrap(), name: "x".into() };
        let s = to_string_pretty(&t, &["createdAt", "missing"]).unwrap();
        assert!(s.contains("\"Date\""));
        assert!(!s.contains("missing"));
        let back: T = from_str(&s).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn bare_envelope_without_meta() {
        let v: Vec<Value> = from_str(r#"{"json": []}"#).unwrap();
        assert!(v.is_empty());
        let s = to_string_pretty(&Vec::<Value>::new(), &[]).unwrap();
        assert_eq!(s, "{\n  \"json\": []\n}");
    }
}
