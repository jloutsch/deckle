// SPDX-License-Identifier: GPL-3.0-only
//! Selector matching as calibre 9.18's own engine, `css_selectors`, does it for the Stylizer
//! (phase 2 plan, Task 7), departures from the CSS specification included.
//!
//! The Stylizer hands each rule's selector text, as css_parser wrote it, to
//! `Select(tree, ignore_inappropriate_pseudo_classes=True)` (`ebooks/oeb/stylizer.py`
//! `Stylizer.__init__`). [`parse_selector`] reads it with a port of `css_selectors/parser.py`'s
//! tokenizer and grammar, refuses what `Select` would refuse ([`Parsed::Dropped`]), works out
//! the specificity css_parser gave the rule, and the pseudo-class name `stylizer.py` looks for
//! in the text. [`select`] then evaluates it as `css_selectors/select.py` does: compound by
//! compound over the whole document, a combinator mapping one set of elements to the next,
//! `:has()` from right to left (`relative_selector_anchors`). Each compound is matched by the
//! `selectors` crate through an `Element` over [`Dom`], in quirks mode so class and id names
//! match ASCII case-insensitively, as in Calibre.
//!
//! Calibre's departures, each ported where its rule is (file and function named there):
//! element and attribute names match ASCII case-insensitively and namespaces are ignored;
//! attribute values stay case-sensitive; `:root` and `:scope` match the root whatever comes
//! before them in their compound; `:hover`, `::before` and the rest of
//! `INAPPROPRIATE_PSEUDO_CLASSES` match every element; `:lang()` reads only `lang`, not
//! `xml:lang`; `:nth-*()` arguments are read by `parse_series`, so `ODD` is refused and `2 n` is
//! `2n`; class lists split on Python's white space.
//!
//! Hostile input: the reading is iterative but for selector lists, which stop at
//! [`MAX_NESTING`] levels; a selector of more than [`MAX_PARTS`] parts is dropped;
//! evaluation is linear in the document for each compound (sibling and ancestor relations are
//! propagated in one pass, never searched per element), and caches every sibling count it
//! needs. Where the port and Calibre differ on purpose, `DIVERGENCES.md` says so.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use cssparser::{
    CowRcStr, ParseError, Parser as CssParser, ParserInput, ToCss, serialize_identifier,
    serialize_string,
};
use precomputed_hash::PrecomputedHash;
use selectors::attr::{
    AttrSelectorOperation, AttrSelectorOperator, CaseSensitivity, NamespaceConstraint,
};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode,
    SelectorCaches,
};
use selectors::matching::{ElementSelectorFlags, matches_selector};
use selectors::parser::{ParseRelative, SelectorParseErrorKind};
use selectors::{OpaqueElement, SelectorList};

use super::value::MAX_NESTING;
use crate::dom::{Dom, NodeId};

/// The most parts of compound selectors one selector holds (type, id, class, attribute and
/// pseudo-class selectors and pseudo-elements, as written, those inside `:is()`, `:not()`,
/// `:has()` and `of` included); past it the selector is [`Parsed::Dropped`]
/// (`DIVERGENCES.md`). Matching costs at most this many tests per element.
const MAX_PARTS: usize = 256;

/// A selector read and checked as Calibre's `Select` reads it, ready to match.
#[derive(Clone)]
pub struct Selector(Arc<Compiled>);

impl fmt::Debug for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Selector").field(&self.0.text).finish()
    }
}

/// What [`parse_selector`] makes of a selector.
#[derive(Debug, Clone)]
pub enum Parsed {
    Ok {
        selector: Selector,
        /// The pseudo-class `stylizer.py` stores the rule's declarations under instead of the
        /// element's own (`pseudo_pat`): one of `INAPPROPRIATE_PSEUDO_CLASSES`, in lower case,
        /// found anywhere in the text.
        pseudo: Option<&'static str>,
        /// css_parser's `Selector.specificity` without its first number (a style attribute's,
        /// 0 for a rule): (ids, classes, types). For a list, which css_parser never hands over,
        /// the largest.
        specificity: (u32, u32, u32),
    },
    /// Calibre's `Select` raises for it, and the Stylizer skips the rule.
    Dropped,
}

/// Reads `text`, a selector (or a list) as css_parser writes it.
pub fn parse_selector(text: &str) -> Parsed {
    match read(text) {
        Ok((compiled, specificity)) => Parsed::Ok {
            selector: Selector(Arc::new(compiled)),
            pseudo: pseudo_name(text),
            specificity,
        },
        Err(_) => Parsed::Dropped,
    }
}

/// Whether `sel` matches `node`. It evaluates the selector over the whole document, as
/// [`select`] does: to test many elements, call [`select`] once.
pub fn matches(sel: &Selector, dom: &Dom, node: NodeId) -> bool {
    let engine = Engine::new(dom, &sel.0);
    let set = engine.list_set(sel.0.top);
    set.get(node.index()).copied().unwrap_or(false)
}

/// The elements `sel` matches, in document order.
pub fn select(sel: &Selector, dom: &Dom) -> Vec<NodeId> {
    let engine = Engine::new(dom, &sel.0);
    let set = engine.list_set(sel.0.top);
    engine
        .order
        .iter()
        .copied()
        .filter(|n| set[n.index()])
        .collect()
}

