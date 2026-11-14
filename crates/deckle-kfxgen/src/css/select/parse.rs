// SPDX-License-Identifier: GPL-3.0-only
//! A selector's grammar, `css_selectors/parser.py`, and its specificity as css_parser 1.1.1
//! counts it.

use super::tokens::{Stream, Tok};
use super::{Fail, MAX_PARTS};
use crate::css::value::MAX_NESTING;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Comb {
    Descendant,
    Child,
    Next,
    Later,
}

/// Compound selectors with the combinator before each (the first's is unused).
#[derive(Debug)]
pub(super) struct Complex(pub(super) Vec<(Comb, Compound)>);

/// Simple selectors, the first a [`Simple::Type`].
#[derive(Debug)]
pub(super) struct Compound(pub(super) Vec<Simple>);

#[derive(Debug)]
pub(super) enum PseudoElement {
    /// In ASCII lower case (`Selector.__init__`).
    Named(String),
    Functional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttrOp {
    Exists,
    Equal,
    Includes,
    Dash,
    Prefix,
    Suffix,
    Substring,
    /// `!=`, which the grammar reads and `Select` has no rule for.
    NotEqual,
}

#[derive(Debug)]
pub(super) enum Simple {
    /// `Element`: the name, None for `*` (its namespace is ignored).
    Type(Option<String>),
    Id(String),
    Class(String),
    Attr {
        /// None for `[*|=…]`.
        name: Option<String>,
        op: AttrOp,
        value: String,
        /// `i` or `s`.
        flag: Option<String>,
    },
    /// `Pseudo`: the name in ASCII lower case.
    Pseudo(String),
    Not(Vec<Complex>),
    /// `:is()`, `:where()`, `:matches()`.
    Matching {
        name: String,
        list: Vec<Branch>,
    },
    Has(Vec<(Comb, Complex)>),
    /// A functional pseudo-class, its name in ASCII lower case; `of` for `:nth-child()` and
    /// `:nth-last-child()` (`parse_nth_child`).
    Function {
        name: String,
        args: Vec<Tok>,
        of: Option<Vec<Complex>>,
    },
}

/// A branch of `:is()`: one with a pseudo-element is a syntax error to Calibre, which skips it,
/// but css_parser counts it in the specificity.
#[derive(Debug)]
pub(super) struct Branch {
    pub(super) tree: Complex,
    pub(super) pseudo_element: bool,
}

/// `special_pseudo_elements`: pseudo-elements a single colon introduces.
const SPECIAL_PSEUDO_ELEMENTS: [&str; 4] = ["first-line", "first-letter", "before", "after"];

/// The grammar of `css_selectors/parser.py`, counting parts as it reads.
pub(super) struct Reader {
    pub(super) parts: usize,
}

impl Reader {
    /// Counts a simple selector or pseudo-element read, and refuses one past [`MAX_PARTS`].
    fn part(&mut self) -> Result<(), Fail> {
        self.parts += 1;
        if self.parts > MAX_PARTS {
            return Err(Fail::Hard);
        }
        Ok(())
    }

    /// `parse_selector_group`.
    pub(super) fn group(
        &mut self,
        st: &mut Stream,
    ) -> Result<Vec<(Complex, Option<PseudoElement>)>, Fail> {
        st.skip_ws();
        let mut out = Vec::new();
        loop {
            out.push(self.selector(st, 0)?);
            if st.is(',') {
                st.next();
                st.skip_ws();
            } else {
                return Ok(out);
            }
        }
    }

    /// `parse_selector`.
    fn selector(
        &mut self,
        st: &mut Stream,
        depth: usize,
    ) -> Result<(Complex, Option<PseudoElement>), Fail> {
        let (first, mut pe) = self.compound(st, depth)?;
        let mut parts = vec![(Comb::Descendant, first)];
        loop {
            st.skip_ws();
            if *st.peek() == Tok::Eof || st.is(',') {
                return Ok((Complex(parts), pe));
            }
            if pe.is_some() {
                return Err(Fail::Invalid);
            }
            let comb = match st.peek() {
                Tok::Delim('>') => Comb::Child,
                Tok::Delim('+') => Comb::Next,
                Tok::Delim('~') => Comb::Later,
                _ => Comb::Descendant,
            };
            if comb != Comb::Descendant {
                st.next();
                st.skip_ws();
            }
            let (next, next_pe) = self.compound(st, depth)?;
            pe = next_pe;
            parts.push((comb, next));
        }
    }

