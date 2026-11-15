// SPDX-License-Identifier: GPL-3.0-only
//! What `Select` makes of a parsed selector (`css_selectors/select.py`): each compound as the
//! tests an element passes, the pseudo-classes it knows, and the pseudo-class name
//! `stylizer.py` stores a rule under.

use super::Fail;
use super::parse::{AttrOp, Comb, Complex, Compound, PseudoElement, Simple};
use super::tokens::{Tok, is_ws};
use crate::generate::py;

/// `INAPPROPRIATE_PSEUDO_CLASSES`: with `ignore_inappropriate_pseudo_classes`, they match every
/// element (`get_func_for_pseudo`, `allow_all`).
const INAPPROPRIATE: [&str; 16] = [
    "active",
    "after",
    "disabled",
    "visited",
    "link",
    "before",
    "focus",
    "first-letter",
    "enabled",
    "first-line",
    "hover",
    "checked",
    "target",
    "focus-within",
    "focus-visible",
    "target-within",
];

/// `default_dispatch_map`'s names: a pseudo-class or function by one of these names that is
/// not handled below is not an unknown one (`get_func_for_pseudo`, `select_function`).
const DISPATCH: [&str; 38] = [
    "combinedselector",
    "descendant",
    "child",
    "direct_adjacent",
    "indirect_adjacent",
    "element",
    "hash",
    "class",
    "negation",
    "matching",
    "relation",
    "attrib",
    "attrib_case_insensitive",
    "exists",
    "equals",
    "includes",
    "dashmatch",
    "prefixmatch",
    "suffixmatch",
    "substringmatch",
    "function",
    "lang",
    "nth_child",
    "nth_last_child",
    "nth_of_type",
    "nth_last_of_type",
    "selector",
    "pseudo",
    "root",
    "scope",
    "any_link",
    "first_child",
    "last_child",
    "only_child",
    "first_of_type",
    "last_of_type",
    "only_of_type",
    "empty",
];

/// Dispatch functions of three parameters that build a generator: called as
/// `func(cache, function, item)` by `select_function`, they return it unrun, which is true, so
/// `p:descendant(x)` matches every `p`.
const TRUE_FUNCTIONS: [&str; 11] = [
    "descendant",
    "child",
    "direct_adjacent",
    "indirect_adjacent",
    "exists",
    "equals",
    "includes",
    "dashmatch",
    "prefixmatch",
    "suffixmatch",
    "substringmatch",
];

/// What a compound asks of an element besides names and attributes ([`Test::Filter`]).
#[derive(Debug)]
pub(super) enum Filter {
    All,
    Never,
    /// `select_any_link`.
    AnyLink,
    /// `select_lang`, the language in ASCII lower case.
    Lang(String),
    /// `select_nth_*`, `select_first_child` and the like: the position among the element's
    /// siblings (or those of its type, or those in the `of` list), from the end for `last`.
    Nth {
        last: bool,
        of_type: bool,
        a: i128,
        b: i128,
        of: Option<usize>,
    },
    /// `select_only_child`, `select_only_of_type`.
    Only {
        of_type: bool,
    },
    /// In the elements a list matches (`:is()`).
    In(usize),
    /// Not in them (`:not()`).
    NotIn(usize),
    /// An anchor of a `:has()`.
    Has(usize),
}

/// What a pseudo-class or pseudo-element name stands for.
enum PseudoFn {
    Root,
    Empty,
    Filter(Filter),
}

/// `get_func_for_pseudo`.
fn pseudo_fn(name: &str) -> Result<PseudoFn, Fail> {
    let key = name.replace('-', "_");
    let nth = |last, of_type| {
        Ok(PseudoFn::Filter(Filter::Nth {
            last,
            of_type,
            a: 0,
            b: 1,
            of: None,
        }))
    };
    match key.as_str() {
        "root" | "scope" => Ok(PseudoFn::Root),
        "empty" => Ok(PseudoFn::Empty),
        "any_link" => Ok(PseudoFn::Filter(Filter::AnyLink)),
        "first_child" => nth(false, false),
        "last_child" => nth(true, false),
        "first_of_type" => nth(false, true),
        "last_of_type" => nth(true, true),
        "only_child" => Ok(PseudoFn::Filter(Filter::Only { of_type: false })),
        "only_of_type" => Ok(PseudoFn::Filter(Filter::Only { of_type: true })),
        // "The pseudo-class :%s is invalid": a dispatch function that is not a pseudo one.
        k if DISPATCH.contains(&k) => Err(Fail::Invalid),
        _ if INAPPROPRIATE.contains(&name) => Ok(PseudoFn::Filter(Filter::All)),
        _ => Err(Fail::Invalid),
    }
}

