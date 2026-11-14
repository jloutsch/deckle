// SPDX-License-Identifier: GPL-3.0-only
//! The flattener: each element's first-pass styles become one class, as Calibre 9.18's
//! `CSSFlattener` makes them for kfxgen's conversion, and the styles kfxgen's own `Stylizer` then
//! reads from those classes (phase 2 plan, Task 9). Ported from `ebooks/oeb/transforms/flatcss.py`
//! (`CSSFlattener.stylize_spine`, `flatten_node`, `flatten_spine`, `flatten_head`; GPL v3,
//! `NOTICE`), for kfxgen's settings: no font rescaling, `minimum_line_height` 0, no
//! `line_height`, side margins of 5pt, no extra CSS, no CSS filter, no paragraph-spacing
//! changes, justification as the book has it.
//!
//! Calibre writes the classes as a stylesheet, `stylesheet.css`, which kfxgen's `Stylizer` reads
//! back with `html.css`. The port writes no CSS: it keeps each class's declarations as that
//! stylesheet is read back (each value as css_parser writes it, the whole normalized), and runs
//! the second cascade on them directly ([`super::cascade`]'s machinery, charged on from the
//! first cascade's use of the limits).
//!
//! Hostile input: elements alike share the work. An element whose first-pass styles no
//! attribute changes is named by its shared declarations, looked up once; one an attribute
//! changes is looked up by those declarations and the change, and a new combination is
//! charged against [`deckle_limits::MAX_CASCADE_DECLS`] for the copy it makes. A class made
//! from first-pass declarations is not charged again: each element's first-pass declarations
//! are let go as it is flattened. The classes a file names are bounded by
//! [`MAX_STYLE_RULES`]. The book holds every file's styles and classes until it is done: the
//! units of its files together are bounded by [`MAX_BOOK_CASCADE_DECLS`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cssparser::{Parser, ParserInput};
use deckle_limits::{MAX_BOOK_CASCADE_DECLS, MAX_STYLE_RULES};

use super::cascade::{Spent, Units, decl_units, run};
use super::style::{py_float, unit_number};
use super::value::{declarations, write_value};
use super::{Cascaded, Decls, Error, Origin, Profile, Rule, normalize};
use crate::dom::{Dom, NodeId, SVG_NS, XHTML_NS, py_space};

/// No class.
pub(crate) const NO_CLASS: u32 = u32::MAX;

/// `stylize_spine`: the side margins kfxgen's conversion asks for (`margin_left`,
/// `margin_right`), in points, as `{:g}` writes them.
const SIDE_MARGIN: &str = "5";

/// `flatcss.py` `CSSFlattener.stylize_spine`, run on a file before its first cascade: the
/// `style` attribute of `<html>` moves to `<body>`, before `<body>`'s own, and `<body>`'s gets
/// kfxgen's page margins, `margin-top: 0pt; margin-bottom: 0pt; margin-left : 5pt;
/// margin-right : 5pt; padding-left: 0pt; padding-right: 0pt`. A file with no `<body>` under
/// its root is left alone.
pub fn prepare_body(dom: &mut Dom) {
    let html = dom.root();
    let Some(body) = dom
        .children(html)
        .iter()
        .copied()
        .find(|&c| dom.tag(c) == "body" && dom.namespace(c) == Some(XHTML_NS))
    else {
        return;
    };
    if let Some(h) = dom.attr(html, "style").map(str::to_owned) {
        let joined = format!("{h};{}", dom.attr(body, "style").unwrap_or(""));
        dom.set_attr(body, "style", Some(&joined));
        dom.set_attr(html, "style", None);
    }
    let own = dom.attr(body, "style").unwrap_or("");
    let left = format!("margin-left : {SIDE_MARGIN}pt");
    let right = format!("margin-right : {SIDE_MARGIN}pt");
    let parts: Vec<&str> = own
        .split(';')
        .chain([
            "margin-top: 0pt",
            "margin-bottom: 0pt",
            &left,
            &right,
            "padding-left: 0pt",
            "padding-right: 0pt",
        ])
        .collect();
    let style = parts.join("; ");
    dom.set_attr(body, "style", Some(&style));
}

