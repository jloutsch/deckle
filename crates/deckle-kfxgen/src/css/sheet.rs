// SPDX-License-Identifier: GPL-3.0-only
//! A stylesheet's text to the rules Calibre's Stylizer holds (`stylizer.py`
//! `StylizerRules.__init__` and `flatten_rule`), with its imports inlined as Calibre reads a
//! book's sheets (`base.py` `_parse_css`, css_parser's `resolveImports`).

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::LazyLock;

use cssparser::{
    AtRuleParser, CowRcStr, ParseError, Parser, ParserInput, ParserState, QualifiedRuleParser,
    RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};
use deckle_limits::MAX_STYLE_RULES;
use regex::{Captures, Regex};

use super::media::{media_list_valid, media_ok};
use super::selector::parse_list;
use super::value::{declarations, map_urls, rel_join};
use super::{Decls, Error, Origin, Rule, Sheet};

/// How deep imports are followed: an `@import` in the eighth imported sheet down is dropped
/// (DIVERGENCES.md, "Imports more than 8 deep").
const MAX_IMPORT_DEPTH: usize = 8;

/// Declarations the rules may hold in all, counting each selector's copy of its list's, and a
/// declaration or selector once for each [`TEXT_UNIT`] bytes of it: copies of one long value
/// or selector are what make the output outgrow the input.
const MAX_STYLE_DECLS: usize = MAX_STYLE_RULES * 20;

/// Bytes of a declaration or selector that count as one more declaration.
const TEXT_UNIT: usize = 64;

/// What a copy of these declarations counts toward [`MAX_STYLE_DECLS`].
fn decl_cost(decls: &Decls) -> usize {
    decls
        .iter()
        .map(|(k, d)| 1 + (k.len() + d.value.len()) / TEXT_UNIT)
        .fold(0, usize::saturating_add)
}

/// `preprocess.py` `CSSPreProcessor.MS_PAT`: a declaration of Microsoft Office's (`mso-`,
/// `panose-`, `text-underline`, `tab-interval`) after the start of a line, a `;` or a `{`.
/// Python's `re.MULTILINE | re.IGNORECASE`, `.` not matching a line feed.
static MS_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?mi)(^|;|\{)\s*(?:mso-|panose-|text-underline|tab-interval).+?($|;|\})")
        .expect("MS_PAT")
});

/// `CSSPreProcessor.__call__` with `add_namespace` off, as both the Stylizer (a `<style>`
/// element) and `_parse_css` (a linked sheet) call it: each match replaced by its start and,
/// unless that is `;`, its end. The match after one starts where it ended, so in
/// `{ mso-a: 1; panose-1: 2 }` the `;` ending the first is gone and `panose-1` stays, as
/// `css_quirks` shows.
fn preprocess(text: &str) -> Cow<'_, str> {
    MS_PAT.replace_all(text, |c: &Captures<'_>| {
        let start = c.get(1).map_or("", |m| m.as_str());
        let end = c.get(2).map_or("", |m| m.as_str());
        format!("{start}{}", if end == ";" { "" } else { end })
    })
}

