// SPDX-License-Identifier: GPL-3.0-only
//! The book's files as chapter assembly takes them: kfxgen's per-file loop
//! (`converter.py` `extract_chapters_from_oeb` up to `_assemble_chapters_by_coordinate`, at
//! kfxgen `ad38ec8`), the chapter dicts, and `_split_by_writing_mode`.
//!
//! Each pass extracts every file once, charged to a [`TextBudget`] of the file's own, made new
//! for the pass (the same one the pass's note and link scans charge), so a file the first pass
//! fits is not refused by the second. The keys this loop copies (an empty file's ids, a file's
//! trailing ids) are charged to it too, in whole 64-byte units, so a key under 64 bytes is
//! free and the book's node limit bounds how many there are. So the work and memory of a pass
//! are bounded by the budgets times the files and by the node limit; the second pass, which
//! kfxgen runs after dropping the first's chapters, at most doubles the work.

use std::collections::{HashMap, HashSet};

use deckle_limits::MAX_CASCADE_DECLS;

use super::args::is_vertical;
use super::blocks::{Block, DictIndex, Field, dedupe, dict_set, s, strs};
use super::href::{key_doc, normalize, unquote};
use super::inline::{Walk, keyword, own_anchor_ids};
use super::notes::{book_link_targets, note_pair_ids, note_target_ids};
use super::toc::{TocEntry, respell_toc_hrefs, spine_in_order, toc_entries};
use super::{
    CalibreOpf, ConvertOptions, Error, ExtractOptions, HORIZONTAL_PT, ItemError, Resolver, Serials,
    TextBudget, VERTICAL_PT, extract_blocks, page_width,
};
use crate::book::{self, Book};
use crate::css::{DEFAULT_PROFILE, TEXT_UNIT};
use crate::dom::{Dom, NodeId};
use crate::generate::chapters::{CHAPTER_KEYS, IGNORED_KEYS};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// One shown spine file as kfxgen hands it to chapter assembly (an item of
/// `spine_items_ordered`). kfxgen's `breaks_known` is always true here: every file has its
/// style resolver.
#[derive(Debug)]
pub struct SpineItem {
    pub href: String,
    /// Its blocks' texts, a blank line between.
    pub text: String,
    pub blocks: Vec<Block>,
    /// Each contents listing left out: the index of the block after it, and its ids.
    pub nav_listing_at: Vec<(usize, Vec<String>)>,
    /// The ids that are notes: contents entries naming one are skipped.
    pub note_ids: HashSet<String>,
    /// The ids at its very end with no block after them (`frozenset(end_ids)`).
    pub trailing_ids: HashSet<String>,
}

impl SpineItem {
    /// The dict as kfxgen holds it, sets sorted.
    pub fn to_py(&self) -> PyValue {
        let set = |ids: &HashSet<String>| {
            let mut ids: Vec<&String> = ids.iter().collect();
            ids.sort();
            PyValue::Set(ids.into_iter().map(|i| s(i)).collect())
        };
        let listings = self
            .nav_listing_at
            .iter()
            .map(|(at, ids)| {
                let ids = ids.iter().map(|i| s(i)).collect();
                PyValue::Tuple(vec![PyValue::Int(*at as i64), PyValue::Set(ids)])
            })
            .collect();
        PyValue::Dict(vec![
            (s("href"), s(&self.href)),
            (s("text"), s(&self.text)),
            (
                s("blocks"),
                PyValue::List(self.blocks.iter().map(Block::to_py).collect()),
            ),
            (s("nav_listing_at"), PyValue::List(listings)),
            (s("note_ids"), set(&self.note_ids)),
            (s("trailing_ids"), set(&self.trailing_ids)),
            (s("breaks_known"), PyValue::Bool(true)),
        ])
    }

