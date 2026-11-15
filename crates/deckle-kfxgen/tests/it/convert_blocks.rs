// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's block walk, `extract_blocks_from_html` (phase 3 plan, Stage D), against the block
//! dumps (`fixtures/kfxgen/convert/<name>/blocks/NNN.json.gz`), and what it refuses.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::convert::{
    BeforeSpacing, Block, CalibreOpf, ConvertOptions, Error, ExtractOptions, FileBlocks,
    HORIZONTAL_PT, ItemError, Resolver, Serials, TextBudget, VERTICAL_PT, Walk,
    blocks_before_spacing, book_args, extract_blocks, page_width,
};
use deckle_kfxgen::css::DEFAULT_PROFILE;
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_resolver::{empty_page, file_of, page};
use crate::convert_support::{dump, model_diff};
use crate::support::{COST_LIMIT, allocated_by};

/// The sources whose every spine file's blocks match the dump, on every pass recorded. A source
/// joins in the task that makes it pass and never leaves.
const FIXTURES: &[&str] = &[
    "block_edges",
    "block_picture_769",
    "block_picture_770",
    "boxes",
    "chapter_starts_773",
    "chapter_starts_777",
    "colour",
    "comments_in_tree",
    "dark_boxes",
    "drop_caps",
    "fake_margins",
    "figures",
    "floats",
    "floats_697",
    "fonts",
    "fonts_woff",
    "frames",
    "link_colours",
    "link_greys",
    "listing_rerun",
    "lists",
    "lists_768",
    "lists_771",
    "lists_horizontal_items_764",
    "malformed",
    "marks",
    "navigation",
    "no_toc",
    "notes",
    "package_more",
    "package_quirks",
    "page_list",
    "pictures_side_by_side_766",
    "refused_selector",
    "rtl",
    "ruby_tcy",
    "rules_spaces",
    "text_edges",
    "turned_words_765",
    "typography",
    "vertical",
    "vertical_contents",
    "vertical_in_horizontal_763",
    "vertical_pictures_761",
];

/// The sources recorded on a second pass (with `keep_nav_listings`): both passes are compared.
const TWO_PASSES: &[&str] = &["listing_rerun", "refused_selector"];

fn blocks_py(blocks: &[Block]) -> PyValue {
    PyValue::List(blocks.iter().map(Block::to_py).collect())
}

fn floats(v: &[f64]) -> PyValue {
    PyValue::List(v.iter().map(|f| PyValue::Float(*f)).collect())
}

