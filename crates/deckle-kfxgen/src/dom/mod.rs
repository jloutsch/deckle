// SPDX-License-Identifier: GPL-3.0-only
//! A book's XHTML file as an owned, mutable element tree, parsed and cleaned the way Calibre
//! 9.18 parses it for its conversion pipeline (`ebooks/oeb/parse_utils.py` `parse_html`, phase 2
//! plan Task 4).
//!
//! [`parse`] decodes the bytes ([`Error`] when it cannot), reads them as XML and, when they are
//! not well-formed, as HTML with `html5ever`, then cleans the tree as Calibre does. The tree is
//! lxml's model: elements only, each with the text before its first child (`text`) and the text
//! after it (`tail`); comments and processing instructions are dropped, their neighbours' text
//! joined. Names (tags, attribute names, namespaces) are stored once each in a table. Element names are kept as Calibre keeps them (as written on the XML path, in lower
//! case on the HTML one), attributes by their qualified name (`epub:type`).
//!
//! Hostile input: every walk is a loop over the arena, never recursion; the tree is never
//! deeper than [`MAX_DEPTH`] below its root; the HTML parser stops at a node budget linear in
//! the input.

mod clean;
mod parse;
mod structure;
mod write;

pub(crate) use parse::decode;
pub use parse::parse;
pub(crate) use parse::{from_xml, py_space};
pub use structure::{generated_toc, mark_chapters};
pub use write::to_xhtml;

use std::collections::{BTreeMap, BTreeSet};

use deckle_limits::MAX_DEPTH;

/// The XHTML namespace.
pub(crate) const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";
/// The SVG namespace.
pub(crate) const SVG_NS: &str = "http://www.w3.org/2000/svg";
/// The namespace of the `xml:` prefix.
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Why a file could not be read as a tree.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Read as HTML, it builds far more elements and attributes than its size accounts for
    /// (the HTML parsing algorithm copies open formatting elements, attributes and all, into
    /// every paragraph), or more than [`deckle_limits::MAX_DOM_NODES`], so the parse was
    /// stopped.
    #[error("this file builds more than {0} elements and attributes when read as HTML")]
    TooLarge(usize),
    /// Read as XML, it holds more than [`deckle_limits::MAX_DOM_NODES`] nodes.
    #[error("this file holds more than {0} elements and runs of text")]
    TooManyNodes(usize),
    /// Read as HTML (it is not XML `deckle_limits::parse_xml` reads), its elements nest more
    /// than `MAX_DEPTH` deep. Calibre falls back to libxml2's HTML parser there
    /// (DIVERGENCES.md, "XHTML that is not well-formed").
    #[error("this file is nested more than {MAX_DEPTH} levels deep")]
    TooDeep,
    /// The entities its DOCTYPE defines expand to more than
    /// [`deckle_limits::MAX_ENTITY_TEXT`] of text (DIVERGENCES.md, "Entities a file defines").
    #[error("this file's own entities make {0} of text")]
    EntityText(deckle_limits::TooLarge),
}

/// An element of a [`Dom`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// An index into [`Dom`]'s table of names: tags, attribute names and namespace URIs, each
/// stored once. [`NO_NS`] is the empty string, no namespace.
type Name = u32;
const NO_NS: Name = 0;

#[derive(Debug, Clone)]
struct Attr {
    /// The qualified name as written: `class`, `epub:type`, `xml:lang`.
    name: Name,
    /// The namespace URI of a prefixed name.
    ns: Name,
    value: String,
}

#[derive(Debug, Clone)]
struct Element {
    tag: Name,
    ns: Name,
    attrs: Vec<Attr>,
    text: String,
    tail: String,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// Its index in its parent's `children`.
    pos: u32,
    /// Where a comment or processing instruction with content stood, which the tree does not
    /// keep: [`IN_TEXT`], [`IN_TAIL`] (read by `structure.rs`).
    notes: u8,
}

/// [`Element::notes`]: a comment or processing instruction whose text is not blank
/// (`structure.py` `isspace`) stood in the element's text, before its first child.
const IN_TEXT: u8 = 1;
/// [`Element::notes`]: one stood in the element's tail.
const IN_TAIL: u8 = 2;