    /// `parse_simple_selector`.
    fn compound(
        &mut self,
        st: &mut Stream,
        depth: usize,
    ) -> Result<(Compound, Option<PseudoElement>), Fail> {
        st.skip_ws();
        let start = st.at;
        let mut simples = Vec::new();
        if matches!(st.peek(), Tok::Ident(_) | Tok::Delim('*')) {
            self.part()?;
            let first = st.next_ident_or_star()?;
            if st.is('|') {
                st.next();
                simples.push(Simple::Type(st.next_ident_or_star()?));
            } else {
                simples.push(Simple::Type(first));
            }
        } else {
            simples.push(Simple::Type(None));
        }
        let mut pe = None;
        loop {
            let peek = st.peek();
            if matches!(peek, Tok::S | Tok::Eof | Tok::Delim(',' | '+' | '>' | '~')) {
                break;
            }
            if pe.is_some() {
                return Err(Fail::Invalid);
            }
            self.part()?;
            match st.next() {
                Tok::Hash(id) => simples.push(Simple::Id(id.clone())),
                Tok::Delim('.') => simples.push(Simple::Class(st.next_ident()?)),
                Tok::Delim('[') => simples.push(attrib(st)?),
                Tok::Delim(':') => {
                    if st.is(':') {
                        st.next();
                        let name = st.next_ident()?;
                        pe = Some(if st.is('(') {
                            st.next();
                            arguments(st)?;
                            PseudoElement::Functional
                        } else {
                            PseudoElement::Named(name.to_ascii_lowercase())
                        });
                        continue;
                    }
                    let ident = st.next_ident()?;
                    let name = ident.to_ascii_lowercase();
                    if SPECIAL_PSEUDO_ELEMENTS.contains(&name.as_str()) {
                        pe = Some(PseudoElement::Named(name));
                        continue;
                    }
                    if !st.is('(') {
                        simples.push(Simple::Pseudo(name));
                        continue;
                    }
                    st.next();
                    st.skip_ws();
                    simples.push(self.function(st, name, depth)?);
                }
                _ => return Err(Fail::Invalid),
            }
        }
        if st.at == start {
            return Err(Fail::Invalid);
        }
        Ok((Compound(simples), pe))
    }

    /// The functional pseudo-classes of `parse_simple_selector`, after `name(`.
    fn function(&mut self, st: &mut Stream, name: String, depth: usize) -> Result<Simple, Fail> {
        match name.as_str() {
            "not" => {
                let toks = function_tokens(st)?;
                Ok(Simple::Not(self.strict_list(toks, depth)?))
            }
            "is" | "where" | "matches" => {
                let toks = function_tokens(st)?;
                let list = self
                    .list(toks, depth, false, true)?
                    .into_iter()
                    .map(|(_, tree, pe)| Branch {
                        tree,
                        pseudo_element: pe,
                    })
                    .collect();
                Ok(Simple::Matching { name, list })
            }
            "has" => {
                let toks = function_tokens(st)?;
                let mut list = Vec::new();
                for (comb, tree, pe) in self.list(toks, depth, true, false)? {
                    if pe || contains_relation(&tree) {
                        return Err(Fail::Invalid);
                    }
                    list.push((comb, tree));
                }
                Ok(Simple::Has(list))
            }
            // `parse_nth_child`
            "nth-child" | "nth-last-child" => {
                let toks = function_tokens(st)?;
                let mut args = Vec::new();
                let mut of = None;
                for (i, t) in toks.iter().enumerate() {
                    match t {
                        Tok::S => continue,
                        Tok::Ident(v) if v.eq_ignore_ascii_case("of") => {
                            if args.is_empty() {
                                return Err(Fail::Invalid);
                            }
                            of = Some(self.strict_list(&toks[i + 1..], depth)?);
                            break;
                        }
                        t if is_argument(t) => args.push(t.clone()),
                        _ => return Err(Fail::Invalid),
                    }
                }
                Ok(Simple::Function { name, args, of })
            }
            _ => Ok(Simple::Function {
                name,
                args: arguments(st)?,
                of: None,
            }),
        }
    }

