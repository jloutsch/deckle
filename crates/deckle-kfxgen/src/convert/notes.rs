// SPDX-License-Identifier: GPL-3.0-only
//! Which ids of the book are notes, and where its links land (`converter.py`
//! `_note_target_ids`, `_note_pair_ids`, `_book_link_targets`, `_footnote_keys`).
//!
//! kfxgen looks through an element's whole subtree or ancestors for each element (a heading
//! below or above, the text of the line round a marker, a hidden ancestor of a footnote), which
//! is quadratic in a deep or wide file. Here each file is walked a fixed number of times, the
//! answers kept per element, so the cost is linear in its elements and text. The keys built
//! (`"<file>#<id>"`, link targets) repeat the file's path for every id or link. Each key made,
//! and each read of the path before a link is resolved, is charged to the file's
//! [`TextBudget`], which counts whole 64-byte units per charge: a key under 64 bytes, as most
//! are, costs nothing, so what bounds the scans' memory is the book's node limit
//! (`MAX_BOOK_DOM_NODES`), about 720 bytes a link (DIVERGENCES.md, "Note and link keys under a
//! long path").

use std::collections::{BTreeSet, HashMap, HashSet};

use super::Error;
use super::href::{key_doc, resolve_link_target, unquote};
use super::inline::{epub_types, is_non_rendered, own_anchor_ids, text_of};
use super::resolver::{Resolver, TextBudget};
use crate::book::Book;
use crate::css::DEFAULT_PROFILE;
use crate::dom::{Dom, NodeId};
use crate::generate::py;

/// `_NOTE_TYPES`, `_NOTE_ROLES` and `_NOTE_CLASSES`.
const NOTE_TYPES: [&str; 5] = ["noteref", "backlink", "footnote", "endnote", "rearnote"];
const NOTE_ROLES: [&str; 4] = ["doc-noteref", "doc-backlink", "doc-footnote", "doc-endnote"];
const NOTE_CLASSES: [&str; 2] = ["footnote-ref", "footnote-backref"];

