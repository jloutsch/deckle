// SPDX-License-Identifier: GPL-3.0-or-later
//! Loads kfxgen's generator input: the `model.json` and `resources/` its `KFXGEN_DUMP_MODEL`
//! hook writes (kfxgen `plugin/kfxgen/model_dump.py`). Test input only: the oracle the port is
//! checked against.
use std::path::{Path, PathBuf};

use crate::pyvalue::PyValue;

/// The `model.json` schema this crate reads.
pub const SCHEMA: u64 = 1;

#[derive(Clone, Debug)]
pub struct Model {
    pub kfxgen_version: String,
    pub kfxgen_commit: String,
    /// `generate_full_book`'s keyword arguments (all but `output_path`), as a dict.
    pub args: PyValue,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("model.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("model.json is schema {0}, this crate reads {SCHEMA}")]
    Schema(u64),
    #[error("{path}: {problem}")]
    Resource {
        path: PathBuf,
        problem: &'static str,
    },
    #[error("{path}: unknown tag {tag}")]
    UnknownTag { path: String, tag: String },
    #[error("{path}: expected {expected}")]
    Shape {
        path: String,
        expected: &'static str,
    },
}

pub fn load_model(dir: &Path) -> Result<Model, LoadError> {
    let file = dir.join("model.json");
    let bytes = std::fs::read(&file).map_err(|source| LoadError::Io { path: file, source })?;
    let doc: serde_json::Value = serde_json::from_slice(&bytes)?;
    let schema = doc["schema"].as_u64().unwrap_or(0);
    if schema != SCHEMA {
        return Err(LoadError::Schema(schema));
    }
    let text = |key: &str| doc[key].as_str().unwrap_or_default().to_owned();
    Ok(Model {
        kfxgen_version: text("kfxgen_version"),
        kfxgen_commit: text("kfxgen_commit"),
        args: PyValue::from_json(&doc["args"], &dir.join("resources"), "args")?,
    })
}