/// An element tree in an arena. Removed elements stay in the arena, detached.
#[derive(Debug, Clone)]
pub struct Dom {
    nodes: Vec<Element>,
    root: NodeId,
    names: Vec<Box<str>>,
    name_ids: BTreeMap<Box<str>, Name>,
    /// The root's last child node is a comment or processing instruction, which the tree does
    /// not keep (lxml's `root[-1]`, read by [`clean`]).
    root_ends_in_comment: bool,
}

impl Dom {
    /// A tree of one element.
    fn new(tag: &str, ns: Option<&str>) -> Dom {
        let mut dom = Dom {
            nodes: Vec::new(),
            root: NodeId(0),
            names: Vec::new(),
            name_ids: BTreeMap::new(),
            root_ends_in_comment: false,
        };
        dom.intern("");
        dom.root = dom.alloc(tag, ns, Vec::new());
        dom
    }

    /// `s`'s index in the table of names, added if it is new.
    fn intern(&mut self, s: &str) -> Name {
        if let Some(&id) = self.name_ids.get(s) {
            return id;
        }
        let id = Name::try_from(self.names.len()).unwrap_or(Name::MAX);
        self.names.push(s.into());
        self.name_ids.insert(s.into(), id);
        id
    }

    fn name(&self, id: Name) -> &str {
        self.names.get(id as usize).map_or("", |s| s)
    }

    fn ns_name(&self, id: Name) -> Option<&str> {
        (id != NO_NS).then(|| self.name(id))
    }

    /// An attribute for [`Dom::alloc`].
    fn new_attr(&mut self, name: &str, ns: Option<&str>, value: String) -> Attr {
        Attr {
            name: self.intern(name),
            ns: ns.map_or(NO_NS, |n| self.intern(n)),
            value,
        }
    }

    fn alloc(&mut self, tag: &str, ns: Option<&str>, attrs: Vec<Attr>) -> NodeId {
        // The arena holds at most one element per three bytes of input, or the HTML parser's
        // budget, far below u32::MAX for any file the archive reader hands over.
        let id = NodeId(u32::try_from(self.nodes.len()).unwrap_or(u32::MAX));
        let (tag, ns) = (self.intern(tag), ns.map_or(NO_NS, |n| self.intern(n)));
        self.nodes.push(Element {
            tag,
            ns,
            attrs,
            text: String::new(),
            tail: String::new(),
            parent: None,
            children: Vec::new(),
            pos: 0,
            notes: 0,
        });
        id
    }

    fn el(&self, n: NodeId) -> &Element {
        &self.nodes[n.index()]
    }

    fn el_mut(&mut self, n: NodeId) -> &mut Element {
        &mut self.nodes[n.index()]
    }

    /// The root element, `<html>` once cleaned.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The element's local name, as Calibre keeps it (see the module documentation).
    pub fn tag(&self, n: NodeId) -> &str {
        self.name(self.el(n).tag)
    }

    /// The element's namespace URI; None for an element in no namespace.
    pub fn namespace(&self, n: NodeId) -> Option<&str> {
        self.ns_name(self.el(n).ns)
    }

    fn set_namespace(&mut self, n: NodeId, ns: Option<&str>) {
        let id = ns.map_or(NO_NS, |s| self.intern(s));
        self.el_mut(n).ns = id;
    }

    /// The value of the attribute of qualified name `name`.
    pub fn attr(&self, n: NodeId, name: &str) -> Option<&str> {
        let id = *self.name_ids.get(name)?;
        self.el(n)
            .attrs
            .iter()
            .find(|a| a.name == id)
            .map(|a| a.value.as_str())
    }

    /// The element's attributes in order: (qualified name, namespace URI, value).
    pub fn attrs(&self, n: NodeId) -> impl Iterator<Item = (&str, Option<&str>, &str)> {
        self.el(n)
            .attrs
            .iter()
            .map(|a| (self.name(a.name), self.ns_name(a.ns), a.value.as_str()))
    }

    /// Sets the attribute `name` (a name in no namespace, or one already on the element) to
    /// `value`, or removes it when `value` is None. A new attribute goes last.
    pub fn set_attr(&mut self, n: NodeId, name: &str, value: Option<&str>) {
        let id = self.intern(name);
        let attrs = &mut self.el_mut(n).attrs;
        let at = attrs.iter().position(|a| a.name == id);
        match (at, value) {
            (Some(i), Some(v)) => v.clone_into(&mut attrs[i].value),
            (Some(i), None) => {
                attrs.remove(i);
            }
            (None, Some(v)) => attrs.push(Attr {
                name: id,
                ns: NO_NS,
                value: v.to_owned(),
            }),
            (None, None) => {}
        }
    }

