// SPDX-License-Identifier: GPL-3.0-only
//! A book's XHTML file as an owned, mutable element tree, parsed and cleaned the way Calibre
//! 9.18 parses it for its conversion pipeline (`ebooks/oeb/parse_utils.py` `parse_html`, phase 2
//! plan Task 4).
//!
//! [`parse`] decodes the bytes ([`Error`] when it cannot), reads them as XML and, when they are
//! not well-formed, as HTML with `html5ever`, then cleans the tree as Calibre does. The tree is
//! lxml's model: elements only, each with the text before its first child (`text`) and the text
//! after it (`tail`); comments and processing instructions are dropped, their neighbours' text
//! joined. Element names are kept as Calibre keeps them (as written on the XML path, in lower
//! case on the HTML one), attributes by their qualified name (`epub:type`).
//!
//! Hostile input: every walk is a loop over the arena, never recursion; the tree is never
//! deeper than [`MAX_DEPTH`] below its root; the HTML parser stops at a node budget linear in
//! the input.

mod clean;
mod parse;
mod write;

pub(crate) use parse::from_xml;
pub use parse::parse;
pub use write::to_xhtml;

use deckle_limits::MAX_DEPTH;

/// The XHTML namespace.
pub(crate) const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Why a file could not be read as a tree.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Not UTF-8, no byte-order mark and no encoding declared. Calibre guesses the encoding
    /// with uchardet; the port does not (DIVERGENCES.md, "Encodings").
    #[error("this file is not UTF-8 and does not say which encoding it is in")]
    UndeclaredEncoding,
    /// Declares an encoding the port does not decode (DIVERGENCES.md, "Encodings").
    #[error("this file is in the {0} encoding, which Deckle does not read")]
    UnsupportedEncoding(String),
    /// Read as HTML, it builds far more elements than its size accounts for (the HTML parsing
    /// algorithm rebuilds formatting elements at every block), so the parse was stopped.
    #[error("this file builds more than {0} elements when read as HTML")]
    TooLarge(usize),
    /// Read as HTML (it is not XML `deckle_limits::parse_xml` reads), its elements nest more
    /// than `MAX_DEPTH` deep. Calibre falls back to libxml2's HTML parser there
    /// (DIVERGENCES.md, "XHTML that is not well-formed").
    #[error("this file is nested more than {MAX_DEPTH} levels deep")]
    TooDeep,
}

/// An element of a [`Dom`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone)]
struct Attr {
    /// The qualified name as written: `class`, `epub:type`, `xml:lang`.
    name: String,
    /// The namespace URI of a prefixed name.
    ns: Option<String>,
    value: String,
}

#[derive(Debug, Clone)]
struct Element {
    tag: String,
    ns: Option<String>,
    attrs: Vec<Attr>,
    text: String,
    tail: String,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// Its index in its parent's `children`.
    pos: u32,
}

/// An element tree in an arena. Removed elements stay in the arena, detached.
#[derive(Debug, Clone)]
pub struct Dom {
    nodes: Vec<Element>,
    root: NodeId,
}

impl Dom {
    /// A tree of one element.
    fn new(tag: &str, ns: Option<&str>) -> Dom {
        let mut dom = Dom {
            nodes: Vec::new(),
            root: NodeId(0),
        };
        dom.root = dom.alloc(tag, ns, Vec::new());
        dom
    }

