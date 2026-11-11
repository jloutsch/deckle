// SPDX-License-Identifier: GPL-3.0-or-later
//! `generate_full_book` (`native_generator.py:4549`): the book's ids, the fragments around its
//! text, its symbol table, and the container.
use std::collections::BTreeSet;
use std::sync::LazyLock;

use deckle_kfx::container::{KfxDoc, WriteOptions, read_kfx, write_kfx};
use deckle_kfx::fragment::{Fragment, FragmentList};
use deckle_kfx::ion::{Decimal, Value};
use deckle_kfx::symtab::SymbolTable;
use regex::Regex;
use sha2::{Digest, Sha256};

use super::chapters::{Chapter, read_chapters};
use super::fonts::FontTable;
use super::fragments::build_fragment_260;
use super::images;
use super::metadata::{BookMetadata, build_fragment_490};
use super::nav::{NavEntry, book_headings, build_fragment_389};
use super::notes;
use super::pages;
use super::positions::{
    build_fragment_264, build_fragment_265, build_fragment_550, build_position_data,
};
use super::standard_symbols::YJ_SYMBOLS_MAX_ID;
use super::text::build_chapter_content;
use super::writing;
use super::{GenerateError, GenerateOutput, Generator, measure, model_err, py, st, sym};
use super::{table_chunks, tables};
use crate::pyvalue::{PyValue, truthy};

/// `generate_full_book`'s parameters, all but `output_path`.
const PARAMS: [&str; 20] = [
    "title",
    "author",
    "chapters",
    "asin",
    "cover_image",
    "language",
    "rtl",
    "writing_mode",
    "publisher",
    "issue_date",
    "authors",
    "description",
    "images",
    "font_table",
    "resolved_image_refs",
    "display_dims",
    "footnote_keys",
    "landmarks",
    "page_list",
    "approximate_pages",
];

/// `_REFLOW_LANGUAGES` (:68).
const REFLOW_LANGUAGES: [&str; 3] = ["ar", "fa", "he"];

/// A string argument, `default` when absent; `None` stays `None`.
fn str_arg<'a>(
    args: &'a PyValue,
    key: &str,
    default: Option<&'a str>,
) -> Result<Option<&'a str>, GenerateError> {
    match args.get(key) {
        None => Ok(default),
        Some(PyValue::None) => Ok(None),
        Some(PyValue::Str(s)) => Ok(Some(s)),
        Some(_) => Err(model_err(format!("args.{key}: a string or None"))),
    }
}

fn required_str<'a>(args: &'a PyValue, key: &str) -> Result<&'a str, GenerateError> {
    str_arg(args, key, None)?.ok_or_else(|| model_err(format!("args.{key}: a string")))
}