fn read(text: &str) -> Result<(Compiled, Spec), Fail> {
    let toks = tokenize(text)?;
    let mut stream = Stream::new(&toks);
    let mut reader = Reader { parts: 0 };
    let selectors = reader.group(&mut stream)?;
    let specificity = selectors
        .iter()
        .map(|(c, pe)| add(spec_complex(c), if pe.is_some() { (0, 0, 1) } else { ZERO }))
        .max()
        .unwrap_or(ZERO);
    Ok((Builder::compile(text, &selectors)?, specificity))
}

/// Why a selector is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fail {
    /// Calibre's `SelectorSyntaxError` or `ExpressionError`, which a branch of `:is()` or
    /// `:where()` forgives (`parser.py` `parse_selector_list_tokens`, `select.py`
    /// `select_matching`).
    Invalid,
    /// Not forgiven: a limit passed, or an exception other than `SelectorError` in Calibre,
    /// which stops its conversion.
    Hard,
}

// --- Tokens: css_selectors/parser.py `tokenize` -------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// White space.
    S,
    Ident(String),
    Hash(String),
    Str(String),
    Num(String),
    Delim(char),
    Eof,
}

static EOF: Tok = Tok::Eof;

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c')
}

/// The end of the escape at `i`: a backslash and 1 to 6 hex digits with one white space after
/// them, or a backslash and a character that is neither a newline nor a hex digit
/// (`TokenMacros.escape`, matched case-insensitively).
fn escape_end(c: &[char], i: usize) -> Option<usize> {
    if c.get(i) != Some(&'\\') {
        return None;
    }
    let next = *c.get(i + 1)?;
    if next.is_ascii_hexdigit() {
        let mut j = i + 1;
        while j < c.len() && j < i + 7 && c[j].is_ascii_hexdigit() {
            j += 1;
        }
        if c.get(j) == Some(&'\r') && c.get(j + 1) == Some(&'\n') {
            j += 2;
        } else if c.get(j).is_some_and(|&w| is_ws(w)) {
            j += 1;
        }
        return Some(j);
    }
    (!matches!(next, '\n' | '\r' | '\x0c')).then_some(i + 2)
}

/// `nmstart`: a letter, `_`, an escape or a character past ASCII.
fn nmstart_end(c: &[char], i: usize) -> Option<usize> {
    match *c.get(i)? {
        ch if ch.is_ascii_alphabetic() || ch == '_' || !ch.is_ascii() => Some(i + 1),
        '\\' => escape_end(c, i),
        _ => None,
    }
}

/// `nmchar`: `nmstart`, a digit or `-`.
fn nmchar_end(c: &[char], i: usize) -> Option<usize> {
    match *c.get(i)? {
        ch if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || !ch.is_ascii() => Some(i + 1),
        '\\' => escape_end(c, i),
        _ => None,
    }
}

fn ident_end(c: &[char], i: usize) -> Option<usize> {
    let start = if c.get(i) == Some(&'-') { i + 1 } else { i };
    let mut j = nmstart_end(c, start)?;
    while let Some(k) = nmchar_end(c, j) {
        j = k;
    }
    Some(j)
}

fn number_end(c: &[char], i: usize) -> Option<usize> {
    let digit = |j: usize| c.get(j).is_some_and(char::is_ascii_digit);
    let mut j = i;
    if matches!(c.get(j), Some('+' | '-')) {
        j += 1;
    }
    let mut d = j;
    while digit(d) {
        d += 1;
    }
    if c.get(d) == Some(&'.') && digit(d + 1) {
        let mut e = d + 1;
        while digit(e) {
            e += 1;
        }
        return Some(e);
    }
    (d > j).then_some(d)
}

/// An identifier's or a hash's value: unicode escapes, then simple ones.
fn unescape(c: &[char]) -> String {
    if c.contains(&'\\') {
        unescape_simple(&unescape_unicode(c))
    } else {
        c.iter().collect()
    }
}