/// The walk's blocks as the dump writes `_space_between`'s input: each block with its noted
/// margins and paddings (`margins.get(block)`, `paddings.get(block)`), or None.
fn before_spacing_py(got: &BeforeSpacing) -> PyValue {
    PyValue::List(
        got.blocks
            .iter()
            .map(|b| {
                PyValue::List(vec![
                    b.to_py(),
                    got.margins
                        .get(&b.id())
                        .map_or(PyValue::None, |m| floats(m)),
                    got.paddings
                        .get(&b.id())
                        .map_or(PyValue::None, |p| floats(p)),
                ])
            })
            .collect(),
    )
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
/// with the margins and paddings noted beside them, and the whole extraction gives the dumped
/// blocks and out-lists, at the page width kfxgen gives the book. The serials run on across
/// the book and its passes, one
/// count for each of the two runs. Every spine file must have a dump holding a record for each
/// of the source's passes, and none other, so no file is skipped without a failure.
#[test]
fn blocks_match_the_dump() {
    let mut failures = Vec::new();
    for name in FIXTURES {
        let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        // kfxgen extracts a vertical book's chapters with percentages measured against a
        // 480pt page, any other's against 384pt.
        let opf = CalibreOpf::new(&book).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let vertical = book_args(&book, &opf, &ConvertOptions::default())
            .iter()
            .any(|(k, v)| k == "writing_mode" && v.as_str() == Some("vertical-rl"));
        let _page = page_width(if vertical { VERTICAL_PT } else { HORIZONTAL_PT });
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
                let expected = file_call.get("blocks").unwrap();
                let mut walk = Walk::new(file, &resolver, &mut walked_serials);
                match blocks_before_spacing(&mut walk, &opts) {
                    Ok(got) => {
                        for d in model_diff(&before_spacing_py(&got), expected) {
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
                        let want = record.get("blocks").unwrap();
                        for d in model_diff(&blocks_py(&got.blocks), want) {
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

/// Paragraphs with % margins inside 200 nested divs: kfxgen measures each paragraph's share of
/// the page through every div round it again (`_box_share`, a style resolved per div). Each
/// div's share is worked out once here, so the deep page costs about what a shallow one does.
#[test]
fn percent_margins_deep_in_the_tree_are_measured_once_per_div() {
    // Built node by node: `dom::parse` of 200 nested divs overflows a test thread's stack.
    let make = |deep: usize, n: usize| {
        let (mut dom, body) = empty_page();
        let mut at = body;
        for _ in 0..deep {
            at = dom.append_child(at, "div");
        }
        for _ in 0..n {
            let p = dom.append_child(at, "p");
            dom.set_text(p, "x");
        }
        file_of(dom, "p { margin: 5% 0 } div { padding: 0 1em }")
    };
    let bytes = |file: deckle_kfxgen::book::File| {
        let (got, bytes) = allocated_by(|| extract(&file));
        got.unwrap();
        bytes
    };
    let (shallow, deep) = (bytes(make(1, 1_000)), bytes(make(200, 1_000)));
    eprintln!("1,000 paragraphs: {shallow} bytes in one div, {deep} in 200");
    assert!(
        deep < 2 * shallow,
        "{shallow} bytes in one div, {deep} in 200"
    );
    allocates_linearly("% margins in 200 divs", 1_000, |n| make(200, n));
    // The share is the page's less each div's side padding, at 384pt: 2em is 2/32 of it.
    let got = extract(&make(2, 2)).unwrap();
    assert_eq!(
        got.blocks[1].get("margin_above"),
        Some(&PyValue::Float(
            5.0 * (384.0 / 1200.0) * (1.0 - 2.0 / 32.0 - 2.0 / 32.0)
        ))
    );
}

/// `n` paragraphs inside `deep` nested divs, each div with a side margin of 1px.
fn paragraphs_in_margined_divs(deep: usize, n: usize) -> deckle_kfxgen::book::File {
    // Built node by node: `dom::parse` of 250 nested divs overflows a debug test thread.
    let (mut dom, body) = empty_page();
    let mut at = body;
    for _ in 0..deep {
        at = dom.append_child(at, "div");
    }
    for _ in 0..n {
        let p = dom.append_child(at, "p");
        dom.set_text(p, "x");
    }
    file_of(dom, "div { margin-left: 1px; margin-right: 1px }")
}

/// Each margined div adds its side margins to every paragraph it holds, as kfxgen does
/// (`_carry_container_margins`, which copies the paragraph's style at each div). Added in
/// place here, so 250 divs cost about what one does.
#[test]
fn margins_carried_through_nested_divs_copy_no_style() {
    let bytes = |file: deckle_kfxgen::book::File| {
        let start = Instant::now();
        let (got, bytes) = allocated_by(|| extract(&file));
        let took = start.elapsed();
        got.unwrap();
        (bytes, took)
    };
    let (shallow, took_shallow) = bytes(paragraphs_in_margined_divs(1, 1_000));
    let (deep, took_deep) = bytes(paragraphs_in_margined_divs(250, 1_000));
    eprintln!(
        "1,000 paragraphs: {shallow} bytes, {took_shallow:?} in one div; \
         {deep} bytes, {took_deep:?} in 250"
    );
    assert!(
        deep < 2 * shallow,
        "{shallow} bytes in one div, {deep} in 250"
    );
    // Two divs of 1px: pinned kfxgen's `_add_margin` gives ('0.1172', '$314') after one and
    // ('0.2344', '$314') after both, on each side.
    let got = extract(&paragraphs_in_margined_divs(2, 1)).unwrap();
    let style = got.blocks[0].get("block_style").unwrap();
    let pair = Some(PyValue::Tuple(vec![
        PyValue::Str("0.2344".into()),
        PyValue::Str("$314".into()),
    ]));
    assert_eq!(style.get("margin_left"), pair.as_ref());
    assert_eq!(style.get("margin_right"), pair.as_ref());
}

/// Each paragraph-and-div step of carrying margins is charged to the file's budget, so a
/// file of many paragraphs in deeply nested margined divs is refused after bounded work.
#[test]
fn margins_carried_through_too_many_divs_are_refused() {
    let file = paragraphs_in_margined_divs(250, 2_100);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let none = HashSet::new();
    let opts = ExtractOptions {
        native_tables: true,
        toc_targets: &none,
        link_targets: &none,
        keep_nav_listings: false,
    };
    let start = Instant::now();
    let got = extract_blocks(&mut Walk::new(&file, &resolver, &mut serials), &opts);
    let took = start.elapsed();
    assert!(
        matches!(got, Err(ItemError::Error(Error::Book(_)))),
        "{got:?}"
    );
    assert!(
        budget.charged() > deckle_limits::MAX_CASCADE_DECLS,
        "{}",
        budget.charged()
    );
    assert!(took < COST_LIMIT, "{took:?}");
}

/// One long paragraph inside 250 nested divs, with or without a side margin on each.
fn long_paragraph_in_divs(margined: bool) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut at = body;
    for _ in 0..250 {
        at = dom.append_child(at, "div");
    }
    let p = dom.append_child(at, "p");
    dom.set_text(p, &"word ".repeat(2 << 20));
    let css = if margined {
        "div { margin-left: 1px; margin-right: 1px }"
    } else {
        "div { }"
    };
    file_of(dom, css)
}

/// kfxgen looks for words outside picture tokens in a paragraph's whole text at each margined
/// div round it (`_has_real_text` in `_carry_container_margins`). The text does not change
/// between divs, so it is read once here: a 10 MB paragraph under 250 margined divs costs
/// about what it costs under 250 divs without margins.
#[test]
fn a_long_paragraph_in_nested_margined_divs_is_read_once() {
    let time = |margined: bool| {
        let file = long_paragraph_in_divs(margined);
        let start = Instant::now();
        let got = extract(&file);
        let took = start.elapsed();
        assert_eq!(got.unwrap().blocks.len(), 1);
        took
    };
    let (plain, margined) = (time(false), time(true));
    eprintln!("10 MB paragraph in 250 divs: {plain:?} without margins, {margined:?} with");
    assert!(
        margined < 2 * plain + Duration::from_millis(200),
        "{plain:?} without margins, {margined:?} with"
    );
}

/// `n` paragraphs holding only a picture inside 250 nested divs, each div with side margins
/// of a whole number `digits + 1` long (calibre keeps a whole number's digits, and rounds a
/// fraction to six places).
fn pictures_in_long_margined_divs(n: usize, digits: usize) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut at = body;
    for _ in 0..250 {
        at = dom.append_child(at, "div");
    }
    for _ in 0..n {
        let p = dom.append_child(at, "p");
        let img = dom.append_child(p, "img");
        dom.set_attr(img, "src", Some("a.png"));
    }
    let number = format!("1{}", "3".repeat(digits));
    file_of(
        dom,
        &format!("div {{ margin-left: {number}px; margin-right: {number}px }}"),
    )
}

/// A paragraph holding only a picture keeps each container's side margins as written, one
/// pair per container (`picture_boxes`), as kfxgen does. Each pair copied is charged by its
/// length, so long margins under many containers are refused after bounded work.
#[test]
fn long_margins_kept_for_pictures_in_many_divs_are_refused() {
    let got = extract(&pictures_in_long_margined_divs(1, 10)).unwrap();
    match got.blocks[0].get("picture_boxes") {
        Some(PyValue::Tuple(boxes)) => assert_eq!(boxes.len(), 250),
        other => panic!("{other:?}"),
    }
    let file = pictures_in_long_margined_divs(20, 3_200);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let none = HashSet::new();
    let opts = ExtractOptions {
        native_tables: true,
        toc_targets: &none,
        link_targets: &none,
        keep_nav_listings: false,
    };
    let start = Instant::now();
    let got = extract_blocks(&mut Walk::new(&file, &resolver, &mut serials), &opts);
    let took = start.elapsed();
    assert!(
        matches!(got, Err(ItemError::Error(Error::Book(_)))),
        "{:?}",
        got.map(|b| b.blocks.len())
    );
    assert!(took < COST_LIMIT, "{took:?}");
}

/// Spacer paragraphs between paragraphs: each is a space block the spacing pass takes out
/// while it builds the file's list, in one pass.
#[test]
fn spacers_between_paragraphs_are_spaced_in_linear_time() {
    let make = |n: usize| page(&"<p>x</p><p>\u{a0}</p>".repeat(n), "");
    allocates_linearly("spacers between paragraphs", 2_000, make);
    let got = extract(&make(3)).unwrap();
    assert_eq!(got.blocks.len(), 3);
}

/// Where kfxgen raises inside the walk (it leaves the spine item out), the port gives
/// `ItemError::Raised` with Python's exception: a box tag's width or side margin in `in`
/// spelt with U+0131 (`_box_width`, `_make_box` → `parse_css_length`), a paragraph's
/// `text-align` that calibre reads as a length (`compute_block_style`), and the rows below.
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
        // `_make_box` → `parse_css_length`: a box's own side margin.
        (
            "<div class=\"b\"><p>x</p></div>",
            ".b { border: 1px solid; margin-left: 2\u{131}n }",
            "KeyError",
        ),
        // `_carry_container_margins` → `parse_css_length`.
        (
            "<div class=\"m\"><p>x</p></div>",
            ".m { margin-left: 2\u{131}n }",
            "KeyError",
        ),
        // `_rule_look` → `parse_css_length`.
        (
            "<p>x</p><hr class=\"r\"/>",
            ".r { width: 2\u{131}n }",
            "KeyError",
        ),
        // `_spacer_em`: `round()` of an infinite line.
        (
            "<p>x</p><p class=\"s\">\u{a0}</p>",
            ".s { line-height: 1e400 }",
            "OverflowError",
        ),
        (
            "<p>x</p><p class=\"s\">\u{a0}</p>",
            // calibre drops `1e400px` as a size; digits past a float's range are infinite.
            concat!(
                ".s { font-size: 1",
                "00000000000000000000000000000000000000000000000000000000000000000000000000000",
                "00000000000000000000000000000000000000000000000000000000000000000000000000000",
                "00000000000000000000000000000000000000000000000000000000000000000000000000000",
                "00000000000000000000000000000000000000000000000000000000000000000000000000000",
                "px; line-height: normal }"
            ),
            "OverflowError",
        ),
    ] {
        match extract(&page(body, css)) {
            Err(ItemError::Raised(r)) => assert_eq!(r.exception, exception, "{css}"),
            other => panic!("{css}: {:?}", other.map(|b| b.blocks.len())),
        }
    }
}

/// `levels` lists nested in the body, `<list><li><list><li>…`, each item holding only the
/// next list, the last a word; the list elements get `attrs`.
fn nested_lists(
    levels: usize,
    list: &str,
    attrs: &[(&str, &str)],
    css: &str,
) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut n = body;
    for _ in 0..levels {
        n = dom.append_child(n, list);
        for (k, v) in attrs {
            dom.set_attr(n, k, Some(v));
        }
        n = dom.append_child(n, "li");
    }
    dom.set_text(n, "x");
    file_of(dom, css)
}

/// The text of the paragraph inside the innermost of nested boxes.
fn innermost_text(blocks: &[Block]) -> String {
    let mut b = blocks[0].to_py();
    loop {
        let PyValue::Dict(pairs) = &b else {
            panic!("not a block: {b:?}")
        };
        let get = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k.as_str() == Some(key))
                .map(|(_, v)| v)
        };
        match get("blocks") {
            Some(PyValue::List(kids)) => b = kids[0].clone(),
            _ => return get("text").and_then(PyValue::as_str).unwrap().to_owned(),
        }
    }
}