pub(crate) fn generate_full_book(args: &PyValue) -> Result<GenerateOutput, GenerateError> {
    let PyValue::Dict(pairs) = args else {
        return Err(model_err("args: a dict"));
    };
    for (key, _) in pairs {
        if !key.as_str().is_some_and(|k| PARAMS.contains(&k)) {
            return Err(model_err(format!("args: no parameter {key:?}")));
        }
    }
    let title = required_str(args, "title")?;
    let author = required_str(args, "author")?;
    let language = str_arg(args, "language", Some("en"))?
        .ok_or_else(|| model_err("args.language: a string"))?;
    let publisher = str_arg(args, "publisher", Some("kfxgen"))?
        .ok_or_else(|| model_err("args.publisher: a string"))?;
    let issue_date = str_arg(args, "issue_date", None)?;
    let description = str_arg(args, "description", None)?;
    let writing_mode = str_arg(args, "writing_mode", Some("horizontal-tb"))?;
    let chapters_arg = args
        .get("chapters")
        .ok_or_else(|| model_err("args.chapters: a list"))?;
    let chapter_count = chapters_arg
        .as_list()
        .ok_or_else(|| model_err("args.chapters: a list"))?
        .len();

    let rtl = truthy(args.get("rtl"));
    let vertical = writing_mode == Some("vertical-rl");
    let base_language = language.replace('_', "-");
    let base_language = base_language.split('-').next().unwrap_or("").to_lowercase();

    // The container id and ASIN come from the book's identity (:4638).
    let seed = [
        title,
        author,
        language,
        publisher,
        issue_date.unwrap_or(""),
        &chapter_count.to_string(),
    ]
    .join("\0");
    let digest: String = Sha256::digest(seed.as_bytes())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    let container_id = format!("CR!{}", &digest[..28]);
    let asin = str_arg(args, "asin", None)?.unwrap_or(&digest[..32]);
    let authors: Vec<&str> = match args.get("authors") {
        Some(a) if a.is_truthy() => a
            .as_list()
            .ok_or_else(|| model_err("args.authors: a list"))?
            .iter()
            .map(|n| n.as_str().ok_or_else(|| model_err("args.authors: strings")))
            .collect::<Result<_, _>>()?,
        _ => vec![author],
    };

    let mut g = Generator::new();
    let mut chapters = read_chapters(chapters_arg)?;
    let footnote_keys = notes::read_footnote_keys(args.get("footnote_keys"))?;
    let table_version =
        tables::feature_version(chapters.iter().flat_map(|c| c.blocks.iter().flatten()));
    let f = build_fragment_585(&mut g, table_version);
    g.push(f);
    let mut images = images::build_resources(&mut g, args)?;
    let fonts = FontTable::read(args.get("font_table"))?;
    fonts.build_fragments(&mut g);
    let override_kindle_font = !fonts.is_empty();
    g.fonts = fonts;
    let f = build_fragment_490(
        &mut g,
        &BookMetadata {
            title,
            asin,
            container_id: &container_id,
            language,
            issue_date: issue_date.filter(|d| !d.is_empty()),
            publisher,
            authors,
            description: description.filter(|d| !d.is_empty()),
            cover_image: images.cover.then_some(images::COVER_RESOURCE),
            override_kindle_font,
        },
    );
    g.push(f);

    if chapter_count == 0 {
        return Err(model_err(
            "args.chapters: generate_full_book requires a non-empty list",
        ));
    }
    if images.cover {
        images::add_cover_page(&mut chapters, &images);
    }
    let data = build_chapter_content(&mut g, &chapters, &images, &footnote_keys, vertical)?;
    // Pictures no entry shows are dropped (#102).
    let shown: Vec<&str> = data
        .chunks
        .iter()
        .flat_map(|c| {
            let among_words = c.pictures.iter().map(|p| p.resource.as_str());
            c.picture
                .iter()
                .map(|p| p.resource.as_str())
                .chain(among_words)
        })
        .collect();
    images::drop_unshown(&mut g, &mut images, &shown);

    // A section per storyline (:4959).
    let mut section_names = Vec::new();
    for (i, storyline) in data.storyline_names.iter().enumerate() {
        let name = format!("c{i}");
        // A chapter set otherwise than the book writes its own on its section (#247).
        let own = chapters[i]
            .writing_mode
            .as_deref()
            .filter(|&m| Some(m) != writing_mode);
        let f = build_fragment_260(&mut g, storyline, &name, data.section_positions[i], own);
        g.push(f);
        section_names.push(name);
    }
    let pos = build_position_data(&section_names, &data);
    let f = build_fragment_538(&mut g, &section_names, rtl, writing_mode);
    g.push(f);
    declare_writing_mode_features(&mut g, writing_mode, &chapters, base_language == "ja");

    // The cover's page, and a chapter its caller leaves out, are in no contents (:4988).
    let mut nav_entries = Vec::new();
    for (ch, &position) in chapters.iter().zip(&data.chapter_start_positions) {
        if !ch.omit_from_toc {
            nav_entries.push(NavEntry {
                title: ch.title.clone(),
                position,
                offset: 0,
                level: ch.toc_level,
            });
        }
        // Entries that name a place in the chapter (#363). One that can't be found opens it.
        for sub in &ch.sub_entries {
            let (position, offset) = sub
                .key
                .as_ref()
                .and_then(|k| data.key_positions.get(k))
                .copied()
                .unwrap_or((position, 0));
            nav_entries.push(NavEntry {
                title: sub.title.clone(),
                position,
                offset,
                level: sub.level,
            });
        }
    }
    let book_nav = pages::read(args, &data, &pos.pages)?;
    let cover_position = chapters[0].is_cover.then(|| data.section_positions[0]);
    let f = build_fragment_389(
        &mut g,
        &nav_entries,
        cover_position,
        &book_headings(&data),
        &book_nav,
    )?;
    g.push(f);
    let f = build_fragment_258(&mut g, &section_names);
    g.push(f);
    g.push(Fragment::root(
        "$395",
        st([("$247", Value::List(Vec::new()))]),
    ));
    let f = build_fragment_264(&mut g, &pos.section_positions_264);
    g.push(f);
    g.push(build_fragment_265(&pos.entries_265));

    // $593 (:5080).
    let mut capabilities = Vec::new();
    if pos.entries_265.iter().any(|e| e.2 != 0) {
        // Positions part-way into a paragraph: around a picture among its words (#529).
        g.local("kfxgen.pidMapWithOffset");
        g.local("version");
        capabilities.push(st([
            ("$492", Value::string("kfxgen.pidMapWithOffset")),
            ("version", Value::Int(1)),
        ]));
    }
    if !data.content_names.is_empty() {
        g.local("kfxgen.textBlock");
        g.local("version");
        capabilities.push(st([
            ("$492", Value::string("kfxgen.textBlock")),
            ("version", Value::Int(1)),
        ]));
    }
    g.push(Fragment::root("$593", Value::List(capabilities)));

    // $597: one per section (:5098).
    g.local("IS_TARGET_SECTION");
    let mut section_ad_names = Vec::new();
    for section in &section_names {
        let name = format!("{section}-ad");
        g.local(&name);
        let value = st([
            ("$598", sym(&name)),
            (
                "$258",
                Value::List(vec![st([
                    ("$492", Value::string("IS_TARGET_SECTION")),
                    ("$307", Value::Bool(true)),
                ])]),
            ),
        ]);
        g.push(Fragment::new("$597", Some(&name), value));
        section_ad_names.push(name);
    }
    g.push(build_fragment_550(&pos.locations));

    // $419: each entity once (:5131), and what sections reach through pictures.
    let mut seen = BTreeSet::new();
    let picture_names = images::entity_names(&images);
    let entity_names: Vec<&String> = data
        .content_names
        .iter()
        .chain(&data.storyline_names)
        .chain(&section_names)
        .chain(&data.story_names)
        .chain(&data.extra_style_names)
        .chain(&data.anchor_names)
        .chain(&section_ad_names)
        .chain(&data.ruby_names)
        .chain(&picture_names)
        .chain(g.fonts.locations())
        .filter(|name| seen.insert(*name))
        .collect();
    let dependencies = images::entity_dependencies(&g.fragments);
    let value = st([
        (
            "$252",
            Value::List(vec![st([
                ("$155", Value::string(&container_id)),
                (
                    "$181",
                    Value::List(entity_names.iter().map(|n| sym(n)).collect()),
                ),
            ])]),
        ),
        ("$253", Value::List(dependencies)),
    ]);
    g.push(Fragment::root("$419", value));
    g.push(build_fragment_270(&container_id));
    if data.chunks.iter().any(table_chunks::has_zoom_button) {
        declare_table_viewer(&mut g);
    }
    if !data.ruby_names.is_empty() && !vertical {
        // `_declare_ruby` (:2304), as Kindle Previewer declares a book with ruby (#236); not
        // in a vertical book, its ruby written all the same (P247a V06).
        declare_feature(&mut g, "yj_ruby");
    }
    if REFLOW_LANGUAGES.contains(&base_language.as_str()) {
        // As Kindle Previewer 4.0.3 declares an Arabic, Persian or Hebrew book, by its
        // language, whatever its text (#246).
        declare_feature(&mut g, &format!("{base_language}-reflow-language"));
    }
    declare_content_features(&mut g, pos.max_section_pids);

    // Only the local symbols a fragment uses, in the table's order (:5192).
    let used = symbols_used(&g.fragments);
    let mut symtab = SymbolTable::with_yj_max_id(YJ_SYMBOLS_MAX_ID);
    for name in g.symtab.local_symbols() {
        if used.contains(&name) {
            symtab.create_local(&name);
        }
    }
    let max_id = max_id(&symtab.local_symbols());
    if let Some(s) = g
        .fragments
        .get_single_mut(&"$538".into())
        .and_then(|f| f.value.as_struct_mut())
    {
        s.insert("max_id", Value::Int(max_id));
    }
    g.push(Fragment::root(
        "$ion_symbol_table",
        symtab.import_fragment(),
    ));
    g.fragments.sort_preferred();

    let doc = KfxDoc {
        symtab,
        fragments: g.fragments,
        container_id: container_id.clone(),
        creator: String::new(),
        package_version: String::new(),
    };
    let bytes = write_kfx(&doc, &WriteOptions::from_doc(&doc))?;
    let fragments: FragmentList = read_kfx(&bytes)?.fragments;
    Ok(GenerateOutput {
        bytes,
        fragments,
        warnings: Vec::new(),
    })
}