/// Reads `text` into a [`Sheet`].
///
/// `origin` is [`Origin::UserAgent`] for [`super::HTML_CSS`], which Calibre reads without its
/// preprocessor, and [`Origin::Book`] for a book's sheet. `import` returns the text of the
/// sheet an `@import` names, given its address relative to this sheet (nested imports are
/// resolved against the importing sheet and passed relative to this one), or `None` when the
/// book has no such stylesheet. Each address is asked for once.
///
/// Imported rules come before the sheet's own, nested ones first. An import whose media query
/// Calibre rejects is skipped, an import more than [`MAX_IMPORT_DEPTH`] levels down is
/// dropped, and an import whose chain comes back to a sheet already on it is dropped whole
/// (DIVERGENCES.md, "Import cycles"). `url()`s in imported sheets are made relative to this
/// one; [`Sheet::rebase_urls`] makes them book-absolute.
///
/// [`Error::TooLarge`] past [`MAX_STYLE_RULES`] rules (selectors of a list, `@font-face`,
/// `@page` and imports followed, counted one each) or 20 times as many declarations (each
/// selector's copy counted, and a long declaration or selector once for each 64 bytes).
pub fn parse_sheet(
    text: &str,
    origin: Origin,
    import: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<Sheet, Error> {
    let text = match origin {
        Origin::Book => preprocess(text),
        Origin::UserAgent => Cow::Borrowed(text),
    };
    let own = parse_text(&text, None);
    let mut r = Resolver {
        import,
        cache: HashMap::new(),
        origin,
        out: Sheet::default(),
        rules: 0,
        decls: 0,
        index: 0,
    };
    if own.over {
        return Err(r.too_large());
    }
    for href in &own.imports {
        let mark = (r.out.rules.len(), r.out.font_face.len());
        let mut path = Vec::new();
        if r.visit(href, 1, &mut path)? == Visit::Cycle {
            r.out.rules.truncate(mark.0);
            r.out.font_face.truncate(mark.1);
        }
    }
    r.emit(&own, true)?;
    Ok(r.out)
}

/// What a sheet holds before its imports are inlined.
#[derive(Default)]
struct Parsed {
    /// The addresses of its imports that apply, relative to the top sheet.
    imports: Vec<String>,
    items: Vec<Item>,
    /// It holds more than [`MAX_STYLE_RULES`] rules; reading stopped there.
    over: bool,
}

enum Item {
    /// A style rule: its selectors, as css_parser writes them, and its declarations.
    Style(Vec<String>, Decls),
    FontFace(Decls),
    Page(Decls),
}

#[derive(Debug, PartialEq, Eq)]
enum Visit {
    Done,
    Missing,
    Cycle,
}

struct Resolver<'a> {
    import: &'a mut dyn FnMut(&str) -> Option<String>,
    /// Each imported address read once: `None` when the book has no such sheet.
    cache: HashMap<String, Option<Rc<Parsed>>>,
    origin: Origin,
    out: Sheet,
    /// Rules and imports counted so far, kept or not.
    rules: usize,
    decls: usize,
    index: u32,
}

impl Resolver<'_> {
    fn too_large(&self) -> Error {
        Error::TooLarge(if self.decls > MAX_STYLE_DECLS {
            MAX_STYLE_DECLS
        } else {
            MAX_STYLE_RULES
        })
    }

    fn charge(&mut self, rules: usize, decls: usize) -> Result<(), Error> {
        self.rules = self.rules.saturating_add(rules);
        self.decls = self.decls.saturating_add(decls);
        if self.rules > MAX_STYLE_RULES || self.decls > MAX_STYLE_DECLS {
            return Err(self.too_large());
        }
        Ok(())
    }

    /// Follows the import of `href`, `depth` levels down, with `path` the sheets above it.
    fn visit(&mut self, href: &str, depth: usize, path: &mut Vec<String>) -> Result<Visit, Error> {
        self.charge(1, 0)?;
        if path.iter().any(|p| p == href) {
            return Ok(Visit::Cycle);
        }
        if depth > MAX_IMPORT_DEPTH {
            return Ok(Visit::Missing);
        }
        let Some(parsed) = self.load(href) else {
            return Ok(Visit::Missing);
        };
        if parsed.over {
            return Err(self.too_large());
        }
        path.push(href.to_owned());
        for next in &parsed.imports {
            if self.visit(next, depth + 1, path)? == Visit::Cycle {
                path.pop();
                return Ok(Visit::Cycle);
            }
        }
        path.pop();
        self.emit(&parsed, false)?;
        Ok(Visit::Done)
    }

    fn load(&mut self, href: &str) -> Option<Rc<Parsed>> {
        if let Some(cached) = self.cache.get(href) {
            return cached.clone();
        }
        // An imported sheet is a linked one: Calibre reads it through the preprocessor.
        let parsed =
            (self.import)(href).map(|text| Rc::new(parse_text(&preprocess(&text), Some(href))));
        self.cache.insert(href.to_owned(), parsed.clone());
        parsed
    }

    /// Adds a sheet's own items to the output. `own` is the top sheet, the one whose `@page`
    /// rules are kept.
    fn emit(&mut self, parsed: &Parsed, own: bool) -> Result<(), Error> {
        for item in &parsed.items {
            match item {
                Item::Style(selectors, decls) => {
                    let cost = decl_cost(decls);
                    for selector in selectors {
                        self.charge(1, cost.saturating_add(selector.len() / TEXT_UNIT))?;
                        self.out.rules.push(Rule {
                            origin: self.origin,
                            selector: selector.clone(),
                            decls: decls.clone(),
                            index: self.index,
                        });
                    }
                }
                Item::FontFace(decls) => {
                    self.charge(1, decl_cost(decls))?;
                    self.out.font_face.push(decls.clone());
                }
                Item::Page(decls) => {
                    if own {
                        self.charge(1, decl_cost(decls))?;
                        self.out.page.push(decls.clone());
                    }
                }
            }
            self.index = self.index.saturating_add(1);
        }
        Ok(())
    }
}