    /// An item from its dict as the assembly dump holds it, for tests that feed kfxgen's
    /// recorded files to assembly. None for what does not read as one, or for a file kfxgen
    /// read without its styles (`breaks_known` false), which the port never has.
    pub fn from_py(v: &PyValue) -> Option<SpineItem> {
        let str_of = |k: &str| v.get(k)?.as_str().map(str::to_owned);
        let strings = |v: &PyValue| -> Option<Vec<String>> {
            let (PyValue::Set(items) | PyValue::List(items)) = v else {
                return None;
            };
            items
                .iter()
                .map(|i| i.as_str().map(str::to_owned))
                .collect()
        };
        if v.get("breaks_known") != Some(&PyValue::Bool(true)) {
            return None;
        }
        let mut next = 0;
        let blocks = v.get("blocks")?.as_list()?;
        let blocks = blocks.iter().map(|b| Block::from_py(b, &mut next));
        let listings = v.get("nav_listing_at")?.as_list()?;
        let listings = listings.iter().map(|l| match l.as_list()? {
            [at, ids] => Some((usize::try_from(at.as_int()?).ok()?, strings(ids)?)),
            _ => None,
        });
        Some(SpineItem {
            href: str_of("href")?,
            text: str_of("text")?,
            blocks: blocks.collect::<Option<_>>()?,
            nav_listing_at: listings.collect::<Option<_>>()?,
            note_ids: strings(v.get("note_ids")?)?.into_iter().collect(),
            trailing_ids: strings(v.get("trailing_ids")?)?.into_iter().collect(),
        })
    }
}

/// One of kfxgen's chapter dicts: its keys in insertion order, `blocks` holding blocks.
#[derive(Debug)]
pub struct Chapter {
    pub(super) fields: Vec<(&'static str, Field)>,
}

impl Chapter {
    /// `{"title": …, "text": …}` and then `pairs`.
    pub(super) fn new(title: String, text: String, pairs: Vec<(&'static str, Field)>) -> Self {
        let mut fields = vec![
            ("title", Field::Py(PyValue::Str(title))),
            ("text", Field::Py(PyValue::Str(text))),
        ];
        fields.extend(pairs);
        Chapter { fields }
    }

    /// `ch.get(key)` for a model value.
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.fields.iter().find_map(|(k, v)| match v {
            Field::Py(p) if *k == key => Some(p),
            _ => None,
        })
    }

    /// `ch[key] = value`: in place when the key is there, else at the end.
    pub(super) fn set(&mut self, key: &'static str, value: PyValue) {
        match self.fields.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = Field::Py(value),
            None => self.fields.push((key, Field::Py(value))),
        }
    }

