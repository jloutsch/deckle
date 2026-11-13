// SPDX-License-Identifier: GPL-3.0-only
//! The first cascade: a book file's rules applied to its elements as Calibre 9.18's first-pass
//! `Stylizer` applies them, before its CSS flattener runs (phase 2 plan, Task 8). Ported from
//! `ebooks/oeb/stylizer.py` (`Stylizer.__init__`, `StylizerRules.__init__`, `Style._update_style`,
//! `Style._update_pseudo_class`, `Style._apply_style_attr`; GPL v3, `NOTICE`).
//!
//! Hostile input: the rules of a file are bounded ([`MAX_STYLE_RULES`] over all its sheets),
//! matching them by [`deckle_limits::MAX_SELECTOR_WORK`] (the selectors start from the
//! document's index of names, classes and ids, not from every element), and the styles stored
//! by [`MAX_CASCADE_DECLS`]. Elements the same rules match share one copy of their
//! declarations: the cascade first works out, rule by rule, which set of rules matches each
//! element (a number for each set), and then builds the declarations of each set once.

use std::collections::HashMap;
use std::sync::Arc;

use deckle_limits::{MAX_CASCADE_DECLS, MAX_STYLE_RULES};

use super::budget::TEXT_UNIT;
use super::select::dropped_specificity;
use super::value::style_attr_in;
use super::{
    Decl, Decls, DocIndex, Error, HTML_CSS, Origin, Parsed, Profile, Rule, Selector, Sheet,
    normalize, parse_selector, parse_sheet, select,
};
use crate::dom::{Dom, NodeId, XHTML_NS, py_space};

/// A file's first cascade: what `Stylizer.__init__` leaves in its `Style` objects and rules.
#[derive(Debug, Clone)]
pub struct Cascaded {
    /// Each element's declarations (`Style._style`), by [`NodeId`] index ([`Self::specified`]):
    /// names in the order they were first set, each with its value and whether it was set
    /// `!important`. Elements the same rules and `style` attribute reach share one copy.
    pub specified: Vec<Arc<Decls>>,
    /// Each element's pseudo-class declarations (`Style._pseudo_classes`), by [`NodeId`] index
    /// ([`Self::pseudo`]), in the order the names were first set. Calibre keeps them as plain
    /// dictionaries: none is `!important`.
    pub pseudo: Vec<Vec<(&'static str, Decls)>>,
    /// Every rule, Calibre's user-agent sheet's first, in the order they were applied
    /// (`StylizerRules.rules`), their declarations normalized.
    pub rules: Vec<Rule>,
    /// The `@font-face` rules of the file's sheets, in order (`StylizerRules.font_face_rules`).
    pub font_face: Vec<Decls>,
}

impl Cascaded {
    /// The declarations of element `n`.
    pub fn specified(&self, n: NodeId) -> &Decls {
        &self.specified[n.index()]
    }

    /// The declarations of element `n`, shared with the elements styled alike.
    pub fn specified_arc(&self, n: NodeId) -> &Arc<Decls> {
        &self.specified[n.index()]
    }

