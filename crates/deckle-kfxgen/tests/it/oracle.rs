// SPDX-License-Identifier: GPL-3.0-or-later
//! Compare generated fragments with kfxgen's KFX, type by type (phase 1 plan, Tasks 5–13).
use std::path::Path;

use deckle_kfx::container::read_kfx;
use deckle_kfx::fragment::{Fragment, FragmentList};

fn of_type<'a>(frags: &'a FragmentList, ftype: &str) -> Vec<&'a Fragment> {
    let mut v: Vec<&Fragment> = frags.iter().filter(|f| f.ftype.as_str() == ftype).collect();
    v.sort_by(|a, b| {
        a.fid
            .as_ref()
            .map(|s| s.as_str())
            .cmp(&b.fid.as_ref().map(|s| s.as_str()))
    });
    v
}

fn shown(f: &Fragment) -> String {
    let s = format!("{f:#?}");
    if s.len() > 4000 {
        format!("{}…", &s[..s.floor_char_boundary(4000)])
    } else {
        s
    }
}

/// Panics, showing the first difference, unless `actual` holds the same fragments of each
/// type in `ftypes` as the KFX at `expected_kfx`: same count, fids and Ion values.
pub fn assert_fragments_match(actual: &FragmentList, expected_kfx: &Path, ftypes: &[&str]) {
    let doc = read_kfx(&std::fs::read(expected_kfx).unwrap()).unwrap();
    for ftype in ftypes {
        let (got, want) = (of_type(actual, ftype), of_type(&doc.fragments, ftype));
        for (g, w) in got.iter().zip(&want) {
            assert!(
                g == w,
                "{}: first different {ftype}\n--- generated\n{}\n--- kfxgen\n{}",
                expected_kfx.display(),
                shown(g),
                shown(w)
            );
        }
        assert_eq!(
            got.len(),
            want.len(),
            "{}: {ftype} count: generated {}, kfxgen {}",
            expected_kfx.display(),
            got.len(),
            want.len()
        );
    }
}

#[test]
fn assert_fragments_match_accepts_the_same_kfx() {
    let kfx = deckle_kfx::fixtures::kfxgen("models/minimal/expected.kfx");
    let doc = read_kfx(&std::fs::read(&kfx).unwrap()).unwrap();
    assert_fragments_match(
        &doc.fragments,
        &kfx,
        &["$490", "$259", "$145", "$157", "$260"],
    );
}

#[test]
#[should_panic(expected = "first different $490")]
fn assert_fragments_match_reports_the_first_difference() {
    let kfx = deckle_kfx::fixtures::kfxgen("models/minimal/expected.kfx");
    let mut doc = read_kfx(&std::fs::read(&kfx).unwrap()).unwrap();
    for f in doc
        .fragments
        .iter_mut()
        .filter(|f| f.ftype.as_str() == "$490")
    {
        f.value = deckle_kfx::ion::Value::Null;
    }
    assert_fragments_match(&doc.fragments, &kfx, &["$490"]);
}

#[test]
#[should_panic(expected = "$145 count")]
fn assert_fragments_match_reports_a_missing_fragment() {
    let kfx = deckle_kfx::fixtures::kfxgen("models/minimal/expected.kfx");
    let mut doc = read_kfx(&std::fs::read(&kfx).unwrap()).unwrap();
    let last = doc
        .fragments
        .0
        .iter()
        .rposition(|f| f.ftype.as_str() == "$145")
        .unwrap();
    doc.fragments.0.remove(last);
    assert_fragments_match(&doc.fragments, &kfx, &["$145"]);
}