    fn alloc(&mut self, tag: &str, ns: Option<&str>, attrs: Vec<Attr>) -> NodeId {
        // The arena holds at most one element per three bytes of input, or the HTML parser's
        // budget, far below u32::MAX for any file the archive reader hands over.
        let id = NodeId(u32::try_from(self.nodes.len()).unwrap_or(u32::MAX));
        self.nodes.push(Element {
            tag: tag.to_owned(),
            ns: ns.map(str::to_owned),
            attrs,
            text: String::new(),
            tail: String::new(),
            parent: None,
            children: Vec::new(),
            pos: 0,
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
        &self.el(n).tag
    }

    /// The element's namespace URI; None for an element in no namespace.
    pub(crate) fn namespace(&self, n: NodeId) -> Option<&str> {
        self.el(n).ns.as_deref()
    }

    /// The value of the attribute of qualified name `name`.
    pub fn attr(&self, n: NodeId, name: &str) -> Option<&str> {
        self.el(n)
            .attrs
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    /// The element's attributes in order: (qualified name, namespace URI, value).
    pub(crate) fn attrs(&self, n: NodeId) -> impl Iterator<Item = (&str, Option<&str>, &str)> {
        self.el(n)
            .attrs
            .iter()
            .map(|a| (a.name.as_str(), a.ns.as_deref(), a.value.as_str()))
    }

    /// Sets the attribute `name` (a name in no namespace, or one already on the element) to
    /// `value`, or removes it when `value` is None. A new attribute goes last.
    pub fn set_attr(&mut self, n: NodeId, name: &str, value: Option<&str>) {
        let attrs = &mut self.el_mut(n).attrs;
        let at = attrs.iter().position(|a| a.name == name);
        match (at, value) {
            (Some(i), Some(v)) => v.clone_into(&mut attrs[i].value),
            (Some(i), None) => {
                attrs.remove(i);
            }
            (None, Some(v)) => attrs.push(Attr {
                name: name.to_owned(),
                ns: None,
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

    /// lxml's `text`: the text before the first child element.
    pub fn text(&self, n: NodeId) -> &str {
        &self.el(n).text
    }

    /// lxml's `tail`: the text after the element's end tag, before its next sibling.
    pub fn tail(&self, n: NodeId) -> &str {
        &self.el(n).tail
    }

    /// A new, empty element named `tag`, in `at`'s namespace, inserted just before `at`. `at`
    /// should not be the root; if it is, the new element becomes the root's first child.
    pub fn insert_before(&mut self, at: NodeId, tag: &str) -> NodeId {
        let ns = self.el(at).ns.clone();
        let n = self.alloc(tag, ns.as_deref(), Vec::new());
        match self.el(at).parent {
            Some(p) => {
                let pos = self.el(at).pos as usize;
                self.insert_child(p, pos, n);
            }
            None => self.insert_child(self.root, 0, n),
        }
        n
    }

    /// Takes `n` and everything under it out of the tree. Its tail stays where it was: it
    /// joins the previous sibling's tail, or the parent's text (`parse_utils.py`
    /// `parse_html`'s `remove_elem`). Removing the root does nothing.
    pub fn remove(&mut self, n: NodeId) {
        let Some(p) = self.el(n).parent else {
            return;
        };
        let pos = self.el(n).pos as usize;
        let tail = std::mem::take(&mut self.el_mut(n).tail);
        match pos.checked_sub(1).map(|i| self.el(p).children[i]) {
            Some(prev) => self.el_mut(prev).tail.push_str(&tail),
            None => self.el_mut(p).text.push_str(&tail),
        }
        self.el_mut(p).children.remove(pos);
        self.renumber(p, pos);
        self.el_mut(n).parent = None;
    }

    /// Renames `n` to `tag`, in the same namespace.
    pub fn rename(&mut self, n: NodeId, tag: &str) {
        tag.clone_into(&mut self.el_mut(n).tag);
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
    fn append_child(&mut self, parent: NodeId, child: NodeId) {
        let pos = self.el(parent).children.len();
        self.el_mut(parent).children.push(child);
        let c = self.el_mut(child);
        c.parent = Some(parent);
        c.pos = u32::try_from(pos).unwrap_or(u32::MAX);
    }

    /// Inserts `child`, which has no parent, at `pos` among `parent`'s children.
    fn insert_child(&mut self, parent: NodeId, pos: usize, child: NodeId) {
        let pos = pos.min(self.el(parent).children.len());
        self.el_mut(parent).children.insert(pos, child);
        self.el_mut(child).parent = Some(parent);
        self.renumber(parent, pos);
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

    /// Appends `s` where text after `parent`'s last child goes: that child's tail, or
    /// `parent`'s text when it has no child.
    fn append_text(&mut self, parent: NodeId, s: &str) {
        match self.el(parent).children.last() {
            Some(&last) => self.el_mut(last).tail.push_str(s),
            None => self.el_mut(parent).text.push_str(s),
        }
    }

    fn is_xhtml(&self, n: NodeId, tag: &str) -> bool {
        let e = self.el(n);
        e.tag == tag && e.ns.as_deref() == Some(XHTML_NS)
    }
}
