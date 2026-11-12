// SPDX-License-Identifier: GPL-3.0-only
//! The values in kfxgen's `model.json`, as Python had them (kfxgen `plugin/kfxgen/model_dump.py`).
//!
//! A dict keeps its insertion order and its keys' types; a set or frozenset keeps the order the
//! dump wrote (sorted by JSON). The generator port reads these the way kfxgen reads its dicts:
//! [`PyValue::get`] is `d.get(key)`, absent and `None` being different things.
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

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

/// The resource `name`: a regular file in `resources` (no link out of it, no device) whose
/// sha256 is its name, as the dump names it. None for a name that is no sha256.
fn read_resource(resources: &Path, name: &str) -> Option<Result<Arc<[u8]>, LoadError>> {
    let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
    if name.len() != 64 || !name.chars().all(hex) {
        return None;
    }
    let path = resources.join(name);
    let problem = |problem| LoadError::Resource {
        path: path.clone(),
        problem,
    };
    let io = |source| LoadError::Io {
        path: path.clone(),
        source,
    };
    Some((|| {
        if !std::fs::symlink_metadata(&path).map_err(io)?.is_file() {
            return Err(problem("not a regular file"));
        }
        let data = std::fs::read(&path).map_err(io)?;
        let digest: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if digest != name {
            return Err(problem("not the bytes its name is the sha256 of"));
        }
        Ok(data.into())
    })())
}

impl PyValue {
    /// Reads one encoded value; `resources` holds the files `$bytes` names, `path` is where
    /// the value sits, for errors.
    pub fn from_json(
        v: &serde_json::Value,
        resources: &Path,
        path: &str,
    ) -> Result<PyValue, LoadError> {
        PyValue::read(v, resources, path, &RefCell::new(HashMap::new()))
    }

    /// `from_json`, with the resources read so far: one named again is shared, not read
    /// again. The map is only looked up, so its order reaches nothing.
    fn read(
        v: &serde_json::Value,
        resources: &Path,
        path: &str,
        seen: &RefCell<HashMap<String, Arc<[u8]>>>,
    ) -> Result<PyValue, LoadError> {
        use serde_json::Value as J;
        let list = |items: &Vec<J>| -> Result<Vec<PyValue>, LoadError> {
            items
                .iter()
                .enumerate()
                .map(|(i, x)| PyValue::read(x, resources, &format!("{path}[{i}]"), seen))
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
                                        PyValue::read(k, resources, &p, seen)?,
                                        PyValue::read(v, resources, &p, seen)?,
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
                        // Cloned out first: the lookup's borrow must end before the insert.
                        let known = seen.borrow().get(name).cloned();
                        PyValue::Bytes(match known {
                            Some(data) => data,
                            None => {
                                let data = read_resource(resources, name)
                                    .ok_or_else(|| shape("a sha256 resource name"))??;
                                seen.borrow_mut().insert(name.to_owned(), data.clone());
                                data
                            }
                        })
                    }
                    "$obj" => {
                        let class = body.as_str().ok_or_else(|| shape("a class name"))?;
                        let state = o
                            .get("state")
                            .ok_or_else(|| shape("an object with state"))?;
                        PyValue::Obj {
                            class: class.to_owned(),
                            state: Box::new(PyValue::read(
                                state,
                                resources,
                                &format!("{path}.{class}"),
                                seen,
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
        // A loop, not a call per object: objects nested without end must not overflow the stack.
        let mut v = self;
        while let PyValue::Obj { state, .. } = v {
            v = state;
        }
        match v {
            PyValue::Dict(pairs) => pairs
                .iter()
                .find(|(k, _)| matches!(k, PyValue::Str(s) if s == key))
                .map(|(_, v)| v),
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
