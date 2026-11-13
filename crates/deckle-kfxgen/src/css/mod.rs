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
//! [`deckle_limits::MAX_STYLE_RULES`] or a nesting limit, and no work is repeated per
//! declaration or per import.

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
    pub decls: Decls,
    /// The position of its CSS rule in the [`Sheet`], imported rules first: the selectors of
    /// one list share it, and it grows in source order. Calibre counts across all of a file's
    /// stylesheets; a caller orders rules of different sheets by the sheets' order first.
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
}

impl Sheet {
    /// Makes the sheet's `url()` values relative to the book's root, given the sheet's own
    /// href from the root: a linked sheet's path, or the document's for a `<style>` element
    /// (`flatcss.py` `CSSFlattener.__call__`, `stylizer.py` `Stylizer.__init__`, which both
    /// call `replaceUrls` with `Manifest.Item.abshref`).
    pub fn rebase_urls(&mut self, sheet_href: &str) {
        let map = |url: &str| value::abshref(sheet_href, url);
        let decls = self
            .rules
            .iter_mut()
            .map(|r| &mut r.decls)
            .chain(self.font_face.iter_mut())
            .chain(self.page.iter_mut());
        for decls in decls {
            for (_, d) in decls.iter_mut() {
                if d.value.contains("url(") {
                    d.value = value::map_urls(&d.value, &map);
                }
            }
        }
    }
}

/// Calibre's user-agent stylesheet, `resources/templates/html.css` of calibre 9.18.0,
/// verbatim (`NOTICE`). Calibre reads it without its preprocessor (`html_css_stylesheet`).
pub const HTML_CSS: &str = include_str!("html.css");

/// Why a stylesheet was not read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Its rules, with its imports and selector lists counted out, pass
    /// [`deckle_limits::MAX_STYLE_RULES`], or the declarations they repeat pass 20 times it
    /// (a long declaration or selector counting once for each 64 bytes).
    #[error("this book file's stylesheets hold more than {0} CSS rules or declarations")]
    TooLarge(usize),
}
