// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's inline walk, `_walk_inline` (phase 3 plan, Task 9), against the run dumps
//! (`fixtures/kfxgen/convert/<name>/inline/NNN.json.gz`), and what it refuses.

use std::collections::BTreeMap;
use std::time::Instant;

use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::convert::{
    Error, ItemError, Part, Resolver, Segment, Serials, TextBudget, Walk,
    normalize_runs_with_anchors,
};
use deckle_kfxgen::css::DEFAULT_PROFILE;
use deckle_kfxgen::dom::NodeId;
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_resolver::{empty_page, file_of, page, sources};
use crate::convert_support::{dump, model_diff};
use crate::support::{COST_LIMIT, find};

/// Calls whose runs cannot match yet, with why: (source, NNN, why). Empty is the goal.
const SKIP: &[(&str, &str, &str)] = &[];

/// Task 10 writes a picture's token: until then the walk stops at a visible `<img>` or `<svg>`
/// with this feature, and the call is skipped. Only these sources have such calls.
const PICTURES: &str = "pictures in text";
const PICTURE_SOURCES: [&str; 12] = [
    "block_picture_769",
    "block_picture_770",
    "css_quirks",
    "figures",
    "floats",
    "floats_697",
    "frames",
    "horizontal_blocks",
    "horizontal_blocks_ltr",
    "pictures_more",
    "pictures_side_by_side_766",
    "vertical_pictures_761",
];

/// The walk's parts as the dump writes `segments`: (text, frozenset) tuples, the tables left
/// out (the hook drops `(_TABLE_SPLIT, table)`).
fn segments(parts: &[Part]) -> Vec<Segment> {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Run(s) => Some(s.clone()),
            Part::Table(_) => None,
        })
        .collect()
}

fn segments_py(segs: &[Segment]) -> PyValue {
    PyValue::List(
        segs.iter()
            .map(|s| PyValue::Tuple(vec![PyValue::Str(s.text.clone()), s.flags.to_py()]))
            .collect(),
    )
}

/// `normalize_runs_with_anchors(segments)` as the dump writes it.
fn normalized_py(segs: &[Segment]) -> PyValue {
    let (text, spans, anchors) = normalize_runs_with_anchors(segs);
    PyValue::Tuple(vec![
        PyValue::Str(text),
        PyValue::List(spans.iter().map(|s| s.to_py()).collect()),
        PyValue::Dict(
            anchors
                .into_iter()
                .map(|(k, v)| (PyValue::Str(k), PyValue::Int(v as i64)))
                .collect(),
        ),
    ])
}

/// The smallest serial of each kind (ruby, tcy, turn, bidi) among a dumped call's flags.
fn first_serials(segments: &PyValue) -> [Option<i64>; 4] {
    let mut first = [None; 4];
    let mut seen = |kind: usize, serial: i64| {
        let slot: &mut Option<i64> = &mut first[kind];
        *slot = Some(slot.map_or(serial, |s| s.min(serial)));
    };
    for seg in segments.as_list().unwrap() {
        let [_, PyValue::Set(flags)] = seg.as_list().unwrap() else {
            panic!("a segment of other than (text, set)");
        };
        for flag in flags {
            let Some([PyValue::Str(kind), value]) = flag.as_list() else {
                continue;
            };
            let at = ["ruby", "tcy", "turn", "bidi"]
                .iter()
                .position(|k| k == kind);
            if let (Some(at), Some(serial)) = (at, value.as_list().and_then(|v| v[0].as_int())) {
                seen(at, serial);
            }
        }
    }
    first
}

/// The counter of serial kind `kind` (ruby, tcy, turn, bidi).
fn counter(s: &mut Serials, kind: usize) -> &mut i64 {
    match kind {
        0 => &mut s.ruby,
        1 => &mut s.tcy,
        2 => &mut s.turn,
        _ => &mut s.bidi,
    }
}