/// Reads one sheet's text. `base` is its address relative to the top sheet, for an imported
/// one: its imports and `url()`s are made relative to the top sheet with it.
fn parse_text(text: &str, base: Option<&str>) -> Parsed {
    let mut input = ParserInput::new(text);
    let mut p = Parser::new(&mut input);
    let mut top = TopParser {
        namespaces: BTreeSet::new(),
        phase: Phase::Imports,
        base,
        parsed: Parsed::default(),
        count: 0,
    };
    let mut rules = StyleSheetParser::new(&mut p, &mut top);
    while rules.next().is_some() {
        if rules.parser.parsed.over {
            break;
        }
    }
    top.parsed
}

/// Where in a sheet the parser is: css_parser reads `@import` only before any other rule but
/// `@charset`, and `@namespace` only before style rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Imports,
    Namespaces,
    Body,
}

struct TopParser<'a> {
    namespaces: BTreeSet<String>,
    phase: Phase,
    base: Option<&'a str>,
    parsed: Parsed,
    /// Rules read so far.
    count: usize,
}

impl TopParser<'_> {
    fn add(&mut self, item: Item) {
        let n = match &item {
            Item::Style(s, _) => s.len(),
            _ => 1,
        };
        self.count = self.count.saturating_add(n);
        if self.count > MAX_STYLE_RULES {
            self.parsed.over = true;
        }
        let item = match (self.base, item) {
            (Some(base), Item::Style(s, d)) => Item::Style(s, rebase(d, base)),
            (Some(base), Item::FontFace(d)) => Item::FontFace(rebase(d, base)),
            (_, item) => item,
        };
        self.parsed.items.push(item);
    }

    /// A style rule's block.
    fn style_block(&mut self, selectors: Vec<String>, input: &mut Parser<'_, '_>) {
        let decls = declarations(input);
        self.add(Item::Style(selectors, decls));
    }
}

/// An imported sheet's `url()`s, made relative to the top sheet.
fn rebase(mut decls: Decls, base: &str) -> Decls {
    for (_, d) in &mut decls {
        if d.value.contains("url(") {
            d.value = map_urls(&d.value, &|u| rel_join(base, u));
        }
    }
    decls
}

/// The prelude of an at-rule Calibre reads.
enum Prelude {
    /// `@import`: the address and the media list as written.
    Import(String, String),
    /// `@namespace`, with its prefix.
    Namespace(Option<String>),
    /// `@media`, and whether its rules apply.
    Media(bool),
    FontFace,
    Page,
}

/// Consumes the rest of a prelude and returns its text.
fn rest<'i>(input: &mut Parser<'i, '_>) -> &'i str {
    let start = input.position();
    while input.next().is_ok() {}
    input.slice_from(start).trim()
}

fn parse_at_prelude<'i>(
    name: &str,
    input: &mut Parser<'i, '_>,
    phase: &mut Phase,
) -> Result<Prelude, ParseError<'i, ()>> {
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "import" if *phase == Phase::Imports => {
            let href = match input.next()?.clone() {
                Token::QuotedString(s) | Token::UnquotedUrl(s) => s.to_string(),
                Token::Function(f) if f.eq_ignore_ascii_case("url") => input
                    .parse_nested_block(|i| i.expect_string_cloned().map_err(ParseError::from))?
                    .to_string(),
                t => return Err(input.new_unexpected_token_error(t)),
            };
            Ok(Prelude::Import(href, rest(input).to_owned()))
        }
        "namespace" if *phase != Phase::Body => {
            let prefix = input.try_parse(|i| i.expect_ident_cloned()).ok();
            match input.next()?.clone() {
                Token::QuotedString(_) | Token::UnquotedUrl(_) => {}
                Token::Function(f) if f.eq_ignore_ascii_case("url") => {
                    input.parse_nested_block(|i| {
                        i.expect_string_cloned().map_err(ParseError::<()>::from)
                    })?;
                }
                t => return Err(input.new_unexpected_token_error(t)),
            }
            input.expect_exhausted()?;
            Ok(Prelude::Namespace(prefix.map(|p| p.to_string())))
        }
        "media" => {
            *phase = Phase::Body;
            let query = rest(input);
            Ok(Prelude::Media(media_list_valid(query) && media_ok(query)))
        }
        "font-face" => {
            *phase = Phase::Body;
            rest(input);
            Ok(Prelude::FontFace)
        }
        "page" => {
            *phase = Phase::Body;
            rest(input);
            Ok(Prelude::Page)
        }
        _ => Err(input.new_custom_error(())),
    }
}

