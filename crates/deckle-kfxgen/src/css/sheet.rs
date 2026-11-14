// SPDX-License-Identifier: GPL-3.0-only
//! A stylesheet's text to the rules Calibre's Stylizer holds (`stylizer.py`
//! `StylizerRules.__init__` and `flatten_rule`), with its imports inlined as Calibre reads a
//! book's sheets (`base.py` `_parse_css`, css_parser's `resolveImports`).

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use cssparser::{
    AtRuleParser, CowRcStr, ParseError, Parser, ParserInput, ParserState, QualifiedRuleParser,
    RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};
use regex::{Captures, Regex};

use super::budget::{Budget, TEXT_UNIT, decl_units};
use super::media::{media_list_valid, media_ok};
use super::selector::parse_list;
use super::value::{abshref, declarations, import_key, rel_join};
use super::{Decls, Error, Origin, Rule, Sheet};

/// How deep imports are followed: an `@import` in the eighth imported sheet down is dropped
/// (DIVERGENCES.md, "Imports more than 8 deep").
const MAX_IMPORT_DEPTH: usize = 8;

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
/// preprocessor and whose addresses it leaves alone, and [`Origin::Book`] for a book's sheet.
/// `href` is the sheet's address from the book's root: a linked sheet's own, or the
/// document's for a `<style>` element; each `url()` is made book-absolute from it as it is
/// read (`flatcss.py` `CSSFlattener.__call__` and `Stylizer.__init__` call `replaceUrls` with
/// `Manifest.Item.abshref`). `first_index` is the [`Rule::index`] of the sheet's first rule:
/// Calibre counts one index across all of a file's sheets, so a caller passes the previous
/// sheet's [`Sheet::next_index`] (0 for the first).
///
/// `import` returns the text of the sheet an `@import` names, given its address relative to
/// this sheet (nested imports are resolved against the importing sheet and passed relative
/// to this one, `./` and `..` resolved), or `None` when the book has no such stylesheet. Each
/// address is asked for once.
///
/// Imported rules come before the sheet's own, nested ones first. An import whose media query
/// Calibre rejects is skipped, an import more than [`MAX_IMPORT_DEPTH`] levels down is
/// dropped, and an import whose chain comes back to a sheet already on it is dropped whole
/// (DIVERGENCES.md, "Import cycles").
///
/// [`Error::TooLarge`] past [`deckle_limits::MAX_STYLE_RULES`] rules or
/// [`deckle_limits::MAX_STYLE_DECLS`] declaration units, charged as they are read: this text
/// and each imported one, every rule, `@font-face`, `@page` and `@import` read (kept or not),
/// and every declaration.
pub fn parse_sheet(
    text: &str,
    origin: Origin,
    href: &str,
    first_index: u32,
    import: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<Sheet, Error> {
    parse_sheet_in(&Budget::default(), text, origin, href, first_index, import)
}

/// [`parse_sheet`], charging `budget`: the one budget of all of a book file's sheets
/// (`book::file_sheets`), so the limits hold for them together.
pub(crate) fn parse_sheet_in(
    budget: &Budget,
    text: &str,
    origin: Origin,
    href: &str,
    first_index: u32,
    import: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<Sheet, Error> {
    if !budget.text(text.len()) {
        return Err(budget.error());
    }
    let text = match origin {
        Origin::Book => preprocess(text),
        Origin::UserAgent => Cow::Borrowed(text),
    };
    let urls = match origin {
        Origin::Book => Urls::Book { href, base: None },
        Origin::UserAgent => Urls::Keep,
    };
    let own = parse_text(&text, urls, budget);
    drop(text);
    if budget.over() {
        return Err(budget.error());
    }
    let mut r = Resolver {
        import,
        cache: HashMap::new(),
        budget,
        origin,
        href,
        out: Sheet::default(),
        index: first_index,
    };
    for key in &own.imports {
        let mark = (r.out.rules.len(), r.out.font_face.len());
        let mut path = Vec::new();
        if r.visit(key, 1, &mut path)? == Visit::Cycle {
            r.out.rules.truncate(mark.0);
            r.out.font_face.truncate(mark.1);
        }
    }
    r.emit(own.items.into_iter().map(Cow::Owned), false, true)?;
    r.out.next_index = r.index;
    Ok(r.out)
}

/// How a sheet's `url()`s are written: as they are ([`super::HTML_CSS`]), or book-absolute
/// from the top sheet's `href`, an imported sheet's first made relative to the top sheet
/// from its address `base` (css_parser's `resolveImports`).
#[derive(Clone, Copy)]
enum Urls<'a> {
    Keep,
    Book {
        href: &'a str,
        base: Option<&'a str>,
    },
}

impl Urls<'_> {
    fn map(self, url: &str) -> String {
        match self {
            Urls::Keep => url.to_owned(),
            Urls::Book { href, base: None } => abshref(href, url),
            Urls::Book {
                href,
                base: Some(base),
            } => abshref(href, &rel_join(base, url)),
        }
    }
}

