// SPDX-License-Identifier: GPL-3.0-only
//! Tables against kfxgen's KFX (phase 1 plan, Task 8).
use deckle_kfxgen::generate::{GenerateError, generate};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;

use crate::generate_inline::{entry, item};
use crate::generate_skeleton::matches_kfxgen;

#[test]
fn table_cells_matches_kfxgen() {
    matches_kfxgen("table_cells");
}

#[test]
fn table_layout_matches_kfxgen() {
    matches_kfxgen("table_layout");
}

#[test]
fn table_paragraph_cells_matches_kfxgen() {
    matches_kfxgen("table_paragraph_cells");
}

/// Borders, widths, padding, groups, header and empty cells, runs and ids no EPUB-made
/// fixture reaches.
#[test]
fn table_parts_matches_kfxgen() {
    matches_kfxgen("table_parts");
}

/// A chapter's title in or over a table.
#[test]
fn table_title_matches_kfxgen() {
    matches_kfxgen("table_title");
}

/// kfxgen's `quantize` raises on a cell padding too wide for 28 digits (:2071): a model
/// error naming the padding, not a panic or a padding of 0. The cell's table is 50% wide,
/// so 1.5e24 ems is 9.375e24 %, 25 digits and 3 decimals: still written; 1.7e24 ems is 1.0625e25 %, one digit too many.
#[test]
fn a_padding_kfxgen_cannot_write_is_a_model_error() {
    for (em, ok) in [(1.5e24, true), (1.7e24, false), (1e300, false)] {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/table_parts")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 1);
        let table = entry(item(entry(chapter, "blocks"), 1), "table");
        let cell = item(entry(item(entry(table, "rows"), 0), "cells"), 0);
        *entry(entry(cell, "padding"), "right") = PyValue::Float(em);
        match generate(&model) {
            Ok(_) => assert!(ok, "{em}"),
            Err(GenerateError::Model(m)) => assert!(!ok && m.contains("padding"), "{em}: {m}"),
            Err(e) => panic!("{em}: {e}"),
        }
    }
}

/// A run that starts outside its cell's text is a model error: the positions map walked
/// from the run's start, to a panic past the end and without limit before it (issue #4).
#[test]
fn a_cell_run_outside_the_text_is_a_model_error() {
    let tcy = PyValue::Tuple(vec![
        PyValue::Str("tcy".into()),
        PyValue::Tuple(vec![PyValue::Int(0), PyValue::Str("12".into())]),
    ]);
    for (start, ok) in [
        (0, true),
        (4, true),
        (5, false),
        (2027, false),
        (1 << 60, false),
        (-1, false),
        (-100_000_000, false),
    ] {
        let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/table_parts")).unwrap();
        let chapter = item(entry(&mut model.args, "chapters"), 1);
        let table = entry(item(entry(chapter, "blocks"), 1), "table");
        let cell = item(entry(item(entry(table, "rows"), 0), "cells"), 0);
        let run = PyValue::Tuple(vec![
            PyValue::Int(start),
            PyValue::Int(1),
            PyValue::Set(vec![tcy.clone()]),
        ]);
        *cell = PyValue::Dict(vec![
            (PyValue::Str("text".into()), PyValue::Str("Kept".into())),
            (PyValue::Str("spans".into()), PyValue::List(vec![run])),
        ]);
        match generate(&model) {
            Ok(_) => assert!(ok, "{start}"),
            Err(GenerateError::Model(m)) => assert!(!ok && m.contains("spans"), "{start}: {m}"),
            Err(e) => panic!("{start}: {e}"),
        }
    }
}

/// Colspans that sum past an i64 are no panic (issue #12): kfxgen's ints have no bound.
#[test]
fn colspans_past_an_i64_do_not_panic() {
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/table_parts")).unwrap();
    let chapter = item(entry(&mut model.args, "chapters"), 1);
    let table = entry(item(entry(chapter, "blocks"), 1), "table");
    let cells = entry(item(entry(table, "rows"), 0), "cells");
    for (i, n) in [(0, i64::MAX), (1, i64::MAX)] {
        let PyValue::Dict(cell) = item(cells, i) else {
            panic!("not a dict")
        };
        cell.retain(|(k, _)| k.as_str() != Some("colspan"));
        cell.push((PyValue::Str("colspan".into()), PyValue::Int(n)));
    }
    let _ = generate(&model);
}

/// The first dict of `"type": kind` under `v`.
fn block_of<'a>(v: &'a mut PyValue, kind: &str) -> Option<&'a mut PyValue> {
    let is = matches!(&*v, PyValue::Dict(p) if p.iter().any(|(k, t)| k.as_str() == Some("type") && t.as_str() == Some(kind)));
    if is {
        return Some(v);
    }
    match v {
        PyValue::Dict(p) => p.iter_mut().find_map(|(_, x)| block_of(x, kind)),
        PyValue::List(l) | PyValue::Tuple(l) => l.iter_mut().find_map(|x| block_of(x, kind)),
        _ => None,
    }
}

fn put(dict: &mut PyValue, key: &str, value: PyValue) {
    let PyValue::Dict(pairs) = dict else {
        panic!("not a dict")
    };
    pairs.retain(|(k, _)| k.as_str() != Some(key));
    pairs.push((PyValue::Str(key.into()), value));
}

/// Keys kfxgen reads on a block of another type are refused, not ignored, and a row order of
/// None, which kfxgen cannot sort, is a model error (issue #13).
#[test]
fn keys_kfxgen_reads_on_another_block_are_refused() {
    let dict = |k: &str, v: PyValue| PyValue::Dict(vec![(PyValue::Str(k.into()), v)]);
    let rtl = || dict("direction", PyValue::Str("rtl".into()));
    let cases: Vec<(&str, &str, Box<dyn Fn(&mut PyValue)>, &str)> = vec![
        (
            "table_parts",
            "table",
            Box::new(|b| put(b, "heading", PyValue::Bool(true))),
            "a table marked as a heading",
        ),
        (
            "table_parts",
            "table",
            Box::new(move |b| put(b, "box", rtl())),
            "boxes",
        ),
        (
            "boxes",
            "box",
            Box::new(move |b| put(b, "block_style", dict("bidi", rtl()))),
            "a direction on a rule's or box's own style",
        ),
        (
            "boxes",
            "box",
            Box::new(|b| put(entry(b, "box"), "clear", PyValue::Str("both".into()))),
            "clearing floats",
        ),
        (
            "rules_spaces",
            "rule",
            Box::new(|b| put(b, "text", PyValue::Str("x".into()))),
            "a rule with text or a box",
        ),
        (
            "rules_spaces",
            "rule",
            Box::new(move |b| put(b, "box", rtl())),
            "a rule with text or a box",
        ),
    ];
    for (fixture, kind, edit, feature) in cases {
        let dir = deckle_kfx::fixtures::kfxgen(&format!("models/{fixture}"));
        let mut model = load_model(&dir).unwrap();
        edit(block_of(&mut model.args, kind).unwrap());
        match generate(&model) {
            Err(GenerateError::Unsupported(f)) => assert_eq!(f, feature),
            other => panic!("{feature}: {:?}", other.map(|_| ())),
        }
    }
    let mut model = load_model(&deckle_kfx::fixtures::kfxgen("models/table_parts")).unwrap();
    let table = entry(block_of(&mut model.args, "table").unwrap(), "table");
    put(item(entry(table, "rows"), 0), "source_order", PyValue::None);
    match generate(&model) {
        Err(GenerateError::Model(m)) => assert!(m.contains("source_order"), "{m}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}