/// The classes of a book, named as `flatten_spine` names them: shared by all its spine files,
/// which are flattened in spine order.
#[derive(Debug, Default)]
pub struct ClassNamer {
    /// How many classes each base name has (`names`).
    counts: HashMap<String, u32>,
    /// An element's declarations as Calibre writes them for the class → the class (`styles`).
    styles: HashMap<String, u32>,
    /// (pseudo-class, declarations as written) → the class (`pseudo_styles`).
    pseudo_styles: HashMap<(&'static str, String), u32>,
    /// The pseudo-classes in the order the book first uses them.
    psels: Vec<&'static str>,
    pub(crate) classes: Vec<Class>,
    /// A name → its classes (two only where Calibre's numbering repeats a name).
    by_name: HashMap<String, Vec<u32>>,
    /// The units the book's files flattened so far used, both cascades of each
    /// ([`MAX_BOOK_CASCADE_DECLS`]).
    units: usize,
}

/// One rule of the stylesheet Calibre writes.
#[derive(Debug)]
pub(crate) struct Class {
    pub(crate) name: String,
    /// The pseudo-class it is written under (`.pcalibre:hover`); None for an element's own.
    pub(crate) pseudo: Option<&'static str>,
    /// Its declarations as kfxgen's `Stylizer` reads them back. `RemoveFakeMargins` edits them.
    pub(crate) decls: Arc<Decls>,
}

impl ClassNamer {
    /// The units of [`deckle_limits::MAX_CASCADE_DECLS`] the book's files flattened so far used
    /// together, counted against [`MAX_BOOK_CASCADE_DECLS`].
    pub fn units(&self) -> usize {
        self.units
    }

    /// Counts against [`MAX_BOOK_CASCADE_DECLS`] a file's first-pass `rules` and `font_face`
    /// rules, which the book keeps: one unit a rule and [`decl_units`] for each declaration,
    /// the declarations the selectors of a list share counted once. [`Error::BookCascade`] past
    /// the limit.
    pub(crate) fn charge_kept(&mut self, rules: &[Rule], font_face: &[Decls]) -> Result<(), Error> {
        let mut seen: HashSet<*const Decls> = HashSet::new();
        let shared = rules
            .iter()
            .filter(|r| seen.insert(Arc::as_ptr(&r.decls)))
            .map(|r| r.decls.as_slice());
        let units: usize = shared
            .chain(font_face.iter().map(Vec::as_slice))
            .flat_map(|d| d.iter().map(|(k, v)| decl_units(k, v)))
            .sum::<usize>()
            + rules.len()
            + font_face.len();
        self.units = self.units.saturating_add(units);
        if self.units > MAX_BOOK_CASCADE_DECLS {
            return Err(Error::BookCascade(MAX_BOOK_CASCADE_DECLS));
        }
        Ok(())
    }

    /// `klass + str(names[klass] or '')`, counted.
    fn next_name(&mut self, klass: &str) -> String {
        let n = self.counts.entry(klass.to_owned()).or_insert(0);
        let name = if *n == 0 {
            klass.to_owned()
        } else {
            format!("{klass}{n}")
        };
        *n += 1;
        name
    }

    fn add(&mut self, name: String, pseudo: Option<&'static str>, decls: Decls) -> u32 {
        let id = u32::try_from(self.classes.len()).unwrap_or(NO_CLASS);
        self.by_name.entry(name.clone()).or_default().push(id);
        self.classes.push(Class {
            name,
            pseudo,
            decls: Arc::new(decls),
        });
        id
    }

    /// The element classes named `name`, in the order they were made, which is their order in
    /// `stylesheet.css` (a stable sort by name).
    pub(crate) fn own_classes(&self, name: &str) -> impl Iterator<Item = u32> + '_ {
        self.by_name
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&id| self.classes[id as usize].pseudo.is_none())
    }

    /// The element's own class named `name`, as `RemoveFakeMargins` finds it by its selector
    /// text (a dictionary, where a later rule of the same name wins): the last one made.
    pub(crate) fn own_class(&self, name: &str) -> Option<u32> {
        self.by_name
            .get(name)?
            .iter()
            .rev()
            .copied()
            .find(|&id| self.classes[id as usize].pseudo.is_none())
    }

    /// Where class `id` stands in the stylesheet Calibre writes (`flatten_spine`): the
    /// elements' classes first, by name with numbers compared as numbers (`numeric_sort_key`,
    /// here for the ASCII names the flattener makes), then the pseudo-classes', `hover` after
    /// the others and `active` last, each group by `name:pseudo`.
    fn sheet_order(&self, id: u32) -> (u8, usize, Vec<NameRun<'_>>, String) {
        let c = &self.classes[id as usize];
        match c.pseudo {
            None => (0, 0, name_runs(&c.name), String::new()),
            Some(p) => {
                let rank = match p {
                    "hover" => 2,
                    "active" => 3,
                    _ => 1,
                };
                let first = self.psels.iter().position(|&q| q == p).unwrap_or(0);
                (rank, first, Vec::new(), format!("{}:{p}", c.name))
            }
        }
    }
}