/// Whether an `@import`'s media list lets it apply: none (css_parser's `all`), or one
/// css_parser reads and `media_ok` accepts.
fn import_media_ok(media: &str) -> bool {
    media.is_empty() || (media_list_valid(media) && media_ok(media))
}

impl<'i> AtRuleParser<'i> for TopParser<'_> {
    type Prelude = Prelude;
    type AtRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Prelude, ParseError<'i, ()>> {
        parse_at_prelude(&name, input, &mut self.phase)
    }

    fn rule_without_block(&mut self, prelude: Prelude, _start: &ParserState) -> Result<(), ()> {
        match prelude {
            Prelude::Import(href, media) => {
                self.count = self.count.saturating_add(1);
                if self.count > MAX_STYLE_RULES {
                    self.parsed.over = true;
                }
                if import_media_ok(&media) {
                    let href = match self.base {
                        Some(base) => rel_join(base, &href),
                        None => href,
                    };
                    self.parsed.imports.push(href);
                }
                Ok(())
            }
            Prelude::Namespace(prefix) => {
                self.phase = Phase::Namespaces;
                self.namespaces.extend(prefix);
                Ok(())
            }
            _ => Err(()),
        }
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        match prelude {
            Prelude::Media(true) => {
                let mut inner = MediaParser { top: self };
                let mut rules = RuleBodyParser::new(input, &mut inner);
                while rules.next().is_some() {
                    if rules.parser.top.parsed.over {
                        break;
                    }
                }
                Ok(())
            }
            // The rules of a query Calibre rejects are not read.
            Prelude::Media(false) => Ok(()),
            Prelude::FontFace => {
                let decls = declarations(input);
                // `flatten_rule`: an `@font-face` of one property (MS Word writes them with
                // only `font-family`) is ignored.
                if decls.len() > 1 {
                    self.add(Item::FontFace(decls));
                }
                Ok(())
            }
            Prelude::Page => {
                let decls = declarations(input);
                self.add(Item::Page(decls));
                Ok(())
            }
            Prelude::Import(..) | Prelude::Namespace(_) => Err(input.new_custom_error(())),
        }
    }
}

impl<'i> QualifiedRuleParser<'i> for TopParser<'_> {
    type Prelude = Vec<String>;
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Vec<String>, ParseError<'i, ()>> {
        self.phase = Phase::Body;
        parse_list(input, &self.namespaces).map_err(|()| input.new_custom_error(()))
    }

    fn parse_block<'t>(
        &mut self,
        selectors: Vec<String>,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        self.style_block(selectors, input);
        Ok(())
    }
}

/// The rules inside an `@media` that applies: style rules and `@page`. css_parser drops an
/// `@font-face` there; a nested `@media` holds nothing Calibre reads (`StylizerRules`
/// flattens one level).
struct MediaParser<'a, 'b> {
    top: &'a mut TopParser<'b>,
}

impl<'i> AtRuleParser<'i> for MediaParser<'_, '_> {
    type Prelude = ();
    type AtRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        if name.eq_ignore_ascii_case("page") {
            rest(input);
            Ok(())
        } else {
            Err(input.new_custom_error(()))
        }
    }

    fn parse_block<'t>(
        &mut self,
        _prelude: (),
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        let decls = declarations(input);
        self.top.add(Item::Page(decls));
        Ok(())
    }
}

impl<'i> QualifiedRuleParser<'i> for MediaParser<'_, '_> {
    type Prelude = Vec<String>;
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Vec<String>, ParseError<'i, ()>> {
        parse_list(input, &self.top.namespaces).map_err(|()| input.new_custom_error(()))
    }

    fn parse_block<'t>(
        &mut self,
        selectors: Vec<String>,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<(), ParseError<'i, ()>> {
        self.top.style_block(selectors, input);
        Ok(())
    }
}

impl<'i> cssparser::DeclarationParser<'i> for MediaParser<'_, '_> {
    type Declaration = ();
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, (), ()> for MediaParser<'_, '_> {
    fn parse_declarations(&self) -> bool {
        false
    }
    fn parse_qualified(&self) -> bool {
        true
    }
}
