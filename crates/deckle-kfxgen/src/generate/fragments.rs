// SPDX-License-Identifier: GPL-3.0-or-later
//! The `$145` content, `$259` storyline and `$260` section fragments a chapter's chunks make,
//! and the `$157` names a storyline uses.
use std::collections::BTreeSet;

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::{Struct, Value};

use super::chunks::{Chunk, Marker, Node};
use super::frames;
use super::images;
use super::inline::{self, EntryLink, Run};
use super::inline_picture_entries::InlineStyles;
use super::notes::Popups;
use super::table_styles::column_widths_152;
use super::{GenerateError, Generator, measure, py, st, sym};

/// `MAX_CONTENT_FRAGMENT_SIZE` (:1585).
const MAX_CONTENT_FRAGMENT_SIZE: usize = 8192;

/// `build_fragment_145` (:3545).
pub(crate) fn build_fragment_145(g: &mut Generator, strings: Vec<String>, name: &str) -> Fragment {
    g.local(name);
    let value = st([
        ("name", sym(name)),
        (
            "$146",
            Value::List(strings.into_iter().map(Value::String).collect()),
        ),
    ]);
    Fragment::new("$145", Some(name), value)
}

/// A `$145` fragment's name and strings.
pub(crate) type ContentGroup = (String, Vec<String>);
/// Where a string is: its `$145` fragment and its index there.
pub(crate) type ContentRef = (String, i64);

/// `pack_content_fragments` (:1675): a chapter's strings in `$145` fragments under kfxlib's
/// cap, which charges a fragment the UTF-8 bytes of all its strings but the last.
pub(crate) fn pack_content_fragments(
    strings: &[&str],
    mut index: usize,
) -> (Vec<ContentGroup>, Vec<ContentRef>) {
    let mut name = format!("content_{index}");
    let (mut groups, mut refs) = (Vec::new(), Vec::new());
    let mut current: Vec<String> = Vec::new();
    let mut committed = 0;
    for s in strings {
        if let Some(last) = current.last() {
            let projected = committed + last.len();
            if projected >= MAX_CONTENT_FRAGMENT_SIZE {
                groups.push((name, std::mem::take(&mut current)));
                index += 1;
                name = format!("content_{index}");
                committed = 0;
            } else {
                committed = projected;
            }
        }
        refs.push((name.clone(), current.len() as i64));
        current.push((*s).to_owned());
    }
    groups.push((name, current));
    (groups, refs)
}

