// SPDX-License-Identifier: GPL-3.0-only
//! The dumps taken inside a kfxgen conversion (`fixtures/kfxgen/convert/`, written by
//! `tools/ref/kfxgen_hook/convert_dump.py`), and the comparison every converter test makes
//! with them.

use std::io::Read;

use deckle_kfx::fixtures;
use deckle_kfxgen::pyvalue::PyValue;
use sha2::{Digest, Sha256};

/// `fixtures/kfxgen/convert/<name>/<file>`, gunzipped when its name ends in `.gz`, as the
/// value it encodes (`model_dump.py`'s encoding). `name` is a source fixture, or `units`.
pub fn dump(name: &str, file: &str) -> PyValue {
    let dir = fixtures::kfxgen(&format!("convert/{name}"));
    let path = dir.join(file);
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let bytes = if file.ends_with(".gz") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice())
            .read_to_end(&mut out)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        out
    } else {
        raw
    };
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    PyValue::from_json(&json, &dir.join("resources"), file)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Where `ours` and `dump` differ, one line per difference, each starting with its path
/// (`["chapters"][0]["blocks"]`). Dict keys must come in the same order, a key holding `None`
/// is not an absent key, an int is not a float, floats are equal bit for bit (`-0.0` is not
/// `0.0`), a list is not a tuple, and bytes are compared by their sha256. Empty when equal.
pub fn model_diff(ours: &PyValue, dump: &PyValue) -> Vec<String> {
    let mut out = Vec::new();
    diff(ours, dump, String::new(), &mut out);
    out
}

fn kind(v: &PyValue) -> &'static str {
    match v {
        PyValue::None => "None",
        PyValue::Bool(_) => "bool",
        PyValue::Int(_) => "int",
        PyValue::Float(_) => "float",
        PyValue::Str(_) => "str",
        PyValue::List(_) => "list",
        PyValue::Tuple(_) => "tuple",
        PyValue::Set(_) => "set",
        PyValue::Dict(_) => "dict",
        PyValue::Bytes(_) => "bytes",
        PyValue::Obj { .. } => "object",
    }
}

fn sha256(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// A short form of `v` for a message; a dict key's form in a path.
fn show(v: &PyValue) -> String {
    match v {
        PyValue::Str(s) => format!("{s:?}"),
        PyValue::Float(f) => format!("{f:?} (bits {:#018x})", f.to_bits()),
        PyValue::Bytes(b) => format!("bytes sha256 {}", sha256(b)),
        other => {
            let text = format!("{other:?}");
            if text.chars().count() > 120 {
                format!("{}…", text.chars().take(120).collect::<String>())
            } else {
                text
            }
        }
    }
}

fn diff(ours: &PyValue, dump: &PyValue, path: String, out: &mut Vec<String>) {
    use PyValue as P;
    match (ours, dump) {
        (P::Float(a), P::Float(b)) => {
            if a.to_bits() != b.to_bits() {
                out.push(format!("{path}: {} != dump {}", show(ours), show(dump)));
            }
        }
        (P::Bytes(a), P::Bytes(b)) => {
            if sha256(a) != sha256(b) {
                out.push(format!("{path}: {} != dump {}", show(ours), show(dump)));
            }
        }
        (P::List(a), P::List(b)) | (P::Tuple(a), P::Tuple(b)) => items(a, b, &path, out),
        (P::Set(a), P::Set(b)) => {
            // Python writes a set's items sorted by their JSON; ours may hold them in any
            // order. Both sorted the same way, then compared in place.
            let sorted = |v: &[PyValue]| {
                let mut v = v.to_vec();
                v.sort_by_cached_key(|x| format!("{x:?}"));
                v
            };
            items(&sorted(a), &sorted(b), &path, out);
        }
        (P::Dict(a), P::Dict(b)) => {
            for (k, _) in b {
                if find(a, k).is_none() {
                    out.push(format!("{path}: key {} missing", show(k)));
                }
            }
            for (k, _) in a {
                if find(b, k).is_none() {
                    out.push(format!("{path}: key {} not in the dump", show(k)));
                }
            }
            let common = |x: &[(PyValue, PyValue)], y: &[(PyValue, PyValue)]| -> Vec<String> {
                x.iter()
                    .filter(|(k, _)| find(y, k).is_some())
                    .map(|(k, _)| show(k))
                    .collect()
            };
            let (ka, kb) = (common(a, b), common(b, a));
            if ka != kb {
                out.push(format!(
                    "{path}: keys in another order: [{}] != dump [{}]",
                    ka.join(", "),
                    kb.join(", ")
                ));
            }
            for (k, v) in a {
                if let Some(w) = find(b, k) {
                    diff(v, w, format!("{path}[{}]", show(k)), out);
                }
            }
        }
        (
            P::Obj {
                class: ca,
                state: sa,
            },
            P::Obj {
                class: cb,
                state: sb,
            },
        ) => {
            if ca != cb {
                out.push(format!("{path}: object {ca} != dump {cb}"));
            } else {
                diff(sa, sb, format!("{path}.{ca}"), out);
            }
        }
        _ if kind(ours) != kind(dump) => {
            out.push(format!(
                "{path}: {} {} != dump {} {}",
                kind(ours),
                show(ours),
                kind(dump),
                show(dump)
            ));
        }
        _ => {
            if ours != dump {
                out.push(format!("{path}: {} != dump {}", show(ours), show(dump)));
            }
        }
    }
}

/// The value of key `k` in `pairs`.
fn find<'a>(pairs: &'a [(PyValue, PyValue)], k: &PyValue) -> Option<&'a PyValue> {
    pairs.iter().find(|(x, _)| same_key(x, k)).map(|(_, v)| v)
}