/// `_sub_unicode_escape(_replace_unicode, …)`: `\` and 1 to 6 hex digits (and one white space)
/// to the character; past U+10FFFF, U+FFFD.
fn unescape_unicode(c: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && c.get(i + 1).is_some_and(char::is_ascii_hexdigit) {
            let mut j = i + 1;
            let mut cp: u32 = 0;
            while j < c.len() && j < i + 7 && c[j].is_ascii_hexdigit() {
                cp = cp * 16 + c[j].to_digit(16).unwrap_or(0);
                j += 1;
            }
            // A surrogate, which Python keeps alone in a string, has no Rust character.
            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
            if c.get(j) == Some(&'\r') && c.get(j + 1) == Some(&'\n') {
                j += 2;
            } else if c.get(j).is_some_and(|&w| is_ws(w)) {
                j += 1;
            }
            i = j;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `_sub_simple_escape(_replace_simple, …)`: `\` and any character but a newline to that
/// character. It runs after [`unescape_unicode`], so an escaped backslash escapes again.
fn unescape_simple(c: &[char]) -> String {
    let mut out = String::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && c.get(i + 1).is_some_and(|&n| n != '\n') {
            out.push(c[i + 1]);
            i += 2;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// `_sub_newline_escape('', …)`: drops `\` and a newline.
fn drop_newline_escapes(c: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(c.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' {
            match c.get(i + 1) {
                Some('\n' | '\x0c') => {
                    i += 2;
                    continue;
                }
                Some('\r') => {
                    i += if c.get(i + 2) == Some(&'\n') { 3 } else { 2 };
                    continue;
                }
                _ => {}
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn tokenize(s: &str) -> Result<Vec<Tok>, Fail> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if is_ws(c[i]) {
            while i < c.len() && is_ws(c[i]) {
                i += 1;
            }
            out.push(Tok::S);
            continue;
        }
        if let Some(end) = ident_end(&c, i) {
            out.push(Tok::Ident(unescape(&c[i..end])));
            i = end;
            continue;
        }
        if c[i] == '#'
            && let Some(mut end) = nmchar_end(&c, i + 1)
        {
            while let Some(k) = nmchar_end(&c, end) {
                end = k;
            }
            out.push(Tok::Hash(unescape(&c[i + 1..end])));
            i = end;
            continue;
        }
        if c[i] == '"' || c[i] == '\'' {
            let quote = c[i];
            let mut j = i + 1;
            loop {
                match c.get(j) {
                    None => break,
                    Some(&ch) if ch == quote || matches!(ch, '\n' | '\r' | '\x0c') => break,
                    Some('\\') => match c.get(j + 1) {
                        Some('\n' | '\x0c') => j += 2,
                        Some('\r') => j += if c.get(j + 2) == Some(&'\n') { 3 } else { 2 },
                        _ => match escape_end(&c, j) {
                            Some(e) => j = e,
                            None => break,
                        },
                    },
                    Some(_) => j += 1,
                }
            }
            // "Unclosed string" or "Invalid string".
            if c.get(j) != Some(&quote) {
                return Err(Fail::Invalid);
            }
            let raw = drop_newline_escapes(&c[i + 1..j]);
            out.push(Tok::Str(unescape_simple(&unescape_unicode(&raw))));
            i = j + 1;
            continue;
        }
        if let Some(end) = number_end(&c, i) {
            out.push(Tok::Num(c[i..end].iter().collect()));
            i = end;
            continue;
        }
        if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let mut j = i + 2;
            while j < c.len() && !(c[j] == '*' && c.get(j + 1) == Some(&'/')) {
                j += 1;
            }
            i = (j + 2).min(c.len());
            continue;
        }
        out.push(Tok::Delim(c[i]));
        i += 1;
    }
    Ok(out)
}

/// `TokenStream`: the tokens and how many were taken. A function's tokens are read through a
/// stream over their slice of the selector's, never copied.
struct Stream<'a> {
    toks: &'a [Tok],
    at: usize,
}

impl<'a> Stream<'a> {
    fn new(toks: &'a [Tok]) -> Stream<'a> {
        Stream { toks, at: 0 }
    }

    fn peek(&self) -> &'a Tok {
        self.toks.get(self.at).unwrap_or(&EOF)
    }

    fn next(&mut self) -> &'a Tok {
        let t = self.peek();
        self.at += 1;
        t
    }

    fn is(&self, c: char) -> bool {
        *self.peek() == Tok::Delim(c)
    }

    fn skip_ws(&mut self) {
        if *self.peek() == Tok::S {
            self.at += 1;
        }
    }

    fn next_ident(&mut self) -> Result<String, Fail> {
        match self.next() {
            Tok::Ident(v) => Ok(v.clone()),
            _ => Err(Fail::Invalid),
        }
    }

    /// An identifier, or None for `*`.
    fn next_ident_or_star(&mut self) -> Result<Option<String>, Fail> {
        match self.next() {
            Tok::Ident(v) => Ok(Some(v.clone())),
            Tok::Delim('*') => Ok(None),
            _ => Err(Fail::Invalid),
        }
    }
}

// --- The parsed selector: css_selectors/parser.py ------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comb {
    Descendant,
    Child,
    Next,
    Later,
}

/// Compound selectors with the combinator before each (the first's is unused).
#[derive(Debug)]
struct Complex(Vec<(Comb, Compound)>);

/// Simple selectors, the first a [`Simple::Type`].
#[derive(Debug)]
struct Compound(Vec<Simple>);

#[derive(Debug)]
enum PseudoElement {
    /// In ASCII lower case (`Selector.__init__`).
    Named(String),
    Functional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttrOp {
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
enum Simple {
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
struct Branch {
    tree: Complex,
    pseudo_element: bool,
}

/// `special_pseudo_elements`: pseudo-elements a single colon introduces.
const SPECIAL_PSEUDO_ELEMENTS: [&str; 4] = ["first-line", "first-letter", "before", "after"];

/// The grammar of `css_selectors/parser.py`, counting parts as it reads.
struct Reader {
    parts: usize,
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
    fn group(&mut self, st: &mut Stream) -> Result<Vec<(Complex, Option<PseudoElement>)>, Fail> {
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

type Spec = (u32, u32, u32);
const ZERO: Spec = (0, 0, 0);

fn add(a: Spec, b: Spec) -> Spec {
    (
        a.0.saturating_add(b.0),
        a.1.saturating_add(b.1),
        a.2.saturating_add(b.2),
    )
}

/// css_parser's count, read from calibre 9.18 (`tests/it/css_select.rs`): ids; classes,
/// attributes and pseudo-classes, functions of no selectors included; types and
/// pseudo-elements. `:not()`, `:is()` and `:has()` count their most specific selector, compared
/// as tuples, `:where()` nothing; css_parser reads `:matches()` as a function of no selectors.
fn spec_complex(c: &Complex) -> Spec {
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
        Simple::Function { of, .. } => add(
            (0, 1, 0),
            of.as_ref().map_or(ZERO, |list| spec_max(list.iter())),
        ),
    }
}

// --- What `Select` evaluates: css_selectors/select.py ---------------------------------------

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

/// What a compound asks of an element besides names and attributes: the crate's selector holds
/// `:-deckle(i)`, `i` an index into [`Compiled::filters`].
#[derive(Debug)]
enum Filter {
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

/// The selector ready to evaluate: its compounds as one crate selector list, how they chain,
/// and what their `:-deckle(i)` stand for.
struct Compiled {
    text: String,
    compounds: SelectorList<Impl>,
    /// Complex selectors: compound indexes, each with the combinator before it.
    complexes: Vec<Vec<(Comb, usize)>>,
    /// Selector lists: complex selector indexes.
    lists: Vec<Vec<usize>>,
    /// `:has()` lists: each relative selector's compounds from right to left, with the
    /// combinator to the left of each (`relative_selector_anchors`).
    has: Vec<Vec<Vec<(Comb, usize)>>>,
    filters: Vec<Filter>,
    /// The list [`select`] matches.
    top: usize,
}

#[derive(Default)]
struct Builder {
    /// The compounds, as CSS the `selectors` crate reads, separated by commas.
    css: String,
    n_compounds: usize,
    complexes: Vec<Vec<(Comb, usize)>>,
    lists: Vec<Vec<usize>>,
    has: Vec<Vec<Vec<(Comb, usize)>>>,
    filters: Vec<Filter>,
}

impl Builder {
    fn compile(
        text: &str,
        selectors: &[(Complex, Option<PseudoElement>)],
    ) -> Result<Compiled, Fail> {
        let mut b = Builder::default();
        let mut top = Vec::new();
        for (tree, pe) in selectors {
            top.push(b.complex(tree, pe.as_ref())?);
        }
        let top = b.list(top);
        let mut input = ParserInput::new(&b.css);
        let compounds = SelectorList::parse(
            &CompoundParser,
            &mut CssParser::new(&mut input),
            ParseRelative::No,
        )
        .map_err(|_| Fail::Hard)?;
        // Every compound is one selector of the list, in order.
        if compounds.len() != b.n_compounds {
            return Err(Fail::Hard);
        }
        Ok(Compiled {
            text: text.to_owned(),
            compounds,
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

    fn filter(&mut self, f: Filter, out: &mut String) {
        self.filters.push(f);
        out.push_str(&format!(":-deckle({})", self.filters.len() - 1));
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
        let simples = &c.0[cut..];
        let mut out = String::new();
        if !matches!(simples.first(), Some(Simple::Type(Some(_)))) {
            out.push('*');
        }
        for s in simples {
            self.simple(s, &mut out)?;
        }
        match pe {
            None => {}
            // "The pseudo-element ::%s is not supported"
            Some(PseudoElement::Functional) => return Err(Fail::Invalid),
            Some(PseudoElement::Named(name)) => match pseudo_fn(name)? {
                // As a pseudo-element, `::root` filters (`select_selector`).
                PseudoFn::Root => out.push_str(":root"),
                PseudoFn::Empty => out.push_str(":empty"),
                PseudoFn::Filter(f) => self.filter(f, &mut out),
            },
        }
        if self.n_compounds > 0 {
            self.css.push_str(", ");
        }
        self.css.push_str(&out);
        self.n_compounds += 1;
        Ok(self.n_compounds - 1)
    }

    fn simple(&mut self, s: &Simple, out: &mut String) -> Result<(), Fail> {
        let ident =
            |out: &mut String, v: &str| serialize_identifier(v, out).map_err(|_| Fail::Hard);
        match s {
            Simple::Type(None) => {}
            Simple::Type(Some(name)) => ident(out, name)?,
            Simple::Id(id) => {
                out.push('#');
                ident(out, id)?;
            }
            Simple::Class(class) => {
                out.push('.');
                ident(out, class)?;
            }
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
                let op = match op {
                    AttrOp::Exists => None,
                    AttrOp::Equal => Some("="),
                    AttrOp::Includes => Some("~="),
                    AttrOp::Dash => Some("|="),
                    AttrOp::Prefix => Some("^="),
                    AttrOp::Suffix => Some("$="),
                    AttrOp::Substring => Some("*="),
                    // `select_attrib`: no entry in `attribute_operator_mapping`, a KeyError;
                    // with `i`, "Unknown attribute operator".
                    AttrOp::NotEqual if flag.as_deref() == Some("i") => return Err(Fail::Invalid),
                    AttrOp::NotEqual => return Err(Fail::Hard),
                };
                out.push('[');
                ident(out, name)?;
                if let Some(op) = op {
                    out.push_str(op);
                    serialize_string(value, out).map_err(|_| Fail::Hard)?;
                    if let Some(flag) = flag {
                        out.push(' ');
                        out.push_str(flag);
                    }
                }
                out.push(']');
            }
            Simple::Pseudo(name) => match pseudo_fn(name)? {
                PseudoFn::Root => out.push_str(":root"),
                PseudoFn::Empty => out.push_str(":empty"),
                PseudoFn::Filter(f) => self.filter(f, out),
            },
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
                    self.filter(Filter::Never, out);
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
fn py_split(s: &str) -> impl Iterator<Item = &str> {
    s.split(py_space).filter(|t| !t.is_empty())
}

/// Python ints past this are held as this (`DIVERGENCES.md`).
const BIG: i128 = i128::MAX / 4;

/// Python's `int(s)` for base 10: white space around, a sign, ASCII digits with single
/// underscores between them, at most 4300 digits (`sys.int_info.default_max_str_digits`).
fn py_int(s: &str) -> Option<i128> {
    let s = s.trim_matches(py_space);
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let mut v: i128 = 0;
    let mut count = 0usize;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)?;
        count += 1;
        v = v.saturating_mul(10).saturating_add(i128::from(d)).min(BIG);
    }
    if count > 4300 {
        return None;
    }
    Some(if neg { -v } else { v })
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
        return Some((0, py_int(s)?));
    };
    let a = match a {
        "" | "+" => 1,
        "-" => -1,
        a => py_int(a)?,
    };
    let b = if b.is_empty() { 0 } else { py_int(b)? };
    Some((a, b))
}

/// `select_nth_child`'s test for position `num` (from 1): `num == b` when `a` is 0, else
/// `(num - b) / a` is a whole number above -1. The division is Python's, to the nearest float,
/// which past 2^53 is always whole.
fn nth_ok(a: i128, b: i128, num: i128) -> bool {
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
fn lang_matches(lang: &str, want: &str) -> bool {
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
fn pseudo_name(text: &str) -> Option<&'static str> {
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

// --- Evaluation -------------------------------------------------------------------------------

/// Counts of an element's siblings before and after it that share something with it.
struct Counts {
    before: Vec<u32>,
    after: Vec<u32>,
}

/// One selector over one document, with what it has worked out so far.
struct Engine<'d> {
    dom: &'d Dom,
    sel: &'d Compiled,
    /// The root is in a namespace (Calibre's `'{' in self.root.tag`), so names drop theirs.
    root_ns: bool,
    /// The document's elements in document order.
    order: Vec<NodeId>,
    /// One past the largest element index.
    len: usize,
    lists: RefCell<Vec<Option<Rc<[bool]>>>>,
    has: RefCell<Vec<Option<Rc<[bool]>>>>,
    of_type: RefCell<Option<Rc<Counts>>>,
    of_list: RefCell<HashMap<usize, Rc<Counts>>>,
    lang_src: RefCell<Option<Rc<[Option<NodeId>]>>>,
    lang_hits: RefCell<HashMap<(usize, NodeId), bool>>,
}

fn context(caches: &mut SelectorCaches) -> MatchingContext<'_, Impl> {
    MatchingContext::new(
        MatchingMode::Normal,
        None,
        caches,
        QuirksMode::Quirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    )
}

impl<'d> Engine<'d> {
    fn new(dom: &'d Dom, sel: &'d Compiled) -> Engine<'d> {
        let order: Vec<NodeId> = dom.descendants().collect();
        let len = order.iter().map(|n| n.index() + 1).max().unwrap_or(0);
        Engine {
            dom,
            sel,
            root_ns: dom.namespace(dom.root()).is_some(),
            order,
            len,
            lists: RefCell::new(vec![None; sel.lists.len()]),
            has: RefCell::new(vec![None; sel.has.len()]),
            of_type: RefCell::new(None),
            of_list: RefCell::new(HashMap::new()),
            lang_src: RefCell::new(None),
            lang_hits: RefCell::new(HashMap::new()),
        }
    }

    /// Calibre's `map_tag_name`: the name in ASCII lower case, its namespace dropped when the
    /// root has one, else written `{uri}name`.
    fn type_key(&self, n: NodeId) -> String {
        let tag = self.dom.tag(n);
        match self.dom.namespace(n) {
            Some(ns) if !self.root_ns => format!("{{{ns}}}{tag}").to_ascii_lowercase(),
            _ => tag.to_ascii_lowercase(),
        }
    }

    fn type_is(&self, n: NodeId, name: &str) -> bool {
        let tag = self.dom.tag(n);
        match self.dom.namespace(n) {
            Some(ns) if !self.root_ns => format!("{{{ns}}}{tag}").eq_ignore_ascii_case(name),
            _ => tag.eq_ignore_ascii_case(name),
        }
    }

    /// Whether an attribute's name, as `attrib_map` keys it, is `name`.
    fn attr_is(&self, qname: &str, ns: Option<&str>, name: &str) -> bool {
        let local = || qname.split_once(':').map_or(qname, |(_, l)| l);
        let key: Cow<'_, str> = match ns {
            Some(_) if self.root_ns => Cow::Borrowed(local()),
            Some(ns) => Cow::Owned(format!("{{{ns}}}{}", local())),
            None => Cow::Borrowed(qname),
        };
        key.eq_ignore_ascii_case(name)
    }

    fn compound(&self, ci: usize, n: NodeId, ctx: &mut MatchingContext<'_, Impl>) -> bool {
        matches_selector(
            &self.sel.compounds.slice()[ci],
            0,
            None,
            &El { e: self, n },
            ctx,
        )
    }

    /// The elements of a list (`selector_list_matches`).
    fn list_set(&self, id: usize) -> Rc<[bool]> {
        let cached = self.lists.borrow()[id].clone();
        if let Some(set) = cached {
            return set;
        }
        let mut set = vec![false; self.len];
        for &c in &self.sel.lists[id] {
            for (a, b) in set.iter_mut().zip(self.complex_set(c)) {
                *a |= b;
            }
        }
        let set: Rc<[bool]> = set.into();
        self.lists.borrow_mut()[id] = Some(Rc::clone(&set));
        set
    }

    /// The elements a complex selector matches, compound by compound from the left
    /// (`select_combinedselector` and the combinators after it): each step keeps the elements
    /// in the right relation to the last step's that match the next compound.
    fn complex_set(&self, id: usize) -> Vec<bool> {
        let mut caches = SelectorCaches::default();
        let mut ctx = context(&mut caches);
        let parts = &self.sel.complexes[id];
        let mut cur = vec![false; self.len];
        for (i, &(comb, ci)) in parts.iter().enumerate() {
            let rel = (i > 0).then(|| self.related(comb, &cur));
            let mut next = vec![false; self.len];
            let mut any = false;
            for &n in &self.order {
                let k = n.index();
                if rel.as_ref().is_none_or(|r| r[k]) && self.compound(ci, n, &mut ctx) {
                    next[k] = true;
                    any = true;
                }
            }
            cur = next;
            if !any {
                break;
            }
        }
        cur
    }

    /// The elements in relation `comb` to one of `prev`, in one pass in document order.
    fn related(&self, comb: Comb, prev: &[bool]) -> Vec<bool> {
        let mut out = vec![false; self.len];
        for &n in &self.order {
            let hit = match comb {
                Comb::Descendant => self
                    .dom
                    .parent(n)
                    .is_some_and(|p| prev[p.index()] || out[p.index()]),
                Comb::Child => self.dom.parent(n).is_some_and(|p| prev[p.index()]),
                Comb::Next => self.dom.prev_sibling(n).is_some_and(|s| prev[s.index()]),
                Comb::Later => self
                    .dom
                    .prev_sibling(n)
                    .is_some_and(|s| prev[s.index()] || out[s.index()]),
            };
            out[n.index()] = hit;
        }
        out
    }

    /// `select_relation`: the anchors of a `:has()`.
    fn has_set(&self, id: usize) -> Rc<[bool]> {
        let cached = self.has.borrow()[id].clone();
        if let Some(set) = cached {
            return set;
        }
        let mut set = vec![false; self.len];
        for rel in &self.sel.has[id] {
            for (a, b) in set.iter_mut().zip(self.anchors(rel)) {
                *a |= b;
            }
        }
        let set: Rc<[bool]> = set.into();
        self.has.borrow_mut()[id] = Some(Rc::clone(&set));
        set
    }

    /// `relative_selector_anchors`: from the rightmost compound's elements, back through each
    /// combinator (`inverse_combinator_matches`) and the compound to its left.
    fn anchors(&self, rel: &[(Comb, usize)]) -> Vec<bool> {
        let mut caches = SelectorCaches::default();
        let mut ctx = context(&mut caches);
        let mut targets: Option<Vec<bool>> = None;
        for &(comb, ci) in rel {
            let mut t = targets.take().unwrap_or_else(|| vec![true; self.len]);
            let mut any = false;
            for &n in &self.order {
                let k = n.index();
                t[k] = t[k] && self.compound(ci, n, &mut ctx);
                any |= t[k];
            }
            if !any {
                return vec![false; self.len];
            }
            targets = Some(self.inverse(comb, &t));
        }
        targets.unwrap_or_else(|| vec![false; self.len])
    }

    /// `inverse_combinator_matches`: the elements `targets` are in relation `comb` to. Walks
    /// stop at an element already found, as Calibre's do, so each is linear.
    fn inverse(&self, comb: Comb, targets: &[bool]) -> Vec<bool> {
        let mut out = vec![false; self.len];
        for &n in self.order.iter().filter(|n| targets[n.index()]) {
            match comb {
                Comb::Descendant => {
                    let mut at = self.dom.parent(n);
                    while let Some(a) = at {
                        if out[a.index()] {
                            break;
                        }
                        out[a.index()] = true;
                        at = self.dom.parent(a);
                    }
                }
                Comb::Child => {
                    if let Some(p) = self.dom.parent(n) {
                        out[p.index()] = true;
                    }
                }
                Comb::Next => {
                    if let Some(s) = self.dom.prev_sibling(n) {
                        out[s.index()] = true;
                    }
                }
                Comb::Later => {
                    let mut at = self.dom.prev_sibling(n);
                    while let Some(s) = at {
                        if out[s.index()] {
                            break;
                        }
                        out[s.index()] = true;
                        at = self.dom.prev_sibling(s);
                    }
                }
            }
        }
        out
    }

    /// For each element, how many of its siblings before and after it share its key; an
    /// element of no key counts nothing.
    fn counts<K: Eq + std::hash::Hash + Clone>(&self, key: impl Fn(NodeId) -> Option<K>) -> Counts {
        let mut before = vec![0u32; self.len];
        let mut after = vec![0u32; self.len];
        let mut seen: HashMap<K, u32> = HashMap::new();
        for &p in &self.order {
            let kids = self.dom.children(p);
            let keys: Vec<Option<K>> = kids.iter().map(|&c| key(c)).collect();
            seen.clear();
            for (&c, k) in kids.iter().zip(&keys) {
                if let Some(k) = k {
                    let n = seen.entry(k.clone()).or_insert(0);
                    before[c.index()] = *n;
                    *n += 1;
                }
            }
            for (&c, k) in kids.iter().zip(&keys) {
                if let Some(total) = k.as_ref().and_then(|k| seen.get(k)) {
                    after[c.index()] = total - before[c.index()] - 1;
                }
            }
        }
        Counts { before, after }
    }

    /// `sibling_count(…, same_type=True)` for every element.
    fn of_type_counts(&self) -> Rc<Counts> {
        let cached = self.of_type.borrow().clone();
        if let Some(c) = cached {
            return c;
        }
        let c = Rc::new(self.counts(|n| Some(self.type_key(n))));
        *self.of_type.borrow_mut() = Some(Rc::clone(&c));
        c
    }

    /// `sibling_count(…, matching=of)` for every element of the list.
    fn of_list_counts(&self, list: usize) -> Rc<Counts> {
        let cached = self.of_list.borrow().get(&list).cloned();
        if let Some(c) = cached {
            return c;
        }
        let set = self.list_set(list);
        let c = Rc::new(self.counts(|n| set[n.index()].then_some(())));
        self.of_list.borrow_mut().insert(list, Rc::clone(&c));
        c
    }

    /// The element whose `lang` attribute an element's language comes from (`lang_map`: the
    /// nearest with a non-empty `lang`; `xml:lang` is read and then overwritten).
    fn lang_source(&self, n: NodeId) -> Option<NodeId> {
        let cached = self.lang_src.borrow().clone();
        let src = match cached {
            Some(src) => src,
            None => {
                let mut src: Vec<Option<NodeId>> = vec![None; self.len];
                for &e in &self.order {
                    src[e.index()] = if self.dom.attr(e, "lang").is_some_and(|v| !v.is_empty()) {
                        Some(e)
                    } else {
                        self.dom.parent(e).and_then(|p| src[p.index()])
                    };
                }
                let src: Rc<[Option<NodeId>]> = src.into();
                *self.lang_src.borrow_mut() = Some(Rc::clone(&src));
                src
            }
        };
        src.get(n.index()).copied().flatten()
    }

    fn filter(&self, f: usize, n: NodeId) -> bool {
        let dom = self.dom;
        let k = n.index();
        match &self.sel.filters[f] {
            Filter::All => true,
            Filter::Never => false,
            Filter::AnyLink => {
                matches!(self.type_key(n).as_str(), "a" | "area" | "link")
                    && dom.attr(n, "href").is_some()
            }
            Filter::Lang(want) => {
                let Some(src) = self.lang_source(n) else {
                    return false;
                };
                let cached = self.lang_hits.borrow().get(&(f, src)).copied();
                if let Some(hit) = cached {
                    return hit;
                }
                let hit = lang_matches(dom.attr(src, "lang").unwrap_or(""), want);
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
                let Some(parent) = dom.parent(n) else {
                    return false;
                };
                let count = if let Some(list) = *of {
                    if !self.list_set(list)[k] {
                        return false;
                    }
                    let c = self.of_list_counts(list);
                    if *last { c.after[k] } else { c.before[k] }
                } else if *of_type {
                    let c = self.of_type_counts();
                    if *last { c.after[k] } else { c.before[k] }
                } else {
                    let pos = dom.index_in_parent(n);
                    let count = if *last {
                        dom.children(parent).len() - pos - 1
                    } else {
                        pos
                    };
                    u32::try_from(count).unwrap_or(u32::MAX)
                };
                nth_ok(*a, *b, i128::from(count) + 1)
            }
            Filter::Only { of_type } => {
                let Some(parent) = dom.parent(n) else {
                    return false;
                };
                if *of_type {
                    let c = self.of_type_counts();
                    c.before[k] == 0 && c.after[k] == 0
                } else {
                    dom.children(parent).len() == 1
                }
            }
            Filter::In(list) => self.list_set(*list)[k],
            Filter::NotIn(list) => !self.list_set(*list)[k],
            Filter::Has(h) => self.has_set(*h)[k],
        }
    }
}

// --- The `selectors` crate over `Dom` ------------------------------------------------------------

/// The `selectors` crate's view of Calibre's names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Impl;

/// A name or value in a compound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Text(String);

impl From<&str> for Text {
    fn from(s: &str) -> Text {
        Text(s.to_owned())
    }
}

impl ToCss for Text {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        serialize_identifier(&self.0, dest)
    }
}

impl PrecomputedHash for Text {
    /// FNV-1a: the crate's bloom filter, which this is for, is not used.
    fn precomputed_hash(&self) -> u32 {
        self.0.bytes().fold(0x811c_9dc5, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        })
    }
}

/// `:-deckle(i)`: [`Compiled::filters`]`[i]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pc(u32);

impl ToCss for Pc {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        write!(dest, ":-deckle({})", self.0)
    }
}

impl selectors::parser::NonTSPseudoClass for Pc {
    type Impl = Impl;

    fn is_active_or_hover(&self) -> bool {
        false
    }

    fn is_user_action_state(&self) -> bool {
        false
    }
}

/// No pseudo-element reaches the crate: [`Builder::compound`] turns them into filters.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pe {}

impl ToCss for Pe {
    fn to_css<W: fmt::Write>(&self, _dest: &mut W) -> fmt::Result {
        match *self {}
    }
}

impl selectors::parser::PseudoElement for Pe {
    type Impl = Impl;
}

impl selectors::parser::SelectorImpl for Impl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = Text;
    type Identifier = Text;
    type LocalName = Text;
    type NamespaceUrl = Text;
    type NamespacePrefix = Text;
    type BorrowedNamespaceUrl = Text;
    type BorrowedLocalName = Text;
    type NonTSPseudoClass = Pc;
    type PseudoElement = Pe;
}