/// Native lists as deep as `read_book` lets a tree be: a box in a box at every level, spaced
/// and keyed with no recursion, on a test thread's stack.
#[test]
fn native_lists_as_deep_as_a_book_may_nest_are_walked() {
    let levels = (deckle_limits::MAX_DEPTH - 2) / 2;
    let got = extract(&nested_lists(levels, "ul", &[], "")).unwrap();
    assert_eq!(got.blocks.len(), 1);
    assert_eq!(got.blocks[0].get("type"), Some(&PyValue::Str("box".into())));
    assert_eq!(innermost_text(&got.blocks), "x");
}

/// `levels` of a `<div>` holding a native list whose last item holds the next level; the
/// innermost list has `inner` more items of text before that one. Every item has an id, so
/// anchor keys go into the boxes' items.
fn lists_in_divs(levels: usize, inner: usize, css: &str) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut n = body;
    let mut id = 0;
    let mut item = |dom: &mut deckle_kfxgen::dom::Dom, list| {
        let li = dom.append_child(list, "li");
        dom.set_attr(li, "id", Some(&format!("i{id}")));
        id += 1;
        li
    };
    for level in 0..levels {
        let div = dom.append_child(n, "div");
        let list = dom.append_child(div, "ul");
        if level + 1 == levels {
            for _ in 0..inner {
                let li = item(&mut dom, list);
                dom.set_text(li, "item");
            }
        }
        n = item(&mut dom, list);
    }
    dom.set_text(n, "item");
    file_of(dom, css)
}

/// What `extract` charges to the file's budget, allocates and takes on `file`.
fn cost_of(file: &deckle_kfxgen::book::File) -> (usize, u64, Duration) {
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
    let start = Instant::now();
    let (got, bytes) =
        allocated_by(|| extract_blocks(&mut Walk::new(file, &resolver, &mut serials), &opts));
    let took = start.elapsed();
    got.unwrap();
    assert!(took < COST_LIMIT, "{took:?}");
    (budget.charged(), bytes, took)
}

