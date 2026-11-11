// SPDX-License-Identifier: GPL-3.0-or-later
//! The `$145` content, `$259` storyline and `$260` section fragments a chapter's chunks make,
//! and the `$157` names a storyline uses.
use std::collections::BTreeSet;

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::Value;

use super::chunks::Chunk;
use super::images;
use super::inline::{self, EntryLink, Run};
use super::{Generator, st, sym};

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

/// `build_fragment_259` (:4199): one entry per chunk, text or picture, the first marked
/// `$790`, a text entry with its link and runs.
pub(crate) fn build_fragment_259(
    g: &mut Generator,
    styles: &[String],
    refs: &[Option<ContentRef>],
    name: &str,
    positions: &[i64],
    runs: &[(Option<EntryLink>, Vec<Run>)],
    chunks: &[Chunk],
) -> Fragment {
    g.local(name);
    for (content, _) in refs.iter().flatten() {
        g.local(content);
    }
    let entries = styles
        .iter()
        .zip(refs)
        .zip(positions)
        .zip(runs)
        .zip(chunks)
        .enumerate()
        .map(|(i, ((((style, r), &position), (link, runs)), chunk))| {
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
                    if let Some((content, index)) = r {
                        entry.insert(
                            "$145",
                            st([("name", sym(content)), ("$403", Value::Int(*index))]),
                        );
                    }
                    entry
                }
            };
            if i == 0 {
                entry.insert("$790", Value::Int(1));
            }
            if chunk.picture.is_none() {
                inline::write_runs(g, &mut entry, link.as_ref(), runs);
            }
            Value::Struct(entry)
        })
        .collect();
    let value = st([("$176", sym(name)), ("$146", Value::List(entries))]);
    Fragment::new("$259", Some(name), value)
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