/// For each `_walk_inline` call a block made in the first pass (and the second, where the
/// book has one), in call order: the walk of the element at the dumped path gives the dumped
/// segments, and normalizing them gives the dumped text, spans and anchors.
///
/// Both of kfxgen's calls with `is_root` true (`_table_cell`, and a leaf block in
/// `extract_blocks_from_html._walk`) pass no flags, no `split_tables` and no
/// `parent_emphasis`, and the file's href as `base_href`, so the path is all a call needs.
/// The serials run on across the book; the block walk's own calls with `is_root` false
/// (which the dump does not record) can use some between two recorded calls, so a counter
/// behind a call's smallest dumped serial is moved up to it, and each such move is counted.
#[test]
fn runs_match_the_dump_for_every_paragraph() {
    let names = sources();
    assert_eq!(names.len(), 55, "the source fixtures");
    let mut failures = Vec::new();
    let (mut files_compared, mut calls, mut skipped, mut pictures) = (0, 0, 0, 0);
    let mut jumps = Vec::new();
    for name in &names {
        let book = read_book(&fixtures::kfxgen(&format!("sources/{name}.epub")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let listed: BTreeMap<String, String> = serde_json::from_slice(
            &std::fs::read(fixtures::kfxgen(&format!("convert/{name}/files.json"))).unwrap(),
        )
        .unwrap();
        let mut serials = Serials::default();
        for pass in ["", ".pass1"] {
            for file in &book.files {
                let nnn = &listed[&file.href];
                let at = fixtures::kfxgen(&format!("convert/{name}/inline/{nnn}{pass}.json.gz"));
                if !at.exists() {
                    continue;
                }
                let PyValue::List(records) = dump(name, &format!("inline/{nnn}{pass}.json.gz"))
                else {
                    panic!("{name} {nnn}{pass}: no list");
                };
                let budget = TextBudget::new();
                let resolver = Resolver::new(file, &DEFAULT_PROFILE, &budget);
                let mut walk = Walk::new(file, &resolver, &mut serials);
                for record in &records {
                    let [path, segs, normalized] = record.as_list().unwrap() else {
                        panic!("{name} {nnn}: a record of other than three items");
                    };
                    let path: Vec<u32> = path
                        .as_list()
                        .unwrap()
                        .iter()
                        .map(|i| i.as_int().unwrap() as u32)
                        .collect();
                    let where_ = format!("{name} {nnn}{pass} {path:?}");
                    let Some(n) = find(&file.dom, &path) else {
                        failures.push(format!("{where_}: no such element"));
                        continue;
                    };
                    if SKIP.iter().any(|(s, f, _)| s == name && f == nnn) {
                        skipped += 1;
                        continue;
                    }
                    for (kind, first) in first_serials(segs).into_iter().enumerate() {
                        let counter = counter(walk.serials(), kind);
                        if let Some(first) = first
                            && first > *counter
                        {
                            jumps.push(format!("{where_}: serial {kind} {} -> {first}", *counter));
                            *counter = first;
                        }
                    }
                    calls += 1;
                    let ours = match walk.inline(n, false) {
                        Ok(parts) => segments(&parts),
                        Err(ItemError::Error(Error::Unsupported {
                            feature: PICTURES, ..
                        })) if PICTURE_SOURCES.contains(&name.as_str()) => {
                            calls -= 1;
                            pictures += 1;
                            continue;
                        }
                        Err(e) => {
                            failures.push(format!("{where_}: {e:?}"));
                            continue;
                        }
                    };
                    for line in model_diff(&segments_py(&ours), segs) {
                        failures.push(format!("{where_} segments{line}"));
                    }
                    for line in model_diff(&normalized_py(&ours), normalized) {
                        failures.push(format!("{where_} normalized{line}"));
                    }
                }
                files_compared += 1;
            }
        }
    }
    eprintln!(
        "{files_compared} files, {calls} calls compared, {skipped} skipped, {pictures} reach a \
         picture (Task 10); serial moves: {}",
        jumps.len()
    );
    for j in &jumps {
        eprintln!("  {j}");
    }
    assert!(
        failures.is_empty(),
        "{}\n{} differences",
        failures[..failures.len().min(80)].join("\n"),
        failures.len()
    );
    assert_eq!(files_compared, 188);
    assert_eq!(pictures, 84, "the calls Task 10 makes pass");
    assert_eq!(calls + skipped + pictures, 1862);
}

/// The paragraph of a page, and the page.
fn first(file: &deckle_kfxgen::book::File, tag: &str) -> NodeId {
    file.dom
        .descendants()
        .find(|&n| file.dom.tag(n) == tag)
        .unwrap()
}

#[test]
fn math_is_refused_by_name() {
    let file = page(
        "<p>x <math xmlns=\"http://www.w3.org/1998/Math/MathML\"><mi>y</mi></math></p>",
        "",
    );
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"), false);
    match got {
        Err(ItemError::Error(Error::Unsupported { feature, file })) => {
            assert_eq!(feature, "math");
            assert_eq!(file, "OEBPS/a.xhtml");
        }
        other => panic!("{other:?}"),
    }
}

/// A paragraph of `deep` nested spans, the innermost holding "x".
fn nested_spans(deep: usize) -> deckle_kfxgen::book::File {
    let (mut dom, body) = empty_page();
    let p = dom.append_child(body, "p");
    let mut n = p;
    for _ in 0..deep {
        n = dom.append_child(n, "span");
    }
    dom.set_text(n, "x");
    file_of(dom, "span { font-weight: bold }")
}

/// `read_book` refuses a tree deeper than `MAX_DEPTH`, but a `File` can be built by hand: the
/// walk refuses it rather than recursing 20,000 times (kfxgen's Python would hit its recursion
/// limit). It runs on a test thread's stack.
#[test]
fn a_span_nested_20000_deep_is_refused_not_overflowed() {
    let file = nested_spans(20_000);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let start = Instant::now();
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"), false);
    let took = start.elapsed();
    assert!(
        matches!(got, Err(ItemError::Error(Error::TooLarge { .. }))),
        "{got:?}"
    );
    assert!(took < COST_LIMIT, "{took:?}");
}

/// As deep as `read_book` lets a tree be, the walk gives every level on a test thread's stack.
#[test]
fn spans_as_deep_as_a_book_may_nest_are_walked() {
    let deep = deckle_limits::MAX_DEPTH - 2; // under <html><body><p>
    let file = nested_spans(deep);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let parts = Walk::new(&file, &resolver, &mut serials)
        .inline(first(&file, "p"), false)
        .unwrap();
    let segs = segments(&parts);
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].text, "x");
    assert!(segs[0].flags.has("bold"));
}

