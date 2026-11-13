// SPDX-License-Identifier: GPL-3.0-only
//! A book's stylesheets as the rules Calibre 9.18's Stylizer holds (phase 2 plan, Tasks 5 and
//! 6).
//!
//! Calibre parses CSS with css_parser 1.1.1 and keeps what it reads in that library's form:
//! property names in lower case, values written back by its serializer (`.5em` as `0.5em`,
//! strings in double quotes), one rule for each selector of a list. [`parse_sheet`] reads a
//! stylesheet into that form with `cssparser`'s tokenizer and rule parser; [`serialize_value`]
//! writes a value as css_parser writes it; [`normalize`] expands shorthands and font-size
//! keywords as `StylizerRules.flatten_style` does. The rules are ported from Calibre's
//! `ebooks/oeb/stylizer.py` (`StylizerRules`, `media_ok`), `ebooks/oeb/base.py` (`_parse_css`,
//! `abshref`), `ebooks/conversion/preprocess.py` (`CSSPreProcessor`),
//! `ebooks/oeb/normalize_css.py`, `tinycss/media3.py` and `tinycss/fonts3.py`;
//! css_parser's own behaviour, which is not in Calibre's source, was read from Calibre 9.18 and
//! is described where it is ported. Where the port differs on purpose, `DIVERGENCES.md` says so.
//!
//! Hostile input: every count the input controls is bounded by
//! [`deckle_limits::MAX_STYLE_RULES`], [`deckle_limits::MAX_STYLE_DECLS`] or a nesting limit,
//! charged as the text is read; the selectors of a list share one copy of their declarations,
//! and no work is repeated per declaration or per import.

use std::collections::HashMap;
use std::sync::Arc;

mod budget;
mod defaults;
mod font_shorthand;
mod media;
mod normalize;
mod selector;
mod sheet;
mod value;

pub use defaults::{DEFAULTS, INHERITED};
pub use media::media_ok;
pub use normalize::{DEFAULT_PROFILE, Profile, normalize};
pub use sheet::parse_sheet;
pub use value::{parse_style_attr, serialize_value};

/// A rule's declarations: property names in lower case, each once, in the order css_parser
/// lists them (where the name was last declared), with the value it keeps (the last
/// `!important` one, else the last).
pub type Decls = Vec<(String, Decl)>;

/// A declaration's value, as css_parser writes it, and its priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    pub value: String,
    pub important: bool,
}

/// Which stylesheet a rule comes from: Calibre's `html.css` ([`HTML_CSS`]) or the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    UserAgent,
    Book,
}

/// One selector and its declarations (Calibre's `StylizerRules.flatten_rule`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub origin: Origin,
    /// The selector as css_parser writes it (`h3 + p`, `:is(p, li)`).
    pub selector: String,
    /// The declarations of its CSS rule, shared by the selectors of one list (Calibre's
    /// `flatten_style` runs once per CSS rule).
    pub decls: Arc<Decls>,
    /// The position of its CSS rule among all of a file's stylesheets, imported rules first:
    /// the selectors of one list share it, and it grows in source order. It starts at the
    /// `first_index` given to [`parse_sheet`].
    pub index: u32,
}

/// A stylesheet with its imports inlined.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sheet {
    /// Style rules, imported ones first (`css_parser.resolveImports`), `@media` rules that
    /// apply flattened into them.
    pub rules: Vec<Rule>,
    /// `@font-face` rules of more than one property (`StylizerRules.flatten_rule`).
    pub font_face: Vec<Decls>,
    /// This sheet's own `@page` rules. Imported sheets' are dropped, as Calibre drops them from
    /// every linked sheet (`base.py` `_parse_css`); a caller reading a linked sheet drops these
    /// too, and keeps them for a `<style>` element.
    pub page: Vec<Decls>,
    /// The [`Rule::index`] after this sheet's last rule: the `first_index` for the file's next
    /// sheet.
    pub next_index: u32,
}

impl Sheet {
    /// [`normalize`]s the style rules' and `@page` rules' declarations, as `flatten_style`
    /// does (`@font-face` rules stay as read). Declarations the selectors of a list share are
    /// normalized once and stay shared.
    pub fn normalize(&mut self, profile: &Profile) {
        // Keyed by the address of the shared declarations; the old `Arc` is kept in the map so
        // the address is not reused while the map is alive.
        let mut done: HashMap<*const Decls, (Arc<Decls>, Arc<Decls>)> = HashMap::new();
        for rule in &mut self.rules {
            // Declarations no other rule holds are normalized where they are.
            if let Some(decls) = Arc::get_mut(&mut rule.decls) {
                *decls = normalize(std::mem::take(decls), profile);
                continue;
            }
            let key = Arc::as_ptr(&rule.decls);
            let normalized = match done.get(&key) {
                Some((_, n)) => Arc::clone(n),
                None => {
                    let n = Arc::new(normalize(Decls::clone(&rule.decls), profile));
                    done.insert(key, (Arc::clone(&rule.decls), Arc::clone(&n)));
                    n
                }
            };
            rule.decls = normalized;
        }
        for page in &mut self.page {
            *page = normalize(std::mem::take(page), profile);
        }
    }
}

/// Calibre's user-agent stylesheet, `resources/templates/html.css` of calibre 9.18.0,
/// verbatim (`NOTICE`). Calibre reads it without its preprocessor (`html_css_stylesheet`).
pub const HTML_CSS: &str = include_str!("html.css");

/// Why a stylesheet was not read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// It and its imports pass [`deckle_limits::MAX_STYLE_RULES`] rules or
    /// [`deckle_limits::MAX_STYLE_DECLS`] declaration units: the limit passed.
    #[error("this book file's stylesheets hold more than {0} CSS rules or declarations")]
    TooLarge(usize),
}
