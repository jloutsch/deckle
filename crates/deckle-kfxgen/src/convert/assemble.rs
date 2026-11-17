// SPDX-License-Identifier: GPL-3.0-only
//! Chapter assembly (`converter.py` `_assemble_chapters_by_coordinate`, `_section_heads` and
//! their helpers, at kfxgen `ad38ec8`), and the chapter for each file kfxgen falls back to
//! when no contents entry lands (`extract_chapters_from_oeb`, before
//! `_listing_without_contents`).
//!
//! kfxgen looks up each entry's file with `list.index`, scans the blocks between two entries
//! for a page break, follows chains of entries on one block one link at a time and tests
//! every listing against every section; each is quadratic in entries, blocks or listings on a
//! hostile book. Here a file is found through a map, the page breaks are counted once
//! (prefix sums), each entry's twin is kept as it is made, and the listings are sorted and
//! searched: assembly is linear in files, blocks, ids and entries, but for sorting.
//!
//! Assembly first works out every chapter over the book's blocks, changing them only as
//! kfxgen does before it makes a chapter (`_spine_file`, an entry's key); then the blocks are
//! moved into their chapters. A block is in one chapter at most: the sections start at
//! entries in document order, so the slices kfxgen takes do not overlap.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use super::blocks::{Block, Field, s};
use super::chapters::{Chapter, SpineItem, fragment, joined_text, toc_fragments};
use super::href::{key_doc, normalize, unquote};
use super::inline::words;
use super::toc::TocEntry;
use crate::generate::images::IMG_TOKEN;
use crate::generate::py;
use crate::pyvalue::{PyValue, truthy};

/// `_CONTENTS_TITLES`.
const CONTENTS_TITLES: [&str; 2] = ["contents", "table of contents"];

/// `CONTENTS_SKIP_TITLES`: `TITLE_PAGE_TITLES`, `HALF_TITLE_TITLES` and the rest.
const CONTENTS_SKIP_TITLES: [&str; 15] = [
    "title page",
    "title",
    "half title",
    "half-title",
    "half title page",
    "half-title page",
    "halftitle",
    "halftitle page",
    "bastard title",
    "cover",
    "contents",
    "front matter",
    "table of contents",
    "copyright",
    "copyright page",
];

/// `SMALL_TEXT_CHAPTERS`.
const SMALL_TEXT_CHAPTERS: [&str; 12] = [
    "copyright",
    "copyright page",
    "also by",
    "also by the author",
    "about the author",
    "about the authors",
    "dedication",
    "epigraph",
    "acknowledgments",
    "acknowledgements",
    "colophon",
    "credits",
];

/// `LEADING_TITLE_FALLBACK`: kfxgen's own label for front matter with no title of its own.
const LEADING_TITLE_FALLBACK: &str = "Front Matter";

/// `_LEADING_TITLE_MAX_LEN`.
const LEADING_TITLE_MAX_LEN: usize = 60;

/// The chapters kfxgen makes of the book's shown files before its title-page step: one per
/// section of the contents entries (`_assemble_chapters_by_coordinate`), else, when no entry
/// lands or none makes a chapter, one per file that shows something besides the cover.
pub fn assemble(
    mut items: Vec<SpineItem>,
    entries: &[TocEntry],
    cover_href: Option<&str>,
) -> Vec<Chapter> {
    if !entries.is_empty()
        && let Some(planned) = plan(&mut items, entries, cover_href)
        && !planned.is_empty()
    {
        return fill(items, planned);
    }
    fallback(items, cover_href)
}

/// The fallback: each file a chapter, titled by its opening heading or "Section N" (N its
/// place among the files), the title never printed; a page of the cover alone left out.
fn fallback(items: Vec<SpineItem>, cover_href: Option<&str>) -> Vec<Chapter> {
    let mut chapters = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if !shows_something(&item.text, cover_href) {
            continue;
        }
        let heading = item.blocks.first().and_then(heading_title);
        let title = heading.unwrap_or_else(|| format!("Section {}", i + 1));
        let mut pairs = vec![
            ("_from_spine", Field::Py(PyValue::Bool(true))),
            ("_omit_title_heading", Field::Py(PyValue::Bool(true))),
        ];
        let listing = !item.nav_listing_at.is_empty();
        if !item.blocks.is_empty() {
            pairs.push(("blocks", Field::Blocks(item.blocks)));
        }
        if listing {
            pairs.push(("_had_nav_listing", Field::Py(PyValue::Bool(true))));
        }
        chapters.push(Chapter::new(title, item.text, pairs));
    }
    chapters
}