/// `build_fragment_259` (:4199): one entry per chunk, text or picture, the first leaf marked
/// `$790`, a text entry with its link and runs. A table's `open` starts a container whose
/// `$146` holds the entries up to its `close` (#219): `$278` table, `$151`/`$454`/`$455` row
/// group, `$279` row, `$269` cell of several blocks, `$270` box (#238), `$269` a linked
/// picture's (#355). `parts`: a row's, group's, box's or link's own `$157`; a link's anchor is
/// its `runs`' link. A rule is a `$596` entry (#230); a pop-up note's entry is marked `$615`
/// (#445). A text entry holding pictures holds them among its words (#529).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_fragment_259(
    g: &mut Generator,
    styles: &[String],
    parts: &[Option<String>],
    refs: &[Option<ContentRef>],
    name: &str,
    positions: &[Option<i64>],
    runs: &[(Option<EntryLink>, Vec<Run>)],
    chunks: &[Chunk],
    popups: &Popups,
    inline_styles: &mut InlineStyles,
) -> Result<Fragment, GenerateError> {
    g.local(name);
    for (content, _) in refs.iter().flatten() {
        g.local(content);
    }
    let mut root = Vec::new();
    // Each open container and its entries so far.
    let mut open: Vec<(Struct, Vec<Value>)> = Vec::new();
    let mut first_leaf = true;
    for (i, chunk) in chunks.iter().enumerate() {
        let style = &styles[i];
        let node = match &chunk.marker {
            Some(Marker::Close) => {
                if let Some((mut entry, kids)) = open.pop() {
                    entry.insert("$146", Value::List(kids));
                    match open.last_mut() {
                        Some((_, siblings)) => siblings.push(Value::Struct(entry)),
                        None => root.push(Value::Struct(entry)),
                    }
                }
                continue;
            }
            Some(Marker::Open(node)) => Some(node),
            None => None,
        };
        let position = positions[i].expect("only a close takes no position");
        if let Some(node) = node {
            let Value::Struct(mut entry) = st([
                ("$155", Value::Int(position)),
                ("$159", sym(node_type(node))),
            ]) else {
                unreachable!()
            };
            match node {
                Node::Table(t) => {
                    g.local(style);
                    entry.insert("$157", sym(style));
                    entry.insert("$150", Value::Bool(false));
                    for key in ["$456", "$457"] {
                        entry.insert(key, measure(py::decimal("0.9"), "$318"));
                    }
                    if let Some(widths) = column_widths_152(t.column_widths.as_ref()) {
                        entry.insert("$152", widths);
                    }
                    if t.zoom {
                        // The table viewer's zoom button, on every table of 2+ columns (#272).
                        entry.insert("$629", Value::List(vec![sym("$581"), sym("$326")]));
                        entry.insert("$630", sym("$632"));
                    }
                }
                Node::Cell(_) => {
                    g.local(style);
                    entry.insert("$157", sym(style));
                }
                Node::Link(_) => {
                    if let Some((anchor, ..)) = &runs[i].0 {
                        let link = st([
                            ("$143", Value::Int(0)),
                            ("$144", Value::Int(1)),
                            ("$179", sym(anchor)),
                        ]);
                        entry.insert("$142", Value::List(vec![link]));
                    }
                    if let Some(part) = &parts[i] {
                        entry.insert("$157", sym(part));
                    }
                }
                Node::Group(..) | Node::Row(..) | Node::Box(_) => {
                    if let Some(part) = &parts[i] {
                        entry.insert("$157", sym(part));
                    }
                }
            }
            open.push((entry, Vec::new()));
            continue;
        }
        if chunk.rule.is_some() {
            g.local(style);
            let Value::Struct(mut entry) = st([
                ("$155", Value::Int(position)),
                ("$157", sym(style)),
                ("$159", sym("$596")),
            ]) else {
                unreachable!()
            };
            if first_leaf {
                entry.insert("$790", Value::Int(1));
                first_leaf = false;
            }
            match open.last_mut() {
                Some((_, siblings)) => siblings.push(Value::Struct(entry)),
                None => root.push(Value::Struct(entry)),
            }
            continue;
        }
        let mut entry = match &chunk.picture {
            Some(p) => images::picture_entry(g, p, position, style),
            None => {
                let Value::Struct(mut entry) = st([
                    ("$155", Value::Int(position)),
                    ("$157", sym(style)),
                    ("$159", sym("$269")),
                ]) else {
                    unreachable!()
                };
                if let Some((content, index)) = &refs[i] {
                    entry.insert(
                        "$145",
                        st([("name", sym(content)), ("$403", Value::Int(*index))]),
                    );
                }
                entry
            }
        };
        if first_leaf {
            entry.insert("$790", Value::Int(1));
            first_leaf = false;
        }
        if chunk.picture.is_none() {
            if popups.positions.contains(&position) {
                entry.insert("$615", sym("$281"));
            }
            if !chunk.pictures.is_empty() {
                let mixed = inline_styles.mixed(g, &chunk.text, &chunk.pictures)?;
                entry.insert("$146", mixed);
            }
            let (link, runs) = &runs[i];
            inline::write_runs(g, &mut entry, link.as_ref(), runs);
        }
        match open.last_mut() {
            Some((_, siblings)) => siblings.push(Value::Struct(entry)),
            None => root.push(Value::Struct(entry)),
        }
    }
    let value = st([("$176", sym(name)), ("$146", Value::List(root))]);
    Ok(Fragment::new("$259", Some(name), value))
}

/// `_TABLE_NODE_TYPES` (:702).
fn node_type(node: &Node) -> &'static str {
    match node {
        Node::Table(_) => "$278",
        Node::Group("head", ..) => "$151",
        Node::Group("foot", ..) => "$455",
        Node::Group(..) => "$454",
        Node::Row(..) => "$279",
        Node::Cell(_) | Node::Link(_) => "$269",
        // A frame is Previewer's plain container (:7485, #382, #565).
        Node::Box(b) if frames::is_frame(&b.spec) => "$269",
        Node::Box(_) => "$270",
    }
}

/// `build_fragment_260` (:4500).
pub(crate) fn build_fragment_260(
    g: &mut Generator,
    storyline: &str,
    section: &str,
    position: i64,
) -> Fragment {
    g.local(section);
    g.local(storyline);
    let entry = st([
        ("$155", Value::Int(position)),
        ("$176", sym(storyline)),
        ("$159", sym("$269")),
    ]);
    let value = st([("$174", sym(section)), ("$141", Value::List(vec![entry]))]);
    Fragment::new("$260", Some(section), value)
}

/// The `$157` names `value`'s `$259` entries apply, as `_refs` (:8102) collects them.
pub(crate) fn style_refs(value: &Value, out: &mut BTreeSet<String>) {
    for entry in value.as_list().into_iter().flatten() {
        let Some(e) = entry.as_struct() else {
            continue;
        };
        if let Some(name) = e.get("$157").and_then(Value::as_sym) {
            out.insert(name.0.clone());
        }
        for span in e.get("$142").and_then(Value::as_list).into_iter().flatten() {
            if let Some(name) = span
                .as_struct()
                .and_then(|s| s.get("$157"))
                .and_then(Value::as_sym)
            {
                out.insert(name.0.clone());
            }
        }
        if let Some(kids) = e.get("$146") {
            style_refs(kids, out);
        }
    }
}