/// A flag as the tests below write it: a string flag as it is, a pair as `kind=value`.
fn show_flag(flag: &PyValue) -> String {
    fn value(v: &PyValue) -> String {
        match v {
            PyValue::Str(s) => s.clone(),
            PyValue::Int(i) => i.to_string(),
            PyValue::None => "None".into(),
            PyValue::Tuple(items) => format!(
                "({})",
                items.iter().map(value).collect::<Vec<_>>().join(",")
            ),
            other => format!("{other:?}"),
        }
    }
    match flag {
        PyValue::Tuple(kv) => format!("{}={}", value(&kv[0]), value(&kv[1])),
        other => value(other),
    }
}

/// Each part as `text|flag flag`, a table as `TABLE`.
fn show(parts: &[Part]) -> Vec<String> {
    parts
        .iter()
        .map(|p| match p {
            Part::Run(s) => {
                let PyValue::Set(flags) = s.flags.to_py() else {
                    unreachable!()
                };
                let flags: Vec<String> = flags.iter().map(show_flag).collect();
                format!("{}|{}", s.text, flags.join(" "))
            }
            Part::Table(_) => "TABLE".into(),
        })
        .collect()
}

/// Every `<p>`, `<td>` and `<pre>` of a page walked in turn, with `split_tables`.
fn walk_page(body: &str, css: &str) -> Vec<Result<Vec<String>, String>> {
    let file = page(body, css);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let mut walk = Walk::new(&file, &resolver, &mut serials);
    let blocks: Vec<NodeId> = file
        .dom
        .descendants()
        .filter(|&n| matches!(file.dom.tag(n), "p" | "td" | "pre"))
        .collect();
    blocks
        .into_iter()
        .map(|n| match walk.inline(n, true) {
            Ok(parts) => Ok(show(&parts)),
            Err(ItemError::Raised(r)) => Err(r.exception.to_owned()),
            Err(e) => Err(format!("{e:?}")),
        })
        .collect()
}