/// A finished list is one box: a margined container round it carries its side margins onto
/// the box, one step, not onto each item again. Each list round a nested one copies its
/// items' texts and ids into its own box, as kfxgen's model holds them, each copy charged:
/// twice the depth costs about twice as much, not four times. 84 levels of `<div><ul><li>`
/// are as deep as `read_book` lets a tree be.
#[test]
fn nested_lists_in_margined_containers_are_worked_once() {
    let levels = (deckle_limits::MAX_DEPTH - 2) / 3;
    let margined = "div { margin: 1px 2px 3px 4px }";
    let items = 1_000 - levels;
    let (plain, _, _) = cost_of(&lists_in_divs(levels, items, ""));
    let (deep, bytes, took) = cost_of(&lists_in_divs(levels, items, margined));
    eprintln!(
        "1,000 items in {levels} lists in margined divs: {deep} units ({plain} unmargined), \
         {bytes} bytes, {took:?}"
    );
    assert!(
        deep - plain <= 2 * levels,
        "{plain} units unmargined, {deep} margined"
    );
    let (half, half_bytes, _) = cost_of(&lists_in_divs(levels / 2, items, margined));
    eprintln!("in {}: {half} units, {half_bytes} bytes", levels / 2);
    assert!(
        deep < 3 * half,
        "{half} units, then {deep} on twice the depth"
    );
    assert!(bytes < 3 * half_bytes, "{half_bytes} bytes, then {bytes}");
}

/// Native lists nested one level past that: the block walk refuses them by name, on a test
/// thread's stack, before any box is spaced.
///
/// Far past it, in a `File` built by hand, each list first reads the start of its items'
/// text (`_list_settings`), down through every list below to the first word: those reads
/// are charged, and the file's budget refuses it after bounded work, before the walk is deep
/// enough for its own refusal.
#[test]
fn native_lists_nested_past_the_limit_are_refused_not_overflowed() {
    let refused = |levels: usize| {
        let file = nested_lists(levels, "ul", &[], "");
        let start = Instant::now();
        let got = extract(&file);
        let took = start.elapsed();
        assert!(took < COST_LIMIT, "{levels}: {took:?}");
        got
    };
    match refused(deckle_limits::MAX_DEPTH / 2 + 1) {
        Err(ItemError::Error(Error::TooLarge { what })) => assert!(
            what.ends_with("levels below the body"),
            "not the block walk's: {what}"
        ),
        other => panic!("{:?}", other.map(|b| b.blocks.len())),
    }
    let got = refused(10_000);
    assert!(
        matches!(got, Err(ItemError::Error(Error::Book(_)))),
        "{:?}",
        got.map(|b| b.blocks.len())
    );
}

/// Typed markers wait for the first text, however many items open before it: each item's
/// marker goes in front of the innermost item's text, the outermost first.
#[test]
fn typed_markers_of_items_that_hold_no_text_wait_for_the_next() {
    let levels = (deckle_limits::MAX_DEPTH - 2) / 2;
    // A `reversed` list is never native: it keeps typed markers. One item, numbered 1.
    let got = extract(&nested_lists(levels, "ol", &[("reversed", "")], "")).unwrap();
    assert_eq!(got.blocks.len(), 1);
    assert_eq!(
        got.blocks[0].get("text"),
        Some(&PyValue::Str(format!("{}x", "1. ".repeat(levels))))
    );
}

/// kfxgen copies the waiting markers before each item (`pending_markers[:]`); the port cuts
/// them back instead, so markers as long as a book's CSS makes them are not copied once per
/// level of nesting.
#[test]
fn waiting_markers_are_not_copied_for_each_item() {
    let marker = "m".repeat(10_000);
    let css = format!("ol {{ list-style-type: \"{marker}\" }}");
    let make = |levels: usize| nested_lists(levels, "ol", &[("reversed", "")], &css);
    let got = extract(&make(2)).unwrap();
    // `style[1:-1]` of the quoted string, twice, then the text.
    assert_eq!(
        got.blocks[0].get("text"),
        Some(&PyValue::Str(format!("{marker}{marker}x")))
    );
    // Each level resolves the inherited marker a few times (linear); a copy of the waiting
    // markers at each item adds the square of the depth: 2.7 times as much on twice the
    // depth, against 2.0 (allocation counts do not depend on the machine).
    let bytes = |levels: usize| {
        let file = make(levels);
        let (got, bytes) = allocated_by(|| extract(&file));
        got.unwrap();
        bytes
    };
    let (once, twice) = (bytes(63), bytes(126));
    assert!(
        twice * 10 < once * 24,
        "{once} bytes at 63 levels, then {twice} at 126"
    );
}

/// One list of many items: its ordinals, settings and box are worked out once each.
#[test]
fn a_long_list_takes_linear_time() {
    let make = |n: usize| page(&format!("<ol>{}</ol>", "<li>item</li>".repeat(n)), "");
    allocates_linearly("items of one list", 4_000, make);
    let make = |n: usize| {
        page(
            &format!("<ol reversed=\"\">{}</ol>", "<li>item</li>".repeat(n)),
            "",
        )
    };
    allocates_linearly("items of one list with typed markers", 4_000, make);
}

/// Whether a list's items type their own markers is read from the start of each item's
/// text, once for each list round the item: items that hold no text but many empty elements,
/// in lists nested deep, are read to their end each time, as kfxgen reads them. Each node
/// read is charged, so such a file is refused after bounded work.
#[test]
fn nested_lists_of_textless_items_are_refused_after_bounded_work() {
    let (mut dom, body) = empty_page();
    let mut n = body;
    for _ in 0..120 {
        n = dom.append_child(n, "ol");
        n = dom.append_child(n, "li");
        for _ in 0..2_000 {
            dom.append_child(n, "span");
        }
    }
    let file = file_of(dom, "");
    let start = Instant::now();
    let got = extract(&file);
    let took = start.elapsed();
    assert!(
        matches!(got, Err(ItemError::Error(Error::Book(_)))),
        "{:?}",
        got.map(|b| b.blocks.len())
    );
    assert!(took < COST_LIMIT, "{took:?}");
}