/// A run of a class name, for its place in the stylesheet.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NameRun<'a> {
    /// Digits, by value: (length without leading zeros, the digits).
    Number(usize, &'a str),
    Text(&'a str),
}

fn name_runs(name: &str) -> Vec<NameRun<'_>> {
    let mut out = Vec::new();
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let digit = b[i].is_ascii_digit();
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() == digit {
            i += 1;
        }
        let run = &name[start..i];
        out.push(if digit {
            let v = run.trim_start_matches('0');
            NameRun::Number(v.len(), v)
        } else {
            NameRun::Text(run)
        });
    }
    out
}

/// A spine file after the flattener: the styles kfxgen's `Stylizer` reports for its elements.
#[derive(Debug, Clone)]
pub struct Flat {
    /// Each element's declarations (`Style.cssdict()`), by [`NodeId`] index
    /// ([`Self::specified`]): its class's over `html.css`'s, a `style` attribute the flattener
    /// left (outside `<body>`, under an element of another namespace) over both.
    pub specified: Vec<Arc<Decls>>,
    /// Each element's pseudo-class declarations (`Style._pseudo_classes`).
    pub pseudo: Vec<Vec<(&'static str, Decls)>>,
    /// Each element's `class` attribute after flattening, the names in byte order (Calibre
    /// writes them in a set's order, which varies from run to run).
    pub class: Vec<Option<Arc<str>>>,
    /// Each element's declarations from `html.css` alone in the second cascade.
    pub(crate) ua: Vec<Arc<Decls>>,
    /// The class the flattener gave each element ([`ClassNamer`]'s), or [`NO_CLASS`].
    pub(crate) main: Vec<u32>,
}

impl Flat {
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