    /// A selector list no part of which may fail: `:not()`, `of`.
    fn strict_list(&mut self, toks: &[Tok], depth: usize) -> Result<Vec<Complex>, Fail> {
        let mut out = Vec::new();
        for (_, tree, pe) in self.list(toks, depth, false, false)? {
            if pe {
                return Err(Fail::Invalid);
            }
            out.push(tree);
        }
        Ok(out)
    }

    /// `parse_selector_list_tokens`: each selector, its leading combinator for a relative one,
    /// and whether it ended on a pseudo-element (which every caller but `:is()` refuses). A
    /// forgiving list skips the selectors that do not parse.
    fn list(
        &mut self,
        toks: &[Tok],
        depth: usize,
        relative: bool,
        forgiving: bool,
    ) -> Result<Vec<(Comb, Complex, bool)>, Fail> {
        if depth + 1 > MAX_NESTING {
            return Err(Fail::Hard);
        }
        let mut out = Vec::new();
        for chunk in split_list(toks) {
            let mut st = Stream::new(chunk);
            st.skip_ws();
            let mut comb = Comb::Descendant;
            if relative {
                match st.peek() {
                    Tok::Delim('>') => comb = Comb::Child,
                    Tok::Delim('+') => comb = Comb::Next,
                    Tok::Delim('~') => comb = Comb::Later,
                    _ => {}
                }
                if comb != Comb::Descendant {
                    st.next();
                    st.skip_ws();
                }
            }
            match self.selector(&mut st, depth + 1) {
                Ok((tree, pe)) => out.push((comb, tree, pe.is_some())),
                Err(Fail::Invalid) if forgiving => {}
                Err(e) => return Err(e),
            }
        }
        if out.is_empty() && !forgiving {
            return Err(Fail::Invalid);
        }
        Ok(out)
    }
}

/// `parse_attrib`, after `[`.
fn attrib(st: &mut Stream) -> Result<Simple, Fail> {
    st.skip_ws();
    let mut name = st.next_ident_or_star()?;
    if name.is_none() && !st.is('|') {
        return Err(Fail::Invalid);
    }
    let mut op = None;
    if st.is('|') {
        st.next();
        if st.is('=') {
            st.next();
            op = Some(AttrOp::Dash);
        } else {
            name = Some(st.next_ident()?);
        }
    }
    let op = match op {
        Some(op) => op,
        None => {
            st.skip_ws();
            let op = match st.next() {
                Tok::Delim(']') => {
                    return Ok(Simple::Attr {
                        name,
                        op: AttrOp::Exists,
                        value: String::new(),
                        flag: None,
                    });
                }
                Tok::Delim('=') => return finish_attrib(st, name, AttrOp::Equal),
                Tok::Delim('^') => AttrOp::Prefix,
                Tok::Delim('$') => AttrOp::Suffix,
                Tok::Delim('*') => AttrOp::Substring,
                Tok::Delim('~') => AttrOp::Includes,
                Tok::Delim('|') => AttrOp::Dash,
                Tok::Delim('!') => AttrOp::NotEqual,
                _ => return Err(Fail::Invalid),
            };
            if !st.is('=') {
                return Err(Fail::Invalid);
            }
            st.next();
            op
        }
    };
    finish_attrib(st, name, op)
}

/// The value, flag and `]` of an attribute selector.
fn finish_attrib(st: &mut Stream, name: Option<String>, op: AttrOp) -> Result<Simple, Fail> {
    st.skip_ws();
    let value = match st.next() {
        Tok::Ident(v) | Tok::Str(v) => v.clone(),
        _ => return Err(Fail::Invalid),
    };
    st.skip_ws();
    let mut next = st.next();
    let mut flag = None;
    if let Tok::Ident(v) = next {
        let v = v.to_ascii_lowercase();
        if v == "i" || v == "s" {
            flag = Some(v);
            st.skip_ws();
            next = st.next();
        }
    }
    if *next != Tok::Delim(']') {
        return Err(Fail::Invalid);
    }
    Ok(Simple::Attr {
        name,
        op,
        value,
        flag,
    })
}

/// `is_argument_token`.
fn is_argument(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Ident(_) | Tok::Str(_) | Tok::Num(_) | Tok::Delim('+' | '-')
    )
}