/// A `$585` feature.
fn feature(namespace: &str, name: &str, major: i64) -> Value {
    st([
        ("$586", Value::string(namespace)),
        ("$492", Value::string(name)),
        (
            "$589",
            st([(
                "version",
                st([("$587", Value::Int(major)), ("$588", Value::Int(0))]),
            )]),
        ),
    ])
}

/// `build_fragment_585` (:2161).
fn build_fragment_585(g: &mut Generator, table_version: i64) -> Fragment {
    for name in [
        "com.amazon.yjconversion",
        "reflow-style",
        "yj_table",
        "version",
        "SDK.Marker",
        "CanonicalFormat",
    ] {
        g.local(name);
    }
    let features = vec![
        feature("com.amazon.yjconversion", "yj_table", table_version),
        feature("com.amazon.yjconversion", "reflow-style", 1),
        feature("SDK.Marker", "CanonicalFormat", 1),
    ];
    Fragment::root("$585", st([("$590", Value::List(features))]))
}

/// `_declare_table_viewer` (:2718): `yj_table_viewer` after `yj_table`, once a table with the
/// zoom button is written (#254, #272).
fn declare_table_viewer(g: &mut Generator) {
    g.local("yj_table_viewer");
    if let Some(features) = g
        .fragments
        .get_single_mut(&"$585".into())
        .and_then(|f| f.value.as_struct_mut())
        .and_then(|s| s.get_mut("$590"))
        .and_then(Value::as_list_mut)
    {
        features.insert(1, feature("com.amazon.yjconversion", "yj_table_viewer", 1));
    }
}