    /// The parent element; None for the root and for a removed element.
    pub fn parent(&self, n: NodeId) -> Option<NodeId> {
        self.el(n).parent
    }

    /// The child elements, in order.
    pub fn children(&self, n: NodeId) -> &[NodeId] {
        &self.el(n).children
    }

    /// The element's index among its parent's child elements; 0 for the root and for a
    /// removed element.
    pub fn index_in_parent(&self, n: NodeId) -> usize {
        match self.el(n).parent {
            Some(_) => self.el(n).pos as usize,
            None => 0,
        }
    }

    /// The child element of the same parent just before `n`.
    pub fn prev_sibling(&self, n: NodeId) -> Option<NodeId> {
        let p = self.el(n).parent?;
        let i = (self.el(n).pos as usize).checked_sub(1)?;
        self.el(p).children.get(i).copied()
    }

    /// The child element of the same parent just after `n`.
    pub fn next_sibling(&self, n: NodeId) -> Option<NodeId> {
        let p = self.el(n).parent?;
        self.el(p)
            .children
            .get(self.el(n).pos as usize + 1)
            .copied()
    }

    /// lxml's `text`: the text before the first child element.
    pub fn text(&self, n: NodeId) -> &str {
        &self.el(n).text
    }

    /// lxml's `tail`: the text after the element's end tag, before its next sibling.
    pub fn tail(&self, n: NodeId) -> &str {
        &self.el(n).tail
    }

    /// Sets lxml's `text` of `n`.
    pub fn set_text(&mut self, n: NodeId, text: &str) {
        text.clone_into(&mut self.el_mut(n).text);
    }

    /// Sets lxml's `tail` of `n`.
    pub fn set_tail(&mut self, n: NodeId, tail: &str) {
        tail.clone_into(&mut self.el_mut(n).tail);
    }

    /// A new, empty element named `tag`, in `parent`'s namespace, after its last child.
    pub fn append_child(&mut self, parent: NodeId, tag: &str) -> NodeId {
        let ns = self.el(parent).ns;
        let n = self.alloc(tag, None, Vec::new());
        self.el_mut(n).ns = ns;
        self.attach(parent, n);
        n
    }

    /// A new, empty element named `tag`, in `at`'s namespace, just before `at`
    /// ([`Dom::insert_many_before`] for one). When `at` is the root or has been removed, the new
    /// element is put nowhere: it is returned with no parent and the tree is unchanged.
    pub fn insert_before(&mut self, at: NodeId, tag: &str) -> NodeId {
        self.insert_many_before(&[(at, tag)])[0]
    }

    /// For each `(at, tag)`, a new, empty element named `tag`, in `at`'s namespace, just before
    /// `at`; several before the same element go in the order given. Each parent's children are
    /// rebuilt once, however many go in. Returns the new elements in the order given; one whose
    /// `at` is the root or removed has no parent, as [`Dom::insert_before`] says.
    pub fn insert_many_before(&mut self, items: &[(NodeId, &str)]) -> Vec<NodeId> {
        // One: shift the siblings after it, not rebuild them all.
        if let [(at, tag)] = items {
            let ns = self.el(*at).ns;
            let n = self.alloc(tag, None, Vec::new());
            self.el_mut(n).ns = ns;
            if let Some(p) = self.el(*at).parent {
                let pos = self.el(*at).pos as usize;
                self.el_mut(p).children.insert(pos, n);
                self.el_mut(n).parent = Some(p);
                self.renumber(p, pos);
            }
            return vec![n];
        }
        let mut made = Vec::with_capacity(items.len());
        // parent → (position of `at`, order given, new element)
        let mut by_parent: BTreeMap<NodeId, Vec<(u32, usize, NodeId)>> = BTreeMap::new();
        for (i, &(at, tag)) in items.iter().enumerate() {
            let ns = self.el(at).ns;
            let n = self.alloc(tag, None, Vec::new());
            self.el_mut(n).ns = ns;
            made.push(n);
            if let Some(p) = self.el(at).parent {
                by_parent
                    .entry(p)
                    .or_default()
                    .push((self.el(at).pos, i, n));
            }
        }
        for (p, mut new) in by_parent {
            new.sort_unstable();
            let old = std::mem::take(&mut self.el_mut(p).children);
            let mut kids = Vec::with_capacity(old.len() + new.len());
            let mut next = new.into_iter().peekable();
            for (i, c) in old.into_iter().enumerate() {
                while let Some((_, _, n)) = next.next_if(|&(pos, ..)| pos as usize == i) {
                    kids.push(n);
                }
                kids.push(c);
            }
            for (i, &c) in kids.iter().enumerate() {
                let e = self.el_mut(c);
                e.parent = Some(p);
                e.pos = u32::try_from(i).unwrap_or(u32::MAX);
            }
            self.el_mut(p).children = kids;
        }
        made
    }

