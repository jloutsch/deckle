// SPDX-License-Identifier: GPL-3.0-only
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

/// `dir/model.json.gz` (how the fixtures are stored), else `dir/model.json` (a fresh dump, a
/// corpus model).
pub fn load_model(dir: &Path) -> Result<Model, LoadError> {
    let doc: serde_json::Value = serde_json::from_slice(&read_model_json(dir)?)?;
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

/// The bytes of the model's JSON, from `model.json.gz` when there is one, else `model.json`.
pub fn read_model_json(dir: &Path) -> Result<Vec<u8>, LoadError> {
    use std::io::Read;
    let gz = dir.join("model.json.gz");
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| LoadError::Io { path, source }
    };
    match std::fs::File::open(&gz) {
        Ok(file) => {
            let mut bytes = Vec::new();
            flate2::read::GzDecoder::new(file)
                .read_to_end(&mut bytes)
                .map_err(io(&gz))?;
            Ok(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let file = dir.join("model.json");
            std::fs::read(&file).map_err(io(&file))
        }
        Err(e) => Err(io(&gz)(e)),
    }
}