    /// The `class` attribute of element `n`.
    pub fn class(&self, n: NodeId) -> Option<&str> {
        self.class[n.index()].as_deref()
    }
}

/// The properties the flattener reads from an element's first-pass declarations.
const PROBED: [&str; 8] = [
    "margin-left",
    "margin-right",
    "vertical-align",
    "font-weight",
    "display",
    "float",
    "font-size",
    "line-height",
];

/// Where [`PROBED`] are in one set of declarations, found once per set.
struct Probe([Option<u32>; 8]);

impl Probe {
    fn of(decls: &Decls) -> Probe {
        let mut at = [None; 8];
        for (i, (k, _)) in decls.iter().enumerate() {
            if let Some(p) = PROBED.iter().position(|p| p == k) {
                at[p] = u32::try_from(i).ok();
            }
        }
        Probe(at)
    }
}

/// What the flattener sets in an element's declarations: (property, value as Calibre sets
/// it), in order.
type Mods = Vec<(&'static str, String)>;

/// An element's declarations as the flattener changes them (`cssdict`): the first pass's,
/// and what has been set since.
struct View<'a> {
    base: &'a Decls,
    probe: &'a Probe,
    mods: &'a Mods,
}

impl<'a> View<'a> {
    fn new(base: &'a Decls, probe: &'a Probe, mods: &'a Mods) -> View<'a> {
        View { base, probe, mods }
    }

    /// The value of `name`, one of [`PROBED`]: the last set, else the first pass's.
    fn get(&self, name: &str) -> Option<&'a str> {
        if let Some((_, v)) = self.mods.iter().rev().find(|(k, _)| *k == name) {
            return Some(v);
        }
        let p = PROBED.iter().position(|p| *p == name)?;
        let i = self.probe.0[p]? as usize;
        Some(&self.base[i].1.value)
    }
}

/// `flatcss.py` `CSSFlattener.flatten_spine` for one spine file, after its first cascade
/// (`first`, from [`super::cascade`] run after [`prepare_body`]), and kfxgen's `Stylizer` on
/// the result. Files are flattened in spine order with one `namer`; run
/// [`super::remove_fake_margins`] once all are.
///
/// `<html>` and then `<body>` and the elements under it in document order are flattened, an
/// element in neither the XHTML nor the SVG namespace skipped with all under it
/// (`flatten_node`): attributes become declarations (`align`, `valign`, `<font>`'s `size` and
/// `face`, `color`, `bgcolor`), `<font>` becomes `<div>` or `<span>`, `ol@type` goes,
/// `font-weight: medium`, `display: in-line` and `vertical-align: sup` are corrected, and the
/// element's declarations become its class, its `style` attribute removed. Then every
/// `<style>` and stylesheet `<link>` leaves the tree (`flatten_head`). Then the elements are
/// cascaded again over `html.css` with the classes as the book's rules, charged on from the
/// first cascade's use of the limits: [`Error::Cascade`] or [`Error::Selectors`] when the two
/// together pass them, [`Error::TooLarge`] when the file has more classes than
/// [`MAX_STYLE_RULES`], [`Error::BookCascade`] when the units of the book's files so far pass
/// [`MAX_BOOK_CASCADE_DECLS`] together.
pub fn flatten(
    dom: &mut Dom,
    href: &str,
    first: Cascaded,
    namer: &mut ClassNamer,
    profile: &Profile,
) -> Result<Flat, Error> {
    let Cascaded {
        specified: mut first_specified,
        pseudo: mut first_pseudo,
        spent,
        ..
    } = first;
    let mut units = Units(spent.units);
    let n = dom.node_count();
    let order = flatten_order(dom);
    let blocks = if order
        .iter()
        .any(|&e| dom.tag(e) == "font" && dom.namespace(e) == Some(XHTML_NS))
    {
        block_below(dom)
    } else {
        Vec::new()
    };

    // Keyed by the address of first-pass declarations. Each element's are let go once it is
    // flattened, and an address is reused only by declarations the flattener makes, which
    // are never keys.
    let mut probes: HashMap<usize, Probe> = HashMap::new();
    // Declarations no attribute changed → their class; with the changes → theirs.
    let mut plain: HashMap<usize, Option<u32>> = HashMap::new();
    let mut changed: HashMap<(usize, Mods), Option<u32>> = HashMap::new();
    // Child tables an `align="center"` parent centres (`ts.set`).
    let mut centred: HashSet<NodeId> = HashSet::new();
    // Whether an element's line height is a negative number of points (`Style.lineHeight`).
    let mut negative_lh = vec![false; n];
    // Whether an element's first-pass styles float it left (read by its children's
    // `is_drop_cap` once its own declarations are gone).
    let mut floats_left = vec![false; n];
    let mut main = vec![NO_CLASS; n];
    let none: Arc<Decls> = Arc::new(Decls::new());

    for e in order {
        let base = std::mem::replace(&mut first_specified[e.index()], Arc::clone(&none));
        let ptr = Arc::as_ptr(&base) as usize;
        probes.entry(ptr).or_insert_with(|| Probe::of(&base));
        let mut mods: Mods = Vec::new();
        if centred.contains(&e) {
            mods.push(("margin-left", "auto".to_owned()));
            mods.push(("margin-right", "auto".to_owned()));
        }
        let mut tag = dom.tag(e).to_owned();

        if let Some(align) = dom.attr(e, "align").map(str::to_owned) {
            if tag != "img" {
                let center = align == "center";
                mods.push(("text-align", align));
                if center && tag == "table" {
                    let v = View::new(&base, &probes[&ptr], &mods);
                    if v.get("margin-left").is_none() && v.get("margin-right").is_none() {
                        mods.push(("margin-left", "auto".to_owned()));
                        mods.push(("margin-right", "auto".to_owned()));
                    }
                } else if center {
                    // `node.iterchildren(XHTML('table'))`: their first-pass styles.
                    for &c in dom.children(e) {
                        if dom.tag(c) != "table" || dom.namespace(c) != Some(XHTML_NS) {
                            continue;
                        }
                        let cb = &first_specified[c.index()];
                        let cp = Arc::as_ptr(cb) as usize;
                        let p = probes.entry(cp).or_insert_with(|| Probe::of(cb));
                        if p.0[0].is_none() && p.0[1].is_none() {
                            centred.insert(c);
                        }
                    }
                }
            } else if matches!(align.as_str(), "middle" | "bottom" | "top") {
                mods.push(("vertical-align", align));
            } else if matches!(align.as_str(), "left" | "right") {
                mods.push(("float", align));
            }
            dom.set_attr(e, "align", None);
        }
        let probe = &probes[&ptr];
        floats_left[e.index()] = probe.0[5].is_some_and(|i| base[i as usize].1.value == "left");
        if tag == "td"
            && let Some(valign) = dom.attr(e, "valign").map(str::to_owned)
        {
            if View::new(&base, probe, &mods).get("vertical-align") == Some("inherit") {
                mods.push(("vertical-align", valign));
            }
            dom.set_attr(e, "valign", None);
        }
        if tag == "font" && dom.namespace(e) == Some(XHTML_NS) {
            tag = if blocks[e.index()] { "div" } else { "span" }.to_owned();
            dom.rename(e, &tag);
            if let Some(size) = dom.attr(e, "size").map(str::to_owned) {
                let size = size.trim_matches(py_space);
                if !size.is_empty() {
                    mods.push(("font-size", format!("{:.1}pt", font_size(size, profile))));
                }
                dom.set_attr(e, "size", None);
            }
            if let Some(face) = dom.attr(e, "face").map(str::to_owned) {
                mods.push(("font-family", face));
                dom.set_attr(e, "face", None);
            }
        }
        for (attr, prop) in [("color", "color"), ("bgcolor", "background-color")] {
            if let Some(v) = dom.attr(e, attr).map(str::to_owned) {
                // `Property(prop, value).value`; css_parser raises for what it cannot read.
                if let Some(v) = write_value(&v, None).filter(|v| !v.is_empty()) {
                    mods.push((prop, v));
                }
                dom.set_attr(e, attr, None);
            }
        }
        if tag == "ol" && dom.attr(e, "type").is_some() {
            dom.set_attr(e, "type", None);
        }
        // `.lower() == 'medium'`: only ASCII letters lower to those of `medium`.
        if View::new(&base, probe, &mods)
            .get("font-weight")
            .is_some_and(|w| w.eq_ignore_ascii_case("medium"))
        {
            // ADE chokes on font-weight medium.
            mods.push(("font-weight", "normal".to_owned()));
        }
        // `minimum_line_height` is 0: only a negative line height is raised, to `0.0`.
        negative_lh[e.index()] = match View::new(&base, probe, &mods).get("line-height") {
            Some(v) => is_negative_length(v),
            None => dom.parent(e).is_some_and(|p| negative_lh[p.index()]),
        };
        if negative_lh[e.index()]
            && !is_drop_cap(dom, e, &View::new(&base, probe, &mods), &floats_left)
        {
            mods.push(("line-height", "0.0".to_owned()));
        }
        let inline = View::new(&base, probe, &mods).get("display") == Some("in-line");
        let sup = View::new(&base, probe, &mods).get("vertical-align") == Some("sup");
        if inline {
            mods.push(("display", "inline".to_owned()));
        }
        if sup {
            mods.push(("vertical-align", "super".to_owned()));
        }

        // The element's own class.
        let own = if mods.is_empty() {
            match plain.get(&ptr) {
                Some(&c) => c,
                None => {
                    let c = if base.is_empty() {
                        None
                    } else {
                        Some(element_class(dom, e, &base, &[], namer, profile))
                    };
                    plain.insert(ptr, c);
                    c
                }
            }
        } else {
            let key = (ptr, mods);
            match changed.get(&key) {
                Some(&c) => c,
                None => {
                    // The copy of the first-pass declarations the changes make.
                    units.charge(
                        base.iter()
                            .map(|(k, d)| decl_units(k, d))
                            .chain(key.1.iter().map(|(k, v)| 1 + (k.len() + v.len()) / 64))
                            .sum(),
                    )?;
                    let c = Some(element_class(dom, e, &base, &key.1, namer, profile));
                    changed.insert(key, c);
                    c
                }
            }
        };
        drop(base);
        let mut pseudo_ids = Vec::new();
        for (psel, decls) in std::mem::take(&mut first_pseudo[e.index()]) {
            pseudo_ids.push(pseudo_class(psel, &decls, namer, profile));
        }
        let mut names: Vec<&str> = Vec::new();
        if let Some(c) = own {
            main[e.index()] = c;
            names.push(&namer.classes[c as usize].name);
        }
        for &c in &pseudo_ids {
            names.push(&namer.classes[c as usize].name);
        }
        if names.is_empty() {
            dom.set_attr(e, "class", None);
        } else {
            names.sort_unstable();
            names.dedup();
            let class = names.join(" ");
            // Each element holds its own copy of the names: a unit for each 64 bytes.
            units.charge(class.len() / 64)?;
            dom.set_attr(e, "class", Some(&class));
        }
        dom.set_attr(e, "style", None);
    }
    drop(first_specified);
    drop(first_pseudo);

    drop_stylesheets(dom);

    // kfxgen's Stylizer: `html.css`, then `stylesheet.css`'s rules for the classes this file
    // names.
    let mut ids: Vec<u32> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut token = String::new();
    for e in dom.descendants() {
        let Some(classes) = dom.attr(e, "class") else {
            continue;
        };
        for t in classes.split(py_space).filter(|t| !t.is_empty()) {
            // Class selectors match in ASCII lower case (`DocIndex`).
            token.clear();
            token.push_str(t);
            token.make_ascii_lowercase();
            if let Some(found) = namer.by_name.get(token.as_str()) {
                for &id in found {
                    if seen.insert(id) {
                        ids.push(id);
                    }
                }
            }
        }
    }
    // `stylesheet.css` is one more of the file's stylesheets: its rules for the file count
    // against the same limit as the book's own.
    if ids.len() > MAX_STYLE_RULES {
        return Err(Error::TooLarge(MAX_STYLE_RULES));
    }
    ids.sort_by_cached_key(|&id| (namer.sheet_order(id), id));
    let rules: Vec<Rule> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let c = &namer.classes[id as usize];
            let selector = match c.pseudo {
                None => format!(".{}", c.name),
                Some(p) => format!(".{}:{p}", c.name),
            };
            Rule {
                origin: Origin::Book,
                selector,
                decls: Arc::clone(&c.decls),
                index: u32::try_from(i).unwrap_or(u32::MAX),
            }
        })
        .collect();
    let spent = Spent {
        units: units.0,
        work: spent.work,
    };
    let (second, ua) = run(dom, href, rules, Vec::new(), profile, spent, true)?;
    // The book holds every file's styles until it is done: what this file's cascades stored
    // counts against the book's limit too.
    namer.units = namer.units.saturating_add(second.spent.units);
    if namer.units > MAX_BOOK_CASCADE_DECLS {
        return Err(Error::BookCascade(MAX_BOOK_CASCADE_DECLS));
    }
    // Elements of one class attribute share its text.
    let mut texts: HashMap<&str, Arc<str>> = HashMap::new();
    let mut class = vec![None; n];
    for e in dom.descendants() {
        class[e.index()] = dom
            .attr(e, "class")
            .map(|c| Arc::clone(texts.entry(c).or_insert_with(|| Arc::from(c))));
    }
    Ok(Flat {
        specified: second.specified,
        pseudo: second.pseudo,
        class,
        ua,
        main,
    })
}

