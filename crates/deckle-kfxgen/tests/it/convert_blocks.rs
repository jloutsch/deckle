// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's block walk, `extract_blocks_from_html` (phase 3 plan, Stage D), against the block
//! dumps (`fixtures/kfxgen/convert/<name>/blocks/NNN.json.gz`), and what it refuses.

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::convert::{
    Block, Error, ExtractOptions, FileBlocks, ItemError, Resolver, Serials, TextBudget, Walk,
    blocks_before_spacing, extract_blocks,
};
use deckle_kfxgen::css::DEFAULT_PROFILE;
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_resolver::{empty_page, file_of, page};
use crate::convert_support::{dump, model_diff};
use crate::support::{COST_LIMIT, allocated_by};

/// The sources whose every spine file's blocks match the dump, on every pass recorded. A source
/// joins in the task that makes it pass and never leaves. (`notes` and `vertical_contents` hold
/// lists: Task 13 adds them.)
const FIXTURES: &[&str] = &[
    "block_edges",
    "block_picture_769",
    "comments_in_tree",
    "fake_margins",
    "fonts",
    "fonts_woff",
    "link_colours",
    "link_greys",
    "listing_rerun",
    "malformed",
    "navigation",
    "no_toc",
    "package_more",
    "package_quirks",
    "page_list",
    "pictures_side_by_side_766",
    "refused_selector",
    "rtl",
    "ruby_tcy",
    "vertical",
];

/// The sources recorded on a second pass (with `keep_nav_listings`): both passes are compared.
const TWO_PASSES: &[&str] = &["listing_rerun", "refused_selector"];

/// Keys left out of both sides of the comparison.
struct LeftOut {
    /// Written during the walk by `_note_margins`, `_note_padding` and
    /// `_carry_container_margins`: left out of D2a and D2.
    walk: &'static [&'static str],
    /// Written by `_space_between`: left out of D2.
    spacing: &'static [&'static str],
}

/// What Task 12 (margins and spacing) writes and this task does not. Task 12 removes this list.
/// Until then the margins and paddings columns of each D2a entry (its second and third items)
/// are not compared either.
const LEFT_OUT_UNTIL_TASK_12: LeftOut = LeftOut {
    walk: &["own_bottom_em", "bottom_in_em", "outer_em_ratio"],
    spacing: &[
        "margin_above",
        "margin_below",
        "padding_below",
        "space_above",
        "tight",
        "trailing_space",
    ],
};

/// `block` without the keys in `left_out`.
fn without(block: &PyValue, left_out: &[&[&str]]) -> PyValue {
    match block {
        PyValue::Dict(pairs) => PyValue::Dict(
            pairs
                .iter()
                .filter(|(k, _)| {
                    !k.as_str()
                        .is_some_and(|k| left_out.iter().any(|keys| keys.contains(&k)))
                })
                .cloned()
                .collect(),
        ),
        other => other.clone(),
    }
}

/// D2a leaves out what the walk's margin hooks write.
fn for_d2a(block: &PyValue) -> PyValue {
    without(block, &[LEFT_OUT_UNTIL_TASK_12.walk])
}

/// D2 leaves out that and what `_space_between` writes.
fn for_d2(block: &PyValue) -> PyValue {
    without(
        block,
        &[LEFT_OUT_UNTIL_TASK_12.walk, LEFT_OUT_UNTIL_TASK_12.spacing],
    )
}

fn blocks_py(blocks: &[Block], left_out: fn(&PyValue) -> PyValue) -> PyValue {
    PyValue::List(blocks.iter().map(|b| left_out(&b.to_py())).collect())
}

fn strs(items: &[String]) -> PyValue {
    PyValue::List(items.iter().map(|s| PyValue::Str(s.clone())).collect())
}