/// Reads the compounds [`Builder`] writes.
struct CompoundParser;

impl<'i> selectors::parser::Parser<'i> for CompoundParser {
    type Impl = Impl;
    type Error = SelectorParseErrorKind<'i>;

    fn parse_non_ts_functional_pseudo_class<'t>(
        &self,
        name: CowRcStr<'i>,
        parser: &mut CssParser<'i, 't>,
        _after_part: bool,
    ) -> Result<Pc, ParseError<'i, Self::Error>> {
        if &*name == "-deckle"
            && let Ok(i) = u32::try_from(parser.expect_integer()?)
        {
            return Ok(Pc(i));
        }
        Err(
            parser.new_custom_error(SelectorParseErrorKind::UnsupportedPseudoClassOrElement(
                name,
            )),
        )
    }
}

/// An element of the document, as the crate matches it.
#[derive(Clone)]
struct El<'a, 'd> {
    e: &'a Engine<'d>,
    n: NodeId,
}

impl fmt::Debug for El<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("El").field(&self.n).finish()
    }
}

impl El<'_, '_> {
    fn at(&self, n: Option<NodeId>) -> Option<Self> {
        n.map(|n| El { e: self.e, n })
    }
}

/// `attrib_map`'s tests (`select_equals`, `select_includes` …, and with `i`,
/// `case_insensitive_attribute_matchers`): an empty value matches nothing but by `=`.
fn attr_value_matches(op: &AttrSelectorOperation<&Text>, val: &str) -> bool {
    let AttrSelectorOperation::WithValue {
        operator,
        case_sensitivity,
        value,
    } = op
    else {
        return true;
    };
    let (val, value): (Cow<'_, str>, Cow<'_, str>) = match case_sensitivity {
        CaseSensitivity::AsciiCaseInsensitive => (
            Cow::Owned(val.to_ascii_lowercase()),
            Cow::Owned(value.0.to_ascii_lowercase()),
        ),
        CaseSensitivity::CaseSensitive => (Cow::Borrowed(val), Cow::Borrowed(&value.0)),
    };
    let (val, value) = (val.as_ref(), value.as_ref());
    match operator {
        AttrSelectorOperator::Equal => val == value,
        // is_non_whitespace(value) and value in val.split()
        AttrSelectorOperator::Includes => {
            !value.is_empty() && !value.contains(is_ws) && py_split(val).any(|t| t == value)
        }
        AttrSelectorOperator::DashMatch => {
            !value.is_empty()
                && (val == value || val.strip_prefix(value).is_some_and(|r| r.starts_with('-')))
        }
        AttrSelectorOperator::Prefix => !value.is_empty() && val.starts_with(value),
        AttrSelectorOperator::Suffix => !value.is_empty() && val.ends_with(value),
        AttrSelectorOperator::Substring => !value.is_empty() && val.contains(value),
    }
}