/// A list number the model cannot hold stops the file by name; one only read (a typed
/// marker its item spells out already) does not.
#[test]
fn list_numbers_past_what_the_port_holds() {
    let refused = |body: &str| match extract(&page(body, "")) {
        Err(ItemError::Error(Error::Unsupported { feature, .. })) => {
            assert_eq!(feature, "list numbers too large", "{body}")
        }
        other => panic!("{body}: {:?}", other.map(|b| b.blocks.len())),
    };
    let big = "9".repeat(20);
    let huge = "9".repeat(40);
    // A native list writes its start and each item's value and number.
    refused(&format!("<ol start=\"{big}\"><li>x</li></ol>"));
    refused(&format!("<ul start=\"{big}\"><li>x</li></ul>"));
    refused(&format!("<ol><li value=\"-{big}\">x</li></ol>"));
    // A typed marker is written in full within an i128, and refused past one.
    let got = extract(&page(
        &format!("<ol reversed=\"\" start=\"{big}\"><li>x</li></ol>"),
        "",
    ))
    .unwrap();
    assert_eq!(
        got.blocks[0].get("text"),
        Some(&PyValue::Str(format!("{big}. x")))
    );
    refused(&format!(
        "<ol reversed=\"\" start=\"{huge}\"><li>x</li></ol>"
    ));
    // Spelt out by hand already, past an i64, in letters: no marker, nothing to write.
    let body = "<ol type=\"a\" reversed=\"\" start=\"1744349715977154962390\">\
                <li>zzzzzzzzzzzzzzz. x</li></ol>";
    let got = extract(&page(body, "")).unwrap();
    assert_eq!(
        got.blocks[0].get("text"),
        Some(&PyValue::Str("zzzzzzzzzzzzzzz. x".into()))
    );
    // A marker no text takes (a picture alone) is never written.
    let body = format!("<ol reversed=\"\" start=\"{huge}\"><li><img src=\"a.png\"/></li></ol>");
    assert_eq!(extract(&page(&body, "")).unwrap().blocks.len(), 1);
}

/// Typed markers, on lists Previewer was not probed on: what the pinned `_list_marker`,
/// `_take_marker` and `_list_settings` give, read from the Python (no dump has these).
#[test]
fn typed_markers_follow_kfxgen_s_rules() {
    let css = ".b { display: block } .n { list-style-type: decimal } \
               .q { list-style-type: \"\u{2192} \" } .none { list-style-type: none }";
    let texts = |body: &str| -> Vec<String> {
        let got = extract(&page(body, css)).unwrap();
        got.blocks
            .iter()
            .map(|b| b.get("text").and_then(PyValue::as_str).unwrap().to_owned())
            .collect()
    };
    let want = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    // `reversed` counts down from the number of items.
    assert_eq!(
        texts("<ol reversed=\"\"><li>a</li><li>b</li></ol>"),
        want(&["2. a", "1. b"])
    );
    // Roman and letters stop at 1: 0 and below are decimal.
    assert_eq!(
        texts("<ol reversed=\"\" start=\"1\" type=\"i\"><li>a</li><li>b</li><li>c</li></ol>"),
        want(&["i. a", "0. b", "-1. c"])
    );
    assert_eq!(
        texts("<ol reversed=\"\" type=\"A\"><li value=\"28\">a</li><li>b</li></ol>"),
        want(&["AB. a", "AA. b"])
    );
    // An item set to display as a block shows no marker, and makes the list typed.
    assert_eq!(
        texts("<ul><li class=\"b\">a</li><li>b</li></ul>"),
        want(&["a", "\u{2022} b"])
    );
    // An item that types its own number or bullet keeps it, and makes the list typed.
    assert_eq!(
        texts("<ol reversed=\"\"><li>1. a</li></ol>"),
        want(&["1. a"])
    );
    assert_eq!(texts("<ul><li>\u{2022} a</li></ul>"), want(&["\u{2022} a"]));
    // Mixed styles: typed, each item numbered among the list's items.
    assert_eq!(
        texts("<ul><li>a</li><li class=\"n\">b</li></ul>"),
        want(&["\u{2022} a", "2. b"])
    );
    // A quoted string is the marker.
    assert_eq!(
        texts("<ul class=\"q\"><li>a</li></ul>"),
        want(&["\u{2192} a"])
    );
    // No markers in right-to-left text: plain paragraphs, no box.
    assert_eq!(
        texts("<ul class=\"none\" dir=\"rtl\"><li>a</li></ul>"),
        want(&["a"])
    );
    // A marker no text takes is dropped at the end of its item.
    assert_eq!(
        texts("<ol reversed=\"\"><li><img src=\"a.png\"/></li><li>b</li></ol>")[1],
        "1. b"
    );
}

/// The walk's blocks of `body` styled by `css`, before spacing.
fn before_spacing(body: &str, css: &str) -> Vec<Block> {
    let file = page(body, css);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let none = HashSet::new();
    let opts = ExtractOptions {
        native_tables: true,
        toc_targets: &none,
        link_targets: &none,
        keep_nav_listings: false,
    };
    blocks_before_spacing(&mut Walk::new(&file, &resolver, &mut serials), &opts)
        .unwrap()
        .blocks
}

/// What no dump holds: an `<hr>` that draws nothing is space (its margins and height as the
/// pinned `_hidden_rule_em` gives them for the style the port resolves), a closing rule's id
/// ends the file without naming a block after it, spacers after a closing rule are its
/// trailing space (the pinned `_space_between` on the same blocks gives 1.9125), and a margin
/// of `0` is a rule's own, not calibre's 0.5em.
#[test]
fn rules_no_dump_holds_are_written_as_kfxgen_writes_them() {
    let f = |x: f64| Some(PyValue::Float(x));
    let space = |b: &Block| {
        assert_eq!(b.get("type"), Some(&PyValue::Str("space".into())));
        ["margin", "line", "height", "margin_after"].map(|k| b.get(k).cloned())
    };
    let body = "<p>a</p><hr class=\"n\"/><p>b</p>";
    let got = before_spacing(body, ".n { border: none }");
    assert_eq!(space(&got[1]), [f(0.3), f(0.0), f(0.0), f(0.0)]);
    assert_eq!(
        extract(&page(body, ".n { border: none }"))
            .unwrap()
            .blocks
            .len(),
        2
    );
    let got = before_spacing(body, ".n { border: none; height: 4px; margin: 1em 0 2em }");
    assert_eq!(space(&got[1]), [f(0.6), f(0.0), f(0.15), f(1.2)]);

    let got = extract(&page("<p>a</p><hr id=\"r\"/>", "")).unwrap();
    assert_eq!(got.trailing_ids, Vec::<String>::new());
    assert_eq!(got.end_ids, ["r"]);
    let ids = |b: &Block| b.get("anchor_ids").cloned();
    assert_eq!(
        ids(&got.blocks[0]),
        Some(PyValue::List(vec![PyValue::Str("r".into())]))
    );
    assert_eq!(ids(&got.blocks[1]), Some(PyValue::List(Vec::new())));

    let got = extract(&page("<p>a</p><hr/><p>\u{a0}</p>", "")).unwrap();
    let rule = &got.blocks[1];
    assert_eq!(rule.get("trailing_space").cloned(), f(1.9125));
    let above = |b: &Block| b.get("rule").and_then(|r| r.get("space_above")).cloned();
    assert_eq!(above(rule), f(1.0));

    let got = before_spacing("<p>a</p><hr class=\"m\"/><p>b</p>", ".m { margin: 0 }");
    let below = |b: &Block| b.get("rule").and_then(|r| r.get("space_below")).cloned();
    assert_eq!((above(&got[1]), below(&got[1])), (f(0.0), f(0.0)));
}