/// The out-lists as the dump writes them.
fn out_lists(got: &FileBlocks) -> Vec<(&'static str, PyValue)> {
    let places = |seen: &[deckle_kfxgen::convert::BlockId]| {
        PyValue::List(
            seen.iter()
                .map(|id| match got.blocks.iter().position(|b| b.id() == *id) {
                    Some(i) => PyValue::List(vec![PyValue::Int(i as i64)]),
                    None => PyValue::None,
                })
                .collect(),
        )
    };
    vec![
        (
            "nav_listing_at",
            PyValue::List(
                got.nav_listing_at
                    .iter()
                    .map(|(at, ids)| {
                        PyValue::Tuple(vec![
                            PyValue::Int(*at as i64),
                            PyValue::Set(ids.iter().map(|i| PyValue::Str(i.clone())).collect()),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("tables_seen", places(&got.tables_seen)),
        ("notes_seen", places(&got.notes_seen)),
        ("trailing_ids", strs(&got.trailing_ids)),
        ("end_ids", strs(&got.end_ids)),
    ]
}

fn string_set(v: Option<&PyValue>) -> HashSet<String> {
    v.and_then(PyValue::as_list)
        .unwrap_or(&[])
        .iter()
        .map(|s| s.as_str().unwrap().to_owned())
        .collect()
}

/// For each source in [`FIXTURES`] and each of its passes, every spine file's walk gives the
/// blocks kfxgen handed its file's `_space_between` (D2a, the last call not inside another),
/// and the whole extraction gives the dumped blocks and out-lists, with
/// [`LEFT_OUT_UNTIL_TASK_12`] left out. The serials run on across the book and its passes, one
/// count for each of the two runs. Every spine file must have a dump holding a record for each
/// of the source's passes, and none other, so no file is skipped without a failure.
#[test]
fn blocks_match_the_dump() {
    let mut failures = Vec::new();
    for name in FIXTURES {
        let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let listed: BTreeMap<String, String> = serde_json::from_slice(
            &std::fs::read(fixtures::kfxgen(&format!("convert/{name}/files.json"))).unwrap(),
        )
        .unwrap();
        let toc = dump(name, "toc.json.gz");
        let link_targets = string_set(toc.get("link_targets"));
        let (mut walked_serials, mut serials) = (Serials::default(), Serials::default());
        let want_passes: &[i64] = if TWO_PASSES.contains(name) {
            &[0, 1]
        } else {
            &[0]
        };
        let mut blocks = 0;
        // Files compared on each pass.
        let mut compared = vec![0usize; want_passes.len()];
        for (p, &pass) in want_passes.iter().enumerate() {
            for file in &book.files {
                let Some(nnn) = listed.get(&file.href) else {
                    failures.push(format!("{name} {}: not in files.json", file.href));
                    continue;
                };
                let at = format!("blocks/{nnn}.json.gz");
                if !fixtures::kfxgen(&format!("convert/{name}/{at}")).exists() {
                    failures.push(format!("{name} {nnn}: no block dump"));
                    continue;
                }
                let PyValue::List(records) = dump(name, &at) else {
                    panic!("{name} {nnn}: no list");
                };
                if p == 0 {
                    let mut recorded: Vec<i64> = records
                        .iter()
                        .filter_map(|r| r.get("pass").and_then(PyValue::as_int))
                        .collect();
                    recorded.sort_unstable();
                    if recorded != want_passes {
                        failures.push(format!(
                            "{name} {nnn}: passes recorded {recorded:?}, want {want_passes:?}"
                        ));
                    }
                }
                let Some(record) = records
                    .iter()
                    .find(|r| r.get("pass").and_then(PyValue::as_int) == Some(pass))
                else {
                    failures.push(format!("{name} {nnn}: no record for pass {pass}"));
                    continue;
                };
                compared[p] += 1;
                let where_ = format!("{name} {nnn} pass {pass}");
                let args = record.get("args").unwrap();
                let toc_targets = string_set(args.get("toc_targets"));
                let opts = ExtractOptions {
                    native_tables: args.get("native_tables") == Some(&PyValue::Bool(true)),
                    toc_targets: &toc_targets,
                    link_targets: &link_targets,
                    keep_nav_listings: args.get("keep_nav_listings") == Some(&PyValue::Bool(true)),
                };
                let budget = TextBudget::new();
                let resolver = Resolver::new(file, &DEFAULT_PROFILE, &budget);

                // D2a: the input of the file's own `_space_between`.
                let spacing = record.get("spacing").and_then(PyValue::as_list).unwrap();
                let file_call = spacing
                    .iter()
                    .rfind(|c| c.get("depth").and_then(PyValue::as_int) == Some(0))
                    .unwrap_or_else(|| panic!("{where_}: no spacing call"));
                let expected = PyValue::List(
                    file_call
                        .get("blocks")
                        .and_then(PyValue::as_list)
                        .unwrap()
                        .iter()
                        .map(|entry| for_d2a(&entry.as_list().unwrap()[0]))
                        .collect(),
                );
                let mut walk = Walk::new(file, &resolver, &mut walked_serials);
                match blocks_before_spacing(&mut walk, &opts) {
                    Ok(got) => {
                        for d in model_diff(&blocks_py(&got, for_d2a), &expected) {
                            failures.push(format!("{where_} D2a{d}"));
                        }
                    }
                    Err(e) => failures.push(format!("{where_} D2a: {e:?}")),
                }

                // D2: the blocks returned and the out-lists.
                let budget = TextBudget::new();
                let resolver = Resolver::new(file, &DEFAULT_PROFILE, &budget);
                let mut walk = Walk::new(file, &resolver, &mut serials);
                match extract_blocks(&mut walk, &opts) {
                    Ok(got) => {
                        let want = PyValue::List(
                            record
                                .get("blocks")
                                .and_then(PyValue::as_list)
                                .unwrap()
                                .iter()
                                .map(for_d2)
                                .collect(),
                        );
                        for d in model_diff(&blocks_py(&got.blocks, for_d2), &want) {
                            failures.push(format!("{where_} D2 blocks{d}"));
                        }
                        for (key, ours) in out_lists(&got) {
                            for d in model_diff(&ours, record.get(key).unwrap()) {
                                failures.push(format!("{where_} D2 {key}{d}"));
                            }
                        }
                        blocks += got.blocks.len();
                    }
                    Err(e) => failures.push(format!("{where_} D2: {e:?}")),
                }
            }
        }
        eprintln!(
            "{name}: {} spine files, compared on passes {want_passes:?}: {compared:?}, {blocks} blocks",
            book.files.len()
        );
        if book.files.is_empty() {
            failures.push(format!("{name}: no spine file"));
        }
        for (p, n) in compared.iter().enumerate() {
            if *n != book.files.len() {
                failures.push(format!(
                    "{name}: {n} of {} files compared on pass {}",
                    book.files.len(),
                    want_passes[p]
                ));
            }
        }
    }
    for f in failures.iter().take(60) {
        eprintln!("{f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
}

/// A `<div>` nested `deep` levels in the body, a paragraph at the bottom.
fn nested_divs(deep: usize) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut n = body;
    for _ in 0..deep {
        n = dom.append_child(n, "div");
    }
    let p = dom.append_child(n, "p");
    dom.set_text(p, "x");
    file_of(dom, "div { margin: 0 }")
}

fn extract(file: &deckle_kfxgen::book::File) -> Result<FileBlocks, ItemError> {
    let budget = TextBudget::new();
    let resolver = Resolver::new(file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let none = HashSet::new();
    let opts = ExtractOptions {
        native_tables: true,
        toc_targets: &none,
        link_targets: &none,
        keep_nav_listings: false,
    };
    extract_blocks(&mut Walk::new(file, &resolver, &mut serials), &opts)
}

/// `read_book` refuses a tree deeper than `MAX_DEPTH`, but a `File` can be built by hand: the
/// walk refuses it rather than recursing 20,000 times. It runs on a test thread's stack.
#[test]
fn a_div_nested_20000_deep_is_refused_not_overflowed() {
    let file = nested_divs(20_000);
    let start = Instant::now();
    let got = extract(&file);
    let took = start.elapsed();
    // The block walk's refusal, not the inline walk's ("levels inside a paragraph").
    match &got {
        Err(ItemError::Error(Error::TooLarge { what })) => assert!(
            what.ends_with("levels below the body"),
            "not the block walk's: {what}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(took < COST_LIMIT, "{took:?}");
}

/// As deep as `read_book` lets a tree be, the walk reaches the paragraph on a test thread's
/// stack.
#[test]
fn divs_as_deep_as_a_book_may_nest_are_walked() {
    let deep = deckle_limits::MAX_DEPTH - 2; // under <html><body>, over the <p>
    let file = nested_divs(deep);
    let got = extract(&file).unwrap();
    assert_eq!(got.blocks.len(), 1);
    assert_eq!(got.blocks[0].get("text"), Some(&PyValue::Str("x".into())));
}

/// The bytes `extract` allocates on `make(n)` and on `make(2 * n)`, each run within
/// `COST_LIMIT`: work linear in the page allocates about twice as much on twice the page,
/// work quadratic in it four times as much.
fn allocates_linearly(what: &str, n: usize, make: impl Fn(usize) -> deckle_kfxgen::book::File) {
    let run = |n: usize| {
        let file = make(n);
        let start = Instant::now();
        let (got, bytes) = allocated_by(|| extract(&file));
        let took = start.elapsed();
        got.unwrap();
        eprintln!("{what}, {n}: {bytes} bytes allocated, {took:?}");
        assert!(took < COST_LIMIT, "{what}, {n}: {took:?}");
        bytes
    };
    let (once, twice) = (run(n), run(2 * n));
    assert!(
        twice < 3 * once,
        "{what}: {once} bytes, then {twice} on twice the page"
    );
}

/// Every paragraph of a `dir="auto"` section asks its direction of the section, whose text
/// holds no letter with one: kfxgen reads the whole section again for each paragraph. Read
/// once here.
#[test]
fn a_dir_auto_section_is_read_once() {
    let make = |n: usize| {
        let body = format!("<div dir=\"auto\">{}</div>", "<p>1 2 3</p>".repeat(n));
        page(&body, "")
    };
    allocates_linearly("paragraphs under dir=auto", 4_000, make);
    let got = extract(&make(2)).unwrap();
    let style = got.blocks[0].get("block_style").unwrap();
    assert_eq!(
        style.get("bidi").and_then(|b| b.get("direction")),
        Some(&PyValue::Str("ltr".into()))
    );
}

/// Contents listings one after another, each with an id and no block between: kfxgen adds
/// every id waiting to its set after each one. Added once here.
#[test]
fn listings_in_a_row_note_their_ids_once() {
    let make = |n: usize| {
        let mut body = String::new();
        for i in 0..n {
            body.push_str(&format!("<div role=\"doc-toc\" id=\"n{i}\"/>"));
        }
        body.push_str("<p>after</p>");
        page(&body, "")
    };
    allocates_linearly("listings", 5_000, make);
    let got = extract(&make(3)).unwrap();
    assert_eq!(got.nav_listing_at.len(), 3);
    let ids = got.blocks[0].get("anchor_ids").unwrap();
    assert_eq!(ids.as_list().map(<[PyValue]>::len), Some(3));
}

/// Empty paragraphs one after another, each with an id: kfxgen copies the ids waiting for a
/// block at each. Moved only to the block that takes them here.
#[test]
fn empty_paragraphs_in_a_row_carry_their_ids_in_linear_time() {
    let make = |n: usize| {
        let mut body = String::new();
        for i in 0..n {
            body.push_str(&format!("<p id=\"e{i}\"></p>"));
        }
        body.push_str("<p>after</p>");
        page(&body, "")
    };
    allocates_linearly("empty paragraphs", 5_000, make);
    let got = extract(&make(3)).unwrap();
    let ids = got.blocks[0].get("anchor_ids").unwrap();
    assert_eq!(ids.as_list().map(<[PyValue]>::len), Some(3));
}

/// Where kfxgen raises inside the walk (it leaves the spine item out), the port gives
/// `ItemError::Raised` with Python's exception: a box tag's width in `in` spelt with U+0131
/// (`_box_width` → `parse_css_length`), and a paragraph's `text-align` that calibre reads as
/// a length (`compute_block_style`).
#[test]
fn what_kfxgen_raises_on_in_the_walk_is_raised() {
    for (body, css, exception) in [
        (
            "<div class=\"w\">x</div>",
            ".w { width: 2\u{131}n }",
            "KeyError",
        ),
        (
            "<p class=\"a\">x</p>",
            ".a { text-align: 5px }",
            "AttributeError",
        ),
    ] {
        match extract(&page(body, css)) {
            Err(ItemError::Raised(r)) => assert_eq!(r.exception, exception, "{css}"),
            other => panic!("{css}: {:?}", other.map(|b| b.blocks.len())),
        }
    }
}