/// No dump has a `<q>`. Expected values from the pinned `_walk_inline`, run under Python 3.14
/// with lxml over these pages, each element given the style the port's resolver gives it.
#[test]
fn quote_marks_are_kfxgens() {
    let got = walk_page(
        "<p>He said <q>hi <q>there</q> you</q>.</p>\
         <p lang=\"fr\">Il dit <q>oui <q>non</q></q> <q lang=\"de\">ja</q> <q lang=\"ZU\">x</q></p>\
         <p><q class=\"a\">A</q><q class=\"b\" title=\"T\">B</q><q class=\"c\">C</q><q class=\"d\">D</q></p>\
         <p><q class=\"e\">E<q class=\"e\">F</q></q><q class=\"f\">G</q></p>",
        "q.a { quotes: '<' '>' '{' '}' } q.b::before { content: attr(title) ' ' open-quote } \
         q.b::after { content: no-close-quote } q.c { quotes: none } \
         q.d::before { content: \"\\201C\" } q.d::after { content: close-quote } \
         q.e::before { content: open-quote } q.e::after { content: none } \
         q.f::before { content: normal } q.f { quotes: \"\\00AB \" \"\\BB\" }",
    );
    let want: &[&[&str]] = &[
        &[
            "He said |",
            "\"|",
            "hi |",
            "'|",
            "there|",
            "'|",
            " you|",
            "\"|",
            ".|",
        ],
        &[
            "Il dit |",
            "«|",
            "oui |",
            "«|",
            "non|",
            "»|",
            "»|",
            " |",
            "„|lang=de",
            "ja|lang=de",
            "“|lang=de",
            " |",
            "“|lang=zu",
            "x|lang=zu",
            "”|lang=zu",
        ],
        &[
            "<|", "A|", ">|", "T |", "B|", "|", "|", "C|", "|", "“|", "D|", "|",
        ],
        &["\"|", "E|", "'|", "F|", "|", "|", "«|", "G|", "»|"],
    ];
    let got: Vec<Vec<String>> = got.into_iter().map(Result::unwrap).collect();
    assert_eq!(got, want);
}

/// kfxgen raises in `_q_marks` on an `attr()` lxml refuses and a CSS escape past U+10FFFF.
#[test]
fn quote_marks_kfxgen_raises_on_are_raised() {
    let empty = walk_page(
        "<p><q class=\"a\">x</q></p>",
        "q.a::before { content: attr() }",
    );
    assert_eq!(empty, [Err("ValueError".to_owned())]);
    let past = walk_page(
        "<p><q class=\"a\">x</q></p>",
        "q.a { quotes: \"\\110000\" \"b\" }",
    );
    assert_eq!(past, [Err("ValueError".to_owned())]);
}