    /// Takes `n` and everything under it out of the tree ([`Dom::remove_many`] for one).
    pub fn remove(&mut self, n: NodeId) {
        self.remove_many(&[n]);
    }

    /// Takes each of `nodes` and everything under it out of the tree. A removed element's tail
    /// stays where it was: it joins the tail of the nearest earlier sibling not removed, or the
    /// parent's text (`parse_utils.py` `parse_html`'s `remove_elem`, applied in document order).
    /// The root, an element already removed and a repeat are passed over. Each parent's
    /// children are compacted once, however many go.
    pub fn remove_many(&mut self, nodes: &[NodeId]) {
        // One: shift the siblings after it, not rebuild them all.
        if let [n] = nodes {
            let n = *n;
            let Some(p) = self.el(n).parent else {
                return;
            };
            let pos = self.el(n).pos as usize;
            let tail = std::mem::take(&mut self.el_mut(n).tail);
            let note = self.take_tail_note(n);
            match pos.checked_sub(1).map(|i| self.el(p).children[i]) {
                Some(prev) => {
                    self.el_mut(prev).tail.push_str(&tail);
                    self.el_mut(prev).notes |= note * IN_TAIL;
                }
                None => {
                    self.el_mut(p).text.push_str(&tail);
                    self.el_mut(p).notes |= note * IN_TEXT;
                }
            }
            self.el_mut(p).children.remove(pos);
            self.renumber(p, pos);
            self.el_mut(n).parent = None;
            return;
        }
        // (parent, position, element), in document order within each parent.
        let mut gone: Vec<(NodeId, u32, NodeId)> = nodes
            .iter()
            .filter_map(|&n| Some((self.el(n).parent?, self.el(n).pos, n)))
            .collect();
        gone.sort_unstable();
        gone.dedup();
        let removed: BTreeSet<NodeId> = gone.iter().map(|g| g.2).collect();
        // For a removed element: the nearest earlier sibling not removed.
        let mut prev_kept: BTreeMap<NodeId, Option<NodeId>> = BTreeMap::new();
        for &(p, pos, n) in &gone {
            let prev = (pos as usize).checked_sub(1).and_then(|i| {
                let s = self.el(p).children[i];
                if removed.contains(&s) {
                    prev_kept.get(&s).copied().flatten()
                } else {
                    Some(s)
                }
            });
            prev_kept.insert(n, prev);
            let tail = std::mem::take(&mut self.el_mut(n).tail);
            let note = self.take_tail_note(n);
            match prev {
                Some(s) => {
                    self.el_mut(s).tail.push_str(&tail);
                    self.el_mut(s).notes |= note * IN_TAIL;
                }
                None => {
                    self.el_mut(p).text.push_str(&tail);
                    self.el_mut(p).notes |= note * IN_TEXT;
                }
            }
        }
        let mut last = None;
        for &(p, ..) in &gone {
            if last == Some(p) {
                continue;
            }
            last = Some(p);
            let mut kids = std::mem::take(&mut self.el_mut(p).children);
            kids.retain(|c| !removed.contains(c));
            for (i, &c) in kids.iter().enumerate() {
                self.el_mut(c).pos = u32::try_from(i).unwrap_or(u32::MAX);
            }
            self.el_mut(p).children = kids;
        }
        for n in removed {
            self.el_mut(n).parent = None;
        }
    }