/// Keys equal as Python would find them in one dict: of one type and value, floats by bits.
fn same_key(a: &PyValue, b: &PyValue) -> bool {
    let mut d = Vec::new();
    diff(a, b, String::new(), &mut d);
    d.is_empty()
}

fn items(a: &[PyValue], b: &[PyValue], path: &str, out: &mut Vec<String>) {
    if a.len() != b.len() {
        out.push(format!("{path}: {} items != dump {}", a.len(), b.len()));
    }
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        diff(x, y, format!("{path}[{i}]"), out);
    }
}

#[test]
fn model_diff_sees_order_presence_and_float_bits() {
    use PyValue as P;
    let s = |x: &str| P::Str(x.into());
    let dict =
        |pairs: Vec<(&str, PyValue)>| P::Dict(pairs.into_iter().map(|(k, v)| (s(k), v)).collect());
    let base = || {
        dict(vec![
            ("a", P::Int(1)),
            ("b", P::Float(0.0)),
            ("c", P::None),
            ("d", P::List(vec![P::Int(1), P::Int(2)])),
            ("e", P::Bytes(b"one".to_vec().into())),
        ])
    };
    assert_eq!(model_diff(&base(), &base()), Vec::<String>::new());

    let reordered = dict(vec![
        ("b", P::Float(0.0)),
        ("a", P::Int(1)),
        ("c", P::None),
        ("d", P::List(vec![P::Int(1), P::Int(2)])),
        ("e", P::Bytes(b"one".to_vec().into())),
    ]);
    let d = model_diff(&reordered, &base());
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with(": keys in another order"), "{d:?}");

    // A key holding None is not an absent key.
    let absent = dict(vec![
        ("a", P::Int(1)),
        ("b", P::Float(0.0)),
        ("d", P::List(vec![P::Int(1), P::Int(2)])),
        ("e", P::Bytes(b"one".to_vec().into())),
    ]);
    assert_eq!(model_diff(&absent, &base()), vec![": key \"c\" missing"]);
    assert_eq!(
        model_diff(&base(), &absent),
        vec![": key \"c\" not in the dump"]
    );

    // An int is not a float, -0.0 is not 0.0, a tuple is not a list, bytes by hash.
    let changed = dict(vec![
        ("a", P::Float(1.0)),
        ("b", P::Float(-0.0)),
        ("c", P::None),
        ("d", P::Tuple(vec![P::Int(1), P::Int(2)])),
        ("e", P::Bytes(b"two".to_vec().into())),
    ]);
    let d = model_diff(&changed, &base());
    assert_eq!(d.len(), 4, "{d:?}");
    assert!(d[0].starts_with("[\"a\"]: float 1.0"), "{d:?}");
    assert!(d[1].starts_with("[\"b\"]: -0.0"), "{d:?}");
    assert!(d[2].starts_with("[\"d\"]: tuple"), "{d:?}");
    assert!(d[3].contains(&sha256(b"one")), "{d:?}");

    // A set's order is Python's, not a difference; a missing item is.
    let set = |v: Vec<PyValue>| P::Set(v);
    assert!(model_diff(&set(vec![s("x"), s("y")]), &set(vec![s("y"), s("x")])).is_empty());
    assert_eq!(
        model_diff(&set(vec![s("x")]), &set(vec![s("y"), s("x")])),
        vec![": 1 items != dump 2"]
    );
}

/// Every dump of every source loads, and each spine file has its resolver and blocks dumps.
#[test]
fn every_convert_dump_loads() {
    let root = fixtures::kfxgen("convert");
    let mut books: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.path().join("files.json").is_file())
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    books.sort();
    let sources = std::fs::read_dir(fixtures::kfxgen("sources"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension() == Some("epub".as_ref()))
        .count();
    assert_eq!(books.len(), sources, "a convert dump per source");
    for book in &books {
        let files: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(root.join(book).join("files.json")).unwrap())
                .unwrap();
        for n in files.values() {
            for kind in ["resolver", "blocks"] {
                assert!(
                    matches!(dump(book, &format!("{kind}/{n}.json.gz")), PyValue::List(_)),
                    "{book} {kind}/{n}"
                );
            }
            let inline = format!("inline/{n}.json.gz");
            if root.join(book).join(&inline).exists() {
                assert!(matches!(dump(book, &inline), PyValue::List(_)));
            }
        }
        assert!(dump(book, "toc.json.gz").get("spine").is_some(), "{book}");
        let PyValue::List(passes) = dump(book, "assembly.json.gz") else {
            panic!("{book}: assembly is no list");
        };
        assert!(
            !passes.is_empty() && passes.iter().all(|p| p.get("chapters").is_some()),
            "{book}: a pass with no chapters"
        );
    }
    assert!(
        dump("units", "inline_style.json")
            .get("emitted_name")
            .is_some()
    );
}
