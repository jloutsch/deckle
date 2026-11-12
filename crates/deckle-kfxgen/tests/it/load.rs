// SPDX-License-Identifier: GPL-3.0-or-later
//! Loading kfxgen's generator input (`model.json`, phase 1 plan Task 4).
use std::path::PathBuf;

use deckle_kfxgen::load::{LoadError, load_model};
use deckle_kfxgen::pyvalue::PyValue;

pub fn model_dirs() -> Vec<PathBuf> {
    let root = deckle_kfx::fixtures::kfxgen("models");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// A copy of fixture `name` in a temp dir, with `edit` applied to its model.json.
fn edited(name: &str, edit: impl FnOnce(&mut serde_json::Value)) -> tempfile::TempDir {
    let src = deckle_kfx::fixtures::kfxgen("models").join(name);
    let dir = tempfile::tempdir().unwrap();
    let res = dir.path().join("resources");
    std::fs::create_dir(&res).unwrap();
    // git keeps no empty folder: a model with no resources has none in a fresh checkout.
    let files = match std::fs::read_dir(src.join("resources")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        r => r.unwrap().collect(),
    };
    for f in files {
        let f = f.unwrap();
        std::fs::copy(f.path(), res.join(f.file_name())).unwrap();
    }
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(src.join("model.json")).unwrap()).unwrap();
    edit(&mut doc);
    std::fs::write(dir.path().join("model.json"), doc.to_string()).unwrap();
    dir
}

fn arg<'a>(doc: &'a mut serde_json::Value, key: &str) -> &'a mut serde_json::Value {
    let pairs = doc["args"]["$dict"].as_array_mut().unwrap();
    let pair = pairs.iter_mut().find(|p| p[0] == key).unwrap();
    &mut pair[1]
}

#[test]
fn every_fixture_model_loads() {
    let dirs = model_dirs();
    assert_eq!(dirs.len(), 73);
    for dir in dirs {
        let m = load_model(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        assert_eq!(m.kfxgen_version, "5.24.0");
        let chapters = m.args.get("chapters");
        assert!(
            matches!(chapters, Some(PyValue::List(c)) if !c.is_empty()),
            "{}",
            dir.display()
        );
    }
}

#[test]
fn unknown_tag_is_an_error() {
    let dir = edited("minimal", |doc| {
        *arg(doc, "asin") = serde_json::json!({"$zzz": []})
    });
    assert!(matches!(
        load_model(dir.path()),
        Err(LoadError::UnknownTag { tag, .. }) if tag == "$zzz"
    ));
}

#[test]
fn another_schema_is_an_error() {
    let dir = edited("minimal", |doc| doc["schema"] = serde_json::json!(2));
    assert!(matches!(load_model(dir.path()), Err(LoadError::Schema(2))));
}

#[test]
fn pyvalue_reads_every_tag() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ab12"), b"\x01\x02").unwrap();
    let json = serde_json::json!([
        null, true, 7, 1.5, "a\u{0}b",
        {"$tuple": [1, "x"]},
        {"$set": ["b", "a"]},
        {"$frozenset": ["italic"]},
        {"$dict": [[{"$tuple": [1, 2]}, "v"]]},
        {"$bytes": "ab12"},
        {"$obj": "kfxgen.font_table.Face", "state": {"$dict": [["weight", 400]]}}
    ]);
    let v = PyValue::from_json(&json, dir.path(), "t").unwrap();
    let PyValue::List(items) = v else {
        panic!("{v:?}")
    };
    assert_eq!(items[0], PyValue::None);
    assert_eq!(items[1], PyValue::Bool(true));
    assert_eq!(items[2], PyValue::Int(7));
    assert_eq!(items[3], PyValue::Float(1.5));
    assert_eq!(items[4], PyValue::Str("a\u{0}b".into()));
    assert_eq!(
        items[5],
        PyValue::Tuple(vec![PyValue::Int(1), PyValue::Str("x".into())])
    );
    assert_eq!(
        items[6],
        PyValue::Set(vec![PyValue::Str("b".into()), PyValue::Str("a".into())])
    );
    assert_eq!(items[7], PyValue::Set(vec![PyValue::Str("italic".into())]));
    assert!(matches!(&items[8], PyValue::Dict(d) if d.len() == 1));
    assert_eq!(items[9].as_bytes().unwrap(), b"\x01\x02");
    assert!(matches!(&items[10], PyValue::Obj { class, .. } if class == "kfxgen.font_table.Face"));
    assert_eq!(items[10].get("weight"), Some(&PyValue::Int(400)));
}