/// `parse_arguments`: the arguments of a function, up to its `)`.
fn arguments(st: &mut Stream) -> Result<Vec<Tok>, Fail> {
    let mut out = Vec::new();
    loop {
        st.skip_ws();
        match st.next() {
            t if is_argument(t) => out.push(t.clone()),
            Tok::Delim(')') => return Ok(out),
            _ => return Err(Fail::Invalid),
        }
    }
}

/// `collect_function_tokens`: the tokens up to the `)` that closes the function.
fn function_tokens<'a>(st: &mut Stream<'a>) -> Result<&'a [Tok], Fail> {
    let start = st.at;
    let mut depth = 0usize;
    loop {
        match st.next() {
            Tok::Eof => return Err(Fail::Invalid),
            Tok::Delim('(') => depth += 1,
            Tok::Delim(')') if depth == 0 => return Ok(&st.toks[start..st.at - 1]),
            Tok::Delim(')') => depth -= 1,
            _ => {}
        }
    }
}

/// `split_selector_list_tokens`: the tokens split on commas outside parentheses; nothing when
/// they are all white space.
fn split_list(toks: &[Tok]) -> Vec<&[Tok]> {
    if toks.iter().all(|t| *t == Tok::S) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut depth = 0i64;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Delim('(') => depth += 1,
            Tok::Delim(')') => depth -= 1,
            Tok::Delim(',') if depth == 0 => {
                out.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&toks[start..]);
    out
}

/// `contains_relation`: whether a `:has()` is anywhere inside.
fn contains_relation(tree: &Complex) -> bool {
    tree.0.iter().any(|(_, c)| {
        c.0.iter().any(|s| match s {
            Simple::Has(_) => true,
            Simple::Not(list) => list.iter().any(contains_relation),
            Simple::Matching { list, .. } => list
                .iter()
                .any(|b| !b.pseudo_element && contains_relation(&b.tree)),
            Simple::Function { of: Some(list), .. } => list.iter().any(contains_relation),
            _ => false,
        })
    })
}

// --- Specificity, as css_parser 1.1.1 counts it ---------------------------------------------

pub(super) type Spec = (u32, u32, u32);
pub(super) const ZERO: Spec = (0, 0, 0);

pub(super) fn add(a: Spec, b: Spec) -> Spec {
    (
        a.0.saturating_add(b.0),
        a.1.saturating_add(b.1),
        a.2.saturating_add(b.2),
    )
}

/// css_parser's count, read from calibre 9.18 (`tests/it/css_select.rs`): ids; classes,
/// attributes and pseudo-classes, functions of no selectors included; types and
/// pseudo-elements. `:not()`, `:is()` and `:has()` count their most specific selector, compared
/// as tuples, `:where()` nothing; css_parser reads `:matches()` as a function of no selectors,
/// and `:nth-child(… of S)` as one pseudo-class, S adding nothing.
pub(super) fn spec_complex(c: &Complex) -> Spec {
    c.0.iter()
        .flat_map(|(_, compound)| &compound.0)
        .fold(ZERO, |acc, s| add(acc, spec_simple(s)))
}

fn spec_max<'a>(list: impl Iterator<Item = &'a Complex>) -> Spec {
    list.map(spec_complex).max().unwrap_or(ZERO)
}

fn spec_simple(s: &Simple) -> Spec {
    match s {
        Simple::Type(None) => ZERO,
        Simple::Type(Some(_)) => (0, 0, 1),
        Simple::Id(_) => (1, 0, 0),
        Simple::Class(_) | Simple::Attr { .. } | Simple::Pseudo(_) => (0, 1, 0),
        Simple::Not(list) => spec_max(list.iter()),
        Simple::Has(list) => spec_max(list.iter().map(|(_, c)| c)),
        Simple::Matching { name, list } => match name.as_str() {
            "is" => list
                .iter()
                .map(|b| {
                    let pe = if b.pseudo_element { (0, 0, 1) } else { ZERO };
                    add(spec_complex(&b.tree), pe)
                })
                .max()
                .unwrap_or(ZERO),
            "where" => ZERO,
            _ => (0, 1, 0),
        },
        Simple::Function { .. } => (0, 1, 0),
    }
}
