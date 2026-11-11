// SPDX-License-Identifier: GPL-3.0-or-later
//! The values in kfxgen's `model.json`, as Python had them (kfxgen `plugin/kfxgen/model_dump.py`).
//!
//! A dict keeps its insertion order and its keys' types; a set or frozenset keeps the order the
//! dump wrote (sorted by JSON). The generator port reads these the way kfxgen reads its dicts:
//! [`PyValue::get`] is `d.get(key)`, absent and `None` being different things.
use std::path::Path;
use std::sync::Arc;

use crate::load::LoadError;

#[derive(Clone, Debug, PartialEq)]
pub enum PyValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<PyValue>),
    Tuple(Vec<PyValue>),
    /// A set or a frozenset: the generator never tells them apart.
    Set(Vec<PyValue>),
    Dict(Vec<(PyValue, PyValue)>),
    Bytes(Arc<[u8]>),
    Obj {
        class: String,
        state: Box<PyValue>,
    },
}

impl PyValue {
    /// Reads one encoded value; `resources` holds the files `$bytes` names, `path` is where
    /// the value sits, for errors.
    pub fn from_json(
        v: &serde_json::Value,
        resources: &Path,
        path: &str,
    ) -> Result<PyValue, LoadError> {
        use serde_json::Value as J;
        let list = |items: &Vec<J>| -> Result<Vec<PyValue>, LoadError> {
            items
                .iter()
                .enumerate()
                .map(|(i, x)| PyValue::from_json(x, resources, &format!("{path}[{i}]")))
                .collect()
        };
        let shape = |expected: &'static str| LoadError::Shape {
            path: path.to_owned(),
            expected,
        };
        Ok(match v {
            J::Null => PyValue::None,
            J::Bool(b) => PyValue::Bool(*b),
            J::Number(n) => match (n.as_i64(), n.is_f64()) {
                (Some(i), _) => PyValue::Int(i),
                (None, true) => PyValue::Float(n.as_f64().unwrap()),
                _ => return Err(shape("an int that fits in 64 bits")),
            },
            J::String(s) => PyValue::Str(s.clone()),
            J::Array(items) => PyValue::List(list(items)?),
            J::Object(o) => {
                let (tag, body) = o.iter().next().ok_or_else(|| shape("a tagged value"))?;
                let items = || body.as_array().ok_or_else(|| shape("an array"));
                match tag.as_str() {
                    "$tuple" => PyValue::Tuple(list(items()?)?),
                    "$set" | "$frozenset" => PyValue::Set(list(items()?)?),
                    "$dict" => PyValue::Dict(
                        items()?
                            .iter()
                            .enumerate()
                            .map(|(i, pair)| {
                                let p = format!("{path}{{{i}}}");
                                match pair.as_array().map(Vec::as_slice) {
                                    Some([k, v]) => Ok((
                                        PyValue::from_json(k, resources, &p)?,
                                        PyValue::from_json(v, resources, &p)?,
                                    )),
                                    _ => Err(LoadError::Shape {
                                        path: p,
                                        expected: "a [key, value] pair",
                                    }),
                                }
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                    "$bytes" => {
                        let name = body.as_str().ok_or_else(|| shape("a resource name"))?;
                        if name.is_empty() || !name.chars().all(|c| c.is_ascii_hexdigit()) {
                            return Err(shape("a hex resource name"));
                        }
                        let file = resources.join(name);
                        let data = std::fs::read(&file)
                            .map_err(|source| LoadError::Io { path: file, source })?;
                        PyValue::Bytes(data.into())
                    }
                    "$obj" => {
                        let class = body.as_str().ok_or_else(|| shape("a class name"))?;
                        let state = o
                            .get("state")
                            .ok_or_else(|| shape("an object with state"))?;
                        PyValue::Obj {
                            class: class.to_owned(),
                            state: Box::new(PyValue::from_json(
                                state,
                                resources,
                                &format!("{path}.{class}"),
                            )?),
                        }
                    }
                    other => {
                        return Err(LoadError::UnknownTag {
                            path: path.to_owned(),
                            tag: other.to_owned(),
                        });
                    }
                }
            }
        })
    }

    /// `d.get(key)` for a dict with string keys, or an object's attribute.
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        match self {
            PyValue::Dict(pairs) => pairs
                .iter()
                .find(|(k, _)| matches!(k, PyValue::Str(s) if s == key))
                .map(|(_, v)| v),
            PyValue::Obj { state, .. } => state.get(key),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            PyValue::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            PyValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            PyValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// A list's or a tuple's items.
    pub fn as_list(&self) -> Option<&[PyValue]> {
        match self {
            PyValue::List(items) | PyValue::Tuple(items) => Some(items),
            _ => None,
        }
    }

    /// Python's `bool(value)`.
    pub fn is_truthy(&self) -> bool {
        match self {
            PyValue::None => false,
            PyValue::Bool(b) => *b,
            PyValue::Int(i) => *i != 0,
            PyValue::Float(f) => *f != 0.0,
            PyValue::Str(s) => !s.is_empty(),
            PyValue::List(v) | PyValue::Tuple(v) | PyValue::Set(v) => !v.is_empty(),
            PyValue::Dict(pairs) => !pairs.is_empty(),
            PyValue::Bytes(b) => !b.is_empty(),
            PyValue::Obj { .. } => true,
        }
    }
}

/// Python's `bool(d.get(key))`: an absent key is false.
pub fn truthy(v: Option<&PyValue>) -> bool {
    v.is_some_and(PyValue::is_truthy)
}
