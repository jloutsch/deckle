// SPDX-License-Identifier: GPL-3.0-only
//! Evaluating a compiled selector over a [`DocIndex`], as `css_selectors/select.py` does:
//! sets of elements, held as sorted positions, mapped compound by compound.
//!
//! A compound's elements are looked for in the smallest set of the index that holds them (the
//! elements of a name, class, id or attribute), else in every element. A combinator maps the
//! elements already matched to those in that relation to them, either by listing those (a
//! subtree is a range of positions) or by testing the next compound's set against them,
//! whichever has fewer elements; neither searches per element. Every element tested and every
//! step listed is charged to the index.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::TooLarge;
use super::compile::{AttrMatch, AttrTest, Compiled, Filter, Test, lang_matches, nth_ok, py_split};
use super::index::{DocIndex, NONE};
use super::parse::Comb;
use crate::dom::{Dom, NodeId};

type R<T> = Result<T, TooLarge>;

/// Sets worked out once per selector, by list or `:has()` index.
type Cache<T> = RefCell<Vec<Option<Rc<[T]>>>>;

/// Bytes of an attribute value that count as one more step when compared.
const VALUE_UNIT: usize = 64;

/// The root's position, as a set.
const ROOT: &[u32] = &[0];

pub(super) fn select(sel: &Compiled, dom: &Dom, ix: &DocIndex) -> R<Vec<NodeId>> {
    let engine = Engine {
        sel,
        dom,
        ix,
        lists: RefCell::new(vec![None; sel.lists.len()]),
        has: RefCell::new(vec![None; sel.has.len()]),
        of_list: RefCell::new(vec![None; sel.lists.len()]),
        lang_hits: RefCell::new(HashMap::new()),
    };
    let set = engine.list_set(sel.top)?;
    Ok(set.iter().map(|&p| ix.nodes[p as usize]).collect())
}

/// One selector over one document, with the sets it has worked out so far.
struct Engine<'a> {
    sel: &'a Compiled,
    dom: &'a Dom,
    ix: &'a DocIndex,
    lists: Cache<u32>,
    has: Cache<u32>,
    /// For a list's elements, in its order, how many of their siblings before and after them
    /// are in the list too (`sibling_count(…, matching=of)`).
    of_list: Cache<(u32, u32)>,
    lang_hits: RefCell<HashMap<(usize, u32), bool>>,
}

/// A compound's tests, resolved against the index.
enum Ready<'a> {
    Type(u32),
    /// A class's or an id's elements.
    In(&'a [u32]),
    Attr(&'a AttrTest),
    Root,
    Empty,
    Filter(usize),
}

/// A compound ready to test, with the smallest set of the index its matches are in (None:
/// every element) and the steps one test costs.
struct Compound<'a> {
    tests: Vec<Ready<'a>>,
    from: Option<&'a [u32]>,
    weight: u64,
}