/// `first.get("heading") and " ".join(first.get("text").split())`, when not empty.
fn heading_title(first: &Block) -> Option<String> {
    if !truthy(first.get("heading")) {
        return None;
    }
    let t = words(first.get("text").and_then(PyValue::as_str).unwrap_or(""));
    (!t.is_empty()).then_some(t)
}

/// A contents entry where it lands: `(flat_index, spine_index, title, key, level)`.
struct Coord<'e> {
    fi: usize,
    si: usize,
    title: &'e str,
    key: Option<String>,
    level: usize,
}

/// A contents listing: its file and its place in that file's `nav_listing_at`.
type Listing = (usize, usize);

/// The blocks of all files in a row, by flat index.
struct Flat {
    /// (file, block) of each flat index.
    at: Vec<(usize, usize)>,
    /// Each file's first flat index.
    offset: Vec<usize>,
}

impl Flat {
    fn new(items: &[SpineItem]) -> Flat {
        let mut at = Vec::new();
        let mut offset = Vec::with_capacity(items.len());
        for (si, item) in items.iter().enumerate() {
            offset.push(at.len());
            at.extend((0..item.blocks.len()).map(|bi| (si, bi)));
        }
        Flat { at, offset }
    }

    fn get<'i>(&self, items: &'i [SpineItem], fi: usize) -> Option<&'i Block> {
        let &(si, bi) = self.at.get(fi)?;
        Some(&items[si].blocks[bi])
    }

    fn get_mut<'i>(&self, items: &'i mut [SpineItem], fi: usize) -> Option<&'i mut Block> {
        let &(si, bi) = self.at.get(fi)?;
        Some(&mut items[si].blocks[bi])
    }

    /// `flat[range]`, clamped as Python slices are.
    fn slice<'i>(
        &self,
        items: &'i [SpineItem],
        r: Range<usize>,
    ) -> impl Iterator<Item = &'i Block> {
        let end = r.end.min(self.at.len());
        let start = r.start.min(end);
        self.at[start..end]
            .iter()
            .map(move |&(si, bi)| &items[si].blocks[bi])
    }

    /// One past the last flat index of file `si`.
    fn file_end(&self, items: &[SpineItem], si: usize) -> usize {
        self.offset[si] + items[si].blocks.len()
    }
}