/// What a sheet holds before its imports are inlined.
#[derive(Default)]
struct Parsed {
    /// The addresses of its imports that apply, relative to the top sheet ([`import_key`]).
    imports: Vec<String>,
    items: Vec<Item>,
    /// Its items are in the output once already: another copy is charged again.
    emitted: Cell<bool>,
}

#[derive(Clone)]
enum Item {
    /// A style rule: its selectors, as css_parser writes them, and the declarations they
    /// share.
    Style(Vec<String>, Arc<Decls>),
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
    /// Each imported sheet read once, by its [`import_key`]: `None` when the book has none.
    cache: HashMap<String, Option<Rc<Parsed>>>,
    budget: &'a Budget,
    origin: Origin,
    /// The top sheet's address from the book's root.
    href: &'a str,
    out: Sheet,
    index: u32,
}

impl Resolver<'_> {
    fn charge(&self, rules: usize, decls: usize) -> Result<(), Error> {
        if self.budget.charge(rules, decls) {
            Ok(())
        } else {
            Err(self.budget.error())
        }
    }

    /// Follows the import of `key`, `depth` levels down, with `path` the sheets above it.
    fn visit(&mut self, key: &str, depth: usize, path: &mut Vec<String>) -> Result<Visit, Error> {
        self.charge(1, 0)?;
        if path.iter().any(|p| p == key) {
            return Ok(Visit::Cycle);
        }
        if depth > MAX_IMPORT_DEPTH {
            return Ok(Visit::Missing);
        }
        let Some(parsed) = self.load(key)? else {
            return Ok(Visit::Missing);
        };
        path.push(key.to_owned());
        for next in &parsed.imports {
            if self.visit(next, depth + 1, path)? == Visit::Cycle {
                path.pop();
                return Ok(Visit::Cycle);
            }
        }
        path.pop();
        let again = parsed.emitted.replace(true);
        self.emit(parsed.items.iter().map(Cow::Borrowed), again, false)?;
        Ok(Visit::Done)
    }

    fn load(&mut self, key: &str) -> Result<Option<Rc<Parsed>>, Error> {
        if let Some(cached) = self.cache.get(key) {
            return Ok(cached.clone());
        }
        let parsed = match (self.import)(key) {
            Some(text) => {
                self.charge(0, text.len() / TEXT_UNIT)?;
                let urls = match self.origin {
                    Origin::Book => Urls::Book {
                        href: self.href,
                        base: Some(key),
                    },
                    Origin::UserAgent => Urls::Keep,
                };
                // An imported sheet is a linked one: Calibre reads it through the
                // preprocessor.
                let parsed = parse_text(&preprocess(&text), urls, self.budget);
                if self.budget.over() {
                    return Err(self.budget.error());
                }
                Some(Rc::new(parsed))
            }
            None => None,
        };
        self.cache.insert(key.to_owned(), parsed.clone());
        Ok(parsed)
    }

    /// Adds a sheet's items to the output: the top sheet's moved (`own`, the sheet whose
    /// `@page` rules are kept), an imported one's copied. Items were charged when read; a
    /// sheet imported `again` is charged again for its selectors and `@font-face` rules (the
    /// declarations of a style rule are shared, not copied).
    fn emit<'p>(
        &mut self,
        items: impl Iterator<Item = Cow<'p, Item>>,
        again: bool,
        own: bool,
    ) -> Result<(), Error> {
        for item in items {
            match &*item {
                Item::Style(selectors, _) if again => {
                    self.charge(selectors.len(), selector_units(selectors))?;
                }
                Item::FontFace(decls) if again => {
                    let units = decls
                        .iter()
                        .map(|(k, d)| decl_units(k, d.value.len()))
                        .fold(0, usize::saturating_add);
                    self.charge(1, units)?;
                }
                Item::Page(_) if !own => {
                    self.index = self.index.saturating_add(1);
                    continue;
                }
                _ => {}
            }
            match item.into_owned() {
                Item::Style(selectors, decls) => {
                    for selector in selectors {
                        self.out.rules.push(Rule {
                            origin: self.origin,
                            selector,
                            decls: Arc::clone(&decls),
                            index: self.index,
                        });
                    }
                }
                Item::FontFace(decls) => self.out.font_face.push(decls),
                Item::Page(decls) => self.out.page.push(decls),
            }
            self.index = self.index.saturating_add(1);
        }
        Ok(())
    }
}

