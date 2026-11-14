// SPDX-License-Identifier: GPL-3.0-only
//! Selector matching as calibre 9.18's own engine, `css_selectors`, does it for the Stylizer
//! (phase 2 plan, Task 7), departures from the CSS specification included.
//!
//! The Stylizer hands each rule's selector text, as css_parser wrote it, to
//! `Select(tree, ignore_inappropriate_pseudo_classes=True)` (`ebooks/oeb/stylizer.py`
//! `Stylizer.__init__`). [`parse_selector`] reads it with a port of `css_selectors/parser.py`'s
//! tokenizer and grammar (`tokens.rs`, `parse.rs`), refuses what `Select` would refuse
//! ([`Parsed::Dropped`]), works out the specificity css_parser gave the rule, and the
//! pseudo-class name `stylizer.py` looks for in the text (`compile.rs`). [`select`] then
//! evaluates it as `css_selectors/select.py` does (`eval.rs`): compound by compound, a
//! combinator mapping one set of elements to the next, `:has()` from right to left
//! (`relative_selector_anchors`), over a [`DocIndex`] of the document built once
//! (`index.rs`), as `select.py`'s maps of names, classes and ids are.
//!
//! Calibre's departures, each ported where its rule is (file and function named there):
//! element and attribute names match ASCII case-insensitively and namespaces are ignored;
//! class and id names match ASCII case-insensitively, attribute values case-sensitively;
//! `:root` and `:scope` match the root whatever comes before them in their compound; `:hover`,
//! `::before` and the rest of `INAPPROPRIATE_PSEUDO_CLASSES` match every element; `:lang()`
//! reads only `lang`, not `xml:lang`; `:nth-*()` arguments are read by `parse_series`, so `ODD`
//! is refused and `2 n` is `2n`; class lists split on Python's white space.
//!
//! Hostile input: a selector longer than [`MAX_SELECTOR_BYTES`] is dropped unread; the reading
//! is iterative but for selector lists, which stop at `MAX_NESTING` levels; a selector of more
//! than [`MAX_PARTS`] parts is dropped. Evaluation starts each compound from the smallest set
//! of the index that holds its matches (a name, class or id), and moves through a combinator
//! from the elements already matched, never searching per element; every element tested is
//! charged to the document's [`DocIndex`], and past
//! [`deckle_limits::MAX_SELECTOR_WORK`] [`select`] gives [`TooLarge`]. Where the port and
//! Calibre differ on purpose, `DIVERGENCES.md` says so.

mod compile;
mod eval;
mod index;
mod parse;
mod tokens;

use std::fmt;
use std::sync::Arc;

use deckle_limits::MAX_SELECTOR_WORK;

use crate::dom::{Dom, NodeId};
use compile::{Builder, Compiled, pseudo_name};
pub use index::DocIndex;
use parse::{Reader, Spec, ZERO, add, spec_complex};
use tokens::{Stream, tokenize};

/// The most parts of compound selectors one selector holds (type, id, class, attribute and
/// pseudo-class selectors and pseudo-elements, as written, those inside `:is()`, `:not()`,
/// `:has()` and `of` included); past it the selector is [`Parsed::Dropped`]
/// (`DIVERGENCES.md`). Matching costs at most this many tests per element.
const MAX_PARTS: usize = 256;

/// The longest selector text read, in bytes; a longer one is [`Parsed::Dropped`] before it is
/// read (`DIVERGENCES.md`).
pub const MAX_SELECTOR_BYTES: usize = 64 * 1024;

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

/// Matching one document's selectors took more than [`MAX_SELECTOR_WORK`] element tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("matching this book file's CSS selectors takes more than {MAX_SELECTOR_WORK} steps")]
pub struct TooLarge;

/// Reads `text`, a selector (or a list) as css_parser writes it.
pub fn parse_selector(text: &str) -> Parsed {
    if text.len() > MAX_SELECTOR_BYTES {
        return Parsed::Dropped;
    }
    match read(text) {
        Ok((compiled, specificity)) => Parsed::Ok {
            selector: Selector(Arc::new(compiled)),
            pseudo: pseudo_name(text),
            specificity,
        },
        Err(_) => Parsed::Dropped,
    }
}

/// The elements `sel` matches, in document order. `index` is `dom`'s, and is charged for the
/// work; once the work of all calls with it passes [`MAX_SELECTOR_WORK`], this and every later
/// call give [`TooLarge`].
pub fn select(sel: &Selector, dom: &Dom, index: &DocIndex) -> Result<Vec<NodeId>, TooLarge> {
    eval::select(&sel.0, dom, index)
}

/// The specificity of a selector [`parse_selector`] drops, where the reader gets far enough to
/// tell: css_parser still sorts the rule by it, though `Select` refuses it (a pseudo-element
/// such as `::marker`). `None` for a selector that does not read.
pub(crate) fn dropped_specificity(text: &str) -> Option<(u32, u32, u32)> {
    if text.len() > MAX_SELECTOR_BYTES {
        return None;
    }
    let toks = tokenize(text).ok()?;
    let mut stream = Stream::new(&toks);
    let mut reader = Reader { parts: 0 };
    let selectors = reader.group(&mut stream).ok()?;
    Some(specificity_of(&selectors))
}

fn specificity_of(selectors: &[(parse::Complex, Option<parse::PseudoElement>)]) -> Spec {
    selectors
        .iter()
        .map(|(c, pe)| add(spec_complex(c), if pe.is_some() { (0, 0, 1) } else { ZERO }))
        .max()
        .unwrap_or(ZERO)
}

fn read(text: &str) -> Result<(Compiled, Spec), Fail> {
    let toks = tokenize(text)?;
    let mut stream = Stream::new(&toks);
    let mut reader = Reader { parts: 0 };
    let selectors = reader.group(&mut stream)?;
    let specificity = specificity_of(&selectors);
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