/// The elements `flatten_spine` flattens, in its order: `<html>` (`recurse=False`), then
/// `<body>` and what is under it, in document order, but for elements in neither the XHTML
/// nor the SVG namespace and all under them.
fn flatten_order(dom: &Dom) -> Vec<NodeId> {
    let flattened = |e: NodeId| matches!(dom.namespace(e), Some(XHTML_NS | SVG_NS));
    let html = dom.root();
    let mut out = Vec::new();
    if flattened(html) {
        out.push(html);
    }
    let Some(&body) = dom
        .children(html)
        .iter()
        .find(|&&c| dom.tag(c) == "body" && dom.namespace(c) == Some(XHTML_NS))
    else {
        return out;
    };
    let mut stack = vec![body];
    while let Some(e) = stack.pop() {
        if !flattened(e) {
            continue;
        }
        out.push(e);
        stack.extend(dom.children(e).iter().rev());
    }
    out
}

/// For each element, by [`NodeId`] index: whether an XHTML block (`p`, `div`, `table`, `h1` to
/// `h6`, `ol`, `ul`, `dl`, `blockquote`) is under it, as the tree is before the flattener
/// renames anything (`<font>`s are renamed parents first). One pass, children before parents.
fn block_below(dom: &Dom) -> Vec<bool> {
    const BLOCKS: [&str; 13] = [
        "p",
        "div",
        "table",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "ol",
        "ul",
        "dl",
        "blockquote",
    ];
    let mut below = vec![false; dom.node_count()];
    let all: Vec<NodeId> = dom.descendants().collect();
    for &e in all.iter().rev() {
        let is_block = dom.namespace(e) == Some(XHTML_NS) && BLOCKS.contains(&dom.tag(e));
        if (is_block || below[e.index()])
            && let Some(p) = dom.parent(e)
        {
            below[p.index()] = true;
        }
    }
    below
}