    /// Takes each of `nodes` out of the tree with its tail, as lxml's `remove` does (Calibre's
    /// own removals, not `remove_elem`): [`Dom::remove_many`] once their tails are dropped.
    pub(crate) fn remove_many_with_tails(&mut self, nodes: &[NodeId]) {
        for &n in nodes {
            self.el_mut(n).tail.clear();
            self.el_mut(n).notes &= !IN_TAIL;
        }
        self.remove_many(nodes);
    }

    /// Renames `n` to `tag`, in the same namespace.
    pub fn rename(&mut self, n: NodeId, tag: &str) {
        let id = self.intern(tag);
        self.el_mut(n).tag = id;
    }

    /// The element's path from the root: its index among its parent's child elements, for
    /// each ancestor below the root and itself (the `nodes` paths of
    /// `cascade/<book>/styles/NNN.json`). Empty for the root.
    pub fn path(&self, n: NodeId) -> Vec<u32> {
        let mut path = Vec::new();
        let mut at = n;
        while let Some(p) = self.el(at).parent {
            path.push(self.el(at).pos);
            at = p;
        }
        path.reverse();
        path
    }

    /// The root and every element under it, in document order.
    pub fn descendants(&self) -> impl Iterator<Item = NodeId> + '_ {
        let mut stack = vec![self.root];
        std::iter::from_fn(move || {
            let n = stack.pop()?;
            stack.extend(self.el(n).children.iter().rev());
            Some(n)
        })
    }

    /// Appends `child`, which has no parent, as the last child of `parent`.
    fn attach(&mut self, parent: NodeId, child: NodeId) {
        let pos = self.el(parent).children.len();
        self.el_mut(parent).children.push(child);
        let c = self.el_mut(child);
        c.parent = Some(parent);
        c.pos = u32::try_from(pos).unwrap_or(u32::MAX);
    }

    /// Makes `child`, which has no parent, `parent`'s first child.
    fn attach_first(&mut self, parent: NodeId, child: NodeId) {
        self.el_mut(parent).children.insert(0, child);
        self.el_mut(child).parent = Some(parent);
        self.renumber(parent, 0);
    }

    /// Sets `pos` for `parent`'s children from index `from` on.
    fn renumber(&mut self, parent: NodeId, from: usize) {
        for i in from..self.el(parent).children.len() {
            let c = self.el(parent).children[i];
            self.el_mut(c).pos = u32::try_from(i).unwrap_or(u32::MAX);
        }
    }

    /// Detaches every child of `parent` (their tails go with them) and returns them.
    fn take_children(&mut self, parent: NodeId) -> Vec<NodeId> {
        let kids = std::mem::take(&mut self.el_mut(parent).children);
        for &c in &kids {
            self.el_mut(c).parent = None;
        }
        kids
    }

    /// Detaches `n` with its tail, as lxml's `remove` does (Calibre's own removals, not
    /// `remove_elem`).
    fn detach_with_tail(&mut self, n: NodeId) {
        self.el_mut(n).tail.clear();
        self.el_mut(n).notes &= !IN_TAIL;
        self.remove(n);
    }

    /// 1 if a comment with content stood in `n`'s tail, which then moves elsewhere; else 0.
    fn take_tail_note(&mut self, n: NodeId) -> u8 {
        let e = self.el_mut(n);
        let note = u8::from(e.notes & IN_TAIL != 0);
        e.notes &= !IN_TAIL;
        note
    }

    /// Records a comment or processing instruction with content where text after `parent`'s
    /// last child goes ([`Dom::append_text`]).
    fn note_comment(&mut self, parent: NodeId) {
        match self.el(parent).children.last() {
            Some(&last) => self.el_mut(last).notes |= IN_TAIL,
            None => self.el_mut(parent).notes |= IN_TEXT,
        }
    }

    /// The number of elements the arena holds, removed ones included: one more than the
    /// largest [`NodeId`] index.
    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Appends `s` where text after `parent`'s last child goes: that child's tail, or
    /// `parent`'s text when it has no child.
    fn append_text(&mut self, parent: NodeId, s: &str) {
        match self.el(parent).children.last() {
            Some(&last) => self.el_mut(last).tail.push_str(s),
            None => self.el_mut(parent).text.push_str(s),
        }
    }

    fn is_xhtml(&self, n: NodeId, tag: &str) -> bool {
        self.tag(n) == tag && self.namespace(n) == Some(XHTML_NS)
    }
}