/// `_declare_feature` (:2365): `name` 1.0 appended to `$585`.
fn declare_feature(g: &mut Generator, name: &str) {
    g.local(name);
    if let Some(features) = g
        .fragments
        .get_single_mut(&"$585".into())
        .and_then(|f| f.value.as_struct_mut())
        .and_then(|s| s.get_mut("$590"))
        .and_then(Value::as_list_mut)
    {
        features.push(feature("com.amazon.yjconversion", name, 1));
    }
}

/// `_declare_content_features` (:2256): reflow-section-size from the largest section, then
/// what the pictures need.
fn declare_content_features(g: &mut Generator, max_section_pids: i64) {
    let n = max_section_pids;
    let size = if n > 65536 {
        ((n - 65536) / 16384 + 2).min(256)
    } else {
        1
    };
    let pictures = images::picture_features(&g.fragments);
    if size > 1 {
        g.local("reflow-section-size");
    }
    for name in &pictures {
        g.local(name);
    }
    let Some(features) = g
        .fragments
        .get_single_mut(&"$585".into())
        .and_then(|f| f.value.as_struct_mut())
        .and_then(|s| s.get_mut("$590"))
        .and_then(Value::as_list_mut)
    else {
        return;
    };
    // After yj_table and its viewer, where it stood before (#455).
    let at = features
        .iter()
        .filter(|f| {
            f.as_struct()
                .and_then(|s| s.get("$492"))
                .and_then(Value::as_str)
                .is_some_and(|n| n == "yj_table" || n == "yj_table_viewer")
        })
        .count();
    if size > 1 {
        features.insert(
            at,
            feature("com.amazon.yjconversion", "reflow-section-size", size),
        );
    }
    for name in pictures {
        features.push(feature("com.amazon.yjconversion", name, 1));
    }
}

fn reading_order(section_names: &[String]) -> Value {
    Value::List(vec![st([
        ("$178", sym("$351")),
        (
            "$170",
            Value::List(section_names.iter().map(|n| sym(n)).collect()),
        ),
    ])])
}

/// `build_fragment_538` (:3011). Its `max_id` is set once the book's symbols are known.
fn build_fragment_538(
    g: &mut Generator,
    section_names: &[String],
    rtl: bool,
    writing_mode: Option<&str>,
) -> Fragment {
    for name in section_names {
        g.local(name);
    }
    let vertical = writing_mode == Some("vertical-rl");
    let Value::Struct(mut value) = st([
        (
            "$560",
            sym(writing::writing_mode_symbol(writing_mode).unwrap_or("$557")),
        ),
        // Page direction: right to left when the spine says so (#246). A vertical-rl book is
        // direction ltr: its pages turn right to left by its writing mode (P247a, #247).
        ("$192", sym(if rtl && !vertical { "$375" } else { "$376" })),
        (
            "$16",
            measure(
                Decimal {
                    coefficient: 1,
                    exponent: 0,
                },
                "$308",
            ),
        ),
        ("$112", sym("$383")),
        ("$436", sym("$441")),
        ("max_id", Value::Int(0)),
        ("$42", measure(py::decimal("1.2"), "$308")),
    ]) else {
        unreachable!()
    };
    if !vertical {
        // Previewer writes no $477 in a vertical book (P247a).
        value.insert("$477", sym("$56"));
    }
    value.insert("$169", reading_order(section_names));
    Fragment::root("$538", Value::Struct(value))
}