/// `<font size>` in points (`flatten_node`): `+n` and `-n` from size 3, kept within 1 to 7;
/// otherwise the first run of digits and signs read as a number, size 3's where it is not one
/// of 1 to 7 or does not read.
fn font_size(size: &str, profile: &Profile) -> f64 {
    let fnum = |i: i64| -> Option<f64> {
        let i = usize::try_from(i).ok()?.checked_sub(1)?;
        profile.fnums.get(i).copied()
    };
    // `force_int`: `int(re.search(r'([0-9+-]+)', raw).group(1))`.
    let force_int = |raw: &str| -> Option<i64> {
        let start = raw.find(|c: char| c.is_ascii_digit() || c == '+' || c == '-')?;
        let rest = &raw[start..];
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '+' || c == '-'))
            .unwrap_or(rest.len());
        py_int(&rest[..end])
    };
    let three = profile.fnums[2];
    if size.starts_with(['+', '-']) {
        let esize = force_int(size)
            .map_or(3, |i| i.saturating_add(3))
            .clamp(1, 7);
        fnum(esize).unwrap_or(three)
    } else {
        force_int(size).and_then(fnum).unwrap_or(three)
    }
}

/// Python's `int()` of a run of digits and signs: one sign, then digits.
fn py_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Past i64 it is no size anyway: size 3's, or clamped.
    match s.parse::<i64>() {
        Ok(v) => Some(v),
        Err(_) => Some(if s.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }),
    }
}