/// `_assemble_chapters_by_coordinate` up to its blocks: each chapter with the flat range its
/// blocks are to come from, or None when no entry lands. Sets `_spine_file` on every block and
/// an entry's key on the block it lands on, as kfxgen does.
fn plan(
    items: &mut [SpineItem],
    entries: &[TocEntry],
    cover_href: Option<&str>,
) -> Option<Vec<(Chapter, Range<usize>)>> {
    let flat = Flat::new(items);
    for (si, item) in items.iter_mut().enumerate() {
        for b in &mut item.blocks {
            b.set("_spine_file", PyValue::Int(si as i64));
        }
    }
    // `_anchor_block_index` of each file: each id's first block.
    let anchors: Vec<HashMap<String, usize>> = items
        .iter()
        .map(|item| {
            let mut out = HashMap::new();
            for (bi, b) in item.blocks.iter().enumerate() {
                for id in ids(b.get("anchor_ids")) {
                    out.entry(id.to_owned()).or_insert(bi);
                }
            }
            out
        })
        .collect();
    let mut order: HashMap<String, usize> = HashMap::new();
    for (si, item) in items.iter().enumerate() {
        order.entry(normalize(&item.href)).or_insert(si);
    }
    // Each file's listing ids: the first listing holding each.
    let mut listing_of: Vec<HashMap<String, usize>> = Vec::with_capacity(items.len());
    for item in items.iter() {
        let mut first = HashMap::new();
        for (n, (_, listing_ids)) in item.nav_listing_at.iter().enumerate() {
            for id in listing_ids {
                first.entry(id.clone()).or_insert(n);
            }
        }
        listing_of.push(first);
    }
    let toc_anchors: HashSet<&str> = entries
        .iter()
        .map(|e| fragment(&e.href))
        .filter(|f| !f.is_empty())
        .collect();

    let mut coords: Vec<Coord> = Vec::new();
    // The first entry of each chain of entries on one block (kfxgen's `same_spot` followed).
    let mut root: Vec<usize> = Vec::new();
    let mut same_spot: HashSet<usize> = HashSet::new();
    let mut out_of_order: HashSet<usize> = HashSet::new();
    let mut last_block_in_file: HashMap<usize, usize> = HashMap::new();
    let mut prev_flat: Option<usize> = None;
    let mut listing_entry: Option<Listing> = None;
    // Page -> (the file of the entry before, the listing): a page of no blocks.
    let mut empty_listing: HashMap<usize, (usize, Listing)> = HashMap::new();
    let mut moved_ids: HashSet<String> = HashSet::new();
    // The keys on each block an entry's key was added to, so each is tested once.
    let mut keyed: HashMap<usize, HashSet<String>> = HashMap::new();
    for entry in entries {
        let Some(&own_si) = order.get(&normalize(&entry.href)) else {
            continue;
        };
        let amap = &anchors[own_si];
        let mut frag = Cow::Borrowed(fragment(&entry.href));
        if !frag.is_empty() && !amap.contains_key(frag.as_ref()) {
            let decoded = unquote(&frag);
            if amap.contains_key(&decoded) {
                frag = Cow::Owned(decoded);
            }
        }
        let item = &items[own_si];
        if !frag.is_empty()
            && (item.note_ids.contains(frag.as_ref()) || item.note_ids.contains(&unquote(&frag)))
        {
            continue;
        }
        let mut bi = match amap.get(frag.as_ref()) {
            Some(&bi) if !frag.is_empty() => bi,
            _ if !frag.is_empty() => last_block_in_file.get(&own_si).map_or(0, |b| b + 1),
            _ => 0,
        };
        bi = bi.min(item.blocks.len().saturating_sub(1));
        let names_listing = if frag.is_empty() {
            None
        } else {
            listing_of[own_si].get(frag.as_ref()).map(|&n| (own_si, n))
        };
        let mut si = own_si;
        if !frag.is_empty() && item.trailing_ids.contains(frag.as_ref()) && si + 1 < items.len() {
            // An id at a file's very end names the next file's start.
            (si, bi) = (si + 1, 0);
            moved_ids.insert(frag.to_string());
        }
        if bi > 0
            && !last_block_in_file.contains_key(&si)
            && items[si].blocks[..bi]
                .iter()
                .all(|b| truthy(b.get("heading")))
        {
            // Only headings above it in its file, and no entry before it there.
            bi = 0;
        }
        let fi = flat.offset[si] + bi;
        let k = coords.len();
        let mut twin = false;
        if prev_flat == Some(fi)
            && let Some(listing) = listing_entry.filter(|_| names_listing.is_none())
        {
            // The entry before named a listing ending here: it keeps it as a page of no
            // blocks, with the listing's ids.
            empty_listing.insert(root[k - 1], (coords[k - 1].si, listing));
        } else if prev_flat == Some(fi) {
            twin = true;
            same_spot.insert(k);
        } else if prev_flat.is_some_and(|p| fi < p) {
            out_of_order.insert(k);
        }
        let mut key =
            (!frag.is_empty()).then(|| format!("{}#{}", key_doc(&items[own_si].href, ""), frag));
        if key.is_none() && out_of_order.contains(&k) {
            key = Some(key_doc(&items[si].href, ""));
        }
        if let Some(key) = &key
            && !amap.contains_key(frag.as_ref())
            && let Some(block) = flat.get_mut(items, fi)
        {
            let there = keyed
                .entry(fi)
                .or_insert_with(|| ids(block.get("anchor_keys")).map(str::to_owned).collect());
            if there.insert(key.clone()) {
                match block.get_mut("anchor_keys") {
                    Some(PyValue::List(keys)) => keys.push(s(key)),
                    _ => block.set("anchor_keys", PyValue::List(vec![s(key)])),
                }
                if block.get("anchor_offsets").is_none() {
                    block.set("anchor_offsets", PyValue::Dict(Vec::new()));
                }
                if let Some(PyValue::Dict(offsets)) = block.get_mut("anchor_offsets") {
                    super::blocks::dict_set(offsets, key, PyValue::Int(0));
                }
            }
        }
        coords.push(Coord {
            fi,
            si,
            title: &entry.title,
            key,
            level: entry.level,
        });
        root.push(if twin { root[k - 1] } else { k });
        if !out_of_order.contains(&k) {
            prev_flat = Some(fi);
        }
        listing_entry = names_listing;
        last_block_in_file.insert(si, bi);
    }
    if coords.is_empty() {
        return None;
    }
    let items: &[SpineItem] = items;

    // Where each discarded listing counts, and where it sat (the block after it).
    let starts: HashSet<usize> = coords.iter().map(|c| c.fi).collect();
    let toc_frag_at = toc_fragments(entries);
    let mut listing_at: HashMap<usize, usize> = HashMap::new();
    for (si, item) in items.iter().enumerate() {
        let Some(last) = item.blocks.len().checked_sub(1) else {
            continue;
        };
        let frags_here = toc_frag_at.get(&normalize(&item.href));
        for (local, listing_ids) in &item.nav_listing_at {
            let mut fi = flat.offset[si] + (*local).min(last);
            let raw = flat.offset[si] + local;
            let named = frags_here.is_some_and(|f| listing_ids.iter().any(|i| f.contains(i)));
            // A listing ending where a chapter starts is the chapter's before, unless the
            // chapter's entry names it.
            if starts.contains(&fi) && 0 < *local && *local <= last && !named {
                fi -= 1;
            }
            listing_at.entry(fi).or_insert(raw);
        }
    }
    let mut nav_flat: Vec<usize> = listing_at.keys().copied().collect();
    nav_flat.sort_unstable();
    // `_flag_nav`: a chapter that held a discarded listing, and the first one's place in it.
    let flag_nav = |ch: &mut Chapter, r: &Range<usize>| {
        let from = nav_flat.partition_point(|&i| i < r.start);
        let here = nav_flat[from..].iter().take_while(|&&i| i < r.end);
        if let Some(first) = here.map(|i| listing_at[i]).min() {
            ch.set("_had_nav_listing", PyValue::Bool(true));
            ch.set(
                "_nav_listing_at",
                PyValue::Int(first.saturating_sub(r.start) as i64),
            );
        }
    };
    // `_mk`: a chapter of the blocks in `r`, or None when they show only the cover.
    let mk = |title: &str, r: &Range<usize>| {
        let text = joined_text(flat.slice(items, r.clone()));
        shows_something(&text, cover_href).then(|| {
            Chapter::new(
                title.to_owned(),
                text,
                vec![("blocks", Field::Blocks(Vec::new()))],
            )
        })
    };

    let mut chapters: Vec<(Chapter, Range<usize>)> = Vec::new();
    let first_fi = coords[0].fi;
    // A head holding an id an entry names is the first chapter's, unless the id ends its file
    // and names the next file's start.
    let head_has_toc_anchor = first_fi > 0
        && flat.slice(items, 0..first_fi).any(|b| {
            ids(b.get("anchor_ids")).any(|a| toc_anchors.contains(a) && !moved_ids.contains(a))
        });
    if first_fi > 0 && !head_has_toc_anchor {
        let r = 0..first_fi;
        let head = flat
            .get(items, 0)
            .and_then(|b| b.get("text"))
            .and_then(PyValue::as_str);
        let title = leading_chapter_title(head.unwrap_or(""));
        if let Some(mut ch) = mk(&title, &r) {
            flag_nav(&mut ch, &r);
            // A title of the words beside a picture: that paragraph prints them. kfxgen's own
            // label is never printed.
            if IMG_TOKEN.is_match(head.unwrap_or("")) || title == LEADING_TITLE_FALLBACK {
                ch.set("_omit_title_heading", PyValue::Bool(true));
            }
            ch.set("_omit_from_toc", PyValue::Bool(true));
            chapters.push((ch, r));
        }
    }

    let mut heads = section_heads(&coords, items, &flat, &nav_flat);
    for k in same_spot.iter().chain(&out_of_order) {
        heads[*k] = false;
    }
    let mut next_head = vec![None; coords.len()];
    for k in (0..coords.len().saturating_sub(1)).rev() {
        next_head[k] = if heads[k + 1] {
            Some(k + 1)
        } else {
            next_head[k + 1]
        };
    }
    for (k, c) in coords.iter().enumerate() {
        if !heads[k] {
            continue;
        }
        let nxt = next_head[k];
        let end = nxt.map_or_else(|| flat.file_end(items, c.si), |j| coords[j].fi);
        let start = if k == 0 && head_has_toc_anchor {
            0
        } else {
            c.fi
        };
        let r = start..end;
        let ch = match empty_listing.get(&k) {
            Some(&(si, (lsi, n))) => {
                let mut ids: Vec<&String> = items[lsi].nav_listing_at[n].1.iter().collect();
                ids.sort();
                let doc = key_doc(&items[si].href, "");
                let keys = ids.iter().map(|i| s(&format!("{doc}#{i}"))).collect();
                Some(Chapter::new(
                    c.title.to_owned(),
                    String::new(),
                    vec![
                        ("blocks", Field::Blocks(Vec::new())),
                        ("_had_nav_listing", Field::Py(PyValue::Bool(true))),
                        ("_listing_keys", Field::Py(PyValue::List(keys))),
                    ],
                ))
            }
            None => mk(c.title, &r).map(|mut ch| {
                flag_nav(&mut ch, &r);
                ch
            }),
        };
        let Some(mut ch) = ch else {
            continue;
        };
        let r = if empty_listing.contains_key(&k) {
            start..start
        } else {
            r
        };
        if is_file_name_label(c.title, &items[c.si].href) {
            // `_retitle_from_heading`: a file name is never printed; a chapter opening with a
            // heading is named after it.
            ch.set("_omit_title_heading", PyValue::Bool(true));
            let first = (!r.is_empty()).then(|| flat.get(items, r.start)).flatten();
            if let Some(heading) = first.and_then(heading_title) {
                ch.set("title", PyValue::Str(heading));
                ch.set("_from_spine", PyValue::Bool(true));
            }
        }
        ch.set("toc_level", PyValue::Int(c.level as i64));
        let subs: Vec<PyValue> = (k + 1..nxt.unwrap_or(coords.len()))
            .map(|j| {
                let twin = root[j];
                // On the section's own block it opens the section; one out of order lands on
                // its own block.
                let key = if out_of_order.contains(&twin) {
                    coords[j].key.as_deref()
                } else if twin == k {
                    None
                } else {
                    coords[twin].key.as_deref()
                };
                PyValue::Dict(vec![
                    (s("title"), s(coords[j].title)),
                    (s("key"), key.map_or(PyValue::None, s)),
                    (s("level"), PyValue::Int(coords[j].level as i64)),
                ])
            })
            .collect();
        if !subs.is_empty() {
            ch.set("sub_entries", PyValue::List(subs));
        }
        chapters.push((ch, r));
    }

    // Files past the furthest one an entry reaches: back matter the book left out of its
    // contents, named by file and never printed or listed.
    let last_si = coords.iter().map(|c| c.si).max().unwrap_or(0);
    for (si, item) in items.iter().enumerate().skip(last_si + 1) {
        if !shows_something(&item.text, cover_href) {
            continue;
        }
        let norm = normalize(&item.href);
        let stem = norm
            .rsplit_once('.')
            .map_or(norm.as_str(), |(stem, _)| stem);
        let title = if stem.is_empty() {
            format!("Section {}", si + 1)
        } else {
            stem.to_owned()
        };
        let r = flat.offset[si]..flat.file_end(items, si);
        if let Some(mut ch) = mk(&title, &r) {
            flag_nav(&mut ch, &r);
            ch.set("_omit_from_toc", PyValue::Bool(true));
            ch.set("_omit_title_heading", PyValue::Bool(true));
            chapters.push((ch, r));
        }
    }
    Some(chapters)
}