    /// The pseudo-class declarations of element `n`.
    pub fn pseudo(&self, n: NodeId) -> &[(&'static str, Decls)] {
        &self.pseudo[n.index()]
    }
}

/// `stylizer.py` `Stylizer.__init__` with no base, extra or user CSS: `dom`'s elements styled
/// by Calibre's `html.css` and then `sheets`, the stylesheets of the file at `href` (from the
/// book's root) in Calibre's order (each `<style>` and `<link rel=stylesheet>` in document
/// order, a `<style>`'s imports before it), as [`parse_sheet`] read them with one rule index
/// running across them. Run [`crate::dom::mark_chapters`] first: Calibre marks chapters before
/// the Stylizer.
///
/// - The rules are sorted by (origin, specificity, index) and applied in that order
///   (`StylizerRules.__init__`): a later rule's value replaces an earlier one's unless the
///   earlier one is `!important` and the later one is not (`Style._update_style`), so a book's
///   `!important` beats one of `html.css`.
/// - A rule whose selector names one of Calibre's inappropriate pseudo-classes (`:hover`,
///   `::before`, ...) goes to that pseudo-class's declarations instead (`_update_pseudo_class`).
///   A rule whose selector Calibre's `Select` refuses is skipped.
/// - Then each XHTML element's `style` attribute, read as `Style._apply_style_attr` reads it,
///   its `url()`s made book-absolute from `href`, applied the same way.
/// - Then each XHTML `<img>` with a `width` or `height` attribute whose CSS width and height
///   are both `auto` (or unset) loses both attributes, which become declarations, a bare
///   number in `px`.
///
/// [`Error::TooLarge`] past [`MAX_STYLE_RULES`] rules in `sheets`, [`Error::Selectors`] past
/// the selector work limit, [`Error::Cascade`] past [`MAX_CASCADE_DECLS`] units.
pub fn cascade(
    dom: &mut Dom,
    href: &str,
    sheets: Vec<Sheet>,
    profile: &Profile,
) -> Result<Cascaded, Error> {
    let book_rules: usize = sheets.iter().map(|s| s.rules.len()).sum();
    if book_rules > MAX_STYLE_RULES {
        return Err(Error::TooLarge(MAX_STYLE_RULES));
    }
    // `stylesheets = [html_css_stylesheet()]`, then the book's.
    let ua = parse_sheet(HTML_CSS, Origin::UserAgent, "", 0, &mut |_| None)?;
    let mut font_face = Vec::new();
    let mut all: Vec<Applied> = Vec::with_capacity(ua.rules.len() + book_rules);
    for mut sheet in std::iter::once(ua).chain(sheets) {
        sheet.normalize(profile);
        font_face.append(&mut sheet.font_face);
        all.extend(sheet.rules.into_iter().map(Applied::new));
    }
    // `self.rules.sort(key=itemgetter(0))`: (origin, 0, ids, classes, types, index); stable.
    all.sort_by_key(|a| (a.rule.origin, a.specificity, a.rule.index));

    let mut units = Units(0);
    let index = DocIndex::new(dom);
    let n = dom.node_count();
    // Which rules reach each element: a state is the set reached so far, made of its parent
    // state and one more rule. State 0 is no rule.
    let mut states = States {
        parent: vec![0],
        rule: vec![u32::MAX],
    };
    let mut state = vec![0u32; n];
    let mut pseudo: Vec<Vec<(&'static str, Decls)>> = vec![Vec::new(); n];
    for (i, a) in all.iter().enumerate() {
        let Some((selector, ps)) = &a.compiled else {
            // `except SelectorError: continue`.
            continue;
        };
        // Updating with no declarations changes nothing; a pseudo-class still gets its
        // (empty) dictionary.
        if ps.is_none() && a.rule.decls.is_empty() {
            continue;
        }
        let matched = select(selector, dom, &index)?;
        match ps {
            // `_update_pseudo_class`: `orig.update(cssdict)`.
            Some(name) => {
                for e in matched {
                    let list = &mut pseudo[e.index()];
                    let at = match list.iter().position(|(k, _)| k == name) {
                        Some(at) => at,
                        None => {
                            list.push((name, Decls::new()));
                            list.len() - 1
                        }
                    };
                    let target = &mut list[at].1;
                    units.charge(target.len() + a.units)?;
                    plain_update(target, &a.rule.decls);
                }
            }
            None => {
                let rule = u32::try_from(i).unwrap_or(u32::MAX);
                let mut moved: HashMap<u32, u32> = HashMap::new();
                // Neighbours are mostly in the same state: the last move first.
                let mut last = (u32::MAX, 0);
                for e in matched {
                    let from = state[e.index()];
                    let to = if last.0 == from {
                        last.1
                    } else if let Some(&to) = moved.get(&from) {
                        to
                    } else {
                        units.charge(1)?;
                        let to = states.push(from, rule);
                        moved.insert(from, to);
                        to
                    };
                    last = (from, to);
                    state[e.index()] = to;
                }
            }
        }
    }

    // Each set's declarations, built once.
    let mut built = Built {
        memo: HashMap::from([(0, Arc::new(Decls::new()))]),
        states: &states,
        all: &all,
    };
    let mut finals = state.clone();
    finals.sort_unstable();
    finals.dedup();
    for s in finals {
        // The set before the last rule too, which sets differing in their last rule share.
        let parent = states.parent[s as usize];
        built.build(parent, &mut units)?;
        built.build(s, &mut units)?;
    }
    let mut specified: Vec<Arc<Decls>> = state.iter().map(|s| Arc::clone(&built.memo[s])).collect();
    drop(built);

    // `xpath(tree, '//h:*[@style]')`: `_apply_style_attr`. Elements whose rules and style
    // attribute are the same share the result.
    let mut styled: HashMap<(usize, &str), Arc<Decls>> = HashMap::new();
    for e in dom.descendants() {
        if dom.namespace(e) != Some(XHTML_NS) {
            continue;
        }
        let Some(text) = dom.attr(e, "style") else {
            continue;
        };
        let base = &specified[e.index()];
        let key = (Arc::as_ptr(base) as usize, text);
        if let Some(done) = styled.get(&key) {
            specified[e.index()] = Arc::clone(done);
            continue;
        }
        units.charge(1 + text.len() / TEXT_UNIT)?;
        let add = normalize(style_attr_in(text, href), profile);
        let result = Arc::new(updated(base, &add, &mut units)?);
        // A base dropped when its last element takes the result frees its address, but only
        // for results, which are never a base: the keys stay unique.
        styled.insert(key, Arc::clone(&result));
        specified[e.index()] = result;
    }
    drop(styled);

    // `xpath(tree, '//h:img[@width or @height]')`.
    let imgs: Vec<NodeId> = dom
        .descendants()
        .filter(|&e| {
            dom.tag(e) == "img"
                && dom.namespace(e) == Some(XHTML_NS)
                && (dom.attr(e, "width").is_some() || dom.attr(e, "height").is_some())
        })
        .collect();
    for e in imgs {
        let base = &specified[e.index()];
        let get = |name: &str| {
            base.iter()
                .find(|(k, _)| k == name)
                .map_or("auto", |(_, d)| d.value.as_str())
        };
        // `is_styled`: compared as written, so `AUTO` is a size.
        if get("width") != "auto" || get("height") != "auto" {
            continue;
        }
        let mut upd = Decls::new();
        for prop in ["width", "height"] {
            let val = dom
                .attr(e, prop)
                .unwrap_or("")
                .trim_matches(py_space)
                .to_owned();
            dom.set_attr(e, prop, None);
            if val.is_empty() {
                continue;
            }
            // `num_pat = re.compile(r'[0-9.]+$')`.
            let value = if val.chars().all(|c| c.is_ascii_digit() || c == '.') {
                format!("{val}px")
            } else {
                val
            };
            upd.push((
                prop.to_owned(),
                Decl {
                    value,
                    important: false,
                },
            ));
        }
        if !upd.is_empty() {
            units.charge(upd.len())?;
            let result = updated(base, &upd, &mut units)?;
            specified[e.index()] = Arc::new(result);
        }
    }

    Ok(Cascaded {
        specified,
        pseudo,
        rules: all.into_iter().map(|a| a.rule).collect(),
        font_face,
    })
}

/// A rule with what applying it needs.
struct Applied {
    rule: Rule,
    /// css_parser's specificity of the selector, by which Calibre sorts.
    specificity: (u32, u32, u32),
    /// The selector and the pseudo-class it is stored under; None when Calibre's `Select`
    /// refuses it.
    compiled: Option<(Selector, Option<&'static str>)>,
    /// The units of its declarations ([`decl_units`]).
    units: usize,
}

impl Applied {
    fn new(rule: Rule) -> Applied {
        let (specificity, compiled) = match parse_selector(&rule.selector) {
            Parsed::Ok {
                selector,
                pseudo,
                specificity,
            } => (specificity, Some((selector, pseudo))),
            // css_parser sorts it all the same.
            Parsed::Dropped => (
                dropped_specificity(&rule.selector).unwrap_or((0, 0, 0)),
                None,
            ),
        };
        let units = rule.decls.iter().map(|(k, d)| decl_units(k, d)).sum();
        Applied {
            rule,
            specificity,
            compiled,
            units,
        }
    }
}

/// The units a declaration costs to store: one, and one more for each 64 bytes of it.
fn decl_units(name: &str, d: &Decl) -> usize {
    1 + (name.len() + d.value.len()) / TEXT_UNIT
}

/// The units charged so far against [`MAX_CASCADE_DECLS`].
struct Units(usize);

impl Units {
    fn charge(&mut self, units: usize) -> Result<(), Error> {
        self.0 = self.0.saturating_add(units);
        if self.0 > MAX_CASCADE_DECLS {
            return Err(Error::Cascade(MAX_CASCADE_DECLS));
        }
        Ok(())
    }
}

/// The sets of rules reached: state `i` is state `parent[i]` and rule `rule[i]` (an index
/// into the sorted rules).
struct States {
    parent: Vec<u32>,
    rule: Vec<u32>,
}

impl States {
    fn push(&mut self, parent: u32, rule: u32) -> u32 {
        let id = u32::try_from(self.parent.len()).unwrap_or(u32::MAX);
        self.parent.push(parent);
        self.rule.push(rule);
        id
    }
}

/// The declarations of states, built from the nearest state already built.
struct Built<'a> {
    memo: HashMap<u32, Arc<Decls>>,
    states: &'a States,
    all: &'a [Applied],
}

impl Built<'_> {
    fn build(&mut self, s: u32, units: &mut Units) -> Result<(), Error> {
        if self.memo.contains_key(&s) {
            return Ok(());
        }
        // The rules from the nearest built state, last first. Each adds at least one
        // declaration, so the walk costs no more than the units charged below.
        let mut rules = Vec::new();
        let mut at = s;
        while !self.memo.contains_key(&at) {
            rules.push(self.states.rule[at as usize]);
            at = self.states.parent[at as usize];
        }
        let base = Arc::clone(&self.memo[&at]);
        let mut decls = Decls::clone(&base);
        units.charge(decls.iter().map(|(k, d)| decl_units(k, d)).sum())?;
        let mut pos = positions(&base);
        let all = self.all;
        for &r in rules.iter().rev() {
            let a = &all[r as usize];
            units.charge(a.units)?;
            update(&mut decls, &mut pos, &a.rule.decls);
        }
        self.memo.insert(s, Arc::new(decls));
        Ok(())
    }
}

/// Each name of `decls` → its position.
fn positions(decls: &Decls) -> HashMap<&str, usize> {
    decls
        .iter()
        .enumerate()
        .map(|(i, (k, _))| (k.as_str(), i))
        .collect()
}

/// `base` updated by `add` ([`update`]), charged for the copy and the additions.
fn updated(base: &Decls, add: &Decls, units: &mut Units) -> Result<Decls, Error> {
    units.charge(
        base.iter()
            .chain(add.iter())
            .map(|(k, d)| decl_units(k, d))
            .sum(),
    )?;
    let mut decls = base.clone();
    let mut pos = positions(base);
    update(&mut decls, &mut pos, add);
    Ok(decls)
}

/// `stylizer.py` `Style._update_style`: each of `add` replaces the value of its name unless
/// that value is `!important` and it is not; an `!important` one marks its name important for
/// good. A new name goes last. `pos` is each name's position in `decls`, its names borrowed
/// from where they were first read.
fn update<'n>(decls: &mut Decls, pos: &mut HashMap<&'n str, usize>, add: &'n Decls) {
    for (name, d) in add {
        match pos.get(name.as_str()) {
            Some(&i) => {
                let cur = &mut decls[i].1;
                if d.important {
                    cur.clone_from(d);
                } else if !cur.important {
                    cur.value.clone_from(&d.value);
                }
            }
            None => {
                pos.insert(name, decls.len());
                decls.push((name.clone(), d.clone()));
            }
        }
    }
}

/// `dict.update` for a pseudo-class's declarations: each name's value replaced, a new name
/// last, nothing `!important`.
fn plain_update(decls: &mut Decls, add: &Decls) {
    let mut new: HashMap<&str, usize> = add
        .iter()
        .enumerate()
        .map(|(i, (k, _))| (k.as_str(), i))
        .collect();
    for (k, d) in decls.iter_mut() {
        if let Some(i) = new.remove(k.as_str()) {
            d.value.clone_from(&add[i].1.value);
        }
    }
    for (k, d) in add {
        if new.contains_key(k.as_str()) {
            decls.push((
                k.clone(),
                Decl {
                    value: d.value.clone(),
                    important: false,
                },
            ));
        }
    }
}