/// `_declare_writing_mode_features` (:2218): the layout features Kindle Previewer 4.0.3
/// declares for vertical text (#247), first in `$585`: a vertical book's, or a horizontal
/// book's with vertical sections.
fn declare_writing_mode_features(
    g: &mut Generator,
    writing_mode: Option<&str>,
    chapters: &[Chapter],
    japanese: bool,
) {
    let add: Vec<(&str, i64)> = if writing_mode == Some("vertical-rl") {
        vec![("jp-reflow-language", 1), ("jpvertical-reflow-language", 6)]
    } else if chapters
        .iter()
        .any(|c| c.writing_mode.as_deref() == Some("vertical-rl"))
    {
        let mut add = if japanese {
            vec![("jp-reflow-language", 1)]
        } else {
            Vec::new()
        };
        add.push(("jpvertical-reflow-language", if japanese { 4 } else { 2 }));
        add.push(("yj_mixed_writing_mode", 1));
        add
    } else {
        return;
    };
    for (n, (name, major)) in add.into_iter().enumerate() {
        g.local(name);
        if let Some(features) = g
            .fragments
            .get_single_mut(&"$585".into())
            .and_then(|f| f.value.as_struct_mut())
            .and_then(|s| s.get_mut("$590"))
            .and_then(Value::as_list_mut)
        {
            features.insert(n, feature("com.amazon.yjconversion", name, major));
        }
    }
}

/// `build_fragment_258` (:2983).
fn build_fragment_258(g: &mut Generator, section_names: &[String]) -> Fragment {
    for name in section_names {
        g.local(name);
    }
    Fragment::root("$258", st([("$169", reading_order(section_names))]))
}

/// `build_fragment_270` (:3487).
fn build_fragment_270(container_id: &str) -> Fragment {
    let value = st([
        ("$409", Value::string(container_id)),
        ("$412", Value::Int(4096)),
        ("$410", Value::Int(0)),
        ("$411", Value::Int(0)),
        ("$587", Value::string("")),
        ("$588", Value::string("")),
        ("$161", Value::string("KFX main")),
        ("version", Value::Int(2)),
    ]);
    Fragment::root("$270", value)
}

/// `_symbols_used` (:84): every symbol the fragments name, as kfxlib counts one used.
fn symbols_used(fragments: &FragmentList) -> BTreeSet<String> {
    fn walk(value: &Value, used: &mut BTreeSet<String>) {
        match value {
            Value::Annotated { annots, value } => {
                used.extend(annots.iter().map(|a| a.0.clone()));
                walk(value, used);
            }
            Value::Struct(s) => {
                for (key, item) in s.iter() {
                    used.insert(key.0.clone());
                    walk(item, used);
                }
            }
            Value::List(items) => items.iter().for_each(|i| walk(i, used)),
            Value::Symbol(s) => {
                used.insert(s.0.clone());
            }
            _ => {}
        }
    }
    let mut used = BTreeSet::new();
    for f in fragments.iter() {
        used.insert(f.ftype.0.clone());
        if let Some(fid) = &f.fid {
            used.insert(fid.0.clone());
        }
        walk(&f.value, &mut used);
    }
    used
}

/// `_SHORT_ID` (:115): a Kindle Previewer short id, its digits captured.
static SHORT_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:resource/rsrc|resource/e|rsrc|[a-z])([0-9A-HJKMNPR-Z]{1,6})",
        r"(?:(?:-hd|-first-frame|-thumb)?(?:-resized-[0-9]+-[0-9]+|-hd-tile-[0-9]+-[0-9]+)?|",
        r"-ad|-spm|_thumbnail|-transcoded|(?:_thumb)?\.jpg|\.ttf|\.otf|\.woff|\.woff2|\.eot|",
        r"\.dfont|\.bin)?$",
    ))
    .unwrap()
});

/// `_ID_DIGITS` (:110): Previewer's base-32 digits.
const ID_DIGITS: &str = "0123456789ABCDEFGHJKMNPRSTUVWXYZ";

/// `_max_id` (:123): one past the highest short id, read in Previewer's base 32.
fn max_id(names: &[String]) -> i64 {
    let mut top = -1;
    for name in names {
        if let Some(digits) = SHORT_ID.captures(name).and_then(|c| c.get(1)) {
            let value = digits
                .as_str()
                .chars()
                .fold(0, |v, d| v * 32 + ID_DIGITS.find(d).unwrap_or(0) as i64);
            top = top.max(value);
        }
    }
    top + 1
}