/// One test of a compound, as `select.py` makes it of a simple selector.
#[derive(Debug)]
pub(super) enum Test {
    /// `select_element`: Calibre's `map_tag_name` key, in ASCII lower case.
    Type(String),
    /// `select_hash`: the whole `id`, in ASCII lower case (`id_map`).
    Id(String),
    /// `select_class`: one of the `class` names, in ASCII lower case (`class_map`).
    Class(String),
    /// `select_attrib`.
    Attr(AttrTest),
    /// `select_root`, `select_scope`.
    Root,
    /// `select_empty`.
    Empty,
    /// `select_attrib` for a value no element's can match, worked out once.
    Never,
    /// [`Compiled::filters`]`[i]`.
    Filter(usize),
}

/// An attribute selector with its value prepared once: `attrib_map`'s and
/// `attrib_space_map`'s tests (`select_equals`, `select_includes` …, and with `i`,
/// `case_insensitive_attribute_matchers`).
#[derive(Debug)]
pub(super) struct AttrTest {
    /// In ASCII lower case.
    pub(super) name: String,
    pub(super) op: AttrMatch,
    /// In ASCII lower case when `ci`.
    pub(super) value: String,
    /// The `i` flag: values compared ASCII case-insensitively.
    pub(super) ci: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttrMatch {
    Exists,
    Equal,
    Includes,
    Dash,
    Prefix,
    Suffix,
    Substring,
}

/// The selector ready to evaluate: its compounds' tests, how they chain, and what their
/// filters stand for.
pub(super) struct Compiled {
    pub(super) text: String,
    /// Compound selectors: the tests an element passes to match.
    pub(super) compounds: Vec<Vec<Test>>,
    /// Complex selectors: compound indexes, each with the combinator before it.
    pub(super) complexes: Vec<Vec<(Comb, usize)>>,
    /// Selector lists: complex selector indexes.
    pub(super) lists: Vec<Vec<usize>>,
    /// `:has()` lists: each relative selector's compounds from right to left, with the
    /// combinator to the left of each (`relative_selector_anchors`).
    pub(super) has: Vec<Vec<Vec<(Comb, usize)>>>,
    pub(super) filters: Vec<Filter>,
    /// The list [`super::select`] matches.
    pub(super) top: usize,
}

#[derive(Default)]
pub(super) struct Builder {
    compounds: Vec<Vec<Test>>,
    complexes: Vec<Vec<(Comb, usize)>>,
    lists: Vec<Vec<usize>>,
    has: Vec<Vec<Vec<(Comb, usize)>>>,
    filters: Vec<Filter>,
}

impl Builder {
    pub(super) fn compile(
        text: &str,
        selectors: &[(Complex, Option<PseudoElement>)],
    ) -> Result<Compiled, Fail> {
        let mut b = Builder::default();
        let mut top = Vec::new();
        for (tree, pe) in selectors {
            top.push(b.complex(tree, pe.as_ref())?);
        }
        let top = b.list(top);
        Ok(Compiled {
            text: text.to_owned(),
            compounds: b.compounds,
            complexes: b.complexes,
            lists: b.lists,
            has: b.has,
            filters: b.filters,
            top,
        })
    }

    fn list(&mut self, complexes: Vec<usize>) -> usize {
        self.lists.push(complexes);
        self.lists.len() - 1
    }

    fn filter(&mut self, f: Filter, out: &mut Vec<Test>) {
        self.filters.push(f);
        out.push(Test::Filter(self.filters.len() - 1));
    }