/// Rules one after another, each with an id, every other one drawing nothing: each is read
/// once, its ids wait for the paragraph after them all, and the space between is linear.
#[test]
fn rules_in_a_row_take_linear_time() {
    let make = |n: usize| {
        let mut body = String::new();
        for i in 0..n {
            let class = if i % 2 == 0 { "" } else { " class=\"n\"" };
            body.push_str(&format!("<hr id=\"r{i}\"{class}/>"));
        }
        body.push_str("<p>after</p>");
        page(&body, ".n { border: none }")
    };
    allocates_linearly("rules", 3_000, make);
    let got = extract(&make(4)).unwrap();
    assert_eq!(got.blocks.len(), 3);
    let ids = got.blocks[2].get("anchor_ids").unwrap();
    assert_eq!(ids.as_list().map(<[PyValue]>::len), Some(4));
}

/// The walk of `body` styled by `css`, before spacing, with the margins noted beside its
/// blocks, `toc` the ids contents entries name.
fn walked_with(body: &str, css: &str, toc: &[&str]) -> BeforeSpacing {
    let file = page(body, css);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let toc: HashSet<String> = toc.iter().map(|t| (*t).to_owned()).collect();
    let none = HashSet::new();
    let opts = ExtractOptions {
        native_tables: true,
        toc_targets: &toc,
        link_targets: &none,
        keep_nav_listings: false,
    };
    blocks_before_spacing(&mut Walk::new(&file, &resolver, &mut serials), &opts).unwrap()
}

