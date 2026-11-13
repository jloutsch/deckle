// SPDX-License-Identifier: GPL-3.0-only
//! What [`super::select`] needs to know of a document, worked out once for all its selectors:
//! `select.py`'s `element_map`, `id_map`, `class_map`, `attrib_map` and `lang_map`, the elements in document
//! order with where each one's subtree ends, sibling positions, and the work charged so far.

use std::cell::Cell;
use std::collections::HashMap;

use deckle_limits::MAX_SELECTOR_WORK;

use super::TooLarge;
use super::compile::py_split;
use crate::dom::{Dom, NodeId};

/// No element: the root's parent, an element's that is not in the tree.
pub(super) const NONE: u32 = u32::MAX;

/// One document's elements as `Select` sees them, built once and shared by every
/// [`super::select`] call on that document, which it charges for their work.
///
/// Elements are named by their position in document order, the root's 0.
pub struct DocIndex {
    /// The elements in document order.
    pub(super) nodes: Vec<NodeId>,
    /// Each element's position, by [`NodeId::index`]; [`NONE`] for one not in the tree.
    pos: Vec<u32>,
    /// The parent's position; [`NONE`] for the root.
    pub(super) parent: Vec<u32>,
    /// One past the last position in the element's subtree.
    pub(super) end: Vec<u32>,
    /// The element's index among its parent's children, and how many they are (the root: 0
    /// of 1).
    pub(super) child_index: Vec<u32>,
    pub(super) siblings: Vec<u32>,
    /// The root is in a namespace (Calibre's `'{' in self.root.tag`), so names drop theirs.
    pub(super) root_ns: bool,
    /// The element's `map_tag_name` key, as an index into `type_names`.
    pub(super) type_id: Vec<u32>,
    type_names: Vec<String>,
    types: HashMap<String, u32>,
    /// Each name's elements, in document order (`element_map`).
    by_type: Vec<Vec<u32>>,
    /// Each class's elements (`class_map`) and each id's (`id_map`), keys in ASCII lower case.
    by_class: HashMap<String, Vec<u32>>,
    by_id: HashMap<String, Vec<u32>>,
    /// Each attribute name's elements, keyed as `attrib_map` keys them, in ASCII lower case:
    /// the local name when the root has a namespace, else `{uri}local` for a namespaced one.
    by_attr: HashMap<String, Vec<u32>>,
    /// How many siblings before and after the element share its name (`sibling_count(…,
    /// same_type=True)`).
    pub(super) type_before: Vec<u32>,
    pub(super) type_after: Vec<u32>,
    /// The element whose `lang` gives this one's language (`lang_map`: the nearest with a
    /// non-empty `lang`; `xml:lang` is read and then overwritten); [`NONE`] for none.
    pub(super) lang_src: Vec<u32>,
    /// Steps charged by every [`super::select`] call so far.
    work: Cell<u64>,
}

impl DocIndex {
    /// Indexes `dom`, in time and memory linear in its size. Index the document again after
    /// changing it.
    pub fn new(dom: &Dom) -> DocIndex {
        let nodes: Vec<NodeId> = dom.descendants().collect();
        let n = nodes.len();
        let at = |i: usize| u32::try_from(i).unwrap_or(NONE);
        let mut pos = vec![NONE; nodes.iter().map(|e| e.index() + 1).max().unwrap_or(0)];
        for (i, e) in nodes.iter().enumerate() {
            pos[e.index()] = at(i);
        }
        let parent: Vec<u32> = nodes
            .iter()
            .map(|&e| dom.parent(e).map_or(NONE, |p| pos[p.index()]))
            .collect();
        // Each subtree's size, from the last element back: an element's descendants come
        // after it.
        let mut size = vec![1u32; n];
        for i in (1..n).rev() {
            let p = parent[i];
            if p != NONE {
                size[p as usize] = size[p as usize].saturating_add(size[i]);
            }
        }
        let end = (0..n).map(|i| at(i).saturating_add(size[i])).collect();
        let child_index = nodes.iter().map(|&e| at(dom.index_in_parent(e))).collect();
        let siblings = nodes
            .iter()
            .map(|&e| dom.parent(e).map_or(1, |p| at(dom.children(p).len())))
            .collect();
        let root_ns = dom.namespace(dom.root()).is_some();

        let mut index = DocIndex {
            nodes,
            pos,
            parent,
            end,
            child_index,
            siblings,
            root_ns,
            type_id: Vec::with_capacity(n),
            type_names: Vec::new(),
            types: HashMap::new(),
            by_type: Vec::new(),
            by_class: HashMap::new(),
            by_id: HashMap::new(),
            by_attr: HashMap::new(),
            type_before: vec![0; n],
            type_after: vec![0; n],
            lang_src: vec![NONE; n],
            work: Cell::new(0),
        };
        index.names(dom);
        index.of_type(dom);
        index
    }