impl selectors::Element for El<'_, '_> {
    type Impl = Impl;

    fn opaque(&self) -> OpaqueElement {
        // An address only to tell elements apart; never read.
        let ptr = NonNull::new(std::ptr::without_provenance_mut::<()>(self.n.index() + 1));
        OpaqueElement::from_non_null_ptr(ptr.unwrap_or(NonNull::dangling()))
    }

    fn parent_element(&self) -> Option<Self> {
        self.at(self.e.dom.parent(self.n))
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.at(self.e.dom.prev_sibling(self.n))
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.at(self.e.dom.next_sibling(self.n))
    }

    fn first_element_child(&self) -> Option<Self> {
        self.at(self.e.dom.children(self.n).first().copied())
    }

    /// False, so the crate compares attribute values case-sensitively, as Calibre does, `type`
    /// and `lang` included.
    fn is_html_element_in_html_document(&self) -> bool {
        false
    }

    fn has_local_name(&self, name: &Text) -> bool {
        self.e.type_is(self.n, &name.0)
    }

    /// Calibre ignores namespaces.
    fn has_namespace(&self, _ns: &Text) -> bool {
        true
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.e.type_key(self.n) == self.e.type_key(other.n)
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&Text>,
        local_name: &Text,
        operation: &AttrSelectorOperation<&Text>,
    ) -> bool {
        self.e.dom.attrs(self.n).any(|(q, ns, v)| {
            self.e.attr_is(q, ns, &local_name.0) && attr_value_matches(operation, v)
        })
    }