    fn pseudo(&mut self, p: PseudoFn, out: &mut Vec<Test>) {
        match p {
            PseudoFn::Root => out.push(Test::Root),
            PseudoFn::Empty => out.push(Test::Empty),
            PseudoFn::Filter(f) => self.filter(f, out),
        }
    }

    fn complex(&mut self, tree: &Complex, pe: Option<&PseudoElement>) -> Result<usize, Fail> {
        let mut parts = Vec::with_capacity(tree.0.len());
        let last = tree.0.len().saturating_sub(1);
        for (i, (comb, compound)) in tree.0.iter().enumerate() {
            let pe = if i == last { pe } else { None };
            parts.push((*comb, self.compound(compound, pe)?));
        }
        self.complexes.push(parts);
        Ok(self.complexes.len() - 1)
    }

    fn strict_list(&mut self, list: &[Complex]) -> Result<usize, Fail> {
        let mut ids = Vec::with_capacity(list.len());
        for tree in list {
            ids.push(self.complex(tree, None)?);
        }
        Ok(self.list(ids))
    }

    /// One compound, as `select.py` evaluates it, and with the pseudo-element its selector ends
    /// on (`select_selector`).
    fn compound(&mut self, c: &Compound, pe: Option<&PseudoElement>) -> Result<usize, Fail> {
        // `select_pseudo`: `:root` and `:scope` yield the root without evaluating what comes
        // before them in the compound.
        let cut = c
            .0
            .iter()
            .rposition(|s| matches!(s, Simple::Pseudo(n) if matches!(n.replace('-', "_").as_str(), "root" | "scope")))
            .unwrap_or(0);
        let mut tests = Vec::new();
        for s in &c.0[cut..] {
            self.simple(s, &mut tests)?;
        }
        match pe {
            None => {}
            // "The pseudo-element ::%s is not supported"
            Some(PseudoElement::Functional) => return Err(Fail::Invalid),
            // As a pseudo-element, `::root` filters (`select_selector`).
            Some(PseudoElement::Named(name)) => self.pseudo(pseudo_fn(name)?, &mut tests),
        }
        self.compounds.push(tests);
        Ok(self.compounds.len() - 1)
    }

    fn simple(&mut self, s: &Simple, out: &mut Vec<Test>) -> Result<(), Fail> {
        match s {
            Simple::Type(None) => {}
            Simple::Type(Some(name)) => out.push(Test::Type(name.to_ascii_lowercase())),
            Simple::Id(id) => out.push(Test::Id(id.to_ascii_lowercase())),
            Simple::Class(class) => out.push(Test::Class(class.to_ascii_lowercase())),
            Simple::Attr {
                name,
                op,
                value,
                flag,
            } => {
                // `ascii_lower(None)` raises AttributeError.
                let Some(name) = name else {
                    return Err(Fail::Hard);
                };
                let ci = flag.as_deref() == Some("i");
                let op = match op {
                    AttrOp::Exists => AttrMatch::Exists,
                    AttrOp::Equal => AttrMatch::Equal,
                    AttrOp::Includes => AttrMatch::Includes,
                    AttrOp::Dash => AttrMatch::Dash,
                    AttrOp::Prefix => AttrMatch::Prefix,
                    AttrOp::Suffix => AttrMatch::Suffix,
                    AttrOp::Substring => AttrMatch::Substring,
                    // `select_attrib`: no entry in `attribute_operator_mapping`, a KeyError;
                    // with `i`, "Unknown attribute operator".
                    AttrOp::NotEqual if ci => return Err(Fail::Invalid),
                    AttrOp::NotEqual => return Err(Fail::Hard),
                };
                // `select_includes` needs `is_non_whitespace(value)`; the others but `=` and
                // `[a]` need a value.
                let never = match op {
                    AttrMatch::Exists | AttrMatch::Equal => false,
                    AttrMatch::Includes => value.is_empty() || value.contains(is_ws),
                    _ => value.is_empty(),
                };
                if never {
                    out.push(Test::Never);
                } else {
                    out.push(Test::Attr(AttrTest {
                        name: name.to_ascii_lowercase(),
                        op,
                        value: if ci {
                            value.to_ascii_lowercase()
                        } else {
                            value.clone()
                        },
                        ci,
                    }));
                }
            }
            Simple::Pseudo(name) => self.pseudo(pseudo_fn(name)?, out),
            Simple::Not(list) => {
                let l = self.strict_list(list)?;
                self.filter(Filter::NotIn(l), out);
            }
            // `select_matching`: a branch that raises `ExpressionError` matches nothing.
            Simple::Matching { list, .. } => {
                let mut ids = Vec::new();
                for branch in list.iter().filter(|b| !b.pseudo_element) {
                    match self.complex(&branch.tree, None) {
                        Ok(id) => ids.push(id),
                        Err(Fail::Invalid) => {}
                        Err(e) => return Err(e),
                    }
                }
                if ids.is_empty() {
                    out.push(Test::Never);
                } else {
                    let l = self.list(ids);
                    self.filter(Filter::In(l), out);
                }
            }
            Simple::Has(list) => {
                let mut rels = Vec::with_capacity(list.len());
                for (comb, tree) in list {
                    // Right to left, each compound with the combinator to its left; the first
                    // with the relative selector's own.
                    let mut rel = Vec::with_capacity(tree.0.len());
                    for (i, (c, compound)) in tree.0.iter().enumerate().rev() {
                        let left = if i == 0 { *comb } else { *c };
                        rel.push((left, self.compound(compound, None)?));
                    }
                    rels.push(rel);
                }
                self.has.push(rels);
                self.filter(Filter::Has(self.has.len() - 1), out);
            }
            Simple::Function { name, args, of } => {
                let f = self.function(name, args, of.as_deref())?;
                self.filter(f, out);
            }
        }
        Ok(())
    }