/// The blocks moved into the chapters planned: the ranges are in order and do not overlap
/// (module documentation), so one pass hands each block to its chapter; a block in no range
/// (a page of the cover alone) is dropped, as kfxgen drops it.
fn fill(items: Vec<SpineItem>, planned: Vec<(Chapter, Range<usize>)>) -> Vec<Chapter> {
    let mut blocks = items
        .into_iter()
        .flat_map(|i| i.blocks)
        .enumerate()
        .peekable();
    let mut out = Vec::with_capacity(planned.len());
    for (mut ch, r) in planned {
        while blocks.next_if(|(i, _)| *i < r.start).is_some() {}
        let mine: Vec<Block> = std::iter::from_fn(|| blocks.next_if(|(i, _)| *i < r.end))
            .map(|(_, b)| b)
            .collect();
        ch.put_blocks(mine);
        out.push(ch);
    }
    out
}

/// `_section_heads`: which entries start a section. One does at its file's start, where the
/// book breaks the page between the entry before and its own block, or where kfxgen rewrites
/// the page (the book's first "Contents", a page it replaces or restyles by title); a run of
/// entries merged into one section holding a discarded listing splits at every entry again.
fn section_heads(
    coords: &[Coord],
    items: &[SpineItem],
    flat: &Flat,
    nav_flat: &[usize],
) -> Vec<bool> {
    let titles: Vec<String> = coords.iter().map(|c| normalize_title(c.title)).collect();
    let is_contents = |k: usize| CONTENTS_TITLES.contains(&titles[k].as_str());
    let book_contents = (0..coords.len()).find(|&k| is_contents(k));
    let rewritten = |k: usize| {
        if is_contents(k) {
            Some(k) == book_contents
        } else {
            let t = titles[k].as_str();
            CONTENTS_SKIP_TITLES.contains(&t) || SMALL_TEXT_CHAPTERS.contains(&t)
        }
    };
    // How many blocks before each flat index break the page before them.
    let mut breaks = Vec::with_capacity(flat.at.len() + 1);
    breaks.push(0usize);
    for b in flat.slice(items, 0..flat.at.len()) {
        breaks.push(breaks[breaks.len() - 1] + usize::from(truthy(b.get("break_before"))));
    }
    let count = |r: Range<usize>| {
        let end = r.end.min(flat.at.len());
        breaks[end] - breaks[r.start.min(end)]
    };
    let mut heads: Vec<bool> = (0..coords.len())
        .map(|k| {
            if k == 0 || coords[k - 1].si != coords[k].si || rewritten(k) || rewritten(k - 1) {
                return true;
            }
            count(coords[k - 1].fi + 1..coords[k].fi + 1) > 0
        })
        .collect();
    let starts: Vec<usize> = (0..coords.len())
        .filter(|&k| heads[k])
        .chain([coords.len()])
        .collect();
    for w in starts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b - a < 2 {
            continue;
        }
        let lo = coords[a].fi;
        let hi = if b < coords.len() {
            coords[b].fi
        } else {
            flat.file_end(items, coords[a].si)
        };
        let from = nav_flat.partition_point(|&i| i < lo);
        if nav_flat.get(from).is_some_and(|&i| i < hi) {
            heads[a..b].fill(true);
        }
    }
    heads
}