/// `repr` of a value the model holds, for the box values below.
fn show(v: Option<&PyValue>) -> String {
    match v {
        None => "absent".into(),
        Some(PyValue::None) => "None".into(),
        Some(PyValue::Bool(b)) => if *b { "True" } else { "False" }.into(),
        Some(PyValue::Float(f)) => format!("{f:?}"),
        Some(PyValue::Str(s)) => format!("'{s}'"),
        Some(PyValue::Tuple(items)) => {
            let inner: Vec<String> = items.iter().map(|i| show(Some(i))).collect();
            format!("({})", inner.join(", "))
        }
        Some(PyValue::Dict(pairs)) => {
            let inner: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", show(Some(k)), show(Some(v))))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Some(other) => format!("{other:?}"),
    }
}

/// A box block's kids, as the model holds them.
fn kids(b: &Block) -> Vec<PyValue> {
    match b.to_py().get("blocks") {
        Some(PyValue::List(kids)) => kids.clone(),
        other => panic!("not a box: {other:?}"),
    }
}

/// What no dump holds, each value read from the pinned `_make_box`, `_note_box_margins`,
/// `_note_padding`, `_note_margins`, `_carry_container_margins` and `_space_between` for the
/// style the port resolves (not run: they are closures of `extract_blocks_from_html`, which
/// needs Calibre's resolver). The trailing space 1.9125 is the pinned `_space_between`'s on
/// a rule and a spacer, as in `rules_no_dump_holds_are_written_as_kfxgen_writes_them`.
#[test]
fn boxes_no_dump_holds_are_made_as_kfxgen_makes_them() {
    let bx = |b: &Block, key: &str| show(b.get("box").and_then(|d| d.get(key)));
    let keys = |b: &Block| match b.to_py() {
        PyValue::Dict(pairs) => pairs
            .iter()
            .map(|(k, _)| k.as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        _ => unreachable!(),
    };

    // A clear set on the container's first block moves to the box, after `break_before`.
    let got = walked_with(
        "<div class=\"b\"><p class=\"c\">x</p></div>",
        ".b { border: 1px solid } .c { clear: left }",
        &[],
    );
    let b = &got.blocks[0];
    assert_eq!(keys(b)[..4], ["type", "break_before", "clear", "text"]);
    assert_eq!(show(b.get("clear")), "'left'");
    assert_eq!(show(kids(b)[0].get("clear")), "absent");

    // A contents entry naming a block past the first keeps the blocks flat; one naming the
    // first does not.
    let body = "<div class=\"b\"><p id=\"f\">x</p><p id=\"t\">y</p></div>";
    let css = ".b { border: 1px solid }";
    assert_eq!(walked_with(body, css, &["t"]).blocks.len(), 2);
    assert_eq!(walked_with(body, css, &["f"]).blocks.len(), 1);

    // A leaf box: the paragraph's side margins and its `set_here` go to the box.
    let got = walked_with(
        "<p class=\"b\" dir=\"rtl\">x</p>",
        ".b { border: 1px solid; margin-left: 2em }",
        &[],
    );
    let b = &got.blocks[0];
    assert_eq!(bx(b, "margin_left"), "6.25");
    assert_eq!(bx(b, "margin_right"), "None");
    let kid = &kids(b)[0];
    let style = kid.get("block_style");
    assert_eq!(show(style.and_then(|s| s.get("margin_left"))), "None");
    assert_eq!(show(style.and_then(|s| s.get("margin_right"))), "None");
    let bidi = style.and_then(|s| s.get("bidi"));
    assert_eq!(show(bidi.and_then(|d| d.get("direction"))), "'rtl'");
    assert_eq!(show(bidi.and_then(|d| d.get("set_here"))), "absent");
    assert_eq!(
        show(
            b.get("box")
                .and_then(|d| d.get("bidi"))
                .and_then(|d| d.get("set_here"))
        ),
        "('direction', 'unicode_bidi')"
    );

    // A padding in % stays one, and stops the margin inside from meeting the box's on that
    // side.
    let got = walked_with(
        "<div class=\"b\"><p>x</p></div>",
        ".b { border-left: 1px solid; padding: 5% 0 0 0; margin: 1em 0 }",
        &[],
    );
    let b = &got.blocks[0];
    assert_eq!(
        bx(b, "padding"),
        "{'top': ('%', 5.0), 'right': 0.0, 'bottom': 0.0, 'left': 0.0}"
    );
    // The paragraph's margin-bottom (calibre's 1em) met the box's; its top one did not.
    assert_eq!(got.margins.get(&b.id()), Some(&[1.0, 1.0]));
    assert_eq!(got.margins.len(), 2);
    assert!(
        got.margins.values().any(|m| *m == [1.0, 0.0]),
        "{:?}",
        got.margins
    );

    // A vertical box: the margins of the blocks inside meet its own right (`start`) and left
    // (`end`), not its top and bottom.
    let got = walked_with(
        "<div data-kfxgen-vertical=\"1\" class=\"v\"><p class=\"m\">x</p></div>",
        ".v { margin-left: 3em; margin-right: 0.5em } .m { margin: 1em 0 2em }",
        &[],
    );
    let b = &got.blocks[0];
    assert_eq!(
        (bx(b, "vertical"), bx(b, "start"), bx(b, "end")),
        ("True".into(), "1.0".into(), "3.0".into())
    );
    assert_eq!(
        (bx(b, "margin_left"), bx(b, "margin_right")),
        ("None".into(), "None".into())
    );
    assert_eq!(got.margins.get(&b.id()), Some(&[0.0, 0.0]));
    assert_eq!(
        got.margins.values().filter(|m| **m == [0.0, 0.0]).count(),
        got.margins.len()
    );

    // A spacer after the last block meets its margin-bottom first: it stays the block's.
    let got = walked_with(
        "<div class=\"b\"><p class=\"m\">x</p><p>\u{a0}</p></div>",
        ".b { background-color: #eee } .m { margin-bottom: 2em }",
        &[],
    );
    let b = &got.blocks[0];
    assert_eq!(got.margins.get(&b.id()), Some(&[1.0, 0.0]));
    assert!(
        got.margins.values().any(|m| *m == [0.0, 2.0]),
        "{:?}",
        got.margins
    );

    // A horizontal block in two padded wrappers: the second padding is added to the first.
    let got = walked_with(
        "<div class=\"o\"><div class=\"i\"><div data-kfxgen-horizontal=\"1\">x</div></div></div>",
        ".i { padding-top: 1em } .o { padding-top: 2em }",
        &[],
    );
    let b = &got.blocks[0];
    assert_eq!(
        (
            bx(b, "before_inner"),
            bx(b, "before_fixed"),
            bx(b, "before")
        ),
        ("0.0".into(), "3.0".into(), "0.0".into())
    );
    // And in two wrappers with margins: they meet.
    let got = walked_with(
        "<div class=\"o\"><div class=\"i\"><div data-kfxgen-horizontal=\"1\">x</div></div></div>",
        ".i { margin-top: 1em } .o { margin-top: 2em }",
        &[],
    );
    assert_eq!(bx(&got.blocks[0], "before"), "2.0");

    // A margined container round a box adds its side margins to the box's.
    let got = walked_with(
        "<div class=\"m\"><div class=\"b\">x</div></div>",
        ".m { margin-left: 2em; margin-right: 10% } .b { border: 1px solid; margin-left: 1em }",
        &[],
    );
    let b = &got.blocks[0];
    assert_eq!(
        (bx(b, "margin_left"), bx(b, "margin_right")),
        ("9.375".into(), "10.0".into())
    );

    // A bordered container of nothing but a spacer is no box.
    let got = walked_with(
        "<div class=\"b\"><p>\u{a0}</p></div>",
        ".b { border: 1px solid }",
        &[],
    );
    assert_eq!(got.blocks.len(), 1);
    assert_eq!(show(got.blocks[0].get("type")), "'space'");

    // Spacers after a rule that ends a bordered box: in its padding, below the rule.
    let got = extract(&page(
        "<div class=\"b\"><p>x</p><hr/><p>\u{a0}</p></div>",
        ".b { border: 1px solid }",
    ))
    .unwrap();
    let b = &got.blocks[0];
    assert_eq!(
        bx(b, "padding"),
        "{'top': 0.0, 'right': 0.0, 'bottom': 1.9125, 'left': 0.0}"
    );
    assert_eq!(show(kids(b)[1].get("trailing_space")), "absent");
    // In a vertical box with no edge: after it, along its axis, meeting what its last
    // paragraph's margin left there.
    let got = extract(&page(
        "<div data-kfxgen-vertical=\"1\"><p>x</p><hr/><p>\u{a0}</p></div>",
        "",
    ))
    .unwrap();
    let b = &got.blocks[0];
    assert_eq!(
        (bx(b, "start"), bx(b, "end")),
        ("1.0".into(), "1.9125".into())
    );
}

/// `deep` bordered `<div>`s nested in the body, a paragraph at the bottom: a box in a box at
/// every level.
fn nested_boxes(deep: usize) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut n = body;
    for _ in 0..deep {
        n = dom.append_child(n, "div");
    }
    let p = dom.append_child(n, "p");
    dom.set_text(p, "x");
    file_of(dom, "div { border: 1px solid; margin: 1em 0 }")
}

/// Bordered boxes as deep as `read_book` lets a tree be: made, spaced and keyed with no
/// recursion, on a test thread's stack. (kfxgen itself raises `RecursionError` at about 249
/// nested divs: DIVERGENCES, "The block walk".)
#[test]
fn bordered_boxes_as_deep_as_a_book_may_nest_are_walked() {
    let deep = deckle_limits::MAX_DEPTH - 2;
    let got = extract(&nested_boxes(deep)).unwrap();
    assert_eq!(got.blocks.len(), 1);
    assert_eq!(got.blocks[0].get("type"), Some(&PyValue::Str("box".into())));
    assert_eq!(innermost_text(&got.blocks), "x");
    let mut levels = 0;
    let mut b = got.blocks[0].to_py();
    while let Some(PyValue::List(kids)) = b.get("blocks") {
        levels += 1;
        b = kids[0].clone();
    }
    assert_eq!(levels, deep);
}

/// Bordered boxes nested one level past that, and 20,000 levels in a `File` built by hand:
/// the block walk refuses them by name before any box is made (each is made at its element's
/// end), within the time limit.
#[test]
fn bordered_boxes_nested_past_the_limit_are_refused_not_overflowed() {
    for deep in [deckle_limits::MAX_DEPTH, 20_000] {
        let file = nested_boxes(deep);
        let start = Instant::now();
        let got = extract(&file);
        let took = start.elapsed();
        match &got {
            Err(ItemError::Error(Error::TooLarge { what })) => assert!(
                what.ends_with("levels below the body"),
                "not the block walk's: {what}"
            ),
            other => panic!("{deep}: {:?}", other.as_ref().map(|b| b.blocks.len())),
        }
        assert!(took < COST_LIMIT, "{deep}: {took:?}");
    }
}

/// `levels` of a `<div class="m">` holding a `<div class="b">`, the innermost holding `n`
/// paragraphs, each with an id.
fn boxes_in_divs(levels: usize, n: usize, css: &str) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut at = body;
    for _ in 0..levels {
        at = dom.append_child(at, "div");
        dom.set_attr(at, "class", Some("m"));
        at = dom.append_child(at, "div");
        dom.set_attr(at, "class", Some("b"));
    }
    for i in 0..n {
        let p = dom.append_child(at, "p");
        dom.set_attr(p, "id", Some(&format!("p{i}")));
        dom.set_text(p, "item");
    }
    file_of(dom, css)
}