/// Whether `Style.lineHeight` makes a line height of `value` a negative number of points,
/// taking the font size as positive and finite (DIVERGENCES.md, "A negative line height"):
/// a number (`float(value)`) or a length `unit_convert` reads (`UNIT_RE`) below zero.
fn is_negative_length(value: &str) -> bool {
    if value == "normal" {
        return false;
    }
    match py_float(value) {
        Some(v) => v < 0.0,
        None => unit_number(value).is_some_and(|(v, _)| v < 0.0),
    }
}

/// `is_drop_cap` in `flatten_node`: a left float with a font size around one character, or
/// the drop cap DOCX input makes (a `<p>` of one character with a line height and a font
/// size, alone in a left-floated `<div>`).
fn is_drop_cap(dom: &Dom, e: NodeId, v: &View<'_>, floats_left: &[bool]) -> bool {
    let text = dom.text(e);
    let childless = dom.children(e).is_empty();
    let has = |name: &str| v.get(name).is_some();
    if v.get("float") == Some("left") && has("font-size") && childless {
        let mut chars = text.chars();
        let one = chars.next();
        let two = chars.next();
        let three = chars.next();
        match (one, two, three) {
            (Some(_), None, _) => return true,
            (Some(c), Some(_), None) if ('\u{2000}'..='\u{206F}').contains(&c) => return true,
            _ => {}
        }
    }
    if dom.tag(e) == "p"
        && childless
        && text.trim_matches(py_space).chars().count() == 1
        && dom.tail(e).is_empty()
        && has("line-height")
        && has("font-size")
        && let Some(dp) = dom.parent(e)
        && dom.tag(dp) == "div"
        && dom.children(dp).len() == 1
        && dom.text(dp).is_empty()
    {
        // `stylizer.style(dp).cssdict()`: the parent's first-pass styles.
        return floats_left[dp.index()];
    }
    false
}

/// Calibre's `ascii_text`: the text in ASCII (`Unihandecoder`; DIVERGENCES.md, "Class names
/// from letters outside ASCII").
fn ascii_text(s: &str) -> String {
    if s.is_ascii() {
        s.to_owned()
    } else {
        deunicode::deunicode(s)
    }
}

/// The name an element's class is numbered from: its first class, its trailing digits and
/// hyphens stripped (`STRIPNUM`), in ASCII, lower case, spaces as `_`; `calibre` for an
/// element with no class.
fn base_name(class: Option<&str>) -> String {
    let classes = class
        .map(|c| c.trim_matches(py_space))
        .filter(|c| !c.is_empty())
        .unwrap_or("calibre");
    let first = classes
        .split(py_space)
        .find(|t| !t.is_empty())
        .unwrap_or("");
    let stripped = first.trim_end_matches(|c: char| c == '-' || c.is_ascii_digit());
    ascii_text(stripped)
        .to_lowercase()
        .trim_matches(py_space)
        .replace(' ', "_")
}

/// `';\n'.join(f'{key}: {val}' for key, val in sorted(cssdict.items()))`.
fn css_text(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push_str(";\n");
        }
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
    }
    out
}