/// The strings of a list (ids or keys); nothing for what is no list.
fn ids(v: Option<&PyValue>) -> impl Iterator<Item = &str> {
    v.and_then(PyValue::as_list)
        .unwrap_or(&[])
        .iter()
        .filter_map(PyValue::as_str)
}

/// `_normalize_title`: words joined by one space, ASCII punctuation and white space taken off
/// both ends, lower-cased.
fn normalize_title(title: &str) -> String {
    words(title)
        .trim_matches(|c: char| c.is_ascii_punctuation() || " \t\n\r\x0b\x0c".contains(c))
        .to_lowercase()
}

/// `_leading_chapter_title`: the first block's text without its pictures, when short enough
/// to be a title and on one line; else kfxgen's own label.
fn leading_chapter_title(text: &str) -> String {
    let stripped = IMG_TOKEN.replace_all(text, "");
    let t = py::strip(&stripped);
    if (1..=LEADING_TITLE_MAX_LEN).contains(&py::len(t)) && !t.contains('\n') {
        t.to_owned()
    } else {
        LEADING_TITLE_FALLBACK.to_owned()
    }
}

/// `_is_file_name_label`: the label is its file's name, with or without the extension, in
/// any case, and holds both a letter and a digit.
fn is_file_name_label(label: &str, href: &str) -> bool {
    let name = normalize(href);
    let stem = name
        .rsplit_once('.')
        .map_or(name.as_str(), |(stem, _)| stem);
    let label = py::strip(label).to_lowercase();
    !stem.is_empty()
        && (label == stem.to_lowercase() || label == name.to_lowercase())
        && label.chars().any(py::is_digit)
        && label.chars().any(py::is_alpha)
}

/// `_page_shows_something`: real text once the pictures are taken out, or a picture other
/// than the cover (by file name, in any case or percent-encoding).
pub(super) fn shows_something(text: &str, cover_href: Option<&str>) -> bool {
    if !py::strip(&IMG_TOKEN.replace_all(text, "")).is_empty() {
        return true;
    }
    let name = |href: &str| {
        let path = href.split('#').next().unwrap_or("");
        unquote(path.rsplit('/').next().unwrap_or("")).to_lowercase()
    };
    let cover = cover_href.filter(|c| !c.is_empty()).map(name);
    IMG_TOKEN
        .captures_iter(text)
        .any(|m| Some(name(m.get(1).map_or("", |g| g.as_str()))) != cover)
}