/// A finished box is one block: a margined container round it carries its side margins onto
/// the box, one step, not onto each paragraph again, and the next box round it looks at one
/// kid. Each box copies its kids' texts and ids into its own, as kfxgen's model holds them,
/// each copy charged: twice the depth costs about twice as much, not four times. 127 levels
/// of `<div class="m"><div class="b">` are as deep as the walk goes.
#[test]
fn nested_boxes_in_margined_containers_are_worked_once() {
    let levels = (deckle_limits::MAX_DEPTH - 2) / 2;
    let boxed = ".b { border: 1px solid }";
    let margined = ".b { border: 1px solid } .m { margin: 1px 2px 3px 4px }";
    let n = 1_000;
    let (plain, _, _) = cost_of(&boxes_in_divs(levels, n, boxed));
    let (deep, bytes, took) = cost_of(&boxes_in_divs(levels, n, margined));
    eprintln!(
        "{n} paragraphs in {levels} boxes in margined divs: {deep} units ({plain} unmargined), \
         {bytes} bytes, {took:?}"
    );
    assert!(
        deep - plain <= 2 * levels,
        "{plain} units unmargined, {deep} margined"
    );
    let (half, half_bytes, half_took) = cost_of(&boxes_in_divs(levels / 2, n, margined));
    eprintln!(
        "in {}: {half} units, {half_bytes} bytes, {half_took:?}",
        levels / 2
    );
    assert!(
        deep < 3 * half,
        "{half} units, then {deep} on twice the depth"
    );
    assert!(bytes < 3 * half_bytes, "{half_bytes} bytes, then {bytes}");
}

/// One long paragraph in 250 nested `<div>`s, each with a width or none.
fn long_paragraph_in_sized_divs(sized: bool) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let mut at = body;
    for _ in 0..250 {
        at = dom.append_child(at, "div");
    }
    let p = dom.append_child(at, "p");
    dom.set_text(p, &"word ".repeat(2 << 20));
    file_of(
        dom,
        if sized {
            "div { width: 50% }"
        } else {
            "div { }"
        },
    )
}

/// kfxgen looks for a picture in the whole text of every block a sized `<div>` made
/// (`_IMG_TOKEN_RE.search` in `_make_box`). With none there is no box, so the paragraph is
/// looked at again by each sized div round it; what is in its text is read once here: a
/// 10 MB paragraph under 250 sized divs costs about what it costs under 250 plain ones.
#[test]
fn a_long_paragraph_in_nested_sized_divs_is_read_once() {
    let time = |sized: bool| {
        let file = long_paragraph_in_sized_divs(sized);
        let start = Instant::now();
        let got = extract(&file);
        let took = start.elapsed();
        assert_eq!(got.unwrap().blocks.len(), 1);
        took
    };
    let (plain, sized) = (time(false), time(true));
    eprintln!("10 MB paragraph in 250 divs: {plain:?} without a width, {sized:?} with");
    assert!(
        sized < 2 * plain + Duration::from_millis(200),
        "{plain:?} without a width, {sized:?} with"
    );
}

/// Widths, paddings, margins, borders and font sizes past what a float holds, negative, NaN
/// or over 100%, on bordered, sized and turned boxes: each page is walked and spaced without a
/// panic (kfxgen's answer for each is the dump-tested arithmetic on the same values).
#[test]
fn odd_box_values_do_not_panic() {
    let huge = "9".repeat(400);
    let values = [
        format!("width: {huge}%"),
        format!("width: {huge}px"),
        "width: -5%".to_owned(),
        "width: 250%".to_owned(),
        "width: 0.0000000000000000000000000000001px".to_owned(),
        format!("padding: {huge}px {huge}em {huge}% 0"),
        format!("margin: {huge}em {huge}% -{huge}px {huge}%"),
        format!("margin-left: {huge}%; margin-right: -{huge}%"),
        format!("border: {huge}px solid red"),
        "border: 1px solid rgb(300, -1, 2)".to_owned(),
        format!("font-size: {huge}px; padding: 1em; margin: 1em"),
        "font-size: 0.0001px; padding: 1em; margin: 1em".to_owned(),
        "float: left; width: 50%; margin: auto".to_owned(),
    ];
    for v in &values {
        for body in [
            "<div class=\"b\"><p>x</p><img src=\"a.png\"/></div>",
            "<p class=\"b\">x</p>",
            "<div class=\"m\"><div class=\"b\" data-kfxgen-vertical=\"1\"><p>x</p></div></div>",
            "<div class=\"m\"><div class=\"b\" data-kfxgen-horizontal=\"1\">x</div></div>",
        ] {
            let css = format!(".b {{ border: 1px solid; {v} }} .m {{ {v} }}");
            let _ = extract(&page(body, &css));
        }
    }
}
