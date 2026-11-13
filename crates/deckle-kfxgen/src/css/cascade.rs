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
    /// What this cascade used of the file's limits, which the flattener's second cascade
    /// goes on charging ([`super::flatten`]).
    pub(crate) spent: Spent,
}

/// What a book file's cascades have used so far of the limits they share: units against
/// [`MAX_CASCADE_DECLS`] and selector steps against [`deckle_limits::MAX_SELECTOR_WORK`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Spent {
    pub(crate) units: usize,
    pub(crate) work: u64,
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
/// running across them. Run [`crate::dom::mark_chapters`] and then [`super::prepare_body`]
/// first: Calibre marks chapters and sets `<body>`'s style before the Stylizer.
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
    let mut font_face = Vec::new();
    let mut rules = Vec::with_capacity(book_rules);
    for mut sheet in sheets {
        sheet.normalize(profile);
        font_face.append(&mut sheet.font_face);
        rules.append(&mut sheet.rules);
    }
    let (c, _) = run(
        dom,
        href,
        rules,
        font_face,
        profile,
        Spent::default(),
        false,
    )?;
    Ok(c)
}

/// [`cascade`] of `book`, rules already normalized, after Calibre's `html.css`, charged on
/// from `spent`. With `ua_decls`, also each element's declarations from `html.css` alone, before
/// the book's rules and its `style` attribute (the flattener's second cascade, which
/// `RemoveFakeMargins` changes afterwards).
pub(crate) fn run(
    dom: &mut Dom,
    href: &str,
    book: Vec<Rule>,
    book_font_face: Vec<Decls>,
    profile: &Profile,
    spent: Spent,
    ua_decls: bool,
) -> Result<(Cascaded, Vec<Arc<Decls>>), Error> {
    // `stylesheets = [html_css_stylesheet()]`, then the book's.
    let mut ua = parse_sheet(HTML_CSS, Origin::UserAgent, "", 0, &mut |_| None)?;
    ua.normalize(profile);
    let ua_rules = ua.rules.len();
    let mut font_face = ua.font_face;
    font_face.extend(book_font_face);
    let mut all: Vec<Applied> = Vec::with_capacity(ua_rules + book.len());
    all.extend(ua.rules.into_iter().map(Applied::new));
    all.extend(book.into_iter().map(Applied::new));
    // `self.rules.sort(key=itemgetter(0))`: (origin, 0, ids, classes, types, index); stable.
    // The user agent's rules stay first, at `0..ua_rules`.
    all.sort_by_key(|a| (a.rule.origin, a.specificity, a.rule.index));

    let mut units = Units(spent.units);
    let index = DocIndex::new(dom);
    index.spend(spent.work);
    let n = dom.node_count();
    // Which rules reach each element: a state is the set reached so far, made of its parent
    // state and one more rule. State 0 is no rule.
    let mut states = States {
        parent: vec![0],
        rule: vec![u32::MAX],
    };
    let mut state = vec![0u32; n];
    let mut pseudo: Vec<Vec<(&'static str, Decls)>> = vec![Vec::new(); n];
    // The pseudo-class rules with declarations reaching each element, applied at the end.
    let mut pseudo_rules: Vec<Vec<u32>> = vec![Vec::new(); n];
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
            // `_update_pseudo_class`: `orig.update(cssdict)`. Charged for what the update may
            // add, and one more for a new pseudo-class.
            Some(name) => {
                let rule = u32::try_from(i).unwrap_or(u32::MAX);
                for e in matched {
                    let list = &mut pseudo[e.index()];
                    if !list.iter().any(|(k, _)| k == name) {
                        units.charge(1)?;
                        list.push((name, Decls::new()));
                    }
                    if !a.rule.decls.is_empty() {
                        units.charge(a.units)?;
                        pseudo_rules[e.index()].push(rule);
                    }
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

    for (list, rules) in pseudo.iter_mut().zip(&pseudo_rules) {
        if rules.is_empty() {
            continue;
        }
        let mut pos: Vec<HashMap<&str, usize>> = vec![HashMap::new(); list.len()];
        for &r in rules {
            let a = &all[r as usize];
            let Some((_, Some(name))) = &a.compiled else {
                continue;
            };
            if let Some(at) = list.iter().position(|(k, _)| k == name) {
                plain_update(&mut list[at].1, &mut pos[at], &a.rule.decls);
            }
        }
        for (_, decls) in list.iter_mut() {
            decls.shrink_to_fit();
        }
    }
    drop(pseudo_rules);

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
    // The set of `html.css` rules each set starts with: its rules up to the first of the book.
    let mut ua_of = Vec::new();
    if ua_decls {
        let mut prefix: HashMap<u32, u32> = HashMap::new();
        for &s in &state {
            let p = match prefix.get(&s) {
                Some(&p) => p,
                None => {
                    let mut at = s;
                    while at != 0 && states.rule[at as usize] as usize >= ua_rules {
                        at = states.parent[at as usize];
                    }
                    built.build(at, &mut units)?;
                    prefix.insert(s, at);
                    at
                }
            };
            ua_of.push(Arc::clone(&built.memo[&p]));
        }
    }
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

    let spent = Spent {
        units: units.0,
        work: index.work(),
    };
    let c = Cascaded {
        specified,
        pseudo,
        rules: all.into_iter().map(|a| a.rule).collect(),
        font_face,
        spent,
    };
    Ok((c, ua_of))
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
pub(crate) fn decl_units(name: &str, d: &Decl) -> usize {
    1 + (name.len() + d.value.len()) / TEXT_UNIT
}

/// The units charged so far against [`MAX_CASCADE_DECLS`].
pub(crate) struct Units(pub(crate) usize);

impl Units {
    pub(crate) fn charge(&mut self, units: usize) -> Result<(), Error> {
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
        // Stored as long as the cascade: no spare room from growing.
        decls.shrink_to_fit();
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
    decls.shrink_to_fit();
    Ok(decls)
}

/// `base` updated by `add` ([`update`]), not charged: for sets of declarations a cascade has
/// already charged.
pub(crate) fn merged(base: &Decls, add: &Decls) -> Decls {
    let mut decls = base.clone();
    let mut pos = positions(base);
    update(&mut decls, &mut pos, add);
    decls.shrink_to_fit();
    decls
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
/// last, nothing `!important`. `pos` is each name's position in `decls`.
fn plain_update<'n>(decls: &mut Decls, pos: &mut HashMap<&'n str, usize>, add: &'n Decls) {
    for (k, d) in add {
        match pos.get(k.as_str()) {
            Some(&i) => decls[i].1.value.clone_from(&d.value),
            None => {
                pos.insert(k, decls.len());
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
}