    /// `ch.get("blocks") or []`.
    pub fn blocks(&self) -> &[Block] {
        self.fields
            .iter()
            .find_map(|(k, v)| match v {
                Field::Blocks(b) if *k == "blocks" => Some(b.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    /// A chapter from its dict as a dump holds it, for tests that feed crafted chapters to the
    /// title-page step; block ids numbered from `next`. None for a key no chapter holds (but
    /// `_listing_keys`, which the title-page step reads), or for what is no dict.
    pub fn from_py(v: &PyValue, next: &mut usize) -> Option<Chapter> {
        let PyValue::Dict(pairs) = v else {
            return None;
        };
        let known = CHAPTER_KEYS
            .iter()
            .chain(&IGNORED_KEYS)
            .chain(&["_listing_keys"]);
        let mut fields = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            let k = k.as_str()?;
            let key = *known.clone().find(|n| **n == k)?;
            let field = if key == "blocks" {
                let blocks = v.as_list()?.iter().map(|b| Block::from_py(b, next));
                Field::Blocks(blocks.collect::<Option<_>>()?)
            } else {
                Field::Py(v.clone())
            };
            fields.push((key, field));
        }
        Some(Chapter { fields })
    }

    /// `ch.pop(key, None)`.
    pub(super) fn remove(&mut self, key: &str) -> Option<Field> {
        let at = self.fields.iter().position(|(k, _)| *k == key)?;
        Some(self.fields.remove(at).1)
    }

    /// The chapter's blocks taken out, to be cut; [`Chapter::put_blocks`] puts some back.
    pub(super) fn take_blocks(&mut self) -> Vec<Block> {
        self.fields
            .iter_mut()
            .find_map(|(k, v)| match v {
                Field::Blocks(b) if *k == "blocks" => Some(std::mem::take(b)),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// `ch["blocks"] = blocks`, in place: a planned chapter's blocks put in.
    pub(super) fn put_blocks(&mut self, blocks: Vec<Block>) {
        if let Some((_, v)) = self.fields.iter_mut().find(|(k, _)| *k == "blocks") {
            *v = Field::Blocks(blocks);
        }
    }

    /// The dict as the model holds it.
    pub fn to_py(&self) -> PyValue {
        PyValue::Dict(
            self.fields
                .iter()
                .map(|(k, v)| {
                    let v = match v {
                        Field::Py(p) => p.clone(),
                        Field::Blocks(b) => PyValue::List(b.iter().map(Block::to_py).collect()),
                    };
                    (s(k), v)
                })
                .collect(),
        )
    }
}

/// `"\n\n".join(b["text"] for b in blocks if b.get("text"))`.
pub(super) fn joined_text<'b>(blocks: impl IntoIterator<Item = &'b Block>) -> String {
    let texts: Vec<&str> = blocks
        .into_iter()
        .filter_map(|b| b.get("text").and_then(PyValue::as_str))
        .filter(|t| !t.is_empty())
        .collect();
    texts.join("\n\n")
}

/// `extract_chapters_from_oeb`'s files, up to chapter assembly: the book's contents entries
/// and, for each spine file in kfxgen's order that shows something, its blocks
/// ([`SpineItem`]), at the page width kfxgen extracts the book at. `keep_nav_listings` is the
/// second pass, which prints the book's contents listings; each call charges a new
/// [`TextBudget`] for each of [`Book::files`]; `serials` runs on across the book and its
/// passes.
///
/// A file kfxgen raises on is left out, as kfxgen's `try` round each file leaves it ("Spine
/// item N parse failed"). What the port refuses stops the book: a feature refused by name
/// ([`Error::Unsupported`]) and a limit passed ([`Error::TooLarge`], a file's budget named as
/// such). [`Error::NoContent`] when no file shows anything.
pub fn spine_items(
    book: &Book,
    opf: &CalibreOpf,
    opts: &ConvertOptions,
    serials: &mut Serials,
    keep_nav_listings: bool,
) -> Result<(Vec<SpineItem>, Vec<TocEntry>), Error> {
    let budgets: Vec<TextBudget> = book.files.iter().map(|_| TextBudget::new()).collect();
    // Percentages are measured against a 480pt page in a vertical book, 384pt otherwise.
    let _page = page_width(if is_vertical(book) {
        VERTICAL_PT
    } else {
        HORIZONTAL_PT
    });
    let native_tables = !opts.disable_native_tables;
    let book_wide = |e: Error| match budget_spent(book, &budgets) {
        Some(i) => budget_refusal(e, &book.files[i].href, &budgets[i]),
        None => e,
    };
    // kfxgen passes None without native tables; the walk reads the targets only with them.
    let link_targets = if native_tables {
        book_link_targets(book, &budgets).map_err(book_wide)?
    } else {
        HashSet::new()
    };
    let note_pairs = if book.oeb.auto_generated_toc {
        note_pair_ids(book, &budgets).map_err(book_wide)?
    } else {
        HashMap::new()
    };
    let mut entries = toc_entries(book, opf);
    let spine = spine_in_order(book, opf, &entries);
    respell_toc_hrefs(&mut entries, book, &spine);
    let toc_targets = toc_fragments(&entries);

    let mut items: Vec<SpineItem> = Vec::new();
    // The keys of files that show nothing, and of ids ending a file: the next shown block's.
    let mut orphan_keys: Vec<String> = Vec::new();
    for &f in &spine {
        let (file, budget) = (&book.files[f], &budgets[f]);
        if is_calibre_inline_toc(&file.dom) {
            continue;
        }
        let resolver = Resolver::new(file, &DEFAULT_PROFILE, budget);
        let href = file.href.as_str();
        let mut note_ids = note_target_ids(&file.dom);
        let doc = if href.is_empty() {
            String::new()
        } else {
            key_doc(href, "")
        };
        if let Some(pairs) = note_pairs.get(&doc) {
            note_ids.extend(pairs.iter().cloned());
        }
        let no_targets = HashSet::new();
        let extract = ExtractOptions {
            native_tables,
            toc_targets: toc_targets.get(&normalize(href)).unwrap_or(&no_targets),
            note_ids: &note_ids,
            link_targets: &link_targets,
            keep_nav_listings,
        };
        let walked = extract_blocks(&mut Walk::new(file, &resolver, serials), &extract)
            .and_then(|got| Ok((file_writing_mode(&file.dom, &resolver)?, got)));
        let (writing_mode, mut got) = match walked {
            Ok(walked) => walked,
            // kfxgen logs the file as failed and goes on without it.
            Err(ItemError::Raised(_)) => continue,
            Err(ItemError::Error(e)) => return Err(budget_refusal(e, href, budget)),
        };
        let text = joined_text(&got.blocks);
        // Each block carries its file's writing mode, so a chapter spanning files knows its
        // own; a file that sets none is horizontal.
        let mode = if writing_mode == "vertical-rl" {
            "vertical-rl"
        } else {
            "horizontal-tb"
        };
        for b in &mut got.blocks {
            b.set("writing_mode", s(mode));
        }
        if py::strip(&text).is_empty() {
            if !doc.is_empty() {
                orphan_keys.push(doc.clone());
                for n in file.dom.descendants() {
                    for id in own_anchor_ids(&file.dom, n) {
                        orphan_keys.push(charged_key(budget, &doc, id, href)?);
                    }
                }
            }
            continue;
        }
        if !orphan_keys.is_empty() {
            prepend_keys(&mut got.blocks[0], std::mem::take(&mut orphan_keys));
        }
        // Ids at the file's very end name what follows: their keys go to the next shown
        // block, as an empty file's do; the ids stay with the file for assembly.
        if !got.trailing_ids.is_empty() && !href.is_empty() {
            let moved = got
                .trailing_ids
                .iter()
                .map(|id| charged_key(budget, &doc, id, href))
                .collect::<Result<Vec<_>, _>>()?;
            drop_keys(&mut got.blocks, &moved.iter().cloned().collect());
            orphan_keys.extend(moved);
        }
        items.push(SpineItem {
            href: href.to_owned(),
            text,
            blocks: got.blocks,
            nav_listing_at: got.nav_listing_at,
            note_ids,
            trailing_ids: got.end_ids.into_iter().collect(),
        });
    }
    // Nothing shown followed: the book's last block takes them.
    if !orphan_keys.is_empty()
        && let Some(last) = items.last_mut().and_then(|i| i.blocks.last_mut())
    {
        let at =
            PyValue::Int(py::len(last.get("text").and_then(PyValue::as_str).unwrap_or("")) as i64);
        let mut keys = strings(last.get("anchor_keys"));
        keys.extend(orphan_keys.iter().cloned());
        last.set("anchor_keys", strs(&dedupe(keys)));
        if last.get("anchor_offsets").is_none() {
            last.set("anchor_offsets", PyValue::Dict(Vec::new()));
        }
        if let Some(PyValue::Dict(offsets)) = last.get_mut("anchor_offsets") {
            let mut index = DictIndex::new(offsets);
            for k in &orphan_keys {
                index.set(offsets, k, at.clone());
            }
        }
    }
    if items.is_empty() {
        return Err(Error::NoContent);
    }
    Ok((items, entries))
}

/// `toc_targets`: for each file name the entries name, the fragments they name, each also
/// decoded once.
pub(super) fn toc_fragments(entries: &[TocEntry]) -> HashMap<String, HashSet<String>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for e in entries {
        let frag = fragment(&e.href);
        if !frag.is_empty() {
            let at = out.entry(normalize(&e.href)).or_default();
            at.insert(frag.to_owned());
            at.insert(unquote(frag));
        }
    }
    out
}

/// `_href_fragment`: what follows the first `#`, or "".
pub(super) fn fragment(href: &str) -> &str {
    href.split_once('#').map_or("", |(_, f)| f)
}

/// The first of the book's files whose budget is spent, if any.
fn budget_spent(book: &Book, budgets: &[TextBudget]) -> Option<usize> {
    (0..book.files.len()).find(|&i| budgets[i].charged() > MAX_CASCADE_DECLS)
}

/// A budget refusal named for what it is: the budget counts the text and styles the
/// converter copies or reads in a file, not only its CSS, which the message the budget gives
/// (`css::Error::Cascade`'s) would say. Any other error unchanged.
fn budget_refusal(e: Error, href: &str, budget: &TextBudget) -> Error {
    match e {
        Error::Book(book::Error::TooLarge { .. }) if budget.charged() > MAX_CASCADE_DECLS => {
            Error::TooLarge {
                what: format!(
                    "{href}: converting this file copies or reads more than {} bytes of its \
                     text and styles",
                    MAX_CASCADE_DECLS * TEXT_UNIT
                ),
            }
        }
        e => e,
    }
}

/// `"<doc>#<id>"`, charged to the file's budget.
fn charged_key(budget: &TextBudget, doc: &str, id: &str, href: &str) -> Result<String, Error> {
    budget
        .charge(doc.len() + 1 + id.len())
        .map_err(|e| budget_refusal(e.into(), href, budget))?;
    Ok(format!("{doc}#{id}"))
}

/// `_is_calibre_inline_toc`: the `<body>` (XHTML's first, else one in no namespace) has the
/// id Calibre gives the contents page it makes for MOBI output.
fn is_calibre_inline_toc(dom: &Dom) -> bool {
    let root = dom.root();
    let below = || dom.descendants().filter(move |&n| n != root);
    let body = below()
        .find(|&n| dom.qualified(n) == (Some(crate::dom::XHTML_NS), "body"))
        .or_else(|| below().find(|&n| dom.qualified(n) == (None, "body")));
    body.is_some_and(|b| dom.attr(b, "id") == Some("calibre_generated_inline_toc"))
}

/// `_file_writing_mode`'s mode before its check: the first element named `body` (the root
/// itself if none), its resolved `writing-mode`, stripped and lower-cased.
fn file_writing_mode(dom: &Dom, resolver: &Resolver) -> Result<String, ItemError> {
    let body: NodeId = dom
        .descendants()
        .find(|&n| dom.tag(n) == "body")
        .unwrap_or(dom.root());
    keyword(resolver.resolve(body)?.as_ref(), "writing-mode")
}

/// A list of strings' values.
fn strings(v: Option<&PyValue>) -> Vec<String> {
    v.and_then(PyValue::as_list)
        .unwrap_or(&[])
        .iter()
        .filter_map(|i| i.as_str().map(str::to_owned))
        .collect()
}

/// `_prepend_keys(block, keys)`: `keys` first on the block, its offsets 0; a native table's
/// own keys get them first too.
fn prepend_keys(block: &mut Block, keys: Vec<String>) {
    let mut all = keys.clone();
    all.extend(strings(block.get("anchor_keys")));
    block.set("anchor_keys", strs(&dedupe(all)));
    if block.get("anchor_offsets").is_none() {
        block.set("anchor_offsets", PyValue::Dict(Vec::new()));
    }
    if let Some(PyValue::Dict(offsets)) = block.get_mut("anchor_offsets") {
        let mut at = DictIndex::new(offsets);
        for k in &keys {
            at.set(offsets, k, PyValue::Int(0));
        }
    }
    if let Some(PyValue::Dict(tbl)) = block.get_mut("table") {
        let mut all = keys;
        all.extend(strings(
            tbl.iter()
                .find(|(k, _)| k.as_str() == Some("anchor_keys"))
                .map(|(_, v)| v),
        ));
        dict_set(tbl, "anchor_keys", strs(&dedupe(all)));
    }
}

/// What [`drop_keys`] takes keys off: a block; a block dict in a box in a table cell (as a
/// block: its table and a box's blocks too); a cell paragraph (a box's blocks too).
enum Part<'b> {
    Block(&'b mut Block),
    Dict(&'b mut PyValue),
    Para(&'b mut PyValue),
}

/// `_drop_keys(blocks, keys)`: `keys` off every block, box block, table, row, cell and cell
/// paragraph holding them, however deep the boxes nest (a stack, not recursion).
pub(super) fn drop_keys(blocks: &mut [Block], keys: &HashSet<String>) {
    let mut stack: Vec<Part> = blocks.iter_mut().map(Part::Block).collect();
    while let Some(part) = stack.pop() {
        let (d, block) = match part {
            Part::Block(b) => {
                for key in ["anchor_keys", "anchor_offsets"] {
                    if let Some(v) = b.get_mut(key) {
                        drop_from(v, keys);
                    }
                }
                if b.kind() == Some("box") {
                    if let Some(kids) = b.blocks_mut() {
                        stack.extend(kids.iter_mut().map(Part::Block));
                    }
                } else if let Some(t) = b.get_mut("table") {
                    table_parts(t, keys, &mut stack);
                }
                continue;
            }
            Part::Dict(PyValue::Dict(d)) => (d, true),
            Part::Para(PyValue::Dict(d)) => (d, false),
            Part::Dict(_) | Part::Para(_) => continue,
        };
        let boxed = d
            .iter()
            .any(|(k, v)| k.as_str() == Some("type") && v.as_str() == Some("box"));
        for (k, v) in d.iter_mut() {
            match k.as_str() {
                Some("anchor_keys" | "anchor_offsets") => drop_from(v, keys),
                Some("blocks") if boxed => stack.extend(items(v).map(Part::Dict)),
                // A paragraph's table is not a part kfxgen looks into.
                Some("table") if block => table_parts(v, keys, &mut stack),
                _ => {}
            }
        }
    }
}

/// A table's own keys, its rows' and its cells' taken off; its cells' paragraphs onto
/// `stack`. A table that is no dict, or an empty one, is left.
fn table_parts<'b>(t: &'b mut PyValue, keys: &HashSet<String>, stack: &mut Vec<Part<'b>>) {
    let PyValue::Dict(tbl) = t else {
        return;
    };
    for (k, v) in tbl.iter_mut() {
        match k.as_str() {
            Some("anchor_keys" | "anchor_offsets") => drop_from(v, keys),
            Some("rows") => {
                for row in items(v) {
                    let PyValue::Dict(row) = row else {
                        continue;
                    };
                    for (k, v) in row.iter_mut() {
                        match k.as_str() {
                            Some("anchor_keys" | "anchor_offsets") => drop_from(v, keys),
                            Some("cells") => {
                                for cell in items(v) {
                                    let PyValue::Dict(cell) = cell else {
                                        continue;
                                    };
                                    for (k, v) in cell.iter_mut() {
                                        match k.as_str() {
                                            Some("anchor_keys" | "anchor_offsets") => {
                                                drop_from(v, keys)
                                            }
                                            Some("paragraphs") => {
                                                stack.extend(items(v).map(Part::Para))
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// A list's items, to change in place; none for what is no list.
fn items(v: &mut PyValue) -> impl Iterator<Item = &mut PyValue> {
    match v {
        PyValue::List(items) => Some(items.iter_mut()),
        _ => None,
    }
    .into_iter()
    .flatten()
}

/// `keys` taken out of a list of keys, or out of a dict of offsets.
fn drop_from(v: &mut PyValue, keys: &HashSet<String>) {
    let kept = |k: &PyValue| k.as_str().is_none_or(|k| !keys.contains(k));
    match v {
        PyValue::List(items) => items.retain(kept),
        PyValue::Dict(pairs) => pairs.retain(|(k, _)| kept(k)),
        _ => {}
    }
}

/// `_split_by_writing_mode`: each chapter cut where its blocks' writing mode changes, a block
/// of no mode staying with the one before. The first part keeps the chapter's keys; the
/// others are `{title, text, blocks}` left out of the contents with no title printed. A
/// contents page kfxgen built (`toc_links`) is left whole.
pub fn split_by_writing_mode(chapters: Vec<Chapter>) -> Vec<Chapter> {
    let mut out = Vec::with_capacity(chapters.len());
    for mut ch in chapters {
        let at = ch
            .fields
            .iter()
            .position(|(k, v)| *k == "blocks" && matches!(v, Field::Blocks(_)));
        let cut = at.is_some() && !ch.get("toc_links").is_some_and(PyValue::is_truthy);
        let Some(at) = at.filter(|_| cut) else {
            out.push(ch);
            continue;
        };
        let Field::Blocks(blocks) =
            std::mem::replace(&mut ch.fields[at].1, Field::Blocks(Vec::new()))
        else {
            unreachable!("found as blocks above");
        };
        let mut groups: Vec<(Option<PyValue>, Vec<Block>)> = Vec::new();
        for b in blocks {
            let mode = b
                .get("writing_mode")
                .filter(|m| **m != PyValue::None)
                .cloned();
            match groups.last_mut() {
                Some((m, part)) if mode.is_none() || mode == *m => part.push(b),
                _ => groups.push((mode, vec![b])),
            }
        }
        if groups.len() < 2 {
            ch.fields[at].1 = Field::Blocks(groups.pop().map(|(_, b)| b).unwrap_or_default());
            out.push(ch);
            continue;
        }
        let title = ch.get("title").cloned().unwrap_or_else(|| s(""));
        let mut groups = groups.into_iter();
        if let Some((_, part)) = groups.next() {
            ch.set("text", s(&joined_text(&part)));
            ch.fields[at].1 = Field::Blocks(part);
            out.push(ch);
        }
        for (_, part) in groups {
            let text = joined_text(&part);
            out.push(Chapter {
                fields: vec![
                    ("title", Field::Py(title.clone())),
                    ("text", Field::Py(s(&text))),
                    ("blocks", Field::Blocks(part)),
                    ("_omit_from_toc", Field::Py(PyValue::Bool(true))),
                    ("_omit_title_heading", Field::Py(PyValue::Bool(true))),
                ],
            });
        }
    }
    out
}