impl<'a> Engine<'a> {
    /// The compound against the index; None when it matches nothing (a name, class, id or
    /// attribute no element has, or a test no element passes).
    fn resolve(&self, ci: usize) -> Option<Compound<'a>> {
        let ix = self.ix;
        let mut tests = Vec::new();
        let mut from: Option<&'a [u32]> = None;
        let mut narrow = |set: &'a [u32]| {
            if from.is_none_or(|f| set.len() < f.len()) {
                from = Some(set);
            }
        };
        for t in &self.sel.compounds[ci] {
            match t {
                Test::Type(name) => {
                    let (id, set) = ix.of_name(name)?;
                    narrow(set);
                    tests.push(Ready::Type(id));
                }
                Test::Id(name) => {
                    let set = ix.of_id(name)?;
                    narrow(set);
                    tests.push(Ready::In(set));
                }
                Test::Class(name) => {
                    let set = ix.of_class(name)?;
                    narrow(set);
                    tests.push(Ready::In(set));
                }
                Test::Attr(a) => {
                    narrow(ix.of_attr(&a.name)?);
                    tests.push(Ready::Attr(a));
                }
                Test::Root => {
                    narrow(ROOT);
                    tests.push(Ready::Root);
                }
                Test::Empty => tests.push(Ready::Empty),
                Test::Never => return None,
                Test::Filter(f) => match self.sel.filters[*f] {
                    Filter::All => {}
                    Filter::Never => return None,
                    _ => tests.push(Ready::Filter(*f)),
                },
            }
        }
        // A test costs a step; one that looks a list's or a `:has()`'s elements up, two
        // (three with the counts of an `of` list): about as long as each other in time.
        let weight = tests
            .iter()
            .map(|t| match *t {
                Ready::Filter(f) => match self.sel.filters[f] {
                    Filter::In(_) | Filter::NotIn(_) | Filter::Has(_) => 2,
                    Filter::Nth { of: Some(_), .. } => 3,
                    _ => 1,
                },
                _ => 1,
            })
            .sum::<u64>()
            .max(1);
        Some(Compound {
            tests,
            from,
            weight,
        })
    }

    /// Whether the element at `p` passes the compound's tests.
    fn test(&self, c: &Compound<'_>, p: u32) -> R<bool> {
        self.ix.charge(c.weight)?;
        for t in &c.tests {
            let ok = match *t {
                Ready::Type(id) => self.ix.type_id[p as usize] == id,
                Ready::In(set) => set.binary_search(&p).is_ok(),
                Ready::Attr(a) => self.attr(a, p)?,
                Ready::Root => p == 0,
                Ready::Empty => self.dom.is_empty(self.ix.nodes[p as usize]),
                Ready::Filter(f) => self.filter(f, p)?,
            };
            if !ok {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The elements of `cands`, in order, that pass the compound's tests.
    fn keep(&self, c: &Compound<'_>, cands: impl IntoIterator<Item = u32>) -> R<Vec<u32>> {
        let mut out = Vec::new();
        for p in cands {
            if self.test(c, p)? {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// The compound's elements.
    fn all(&self, c: &Compound<'_>) -> R<Vec<u32>> {
        match c.from {
            Some(set) => self.keep(c, set.iter().copied()),
            None => self.keep(c, 0..self.ix.len()),
        }
    }

    /// The elements of a list (`selector_list_matches`), sorted.
    fn list_set(&self, id: usize) -> R<Rc<[u32]>> {
        let cached = self.lists.borrow()[id].clone();
        if let Some(set) = cached {
            return Ok(set);
        }
        let mut set = Vec::new();
        let complexes = &self.sel.lists[id];
        for &c in complexes {
            set.extend(self.complex_set(c)?);
        }
        if complexes.len() > 1 {
            self.ix.charge(set.len() as u64)?;
            set.sort_unstable();
            set.dedup();
        }
        let set: Rc<[u32]> = set.into();
        self.lists.borrow_mut()[id] = Some(Rc::clone(&set));
        Ok(set)
    }

    /// The elements a complex selector matches, compound by compound from the left
    /// (`select_combinedselector` and the combinators after it): each step keeps the elements
    /// in the right relation to the last step's that match the next compound.
    fn complex_set(&self, id: usize) -> R<Vec<u32>> {
        let parts = &self.sel.complexes[id];
        let Some(ready) = parts
            .iter()
            .map(|&(_, ci)| self.resolve(ci))
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(Vec::new());
        };
        let mut cur = Vec::new();
        for (i, c) in ready.iter().enumerate() {
            cur = if i == 0 {
                self.all(c)?
            } else {
                self.step(parts[i].0, &cur, c)?
            };
            if cur.is_empty() {
                break;
            }
        }
        Ok(cur)
    }

    /// The element before `p` among its siblings, comments passed over (`itersiblings('*')`).
    fn prev_sibling(&self, p: u32) -> Option<u32> {
        let e = self.dom.prev_element(self.ix.nodes[p as usize])?;
        Some(self.ix.at(e))
    }

    /// The element after `p` among its siblings, comments passed over.
    fn next_sibling(&self, p: u32) -> Option<u32> {
        let e = self.dom.next_element(self.ix.nodes[p as usize])?;
        Some(self.ix.at(e))
    }

    /// The child elements of `p` (`iterchildren('*')`).
    fn children(&self, p: u32) -> impl Iterator<Item = u32> + '_ {
        let e = self.ix.nodes[p as usize];
        self.dom.child_elements(e).map(|c| self.ix.at(c))
    }

    /// The elements in relation `comb` to one of `prev` that pass compound `c`.
    fn step(&self, comb: Comb, prev: &[u32], c: &Compound<'_>) -> R<Vec<u32>> {
        let ix = self.ix;
        self.ix.charge(prev.len() as u64)?;
        let in_prev = |p: u32| p != NONE && prev.binary_search(&p).is_ok();
        // The candidates `c.from` holds when it is smaller than the elements in relation.
        let from = |listed: u64| c.from.filter(|set| (set.len() as u64) <= listed);
        match comb {
            Comb::Descendant => {
                // The subtrees below `prev`, as ranges of positions; a subtree inside one
                // already listed adds nothing.
                let mut ranges: Vec<(u32, u32)> = Vec::new();
                let mut listed = 0u64;
                for &p in prev {
                    let (start, end) = (p + 1, ix.end[p as usize]);
                    if ranges.last().is_some_and(|&(_, e)| start < e) || start >= end {
                        continue;
                    }
                    ranges.push((start, end));
                    listed += u64::from(end - start);
                }
                if let Some(set) = from(listed) {
                    self.ix.charge(set.len() as u64)?;
                    let mut r = 0;
                    let inside = set.iter().copied().filter(|&p| {
                        while r < ranges.len() && ranges[r].1 <= p {
                            r += 1;
                        }
                        r < ranges.len() && ranges[r].0 <= p
                    });
                    return self.keep(c, inside.collect::<Vec<_>>());
                }
                self.keep(c, ranges.iter().flat_map(|&(s, e)| s..e))
            }
            Comb::Child => {
                let listed: u64 = prev
                    .iter()
                    .map(|&p| self.dom.element_count(ix.nodes[p as usize]) as u64)
                    .sum();
                if let Some(set) = from(listed) {
                    self.ix.charge(set.len() as u64)?;
                    let inside: Vec<u32> = set
                        .iter()
                        .copied()
                        .filter(|&p| in_prev(ix.parent[p as usize]))
                        .collect();
                    return self.keep(c, inside);
                }
                let mut kids: Vec<u32> = prev.iter().flat_map(|&p| self.children(p)).collect();
                kids.sort_unstable();
                self.keep(c, kids)
            }
            Comb::Next => {
                if let Some(set) = from(prev.len() as u64) {
                    self.ix.charge(set.len() as u64)?;
                    let inside: Vec<u32> = set
                        .iter()
                        .copied()
                        .filter(|&p| self.prev_sibling(p).is_some_and(in_prev))
                        .collect();
                    return self.keep(c, inside);
                }
                let mut next: Vec<u32> =
                    prev.iter().filter_map(|&p| self.next_sibling(p)).collect();
                next.sort_unstable();
                self.keep(c, next)
            }
            Comb::Later => {
                // Each parent's first element in `prev`: the siblings after it are all the
                // siblings after any.
                let mut first: HashMap<u32, u32> = HashMap::new();
                for &p in prev {
                    first.entry(ix.parent[p as usize]).or_insert(p);
                }
                let listed: u64 = first
                    .values()
                    .map(|&p| u64::from(ix.siblings[p as usize] - ix.child_index[p as usize] - 1))
                    .sum();
                if let Some(set) = from(listed) {
                    self.ix.charge(set.len() as u64)?;
                    let inside: Vec<u32> = set
                        .iter()
                        .copied()
                        .filter(|&p| {
                            let parent = ix.parent[p as usize];
                            parent != NONE && first.get(&parent).is_some_and(|&f| f < p)
                        })
                        .collect();
                    return self.keep(c, inside);
                }
                let mut later = Vec::new();
                for &p in first.values() {
                    let mut at = self.next_sibling(p);
                    while let Some(s) = at {
                        later.push(s);
                        at = self.next_sibling(s);
                    }
                }
                later.sort_unstable();
                self.keep(c, later)
            }
        }
    }

    /// `select_relation`: the anchors of a `:has()`, sorted.
    fn has_set(&self, id: usize) -> R<Rc<[u32]>> {
        let cached = self.has.borrow()[id].clone();
        if let Some(set) = cached {
            return Ok(set);
        }
        let rels = &self.sel.has[id];
        let mut set = Vec::new();
        for rel in rels {
            set.extend(self.anchors(rel)?);
        }
        if rels.len() > 1 {
            self.ix.charge(set.len() as u64)?;
            set.sort_unstable();
            set.dedup();
        }
        let set: Rc<[u32]> = set.into();
        self.has.borrow_mut()[id] = Some(Rc::clone(&set));
        Ok(set)
    }

    /// `relative_selector_anchors`: from the rightmost compound's elements, back through each
    /// combinator (`inverse_combinator_matches`) and the compound to its left.
    fn anchors(&self, rel: &[(Comb, usize)]) -> R<Vec<u32>> {
        let Some(ready) = rel
            .iter()
            .map(|&(_, ci)| self.resolve(ci))
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(Vec::new());
        };
        let mut targets: Option<Vec<u32>> = None;
        for (&(comb, _), c) in rel.iter().zip(&ready) {
            let t = match targets.take() {
                None => self.all(c)?,
                Some(t) => self.keep(c, t)?,
            };
            if t.is_empty() {
                return Ok(Vec::new());
            }
            targets = Some(self.inverse(comb, &t)?);
        }
        Ok(targets.unwrap_or_default())
    }

    /// `inverse_combinator_matches`: the elements `targets` are in relation `comb` to, sorted.
    /// Walks stop at an element already found, as Calibre's do, so each is linear.
    fn inverse(&self, comb: Comb, targets: &[u32]) -> R<Vec<u32>> {
        let ix = self.ix;
        self.ix.charge(targets.len() as u64)?;
        let mut out = Vec::new();
        match comb {
            Comb::Child => {
                out.extend(
                    targets
                        .iter()
                        .map(|&p| ix.parent[p as usize])
                        .filter(|&p| p != NONE),
                );
            }
            Comb::Next => out.extend(targets.iter().filter_map(|&p| self.prev_sibling(p))),
            Comb::Descendant | Comb::Later => {
                let mut found = HashSet::new();
                for &p in targets {
                    let up = |q: u32| match comb {
                        Comb::Descendant => Some(ix.parent[q as usize]).filter(|&a| a != NONE),
                        _ => self.prev_sibling(q),
                    };
                    let mut at = up(p);
                    while let Some(a) = at {
                        if !found.insert(a) {
                            break;
                        }
                        self.ix.charge(1)?;
                        out.push(a);
                        at = up(a);
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    /// For each element of a list, how many of its siblings before and after it are in it.
    fn of_list_counts(&self, list: usize, set: &[u32]) -> R<Rc<[(u32, u32)]>> {
        let cached = self.of_list.borrow()[list].clone();
        if let Some(c) = cached {
            return Ok(c);
        }
        self.ix.charge(set.len() as u64)?;
        let mut seen: HashMap<u32, u32> = HashMap::new();
        let before: Vec<u32> = set
            .iter()
            .map(|&p| {
                let n = seen.entry(self.ix.parent[p as usize]).or_insert(0);
                *n += 1;
                *n - 1
            })
            .collect();
        let counts: Rc<[(u32, u32)]> = set
            .iter()
            .zip(before)
            .map(|(&p, b)| (b, seen[&self.ix.parent[p as usize]] - b - 1))
            .collect();
        self.of_list.borrow_mut()[list] = Some(Rc::clone(&counts));
        Ok(counts)
    }

    /// `select_attrib`: whether an attribute of the element named as `a` asks has a value that
    /// passes.
    fn attr(&self, a: &AttrTest, p: u32) -> R<bool> {
        let e = self.ix.nodes[p as usize];
        for (qname, ns, value) in self.dom.attrs(e) {
            if !attr_is(qname, ns, self.ix.root_ns, &a.name) {
                continue;
            }
            if a.op == AttrMatch::Exists {
                return Ok(true);
            }
            self.ix.charge((value.len() / VALUE_UNIT) as u64)?;
            if value_matches(a, value) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn filter(&self, f: usize, p: u32) -> R<bool> {
        let ix = self.ix;
        let i = p as usize;
        Ok(match &self.sel.filters[f] {
            Filter::All => true,
            Filter::Never => false,
            Filter::AnyLink => {
                matches!(ix.type_name(p), "a" | "area" | "link")
                    && self.dom.attr(ix.nodes[i], "href").is_some()
            }
            Filter::Lang(want) => {
                let src = ix.lang_src[i];
                if src == NONE {
                    return Ok(false);
                }
                let cached = self.lang_hits.borrow().get(&(f, src)).copied();
                if let Some(hit) = cached {
                    return Ok(hit);
                }
                let lang = self.dom.attr(ix.nodes[src as usize], "lang").unwrap_or("");
                ix.charge((lang.len() / VALUE_UNIT) as u64)?;
                let hit = lang_matches(lang, want);
                self.lang_hits.borrow_mut().insert((f, src), hit);
                hit
            }
            Filter::Nth {
                last,
                of_type,
                a,
                b,
                of,
            } => {
                // `sibling_count` raises for the root: no match.
                if ix.parent[i] == NONE {
                    return Ok(false);
                }
                let (before, after) = if let Some(list) = *of {
                    let set = self.list_set(list)?;
                    let Ok(k) = set.binary_search(&p) else {
                        return Ok(false);
                    };
                    self.of_list_counts(list, &set)?[k]
                } else if *of_type {
                    (ix.type_before[i], ix.type_after[i])
                } else {
                    let k = ix.child_index[i];
                    (k, ix.siblings[i] - k - 1)
                };
                let count = if *last { after } else { before };
                nth_ok(*a, *b, i128::from(count) + 1)
            }
            Filter::Only { of_type } => {
                if ix.parent[i] == NONE {
                    return Ok(false);
                }
                if *of_type {
                    ix.type_before[i] == 0 && ix.type_after[i] == 0
                } else {
                    ix.siblings[i] == 1
                }
            }
            Filter::In(list) => self.list_set(*list)?.binary_search(&p).is_ok(),
            Filter::NotIn(list) => self.list_set(*list)?.binary_search(&p).is_err(),
            Filter::Has(h) => self.has_set(*h)?.binary_search(&p).is_ok(),
        })
    }
}

/// Whether an attribute's name, as `attrib_map` keys it, is `name` (in ASCII lower case): the
/// local name when the root has a namespace, else `{uri}local` for a namespaced one.
fn attr_is(qname: &str, ns: Option<&str>, root_ns: bool, name: &str) -> bool {
    let local = || qname.split_once(':').map_or(qname, |(_, l)| l);
    match ns {
        None => qname.eq_ignore_ascii_case(name),
        Some(_) if root_ns => local().eq_ignore_ascii_case(name),
        Some(ns) => {
            let (b, ns, local) = (name.as_bytes(), ns.as_bytes(), local().as_bytes());
            b.len() == ns.len() + local.len() + 2
                && b[0] == b'{'
                && b[1..=ns.len()].eq_ignore_ascii_case(ns)
                && b[ns.len() + 1] == b'}'
                && b[ns.len() + 2..].eq_ignore_ascii_case(local)
        }
    }
}

/// `select_equals`, `select_includes` and the rest, with the value [`AttrTest`] prepared;
/// with `i`, both compared in ASCII lower case. Only `*=` with `i` copies the element's value.
fn value_matches(a: &AttrTest, val: &str) -> bool {
    let (v, w) = (val.as_bytes(), a.value.as_bytes());
    let same = |x: &[u8]| {
        if a.ci {
            x.eq_ignore_ascii_case(w)
        } else {
            x == w
        }
    };
    match a.op {
        AttrMatch::Exists => true,
        AttrMatch::Equal => same(v),
        AttrMatch::Includes => py_split(val).any(|t| same(t.as_bytes())),
        AttrMatch::Dash => {
            same(v) || (v.len() > w.len() && same(&v[..w.len()]) && v[w.len()] == b'-')
        }
        AttrMatch::Prefix => v.len() >= w.len() && same(&v[..w.len()]),
        AttrMatch::Suffix => v.len() >= w.len() && same(&v[v.len() - w.len()..]),
        AttrMatch::Substring if a.ci => val.to_ascii_lowercase().contains(a.value.as_str()),
        AttrMatch::Substring => val.contains(a.value.as_str()),
    }
}