    /// `select_function`.
    fn function(
        &mut self,
        name: &str,
        args: &[Tok],
        of: Option<&[Complex]>,
    ) -> Result<Filter, Fail> {
        let key = name.replace('-', "_");
        match key.as_str() {
            "lang" => match args {
                // `select_lang`: one string or identifier; an empty one matches nothing.
                [Tok::Str(v) | Tok::Ident(v)] if v.is_empty() => Ok(Filter::Never),
                [Tok::Str(v) | Tok::Ident(v)] => Ok(Filter::Lang(v.to_ascii_lowercase())),
                _ => Err(Fail::Invalid),
            },
            "nth_child" | "nth_last_child" | "nth_of_type" | "nth_last_of_type" => {
                let of = match of {
                    Some(list) => Some(self.strict_list(list)?),
                    None => None,
                };
                // "Invalid series"
                let (a, b) = series(args).ok_or(Fail::Invalid)?;
                Ok(Filter::Nth {
                    last: key.starts_with("nth_last"),
                    of_type: key.ends_with("of_type"),
                    a,
                    b,
                    of,
                })
            }
            k if TRUE_FUNCTIONS.contains(&k) => Ok(Filter::All),
            // The others take another number of arguments: a TypeError.
            k if DISPATCH.contains(&k) => Err(Fail::Hard),
            // "The pseudo-class :%s() is unknown"
            _ => Err(Fail::Invalid),
        }
    }
}

/// Python's `str.isspace`.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Python's `str.split()`.
pub(super) fn py_split(s: &str) -> impl Iterator<Item = &str> {
    s.split(py_space).filter(|t| !t.is_empty())
}

/// `int(s)`, held as an i128 for `nth_ok`'s arithmetic.
fn int(s: &str) -> Option<i128> {
    py::py_int(s).map(i128::from)
}

/// `parse_series`: `(a, b)` of `an+b` from the argument tokens, joined without white space.
fn series(args: &[Tok]) -> Option<(i128, i128)> {
    let mut s = String::new();
    for t in args {
        match t {
            Tok::Str(_) => return None,
            Tok::Ident(v) | Tok::Num(v) => s.push_str(v),
            Tok::Delim(c) => s.push(*c),
            _ => {}
        }
    }
    let s = s.trim_matches(py_space);
    match s {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        "n" => return Some((1, 0)),
        _ => {}
    }
    let Some((a, b)) = s.split_once('n') else {
        return Some((0, int(s)?));
    };
    let a = match a {
        "" | "+" => 1,
        "-" => -1,
        a => int(a)?,
    };
    let b = if b.is_empty() { 0 } else { int(b)? };
    Some((a, b))
}

/// `select_nth_child`'s test for position `num` (from 1): `num == b` when `a` is 0, else
/// `(num - b) / a` is a whole number above -1. The division is Python's, to the nearest float,
/// which past 2^53 is always whole.
pub(super) fn nth_ok(a: i128, b: i128, num: i128) -> bool {
    if a == 0 {
        return num == b;
    }
    let diff = num.saturating_sub(b);
    if diff.checked_rem(a) == Some(0) {
        return diff / a >= 0;
    }
    let q = diff as f64 / a as f64;
    q.fract() == 0.0 && q > -1.0
}

/// `normalize_language_tag` and `select_lang`'s test: whether one of the tags listed for
/// `lang` is `want` or starts with `want-`. The tags are the first subtag joined with every
/// selection of the others in order, 2^n of them; this follows the selections through `want`
/// instead of listing them.
pub(super) fn lang_matches(lang: &str, want: &str) -> bool {
    let c: Vec<char> = lang
        .to_ascii_lowercase()
        .replace('_', "-")
        .chars()
        .collect();
    // re.sub(r'-([a-zA-Z0-9])-', r'-\1_', tag): a one-character subtag joins the next.
    let mut marked = String::with_capacity(c.len() + 1);
    let mut i = 0;
    while i < c.len() {
        if c[i] == '-' && i + 2 < c.len() && c[i + 1].is_ascii_alphanumeric() && c[i + 2] == '-' {
            marked.push('-');
            marked.push(c[i + 1]);
            marked.push('_');
            i += 3;
        } else {
            marked.push(c[i]);
            i += 1;
        }
    }
    let subtags: Vec<String> = marked.split('-').map(|s| s.replace('_', "-")).collect();
    let (base, rest) = subtags
        .split_first()
        .map_or(("", &[][..]), |(b, r)| (b.as_str(), r));
    let Some(after) = want.strip_prefix(base) else {
        return false;
    };
    if after.is_empty() {
        return true;
    }
    let Some(r) = after.strip_prefix('-') else {
        return false;
    };
    // Positions p where some selection so far spells r[..p] followed by `-`, each just past a
    // `-` of `want`: the walk costs at most the subtags times those.
    let mut seen = vec![false; r.len() + 1];
    seen[0] = true;
    let mut reach = vec![0];
    let mut added = Vec::new();
    for s in rest {
        added.clear();
        for &p in &reach {
            let tail = &r[p..];
            if let Some(more) = tail.strip_prefix(s.as_str()) {
                if more.is_empty() {
                    return true;
                }
                if more.starts_with('-') {
                    added.push(p + s.len() + 1);
                }
            }
            if s.starts_with(tail) && s.as_bytes().get(tail.len()) == Some(&b'-') {
                return true;
            }
        }
        for &q in &added {
            if !seen[q] {
                seen[q] = true;
                reach.push(q);
            }
        }
    }
    false
}

/// `stylizer.py`'s `pseudo_pat`, `:{1,2}(names)` searched case-insensitively in the text: the
/// first place it matches, the longer of two names that start alike first (`DIVERGENCES.md`).
pub(super) fn pseudo_name(text: &str) -> Option<&'static str> {
    let b = text.as_bytes();
    (0..b.len()).filter(|&i| b[i] == b':').find_map(|i| {
        let j = if b.get(i + 1) == Some(&b':') {
            i + 2
        } else {
            i + 1
        };
        let rest = &b[j..];
        INAPPROPRIATE
            .iter()
            .filter(|n| rest.len() >= n.len() && rest[..n.len()].eq_ignore_ascii_case(n.as_bytes()))
            .max_by_key(|n| n.len())
            .copied()
    })
}