/// What a list of selectors counts toward the declaration units: one for each
/// [`TEXT_UNIT`] bytes of each.
fn selector_units(selectors: &[String]) -> usize {
    selectors
        .iter()
        .map(|s| s.len() / TEXT_UNIT)
        .fold(0, usize::saturating_add)
}

/// Reads one sheet's text, charging what it reads to `budget` and stopping when that runs
/// out.
fn parse_text(text: &str, urls: Urls<'_>, budget: &Budget) -> Parsed {
    let mut input = ParserInput::new(text);
    let mut p = Parser::new(&mut input);
    let mut top = TopParser {
        namespaces: BTreeSet::new(),
        phase: Phase::Imports,
        urls,
        budget,
        parsed: Parsed::default(),
    };
    let mut rules = StyleSheetParser::new(&mut p, &mut top);
    while rules.next().is_some() {
        if budget.over() {
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
    urls: Urls<'a>,
    budget: &'a Budget,
    parsed: Parsed,
}

impl TopParser<'_> {
    /// A style rule's selectors, charged as they are read.
    fn selectors<'i>(&self, input: &mut Parser<'i, '_>) -> Result<Vec<String>, ParseError<'i, ()>> {
        let selectors =
            parse_list(input, &self.namespaces).map_err(|()| input.new_custom_error(()))?;
        if !self
            .budget
            .charge(selectors.len(), selector_units(&selectors))
        {
            return Err(input.new_custom_error(()));
        }
        Ok(selectors)
    }

    /// A block's declarations, each charged as it is read.
    fn declarations(&self, input: &mut Parser<'_, '_>) -> Decls {
        let urls = self.urls;
        match urls {
            Urls::Keep => declarations(input, Some(self.budget), None),
            Urls::Book { .. } => declarations(input, Some(self.budget), Some(&|u| urls.map(u))),
        }
    }

    /// An `@font-face` or `@page` rule, kept or not, counts as a rule.
    fn at_rule(&mut self, item: Option<Item>) {
        if self.budget.charge(1, 0)
            && let Some(item) = item
        {
            self.parsed.items.push(item);
        }
    }

    /// A style rule's block.
    fn style_block(&mut self, selectors: Vec<String>, input: &mut Parser<'_, '_>) {
        let decls = self.declarations(input);
        if !self.budget.over() {
            self.parsed
                .items
                .push(Item::Style(selectors, Arc::new(decls)));
        }
    }
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
                if self.budget.charge(1, 0) && import_media_ok(&media) {
                    let href = match self.urls {
                        Urls::Book {
                            base: Some(base), ..
                        } => rel_join(base, &href),
                        _ => href,
                    };
                    self.parsed.imports.push(import_key(&href));
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
                    if rules.parser.top.budget.over() {
                        break;
                    }
                }
                Ok(())
            }
            // The rules of a query Calibre rejects are not read.
            Prelude::Media(false) => Ok(()),
            Prelude::FontFace => {
                let decls = self.declarations(input);
                // `flatten_rule`: an `@font-face` of one property (MS Word writes them with
                // only `font-family`) is ignored.
                self.at_rule((decls.len() > 1).then_some(Item::FontFace(decls)));
                Ok(())
            }
            Prelude::Page => {
                let decls = self.declarations(input);
                self.at_rule(Some(Item::Page(decls)));
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
        self.selectors(input)
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
        let decls = self.top.declarations(input);
        self.top.at_rule(Some(Item::Page(decls)));
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
        self.top.selectors(input)
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