fn is_heading(dom: &Dom, n: NodeId) -> bool {
    matches!(dom.tag(n), "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

/// `_LINE_TAGS`.
fn is_line(dom: &Dom, n: NodeId) -> bool {
    matches!(
        dom.tag(n),
        "p" | "div" | "li" | "td" | "th" | "dd" | "dt" | "blockquote" | "body" | "caption"
    )
}

/// Whether `word` is one of the white-space separated words of `n`'s `class`.
fn has_class(dom: &Dom, n: NodeId, word: &str) -> bool {
    dom.attr(n, "class")
        .unwrap_or("")
        .split(py::is_space)
        .any(|w| w == word)
}

/// `_is_note_element`.
fn is_note_element(dom: &Dom, n: NodeId) -> bool {
    if epub_types(dom, n)
        .iter()
        .any(|t| NOTE_TYPES.contains(&t.as_str()))
    {
        return true;
    }
    let role = py::strip(dom.attr(n, "role").unwrap_or("")).to_lowercase();
    NOTE_ROLES.contains(&role.as_str()) || NOTE_CLASSES.iter().any(|c| has_class(dom, n, c))
}

/// A file's elements in document order, and by node index whether each is or holds a
/// heading, and whether an element above it is one.
struct Tree {
    order: Vec<NodeId>,
    heading_here_or_below: Vec<bool>,
    heading_above: Vec<bool>,
}

impl Tree {
    fn new(dom: &Dom) -> Tree {
        let order: Vec<NodeId> = dom.descendants().collect();
        let mut below = vec![false; dom.node_count()];
        let mut above = vec![false; dom.node_count()];
        for &n in &order {
            below[n.index()] = is_heading(dom, n);
            if let Some(p) = dom.parent(n) {
                above[n.index()] = above[p.index()] || is_heading(dom, p);
            }
        }
        // Children come after their parent in document order.
        for &n in order.iter().rev() {
            if below[n.index()]
                && let Some(p) = dom.parent(n)
            {
                below[p.index()] = true;
            }
        }
        Tree {
            order,
            heading_here_or_below: below,
            heading_above: above,
        }
    }
}

/// `_note_target_ids(root)`: the ids of `dom` that name a note marker, a back-link or a note.
/// An id counts on an element that is note-marked, has a note-marked child element, or is an
/// `<li>` two levels inside a `class="footnote"` element, unless it is or holds a heading; or
/// on an `<a>` whose fragment names such an id, or a `sup`, `span` or `small` holding nothing
/// but such an `<a>`.
pub fn note_target_ids(dom: &Dom) -> HashSet<String> {
    let tree = Tree::new(dom);
    let mut footnote_box: HashMap<NodeId, bool> = HashMap::new();
    let mut ids: HashSet<String> = HashSet::new();
    for &n in &tree.order {
        if own_anchor_ids(dom, n).next().is_none() || tree.heading_here_or_below[n.index()] {
            continue;
        }
        let note = is_note_element(dom, n)
            || dom.child_elements(n).any(|c| is_note_element(dom, c))
            || (dom.tag(n) == "li"
                && dom.parent(n).and_then(|l| dom.parent(l)).is_some_and(|b| {
                    // A box's class is read once, however many items it holds.
                    *footnote_box
                        .entry(b)
                        .or_insert_with(|| has_class(dom, b, "footnote"))
                }));
        if note {
            ids.extend(own_anchor_ids(dom, n).map(str::to_owned));
        }
    }
    let points_at_note = |a: NodeId| {
        let href = dom.attr(a, "href").unwrap_or("");
        let frag = href.split_once('#').map_or("", |(_, f)| f);
        !frag.is_empty() && (ids.contains(frag) || ids.contains(&unquote(frag)))
    };
    let mut markers = Vec::new();
    for &n in &tree.order {
        let mut own = own_anchor_ids(dom, n).peekable();
        if own.peek().is_none() || own_anchor_ids(dom, n).any(|i| ids.contains(i)) {
            continue;
        }
        let hit = match dom.tag(n) {
            "a" => points_at_note(n),
            "sup" | "span" | "small" => {
                dom.element_count(n) == 1
                    && py::strip(dom.text(n)).is_empty()
                    && dom.child_elements(n).next().is_some_and(|kid| {
                        dom.tag(kid) == "a"
                            && py::strip(dom.tail(kid)).is_empty()
                            && points_at_note(kid)
                    })
            }
            _ => false,
        };
        if hit {
            markers.extend(own.map(str::to_owned));
        }
    }
    ids.extend(markers);
    ids
}

/// `"<doc>#<id>"`, charged to `budget` first.
fn key(budget: &TextBudget, doc: &str, id: &str) -> Result<String, Error> {
    budget.charge(doc.len() + 1 + id.len())?;
    Ok(format!("{doc}#{id}"))
}

/// `_resolve_link_target(href, base)`, charged to `budget`: the file's path, which it reads
/// again for every link, before; the target made, after.
fn link_target(budget: &TextBudget, href: &str, base: &str) -> Result<Option<String>, Error> {
    budget.charge(base.len())?;
    let target = resolve_link_target(href, base);
    if let Some(t) = &target {
        budget.charge(t.len())?;
    }
    Ok(target)
}

/// `_book_link_targets`: every `"<file>#<id>"` key (or file alone) an `<a href>` of a spine
/// file resolves to, and each also decoded once. `budgets` holds one [`TextBudget`] for each
/// of [`Book::files`].
pub fn book_link_targets(book: &Book, budgets: &[TextBudget]) -> Result<HashSet<String>, Error> {
    let mut targets = HashSet::new();
    for (i, file) in book.files.iter().enumerate() {
        let dom = &file.dom;
        for n in dom.descendants() {
            let Some(href) = dom
                .attr(n, "href")
                .filter(|h| !h.is_empty() && dom.tag(n) == "a")
            else {
                continue;
            };
            if let Some(t) = link_target(&budgets[i], href, &file.href)? {
                // An id such as "n:1" is linked as "#n%3A1".
                targets.insert(unquote(&t));
                targets.insert(t);
            }
        }
    }
    Ok(targets)
}

/// Per element of a file, by node index: the characters that are not white space in
/// `"".join(e.itertext())`, and the nearest element above it named in `_LINE_TAGS`.
struct Lines {
    shown: Vec<usize>,
    line: Vec<Option<NodeId>>,
}

impl Lines {
    fn new(dom: &Dom, order: &[NodeId]) -> Lines {
        let words = |s: &str| s.chars().filter(|&c| !py::is_space(c)).count();
        let mut shown = vec![0usize; dom.node_count()];
        let mut line = vec![None; dom.node_count()];
        for &n in order {
            if let Some(p) = dom.parent(n) {
                line[n.index()] = if is_line(dom, p) {
                    Some(p)
                } else {
                    line[p.index()]
                };
            }
        }
        // A comment's own text is not in `itertext`; its tail is, as an element's is.
        for &n in order.iter().rev() {
            let mut count = words(dom.text(n));
            for &c in dom.children(n) {
                if !dom.is_comment(c) {
                    count += shown[c.index()];
                }
                count += words(dom.tail(c));
            }
            shown[n.index()] = count;
        }
        Lines { shown, line }
    }

    /// `_in_running_text(marker)`: whether the line round `marker` shows something besides
    /// it. `whole.replace(own, "", 1).strip()` is not empty exactly when `whole` has more
    /// characters that are not white space than `own`: `own` is always in `whole`, and taking
    /// one copy of it out takes out its own.
    fn in_running_text(&self, marker: NodeId) -> bool {
        self.line[marker.index()]
            .is_some_and(|l| self.shown[l.index()] > self.shown[marker.index()])
    }
}

/// `_note_pair_ids`: `{file: ids}` of notes and their markers found by the links between them,
/// for a contents Calibre made: a marker in running text carrying id R links to T, and a link
/// in T's file links back to R; then T and R count. A target a `noteref` link names counts
/// too. Nothing in, on or round a heading counts. `budgets` holds one [`TextBudget`] for each
/// of [`Book::files`].
pub fn note_pair_ids(
    book: &Book,
    budgets: &[TextBudget],
) -> Result<HashMap<String, HashSet<String>>, Error> {
    // (target key, keys of the ids its link carries)
    let mut links: Vec<(String, Vec<String>)> = Vec::new();
    let mut noterefs: HashSet<String> = HashSet::new();
    let mut back: HashMap<String, HashSet<String>> = HashMap::new();
    let mut heading: HashSet<String> = HashSet::new();
    for (i, file) in book.files.iter().enumerate() {
        let (dom, budget, base) = (&file.dom, &budgets[i], file.href.as_str());
        // kfxgen's "" for a file of no href is `key_doc`'s too.
        let doc = key_doc(base, "");
        let tree = Tree::new(dom);
        let lines = Lines::new(dom, &tree.order);
        let mut targets_here: HashSet<String> = HashSet::new();
        for &n in &tree.order {
            let in_heading = tree.heading_here_or_below[n.index()] || tree.heading_above[n.index()];
            if in_heading {
                for aid in own_anchor_ids(dom, n) {
                    heading.insert(key(budget, &doc, aid)?);
                }
            }
            let Some(href) = dom
                .attr(n, "href")
                .filter(|h| !h.is_empty() && dom.tag(n) == "a")
            else {
                continue;
            };
            let Some(target) = link_target(budget, href, base)?.filter(|t| t.contains('#')) else {
                continue;
            };
            if epub_types(dom, n).iter().any(|t| t == "noteref")
                || dom.attr(n, "role") == Some("doc-noteref")
            {
                noterefs.insert(target.clone());
            }
            if !in_heading {
                targets_here.insert(target.clone());
            }
            let mut carriers: Vec<&str> = own_anchor_ids(dom, n).collect();
            let mut carrier = n;
            if let Some(p) = dom.parent(n)
                && matches!(dom.tag(p), "sup" | "span" | "small")
                && dom.element_count(p) == 1
                && py::strip(dom.text(p)).is_empty()
                && py::strip(dom.tail(n)).is_empty()
            {
                carriers.extend(own_anchor_ids(dom, p));
                carrier = p;
            }
            if !carriers.is_empty() && lines.in_running_text(carrier) {
                let keys = carriers
                    .iter()
                    .map(|aid| key(budget, &doc, aid))
                    .collect::<Result<_, _>>()?;
                links.push((target, keys));
            }
        }
        if !targets_here.is_empty() {
            back.entry(doc).or_default().extend(targets_here);
        }
    }

    let mut found: HashMap<String, HashSet<String>> = HashMap::new();
    let mut add = |key: &str| {
        let (file, aid) = key.split_once('#').unwrap_or((key, ""));
        found
            .entry(file.to_owned())
            .or_default()
            .insert(aid.to_owned());
    };
    for target in noterefs.difference(&heading) {
        add(target);
    }
    for (target, carriers) in &links {
        if heading.contains(target) {
            continue;
        }
        let target_doc = target.split('#').next().unwrap_or("");
        let Some(linked_back) = back.get(target_doc) else {
            continue;
        };
        for marker in carriers {
            if linked_back.contains(marker) && !heading.contains(marker) {
                add(target);
                add(marker);
            }
        }
    }
    Ok(found)
}

/// `_footnote_keys(oeb_book, log)`: the key of every id on or inside an element whose
/// `epub:type` holds `footnote`, unless it or an element above it is hidden (`_is_hidden`, with
/// the file's resolver). `budgets` holds one [`TextBudget`] for each of [`Book::files`].
pub fn footnote_keys(book: &Book, budgets: &[TextBudget]) -> Result<BTreeSet<String>, Error> {
    let mut keys = BTreeSet::new();
    for (i, file) in book.files.iter().enumerate() {
        let dom = &file.dom;
        let order: Vec<NodeId> = dom.descendants().collect();
        let mut footnote = vec![false; dom.node_count()];
        for &n in &order {
            footnote[n.index()] = epub_types(dom, n).iter().any(|t| t == "footnote");
        }
        if !footnote.iter().any(|&f| f) {
            continue;
        }
        let resolver = Resolver::new(file, &DEFAULT_PROFILE, &budgets[i]);
        let doc = key_doc(&file.href, "");
        // `_is_hidden` of each element asked about, then whether it or one above it is.
        let mut hidden: Vec<Option<bool>> = vec![None; dom.node_count()];
        let mut shown = vec![false; dom.node_count()];
        for &n in &order {
            let inside = dom.parent(n).is_some_and(|p| shown[p.index()]);
            // A footnote inside one shown adds nothing new: its ids are kept already.
            shown[n.index()] = inside
                || (footnote[n.index()] && !hidden_here_or_above(&resolver, dom, n, &mut hidden)?);
            if shown[n.index()] {
                for aid in own_anchor_ids(dom, n) {
                    keys.insert(key(&budgets[i], &doc, aid)?);
                }
            }
        }
    }
    Ok(keys)
}

/// Whether `n` or an element above it is hidden, asked from `n` upwards as kfxgen asks
/// (`any(_is_hidden(a) for a in (elem, *elem.iterancestors()))`), each element's answer kept in
/// `memo` for it and every element below it on the way, so each element is resolved once.
fn hidden_here_or_above(
    resolver: &Resolver,
    dom: &Dom,
    n: NodeId,
    memo: &mut [Option<bool>],
) -> Result<bool, Error> {
    let mut path = Vec::new();
    let mut at = Some(n);
    let mut answer = false;
    while let Some(x) = at {
        if let Some(known) = memo[x.index()] {
            answer = known;
            break;
        }
        let hidden = is_non_rendered(dom, x)
            || match resolver.resolve(x)? {
                Some(css) => py::strip(&text_of(css.get("display")?)).to_lowercase() == "none",
                None => false,
            };
        if hidden {
            memo[x.index()] = Some(true);
            answer = true;
            break;
        }
        path.push(x);
        at = dom.parent(x);
    }
    for x in path {
        memo[x.index()] = Some(answer);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// kfxgen's `_note_target_ids` rules (`ad38ec8`; its function was run on this page in
    /// review and gives this set): a Markdown footnote list's items, a note-marked element and
    /// its wrapper, markers pointing at them by an encoded fragment, and nothing in or holding a
    /// heading.
    #[test]
    fn note_targets_by_structure() {
        let page = br##"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body>
<p>Text<sup id="fnref:1"><a href="#fn%3A1">1</a></sup> and <span id="w"><a epub:type="noteref" href="#x">2</a></span>.</p>
<p id="para">A paragraph <a href="#fn:1">holding</a> a link is no marker.</p>
<div class="footnote"><ol><li id="fn:1">One.</li></ol></div>
<section id="sec" epub:type="footnote"><h2 id="h">Notes</h2></section>
<aside id="n2" epub:type="endnote"><p>Two.</p></aside>
<p><a id="m2" href="#n2">2</a> <span id="s"> x<a href="#n2">2</a></span></p>
</body></html>"##;
        let dom = crate::dom::parse(page).unwrap();
        let mut ids: Vec<String> = note_target_ids(&dom).into_iter().collect();
        ids.sort();
        assert_eq!(ids, ["fn:1", "fnref:1", "m2", "n2", "w"]);
    }
}