/// Hidden content, `epub:switch`, white space and `<br>`. Expected values as in
/// `quote_marks_are_kfxgens`.
#[test]
fn hidden_switches_and_white_space_are_kfxgens() {
    let got = walk_page(
        "<p>x<epub:switch xmlns:epub=\"http://www.idpf.org/2007/ops\" id=\"sw\">\
         <epub:case required-namespace=\"m\"><span id=\"c1\">case</span></epub:case>\
         <epub:default><b id=\"d1\">dflt</b></epub:default></epub:switch>y</p>\
         <p>a<span hidden=\"\" id=\"h1\">hid<a id=\"h2\">den</a></span>b<span class=\"none\" id=\"h3\">n</span>c\
         <img src=\"x.png\" hidden=\"\" id=\"i1\"/>d<br id=\"b1\"/>e</p>\
         <p><span class=\"pw\">a  b<br/>c</span> and <span class=\"pre\">\n x\ty </span></p>\
         <pre>\nline1\n  line2</pre><pre>\r\nwin</pre>\
         <table><tr><td>a<p>b</p>c</td><td>d<select><option>o1</option><option>o2</option></select></td></tr></table>\
         <p>x<span class=\"blk\">line</span>y<table><tr><td>t</td></tr></table></p>",
        ".none { display: none } .pw { white-space: pre-wrap } .pre { white-space: pre-line } \
         .blk { display: block }",
    );
    let want: &[&[&str]] = &[
        &[
            "x|",
            "|anchor-mark=sw",
            "|anchor-mark=c1",
            "|anchor-mark=d1",
            "dflt|bold",
            "y|",
        ],
        &[
            "a|",
            "|anchor-mark=h1",
            "|anchor-mark=h2",
            "b|",
            "|anchor-mark=h3",
            "c|",
            "|anchor-mark=i1",
            "d|",
            "|anchor-mark=b1",
            "\u{2028}|",
            "e|",
        ],
        &[
            "a  b|ws-pre",
            "\u{2028}|",
            "c|ws-pre",
            " and |",
            "\n x\ty |ws-pre-line",
        ],
        &["line1\n  line2|ws-pre"],
        &["win|ws-pre"],
        &["a|", " |", "b|", " |", "c|"],
        &["b|"],
        &["d|", " |", "o1|", " |", " |", "o2|", " |"],
        &["x|", "|line-start", "line|", "|line-start", "y|", "TABLE"],
        &["t|"],
    ];
    let got: Vec<Vec<String>> = got.into_iter().map(Result::unwrap).collect();
    assert_eq!(got, want);
}

/// A link target of 1 MiB over 2,000 runs: each run's flags are a copy of it (kfxgen shares
/// one set), charged to the file's budget, so the file is refused, not copied 2 GB.
#[test]
fn a_long_link_target_over_many_runs_is_too_large() {
    let target = "x".repeat(1 << 20);
    let mut body = format!("<p><a href=\"#{target}\">");
    for _ in 0..2_000 {
        body.push_str("<b>w</b> ");
    }
    body.push_str("</a></p>");
    let file = page(&body, "");
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let start = Instant::now();
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"), false);
    let took = start.elapsed();
    assert!(
        matches!(got, Err(ItemError::Error(Error::Book(_)))),
        "{:?}",
        got.map(|p| p.len())
    );
    assert!(took < COST_LIMIT, "{took:?}");
}

/// 20,000 black words in a red link after 20,000 empty spans: whether each word is all its
/// link shows asks for the text before the link, which kfxgen finds again for every word by
/// walking back over the spans; the port keeps the answer.
#[test]
fn the_text_before_a_link_is_found_once() {
    let mut body = String::from("<p>");
    for _ in 0..20_000 {
        body.push_str("<span></span>");
    }
    body.push_str("<a href=\"#x\" class=\"red\">");
    for _ in 0..20_000 {
        body.push_str("<b class=\"blk\">w</b>");
    }
    body.push_str("</a></p>");
    let file = page(&body, ".red { color: #c00 } .blk { color: black }");
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let start = Instant::now();
    let parts = Walk::new(&file, &resolver, &mut serials)
        .inline(first(&file, "p"), false)
        .unwrap();
    let took = start.elapsed();
    eprintln!("20,000 black words after 20,000 empty spans: {took:?}");
    assert!(took < COST_LIMIT, "{took:?}");
    // A black word inside a coloured link, not all it shows, keeps the link's colour (value
    // from the pinned `_walk_inline` on three spans and three words).
    assert_eq!(
        show(&parts[..1]),
        ["w|bold link=OEBPS/a.xhtml#x link-colour=(4291559424,4291559424)"]
    );
}