/// The class of an element whose declarations are `base` with `mods` set: found by the text
/// Calibre writes for them, else made, named from the element's own class.
fn element_class(
    dom: &Dom,
    e: NodeId,
    base: &Decls,
    mods: &[(&'static str, String)],
    namer: &mut ClassNamer,
    profile: &Profile,
) -> u32 {
    // The dictionary: the first pass's, each change replacing a value or added last.
    let mut pairs: Vec<(&str, &str)> = base
        .iter()
        .map(|(k, d)| (k.as_str(), d.value.as_str()))
        .collect();
    for (k, v) in mods {
        match pairs.iter_mut().find(|(pk, _)| pk == k) {
            Some(p) => p.1 = v,
            None => pairs.push((k, v)),
        }
    }
    pairs.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let text = css_text(&pairs);
    if let Some(&id) = namer.styles.get(&text) {
        return id;
    }
    let decls = read_back(&text, profile);
    let name = namer.next_name(&base_name(dom.attr(e, "class")));
    let id = namer.add(name, None, decls);
    namer.styles.insert(text, id);
    id
}

/// A class's declarations as css_parser reads `stylesheet.css` back and the `Stylizer`
/// flattens them: `text`, the block Calibre writes for the class, read as a declaration block
/// (each value as css_parser writes values, `1.0rem` as `1rem` and `22.0pt` as `22pt`; one
/// it cannot read dropped; a value an attribute gave ending in `!important` read as important),
/// then normalized. A copy of declarations the first cascade charged for, which it no longer
/// holds once the file is flattened.
fn read_back(text: &str, profile: &Profile) -> Decls {
    let mut input = ParserInput::new(text);
    let mut p = Parser::new(&mut input);
    normalize(declarations(&mut p, None, None), profile)
}

/// The class of a pseudo-class's declarations: found by the text Calibre writes for them,
/// else made, named `pcalibre`, `pcalibre1`, …
fn pseudo_class(
    psel: &'static str,
    decls: &Decls,
    namer: &mut ClassNamer,
    profile: &Profile,
) -> u32 {
    let mut pairs: Vec<(&str, &str)> = decls
        .iter()
        .map(|(k, d)| (k.as_str(), d.value.as_str()))
        .collect();
    pairs.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let text = css_text(&pairs);
    let key = (psel, text);
    if let Some(&id) = namer.pseudo_styles.get(&key) {
        return id;
    }
    let kept = read_back(&key.1, profile);
    if !namer.psels.contains(&psel) {
        namer.psels.push(psel);
    }
    let name = namer.next_name("pcalibre");
    let id = namer.add(name, Some(psel), kept);
    namer.pseudo_styles.insert(key, id);
    id
}

/// `flatten_head`: every `<style>` (XHTML or SVG) and stylesheet `<link>` leaves the tree,
/// with its tail, as lxml's `remove` takes it.
fn drop_stylesheets(dom: &mut Dom) {
    // `OEB_STYLES`.
    const STYLES: [&str; 4] = ["text/css", "text/x-oeb1-css", "text/x-oeb-css", "xhtml/css"];
    let gone: Vec<NodeId> = dom
        .descendants()
        .filter(|&e| match (dom.tag(e), dom.namespace(e)) {
            ("link", Some(XHTML_NS)) => {
                dom.attr(e, "rel").unwrap_or("stylesheet").to_lowercase() == "stylesheet"
                    && STYLES.contains(
                        &dom.attr(e, "type")
                            .unwrap_or("text/css")
                            .to_lowercase()
                            .as_str(),
                    )
            }
            ("style", Some(XHTML_NS | SVG_NS)) => {
                STYLES.contains(&dom.attr(e, "type").unwrap_or("text/css"))
            }
            _ => false,
        })
        .collect();
    if !gone.is_empty() {
        dom.remove_many_with_tails(&gone);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{DEFAULT_PROFILE, cascade};

    /// The book's limit is checked as each file is flattened: a book whose files so far used
    /// it all has its next file refused, however small.
    #[test]
    fn a_book_at_its_limit_refuses_its_next_file() {
        let mut dom = crate::dom::parse(
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>T</title></head>\
              <body><p style=\"color: red\">x</p></body></html>",
        )
        .unwrap();
        prepare_body(&mut dom);
        let first = cascade(&mut dom, "a.xhtml", Vec::new(), &DEFAULT_PROFILE).unwrap();
        let mut namer = ClassNamer {
            units: MAX_BOOK_CASCADE_DECLS,
            ..ClassNamer::default()
        };
        let got = flatten(&mut dom, "a.xhtml", first, &mut namer, &DEFAULT_PROFILE);
        assert!(
            matches!(got, Err(Error::BookCascade(MAX_BOOK_CASCADE_DECLS))),
            "{got:?}"
        );
    }
}