    /// Element names, classes, ids and languages.
    fn names(&mut self, dom: &Dom) {
        let mut key = String::new();
        for i in 0..self.nodes.len() {
            let e = self.nodes[i];
            let p = u32::try_from(i).unwrap_or(NONE);
            // `map_tag_name`: the name in ASCII lower case, its namespace dropped when the
            // root has one, else written `{uri}name`.
            key.clear();
            if let Some(ns) = dom.namespace(e).filter(|_| !self.root_ns) {
                key.push('{');
                key.push_str(ns);
                key.push('}');
            }
            key.push_str(dom.tag(e));
            key.make_ascii_lowercase();
            let id = match self.types.get(key.as_str()) {
                Some(&id) => id,
                None => {
                    let id = u32::try_from(self.type_names.len()).unwrap_or(NONE);
                    self.types.insert(key.clone(), id);
                    self.type_names.push(key.clone());
                    self.by_type.push(Vec::new());
                    id
                }
            };
            self.type_id.push(id);
            self.by_type[id as usize].push(p);

            if let Some(classes) = dom.attr(e, "class") {
                for class in py_split(classes) {
                    add(&mut self.by_class, &mut key, class, p);
                }
            }
            if let Some(v) = dom.attr(e, "id") {
                add(&mut self.by_id, &mut key, v, p);
            }
            for (qname, ns, _) in dom.attrs(e) {
                let local = qname.split_once(':').map_or(qname, |(_, l)| l);
                match ns {
                    None => add(&mut self.by_attr, &mut key, qname, p),
                    Some(_) if self.root_ns => add(&mut self.by_attr, &mut key, local, p),
                    Some(ns) => add(&mut self.by_attr, &mut key, &format!("{{{ns}}}{local}"), p),
                }
            }
            self.lang_src[i] = if dom.attr(e, "lang").is_some_and(|v| !v.is_empty()) {
                p
            } else {
                match self.parent[i] {
                    NONE => NONE,
                    q => self.lang_src[q as usize],
                }
            };
        }
    }

    /// [`DocIndex::type_before`] and [`DocIndex::type_after`], one parent at a time.
    fn of_type(&mut self, dom: &Dom) {
        let mut seen = vec![0u32; self.type_names.len()];
        for &e in &self.nodes {
            let kids = dom.children(e);
            for &c in kids {
                let c = self.pos[c.index()] as usize;
                let t = self.type_id[c] as usize;
                self.type_before[c] = seen[t];
                seen[t] += 1;
            }
            for &c in kids {
                let c = self.pos[c.index()] as usize;
                let t = self.type_id[c] as usize;
                self.type_after[c] = seen[t] - self.type_before[c] - 1;
            }
            for &c in kids {
                seen[self.type_id[self.pos[c.index()] as usize] as usize] = 0;
            }
        }
    }

    /// How many elements the document has.
    pub(super) fn len(&self) -> u32 {
        u32::try_from(self.nodes.len()).unwrap_or(NONE)
    }

    /// The position of `e`, in the tree.
    pub(super) fn at(&self, e: NodeId) -> u32 {
        self.pos[e.index()]
    }

    pub(super) fn type_name(&self, p: u32) -> &str {
        &self.type_names[self.type_id[p as usize] as usize]
    }

    /// The name's index, and its elements; None when no element has it.
    pub(super) fn of_name(&self, name: &str) -> Option<(u32, &[u32])> {
        let &id = self.types.get(name)?;
        Some((id, &self.by_type[id as usize]))
    }

    pub(super) fn of_class(&self, name: &str) -> Option<&[u32]> {
        self.by_class.get(name).map(Vec::as_slice)
    }

    pub(super) fn of_id(&self, name: &str) -> Option<&[u32]> {
        self.by_id.get(name).map(Vec::as_slice)
    }

    pub(super) fn of_attr(&self, name: &str) -> Option<&[u32]> {
        self.by_attr.get(name).map(Vec::as_slice)
    }

    /// Adds `n` steps to the work done on this document; [`TooLarge`] once the total is past
    /// [`MAX_SELECTOR_WORK`].
    pub(super) fn charge(&self, n: u64) -> Result<(), TooLarge> {
        let work = self.work.get().saturating_add(n);
        self.work.set(work);
        if work > MAX_SELECTOR_WORK {
            return Err(TooLarge);
        }
        Ok(())
    }

    /// The steps [`super::select`] has charged to this document so far.
    pub fn work(&self) -> u64 {
        self.work.get()
    }
}

/// Files element `p` under `name` in ASCII lower case, once.
fn add(map: &mut HashMap<String, Vec<u32>>, key: &mut String, name: &str, p: u32) {
    key.clear();
    key.push_str(name);
    key.make_ascii_lowercase();
    match map.get_mut(key.as_str()) {
        Some(v) => {
            if v.last() != Some(&p) {
                v.push(p);
            }
        }
        None => {
            map.insert(key.clone(), vec![p]);
        }
    }
}
