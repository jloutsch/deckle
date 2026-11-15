// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's inline walk, `_walk_inline` (phase 3 plan, Task 9), against the run dumps
//! (`fixtures/kfxgen/convert/<name>/inline/NNN.json.gz`), and what it refuses.

use std::collections::BTreeMap;
use std::time::Instant;

use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::convert::{
    Error, ItemError, Resolver, Segment, Serials, TextBudget, Walk, css_background_image, initial,
    normalize_runs_with_anchors,
};
use deckle_kfxgen::css::DEFAULT_PROFILE;
use deckle_kfxgen::dom::NodeId;
use deckle_kfxgen::pyvalue::PyValue;

use crate::convert_resolver::{empty_page, file_of, page, sources};
use crate::convert_support::{dump, model_diff};
use crate::support::{COST_LIMIT, allocated_by, find};

/// Calls whose runs cannot match yet, with why: (source, NNN, why). Empty is the goal.
const SKIP: &[(&str, &str, &str)] = &[];

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
    let (mut files_compared, mut calls, mut skipped) = (0, 0, 0);
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
                for record in records {
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
                    let ours = match walk.inline(n) {
                        Ok(segs) => segs,
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
        "{files_compared} files, {calls} calls compared, {skipped} skipped; serial moves: {}",
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
    assert_eq!(calls + skipped, 1862);
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
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"));
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
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"));
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
    let segs = Walk::new(&file, &resolver, &mut serials)
        .inline(first(&file, "p"))
        .unwrap();
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

/// Each segment as `text|flag flag`.
fn show(segs: &[Segment]) -> Vec<String> {
    segs.iter()
        .map(|s| {
            let PyValue::Set(flags) = s.flags.to_py() else {
                unreachable!()
            };
            let flags: Vec<String> = flags.iter().map(show_flag).collect();
            format!("{}|{}", s.text, flags.join(" "))
        })
        .collect()
}

/// Every `<p>`, `<td>` and `<pre>` of a page walked in turn.
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
        .map(|n| match walk.inline(n) {
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
        // No caller passes `split_tables`: the table is walked as content, a cell's edges
        // spaced.
        &[
            "x|",
            "|line-start",
            "line|",
            "|line-start",
            "y|",
            " |",
            "t|",
            " |",
        ],
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
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"));
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
        .inline(first(&file, "p"))
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

/// A red link holding many hidden words and then many black ones: each black word asks whether
/// it is all its link shows, and kfxgen looks again at every child of the link for each
/// (n × 2n children). The port looks at each element's children once.
#[test]
fn whether_a_word_is_all_its_link_shows_is_found_once() {
    let n = 2_000;
    let mut body = String::from("<p><a href=\"#x\" class=\"red\">");
    for _ in 0..n {
        body.push_str("<b class=\"h\">z</b>");
    }
    for _ in 0..n {
        body.push_str("<i class=\"blk\">w</i>");
    }
    body.push_str("</a></p>");
    let file = page(
        &body,
        ".red { color: #c00 } .blk { color: #111111 } .h { display: none }",
    );
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let mut walk = Walk::new(&file, &resolver, &mut serials);
    let parts = walk.inline(first(&file, "p")).unwrap();
    let work = walk.link_work();
    eprintln!("{n} hidden and {n} black words in a link: {work} looked at");
    // The link's 2n children once, and one step up from each black word.
    assert!(work <= 3 * n as u64, "{work}");
    assert_eq!(parts.len(), n);
    // Not all its link shows: a black word keeps the link's colour.
    assert_eq!(
        show(&parts[..1]),
        ["w|italic link=OEBPS/a.xhtml#x link-colour=(4291559424,4291559424)"]
    );
}

/// 2,000 empty `<q>` under a `lang` of a megabyte: each mark looks the lang up in the quote
/// table, which kfxgen does on a lowered copy of it (four copies of the lang per `<q>`, 8 GB
/// here). The port neither copies nor lowers a lang longer than the table's longest: the walk
/// allocates less than one more copy of it than under a short lang.
#[test]
fn a_long_lang_is_not_copied_for_each_quote() {
    let walk = |lang: &str| {
        let body = format!(
            "<div lang=\"{lang}\"><p>{}</p></div>",
            "<q></q>".repeat(2_000)
        );
        let file = page(&body, "");
        let budget = TextBudget::new();
        let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
        let mut serials = Serials::default();
        let (parts, bytes) = allocated_by(|| {
            Walk::new(&file, &resolver, &mut serials)
                .inline(first(&file, "p"))
                .unwrap()
        });
        (show(&parts), bytes)
    };
    let long = "x".repeat(1 << 20);
    let (short_parts, short_bytes) = walk("xxxxxxxx");
    let (long_parts, long_bytes) = walk(&long);
    eprintln!(
        "2,000 quotes: {short_bytes} bytes allocated under a short lang, {long_bytes} under 1 MiB"
    );
    assert!(
        long_bytes < short_bytes + long.len() as u64,
        "{long_bytes} vs {short_bytes}"
    );
    assert_eq!(long_parts, short_parts);
    assert_eq!(long_parts.len(), 4_000);
    assert_eq!(long_parts[..2], ["\"|", "\"|"]);
}

/// [`walk_page`] with the token's control characters written `<` (delimiter), `^` (field)
/// and `~` (a space in alt text).
fn walk_page_readable(body: &str, css: &str) -> Vec<Result<Vec<String>, String>> {
    let readable = |s: String| {
        s.replace('\0', "<")
            .replace('\u{1}', "^")
            .replace('\u{2}', "~")
    };
    walk_page(body, css)
        .into_iter()
        .map(|r| r.map(|parts| parts.into_iter().map(readable).collect()))
        .collect()
}

/// Pictures in text the dumps do not hold: a wrapper's float and width, centring, margins,
/// vertical-align, links, attribute sizes, unsafe and `data:` sources, absurd sizes, an
/// `<svg>`'s pictures and text. Expected values from the pinned `_walk_inline`, run under
/// Python 3.14 with lxml 6.0.2 over these pages, each element given the style the port's
/// resolver gives it. (In the wrapper with a comment, lxml's `len(parent)` counts the comment,
/// so the picture is not alone and does not float: checked on lxml separately, as the harness
/// wrote the page without comments.)
#[test]
fn pictures_in_text_are_kfxgens() {
    let pages: &[(&str, &str, &[&[&str]])] = &[
        (
            "<p class=\"fl\"><img src=\"a.png\" alt=\"x y\"/></p>\
             <p class=\"fl\"><img src=\"a.png\" width=\"100%\"/></p>\
             <p class=\"fl\"><img src=\"a.png\" width=\"50%\"/></p>\
             <p class=\"fl\">t<img src=\"a.png\"/></p>",
            ".fl{float:left;width:30%}",
            &[
                &["<IMG^OEBPS/a.png^x~y^w=30%^left<|"],
                &["<IMG^OEBPS/a.png^^w=30%^left<|"],
                &["<IMG^OEBPS/a.png^^w=50%^left<|"],
                &["t|", "<IMG^OEBPS/a.png^<|"],
            ],
        ),
        (
            "<p class=\"w\"><img src=\"a.png\"/></p>\
             <p class=\"w\"><!--c--><img src=\"a.png\"/></p>\
             <p class=\"w\"><img src=\"a.png\"/> </p>\
             <p class=\"w2\"><img src=\"a.png\" style=\"width:100%\"/></p>\
             <p class=\"w2\"><img src=\"a.png\" style=\"width:10px\"/></p>\
             <p class=\"w3\"><img src=\"a.png\"/></p>",
            ".w{float:right;width:auto} .w2{float:left;width:40%} .w3{float:left;width:12ex}",
            &[
                &["<IMG^OEBPS/a.png^^^right<|"],
                &["<IMG^OEBPS/a.png^<|"],
                &["<IMG^OEBPS/a.png^^^right<|", " |"],
                &["<IMG^OEBPS/a.png^^w=40%^left<|"],
                &["<IMG^OEBPS/a.png^^w=10px^left<|"],
                &["<IMG^OEBPS/a.png^^^left<|"],
            ],
        ),
        (
            "<p><img src=\"../a.png\" style=\"display:block;margin-left:auto;margin-right:auto\"/></p>\
             <p><img src=\"a.png\" style=\"display:block;margin-left:auto\"/></p>\
             <p><img src=\"a.png\" style=\"margin-left:auto;margin-right:auto\"/></p>\
             <p>x<img src=\"a.png\" style=\"margin-left:-1em;margin-right:2px;vertical-align:-0.2em;\
             height:1em;width:2em\"/>y<img src=\"b.png\" style=\"vertical-align:Middle;float:right\"/></p>\
             <p><a href=\"#x\">t<img src=\"a.png\" alt=\"\" id=\"i1\"/></a><span id=\"x\">s</span></p>",
            "",
            &[
                &["<IMG^a.png^^^^c=1<|"],
                &["<IMG^OEBPS/a.png^^^^c=r<|"],
                &["<IMG^OEBPS/a.png^<|"],
                &[
                    "x|",
                    "<IMG^OEBPS/a.png^^w=2em^^h=1em;va=-0.2em;ml=-1em;mr=2px<|",
                    "y|",
                    "<IMG^OEBPS/b.png^^^right^va=middle<|",
                ],
                &[
                    "t|link=OEBPS/a.xhtml#x",
                    "|anchor-mark=i1",
                    "<IMG^OEBPS/a.png^<|link=OEBPS/a.xhtml#x",
                    "|anchor-mark=x",
                    "s|",
                ],
            ],
        ),
        (
            "<p><img src=\"a.png\" width=\"120\" height=\"80\"/><img src=\"a.png\" height=\" 3em \"/>\
             <img src=\"a.png\" width=\"auto\" height=\"7pt\"/><img src=\"a.png\" width=\"1e3\"/>\
             <img src=\"/etc/passwd\" alt=\"a\u{1}b\"/><img src=\"data:image/png;base64,AAAA\"/><img/></p>\
             <p><img src=\"a.png\" width=\"99999999999999999999999999\"/>\
             <img src=\"a.png\" width=\"-5\" height=\"1e309\"/>\
             <img src=\"a.png\" width=\"NaN\" height=\"\u{661}\u{660}\"/>\
             <img src=\"a.png\" style=\"width:1e3px;height:50%\"/>\
             <img src=\"a.png\" style=\"vertical-align:-99999999999999999999em;\
             margin-left:99999999999999999999999999999em;margin-right:-0\"/>\
             <img src=\"a.png\" style=\"vertical-align:+.5%;margin-left:1in\"/>\
             <img src=\"a.png\" style=\"vertical-align:0.000;margin-left:1.5Q\"/>\
             <img src=\"a.png\" style=\"width:\u{661}\u{660}px\"/></p>",
            "",
            &[
                &[
                    "<IMG^OEBPS/a.png^^w=120px^^h=80px<|",
                    "<IMG^OEBPS/a.png^^h=3em^^h=3em<|",
                    "<IMG^OEBPS/a.png^^h=7pt^^h=7pt<|",
                    "<IMG^OEBPS/a.png^<|",
                    "<IMG^/etc/passwd^ab<|",
                    "<IMG^data:image/png;base64,AAAA^<|",
                    "<IMG^^<|",
                ],
                &[
                    "<IMG^OEBPS/a.png^^w=99999999999999999999999999px<|",
                    "<IMG^OEBPS/a.png^<|",
                    "<IMG^OEBPS/a.png^^h=\u{661}\u{660}^^h=\u{661}\u{660}<|",
                    "<IMG^OEBPS/a.png^^h=50%^^h=50%<|",
                    "<IMG^OEBPS/a.png^^^^va=-99999999999999999999em;\
                     ml=99999999999999999999999999999em<|",
                    "<IMG^OEBPS/a.png^^^^va=+0.5%;ml=1in<|",
                    "<IMG^OEBPS/a.png^^^^va=0<|",
                    "<IMG^OEBPS/a.png^^w=\u{661}\u{660}px<|",
                ],
            ],
        ),
        (
            "<p>a<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
             id=\"s1\"><defs><image xlink:href=\"no.png\"/></defs><clipPath><image href=\"no2.png\"/>\
             </clipPath><image xlink:href=\"../i.png\"/><image href=\"j.png\"/><g><svg>\
             <image xlink:href=\"\" href=\"k.png\"/></svg></g></svg>b</p>\
             <p><svg xmlns=\"http://www.w3.org/2000/svg\"><text>Hello <tspan x=\"1\">world</tspan>\
             <tspan>!</tspan>tail</text><text id=\"t2\"><a>link<tspan y=\"2\">y</tspan></a><b>no</b>after\
             </text><switch><text systemLanguage=\"fr\">fr</text><text>en</text></switch><switch>\
             <text requiredFeatures=\"x\">only</text></switch><title>no</title><foreignObject><p>no</p>\
             </foreignObject><text>  </text></svg>z</p>\
             <p>a<svg xmlns=\"http://www.w3.org/2000/svg\" visibility=\" Hidden \"><circle/></svg>\
             <svg xmlns=\"http://www.w3.org/2000/svg\"><defs><path/></defs></svg>\
             <svg xmlns=\"http://www.w3.org/2000/svg\"><svg><rect/></svg></svg>\
             <svg xmlns=\"http://www.w3.org/2000/svg\"><rect/><text>t</text></svg>b</p>",
            "",
            &[
                &[
                    "a|",
                    "|anchor-mark=s1",
                    "<IMG^../i.png^^h=100%<|",
                    "<IMG^j.png^^h=100%<|",
                    "<IMG^k.png^^h=100%<|",
                    "b|",
                ],
                &[
                    " |",
                    "Hello world!tail |",
                    " |",
                    "|anchor-mark=t2",
                    "link yafter |",
                    " |",
                    "en |",
                    " |",
                    "only |",
                    "z|",
                ],
                &["no|"],
                &["a|", " |", "t |", "b|"],
            ],
        ),
    ];
    for (body, css, want) in pages {
        let got = walk_page_readable(body, css);
        let want: Vec<Result<Vec<String>, String>> = want
            .iter()
            .map(|w| Ok(w.iter().map(|s| s.to_string()).collect()))
            .collect();
        assert_eq!(got, want, "{body}");
    }
}

/// An `<svg>` that draws shapes and nothing kfxgen takes from it: kfxgen draws it as a picture
/// of its own (#709); the converter refuses it by name (plan decision 2).
#[test]
fn an_svg_of_shapes_only_is_refused_by_name() {
    let file = page(
        "<p>a<svg xmlns=\"http://www.w3.org/2000/svg\" id=\"s\"><g><rect width=\"1\"/></g></svg></p>",
        "",
    );
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"));
    match got {
        Err(ItemError::Error(Error::Unsupported { feature, file })) => {
            assert_eq!(feature, "SVG pictures");
            assert_eq!(file, "OEBPS/a.xhtml");
        }
        other => panic!("{other:?}"),
    }
}

/// A paragraph's initial. Expected values from the pinned `_initial` (after `_walk_inline` and
/// `normalize_runs_with_anchors`), run as in [`pictures_in_text_are_kfxgens`].
#[test]
fn drop_caps_are_kfxgens() {
    let file = page(
        "<p class=\"dc\">\u{201c}Once upon</p><p class=\"r\">Raised</p>\
         <p><span class=\"drop\">O</span>nce</p><p><b><span class=\"drop\">W</span></b>hen</p>\
         <p> <img src=\"a.png\"/><span class=\"drop\">X</span>x</p><p>text<span class=\"drop\">X</span></p>\
         <p class=\"dc\"><span style=\"font-size:2em\">B</span>ig <i>x</i></p><p class=\"dc\">\u{2014} 1</p>\
         <p class=\"dc\">$x</p><p><span><span class=\"drop\">T w</span></span>o</p>\
         <p><span></span> <span class=\"drop\">Q</span>q</p><p class=\"k\">K</p>",
        ".dc::first-letter{float:left;font-size:3em;color:red;font-weight:bold} \
         .r::first-letter{font-size:2em;font-style:italic;color:black} .drop{float:left;font-size:3em} \
         .k::first-letter{color:#000;float:Left}",
    );
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let mut walk = Walk::new(&file, &resolver, &mut serials);
    let paragraphs: Vec<NodeId> = file
        .dom
        .descendants()
        .filter(|&n| file.dom.tag(n) == "p")
        .collect();
    let mut got = Vec::new();
    for p in paragraphs {
        let segs = walk.inline(p).unwrap();
        let (text, spans, _) = normalize_runs_with_anchors(&segs);
        let css = resolver.resolve(p).unwrap();
        let (spans, drop) = initial(&file.dom, &resolver, p, css.as_ref(), &text, spans).unwrap();
        let spans: Vec<String> = spans
            .iter()
            .map(|s| {
                let PyValue::Set(flags) = s.flags.to_py() else {
                    unreachable!()
                };
                let mut flags: Vec<String> = flags.iter().map(show_flag).collect();
                flags.sort();
                format!("{}+{}|{}", s.start, s.len, flags.join(" "))
            })
            .collect();
        got.push((spans, drop));
    }
    let red = "colour=4293787648";
    let want: Vec<(Vec<String>, Option<usize>)> = vec![
        (vec![format!("0+2|bold {red} size=Float(48.0)")], Some(2)),
        (vec!["0+1|italic size=Float(32.0)".into()], None),
        (vec!["0+1|size=Float(48.0)".into()], Some(1)),
        (vec!["0+1|bold size=Float(48.0)".into()], Some(1)),
        (vec!["18+1|size=Float(48.0)".into()], None),
        (vec!["4+1|size=Float(48.0)".into()], None),
        (
            vec![
                format!("0+1|bold {red} size=Float(48.0)"),
                "4+1|italic".into(),
            ],
            Some(1),
        ),
        (vec![format!("0+3|bold {red} size=Float(48.0)")], Some(3)),
        (vec![], None),
        (vec!["0+3|size=Float(48.0)".into()], Some(3)),
        (vec!["0+1|size=Float(48.0)".into()], Some(1)),
        (vec![], Some(1)),
    ];
    assert_eq!(got, want);
}

/// What `_css_background_image` gives for these, from the pinned Python as above. (calibre's
/// CSS reads `url(   )` and `url('')` as the file itself, so the pattern's own backtracking is
/// tested in `pictures.rs`.)
#[test]
fn background_pictures_are_kfxgens() {
    let file = page(
        "<div class=\"bg\"></div><div class=\"bg2\"></div><div class=\"bg3\"></div>\
         <div class=\"bg4\"></div><div class=\"bg\">t</div><div class=\"bg5\"></div>",
        ".bg{background-image:url( \"scan.jpg\" );background-repeat:no-repeat} \
         .bg2{background-image:url(   );background-repeat:no-repeat} \
         .bg3{background-image:url('') url(x.png);background-repeat:No-Repeat} \
         .bg4{background-image:url(x.png)} .bg5{background:url(y.png) no-repeat}",
    );
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let got: Vec<Option<String>> = file
        .dom
        .descendants()
        .filter(|&n| file.dom.tag(n) == "div")
        .map(|n| {
            let css = resolver.resolve(n).unwrap();
            css_background_image(&file.dom, n, css.as_ref()).unwrap()
        })
        .collect();
    let some = |s: &str| Some(s.to_owned());
    assert_eq!(
        got,
        [
            some("OEBPS/scan.jpg"),
            some("OEBPS/a.xhtml"),
            some("OEBPS/a.xhtml"),
            None,
            None,
            None
        ]
    );
}

/// A `data:` source is written into its token as it is, with no decoding beyond
/// `_is_unsafe_href`'s (it has no `%`), and the copy is charged to the file's budget.
#[test]
fn a_data_uri_of_several_mb_is_written_as_it_is_and_charged() {
    let uri = format!("data:image/png;base64,{}", "QUJD".repeat(1 << 20));
    let file = page(&format!("<p>x<img src=\"{uri}\" alt=\"a\"/></p>"), "");
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let before = budget.charged();
    let start = Instant::now();
    let (segs, bytes) = allocated_by(|| {
        Walk::new(&file, &resolver, &mut serials)
            .inline(first(&file, "p"))
            .unwrap()
    });
    assert!(start.elapsed() < COST_LIMIT);
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[1].text, format!("\0IMG\u{1}{uri}\u{1}a\0"));
    assert!(budget.charged() - before >= uri.len() / 64);
    // `_is_unsafe_href` lowers it twice and decodes it once; the token is one more copy.
    assert!(bytes < 8 * uri.len() as u64, "{bytes}");
}

/// The copies a file's pictures make share the file's budget: 36 MiB of `data:` sources in
/// one file pass it, and the file is refused after bounded work.
#[test]
fn data_uris_past_the_files_budget_are_refused() {
    let uri = format!("data:image/png;base64,{}", "QUJD".repeat(1 << 20));
    let mut body = String::from("<p>");
    for _ in 0..9 {
        body.push_str(&format!("<img src=\"{uri}\"/>"));
    }
    body.push_str("</p>");
    let file = page(&body, "");
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let start = Instant::now();
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"));
    assert!(start.elapsed() < COST_LIMIT);
    assert!(
        matches!(got, Err(ItemError::Error(Error::Book(_)))),
        "{:?}",
        got.map(|s| s.len())
    );
}

/// `read_book` refuses a tree deeper than `MAX_DEPTH`; built by hand, an `<svg>` whose
/// picture, text and shapes sit 20,000 levels down is read with loops, not recursion (kfxgen's
/// Python would hit its recursion limit), on a test thread's stack.
#[test]
fn an_svg_nested_20000_deep_is_read_without_recursion() {
    const DEEP: usize = 20_000;
    let deep_svg = |shapes: bool| {
        let (mut dom, body) = empty_page();
        let p = dom.append_child(body, "p");
        let svg = dom.append_child(p, "svg");
        let mut g = svg;
        for _ in 0..DEEP {
            g = dom.append_child(g, "g");
        }
        if shapes {
            dom.append_child(g, "rect");
        } else {
            let image = dom.append_child(g, "image");
            dom.set_attr(image, "href", Some("deep.png"));
            let mut t = dom.append_child(g, "text");
            dom.set_text(t, "a");
            for _ in 0..DEEP {
                t = dom.append_child(t, "tspan");
            }
            dom.set_text(t, "b");
        }
        file_of(dom, "")
    };
    let file = deep_svg(false);
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let mut serials = Serials::default();
    let start = Instant::now();
    let segs = Walk::new(&file, &resolver, &mut serials)
        .inline(first(&file, "p"))
        .unwrap();
    assert!(start.elapsed() < COST_LIMIT);
    assert_eq!(
        show(&segs),
        ["\0IMG\u{1}deep.png\u{1}\u{1}h=100%\0|", " |", "ab |"]
    );

    let file = deep_svg(true);
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let got = Walk::new(&file, &resolver, &mut serials).inline(first(&file, "p"));
    assert!(
        matches!(
            got,
            Err(ItemError::Error(Error::Unsupported {
                feature: "SVG pictures",
                ..
            }))
        ),
        "{got:?}"
    );
}

/// A drop cap 20,000 elements down the start of a paragraph built by hand is found with a
/// loop.
#[test]
fn a_drop_cap_20000_deep_is_found_without_recursion() {
    let (mut dom, body) = empty_page();
    let p = dom.append_child(body, "p");
    let mut n = p;
    for _ in 0..20_000 {
        n = dom.append_child(n, "span");
    }
    let drop = dom.append_child(n, "span");
    dom.set_attr(drop, "class", Some("drop"));
    dom.set_text(drop, "W");
    dom.set_tail(drop, "hen");
    let file = file_of(dom, ".drop { float: left }");
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let p = first(&file, "p");
    let css = resolver.resolve(p).unwrap();
    let start = Instant::now();
    let got = initial(&file.dom, &resolver, p, css.as_ref(), "When", Vec::new()).unwrap();
    assert!(start.elapsed() < COST_LIMIT);
    assert_eq!(got, (Vec::new(), Some(1)));
}

/// 50,000 pictures in one floated paragraph, and an `<svg>` of 50,000 `<text>`s and a
/// `<switch>` of 50,000 children with conditions: each element is looked at a bounded number
/// of times.
#[test]
fn many_pictures_and_svg_parts_take_linear_time() {
    const MANY: usize = 50_000;
    let mut body = String::from("<p class=\"fl\">");
    for _ in 0..MANY {
        body.push_str("<img src=\"a.png\"/>");
    }
    body.push_str("</p><p><svg xmlns=\"http://www.w3.org/2000/svg\">");
    for _ in 0..MANY {
        body.push_str("<text>w</text>");
    }
    body.push_str("<switch>");
    for _ in 0..MANY {
        body.push_str("<text systemLanguage=\"x\">c</text>");
    }
    body.push_str("</switch></svg></p>");
    let start = Instant::now();
    let got = walk_page(&body, ".fl { float: left; width: 10% }");
    assert!(start.elapsed() < COST_LIMIT, "{:?}", start.elapsed());
    let [Ok(pictures), Ok(svg)] = &got[..] else {
        panic!("{got:?}");
    };
    assert_eq!(pictures.len(), MANY);
    assert_eq!(pictures[0], "\0IMG\u{1}OEBPS/a.png\u{1}\0|");
    assert_eq!(svg.len(), 2 * MANY + 2);
    assert_eq!(svg[2 * MANY + 1], "c |");
}

/// Where kfxgen raises in these helpers, the port gives the same exception as a
/// [`ItemError::Raised`] (the spine item is left out): a picture's side margin in `in` spelt
/// with a dotless or dotted i, and a `::first-letter` letter-spacing spelt so
/// (`parse_css_length`, through `_css_emphasis` for the initial). Checked against the pinned
/// helpers under Python 3.14.
#[test]
fn what_kfxgen_raises_on_in_pictures_and_initials_is_raised() {
    let got = walk_page(
        "<p>x<img src=\"a.png\" class=\"m\"/></p>",
        "img.m { margin-left: 1\u{131}n }",
    );
    assert_eq!(got, [Err("KeyError".to_owned())]);

    let file = page(
        "<p class=\"dc\">Once</p>",
        ".dc::first-letter { letter-spacing: 2\u{131}n }",
    );
    let budget = TextBudget::new();
    let resolver = Resolver::new(&file, &DEFAULT_PROFILE, &budget);
    let p = first(&file, "p");
    let css = resolver.resolve(p).unwrap();
    match initial(&file.dom, &resolver, p, css.as_ref(), "Once", Vec::new()) {
        Err(ItemError::Raised(r)) => assert_eq!(r.exception, "KeyError"),
        other => panic!("{other:?}"),
    }
}