    fn match_non_ts_pseudo_class(&self, pc: &Pc, _ctx: &mut MatchingContext<'_, Impl>) -> bool {
        self.e.filter(pc.0 as usize, self.n)
    }

    fn match_pseudo_element(&self, pe: &Pe, _ctx: &mut MatchingContext<'_, Impl>) -> bool {
        match *pe {}
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        matches!(self.e.type_key(self.n).as_str(), "a" | "area" | "link")
            && self.e.dom.attr(self.n, "href").is_some()
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    /// `id_map`: the whole `id` value, compared as the quirks-mode context says (ASCII
    /// case-insensitively).
    fn has_id(&self, id: &Text, case_sensitivity: CaseSensitivity) -> bool {
        self.e
            .dom
            .attr(self.n, "id")
            .is_some_and(|v| case_sensitivity.eq(v.as_bytes(), id.0.as_bytes()))
    }

    /// `class_map`: the `class` value split as Python splits it.
    fn has_class(&self, name: &Text, case_sensitivity: CaseSensitivity) -> bool {
        self.e.dom.attr(self.n, "class").is_some_and(|v| {
            py_split(v).any(|c| case_sensitivity.eq(c.as_bytes(), name.0.as_bytes()))
        })
    }

    fn has_custom_state(&self, _name: &Text) -> bool {
        false
    }

    fn imported_part(&self, _name: &Text) -> Option<Text> {
        None
    }

    fn is_part(&self, _name: &Text) -> bool {
        false
    }

    /// `is_empty`: no child element and no text (the tree keeps no comments).
    fn is_empty(&self) -> bool {
        self.e.dom.children(self.n).is_empty() && self.e.dom.text(self.n).is_empty()
    }

    fn is_root(&self) -> bool {
        self.n == self.e.dom.root()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}
