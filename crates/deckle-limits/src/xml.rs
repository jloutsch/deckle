//! The depth check that runs before an XML parser that recurses (nesting-depth design,
//! mechanism 1).
//!
//! `roxmltree` and `feed-rs` (through `quick-xml`) recurse once per element level before they
//! reject anything, and a stack overflow aborts the process. So the text is scanned first, in a
//! single pass that never counts lower than either parser nests.

use std::fmt;

use crate::MAX_DEPTH;

/// The deepest element nesting in `xml`, counted from tags alone; stops at `limit + 1`.
///
/// True when `xml` nests more than `limit` elements deep, defines an entity whose value holds
/// markup, or ends inside a comment, CDATA section, processing instruction, quoted value or
/// DOCTYPE.
pub fn xml_depth_over(xml: &[u8], limit: usize) -> bool {
    let verdict = match xml {
        [0xFF, 0xFE, rest @ ..] => Scan::new(Utf16 {
            bytes: rest,
            little: true,
        })
        .run(limit),
        [0xFE, 0xFF, rest @ ..] => Scan::new(Utf16 {
            bytes: rest,
            little: false,
        })
        .run(limit),
        [b'<', 0, ..] => Scan::new(Utf16 {
            bytes: xml,
            little: true,
        })
        .run(limit),
        [0, b'<', ..] => Scan::new(Utf16 {
            bytes: xml,
            little: false,
        })
        .run(limit),
        _ => Scan::new(Bytes(xml)).run(limit),
    };
    verdict != Verdict::Within
}

/// What the scan found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// No deeper than the limit.
    Within,
    TooDeep,
    /// An `<!ENTITY` value holds a literal `<`, so it can hold elements.
    EntityMarkup,
    /// A comment, CDATA section, PI, quoted value or DOCTYPE runs to the end of the text, so
    /// what follows its start was not counted.
    Unterminated,
}

/// The text as code units: bytes for UTF-8 and other ASCII-compatible encodings, 16-bit units
/// for UTF-16. Markup characters are ASCII, so a unit is compared with an ASCII byte.
trait Units {
    fn len(&self) -> usize;
    fn at(&self, i: usize) -> u32;
}

struct Bytes<'a>(&'a [u8]);

impl Units for Bytes<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn at(&self, i: usize) -> u32 {
        u32::from(self.0[i])
    }
}

struct Utf16<'a> {
    bytes: &'a [u8],
    little: bool,
}

impl Units for Utf16<'_> {
    fn len(&self) -> usize {
        self.bytes.len() / 2
    }
    fn at(&self, i: usize) -> u32 {
        let pair = [self.bytes[2 * i], self.bytes[2 * i + 1]];
        u32::from(if self.little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    }
}

/// The scan. It follows the XML rules where `roxmltree` and `quick-xml` agree, and where they
/// differ on malformed input it follows `quick-xml`, which reads on where `roxmltree` stops
/// with an error. So the count is never lower than the nesting either parser reaches.
struct Scan<U> {
    u: U,
    n: usize,
}

impl<U: Units> Scan<U> {
    fn new(u: U) -> Self {
        let n = u.len();
        Scan { u, n }
    }

    fn is(&self, i: usize, c: u8) -> bool {
        i < self.n && self.u.at(i) == u32::from(c)
    }

    fn starts(&self, i: usize, pat: &[u8]) -> bool {
        i + pat.len() <= self.n && pat.iter().enumerate().all(|(k, &c)| self.is(i + k, c))
    }

    /// The first `pat` at or after `from`.
    fn find(&self, from: usize, pat: &[u8]) -> Result<usize, Verdict> {
        (from..self.n)
            .find(|&i| self.starts(i, pat))
            .ok_or(Verdict::Unterminated)
    }

    /// Past the first `pat` at or after `from`.
    fn past(&self, from: usize, pat: &[u8]) -> Result<usize, Verdict> {
        Ok(self.find(from, pat)? + pat.len())
    }

    fn run(&self, limit: usize) -> Verdict {
        match self.elements(limit) {
            Ok(()) => Verdict::Within,
            Err(v) => v,
        }
    }

    fn elements(&self, limit: usize) -> Result<(), Verdict> {
        let mut depth = 0usize;
        let mut i = 0;
        while i < self.n {
            if !self.is(i, b'<') {
                i += 1;
            } else if self.starts(i, b"<!-") {
                // A comment ends at the first `-->` after `<!--`; `<!-->` does not end it.
                // quick-xml takes `<!-x` for a comment too, with the same end.
                i = self.past(i + 4, b"-->")?;
            } else if self.starts(i, b"<![CDATA[") {
                i = self.past(i + 9, b"]]>")?;
            } else if self.starts(i, b"<!D") || self.starts(i, b"<!d") {
                // quick-xml takes any `<!D` or `<!d` for a DOCTYPE.
                i = self.doctype(i + 3)?;
            } else if self.starts(i, b"<!") {
                // Neither parser reads on past this; count what follows as content.
                i += 1;
            } else if self.starts(i, b"<?") {
                i = self.past(i + 2, b"?>")?;
            } else if self.starts(i, b"</") {
                depth = depth.saturating_sub(1);
                i = self.past(i + 2, b">").unwrap_or(self.n);
            } else if i + 1 == self.n {
                break;
            } else {
                // quick-xml reads anything else after `<` as a start tag.
                let (end, empty) = self.tag_end(i + 1)?;
                // A self-closing element is a level too, while it is parsed.
                if depth + 1 > limit {
                    return Err(Verdict::TooDeep);
                }
                if !empty {
                    depth += 1;
                }
                i = end + 1;
            }
        }
        Ok(())
    }

    /// The `>` that ends the start tag whose name begins at `from`, and whether the tag closes
    /// itself (`/>`). A quote opens a quoted run wherever it stands, as in quick-xml; in
    /// well-formed XML it only stands at the start of a value, so unquoted values (`href=x`)
    /// are ordinary text. A tag cut off by the end of the text ends there.
    fn tag_end(&self, from: usize) -> Result<(usize, bool), Verdict> {
        let mut quote = None;
        for j in from..self.n {
            let c = self.u.at(j);
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => {}
                None if c == u32::from(b'"') || c == u32::from(b'\'') => quote = Some(c),
                None if c == u32::from(b'>') => return Ok((j, j > from && self.is(j - 1, b'/'))),
                None => {}
            }
        }
        match quote {
            Some(_) => Err(Verdict::Unterminated),
            None => Ok((self.n, false)),
        }
    }

    /// Past the `>` that ends a DOCTYPE whose body starts at `from`, read as quick-xml reads
    /// it (`roxmltree` agrees on every DOCTYPE it accepts). Fails on an entity value holding
    /// `<`.
    fn doctype(&self, from: usize) -> Result<usize, Verdict> {
        // Before the internal subset: quoted ids, then `[` or `>`.
        let mut j = from;
        loop {
            if j >= self.n {
                return Err(Verdict::Unterminated);
            }
            if self.is(j, b'"') || self.is(j, b'\'') {
                j = self.past(j + 1, &[self.u.at(j) as u8])?;
            } else if self.is(j, b'[') {
                j += 1;
                break;
            } else if self.is(j, b'>') {
                return Ok(j + 1);
            } else {
                j += 1;
            }
        }
        // The internal subset: declarations, comments and PIs up to `]`. Quotes count only
        // inside a declaration.
        while j < self.n {
            if self.is(j, b']') {
                return self.past(j + 1, b">");
            } else if self.starts(j, b"<?") {
                j = self.past(j + 2, b"?>")?;
            } else if self.starts(j, b"<!--") {
                j = self.past(j + 4, b"-->")?;
            } else if self.starts(j, b"<!ELEMENT") {
                j = self.past(j + 9, b">")?;
            } else if self.starts(j, b"<!ENTITY") {
                j = self.declaration(j + 8, true)?;
            } else if self.starts(j, b"<!ATTLIST") || self.starts(j, b"<!NOTATION") {
                j = self.declaration(j + 9, false)?;
            } else if self.is(j, b'<') {
                j = self.past(j + 1, b">")?;
            } else {
                j += 1;
            }
        }
        Err(Verdict::Unterminated)
    }

    /// Past the `>` ending a declaration whose quoted values may hold `>`. In an entity
    /// declaration, a `<` inside a value is markup `roxmltree` would expand into elements.
    fn declaration(&self, from: usize, entity: bool) -> Result<usize, Verdict> {
        let mut quote = None;
        for j in from..self.n {
            let c = self.u.at(j);
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) if entity && c == u32::from(b'<') => return Err(Verdict::EntityMarkup),
                Some(_) => {}
                None if c == u32::from(b'"') || c == u32::from(b'\'') => quote = Some(c),
                None if c == u32::from(b'>') => return Ok(j + 1),
                None => {}
            }
        }
        Err(Verdict::Unterminated)
    }
}

/// Why [`parse_xml`] refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlError {
    /// Elements nest more than [`MAX_DEPTH`] deep.
    TooDeep,
    /// The DOCTYPE defines an entity whose value holds markup.
    EntityMarkup,
    /// `roxmltree` could not parse it.
    Parse(roxmltree::Error),
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XmlError::TooDeep => write!(f, "this file is nested more than {MAX_DEPTH} levels deep"),
            XmlError::EntityMarkup => {
                f.write_str("this file defines an entity that contains markup")
            }
            XmlError::Parse(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for XmlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            XmlError::Parse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<roxmltree::Error> for XmlError {
    fn from(e: roxmltree::Error) -> Self {
        XmlError::Parse(e)
    }
}

/// `roxmltree::Document::parse_with_options`, after checking that `text` nests no more than
/// [`MAX_DEPTH`] deep and defines no entity holding markup. Every parse of file content goes
/// through this.
///
/// Text that ends inside a comment, CDATA section, PI, quoted value or DOCTYPE is left to
/// `roxmltree`, which names the error: everything after that start is inside it, so the parser
/// cannot nest deeper than what was counted before it.
///
/// An `entity_resolver` in `options` is not covered: text it returns is not scanned.
pub fn parse_xml<'input>(
    text: &'input str,
    options: roxmltree::ParsingOptions<'input>,
) -> Result<roxmltree::Document<'input>, XmlError> {
    // `text` is UTF-8 whatever it starts with, so it is scanned as bytes.
    match Scan::new(Bytes(text.as_bytes())).run(MAX_DEPTH) {
        Verdict::TooDeep => Err(XmlError::TooDeep),
        Verdict::EntityMarkup => Err(XmlError::EntityMarkup),
        Verdict::Within | Verdict::Unterminated => {
            Ok(roxmltree::Document::parse_with_options(text, options)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The depth the scanner reports: the smallest limit it accepts. None when it refuses at
    /// every limit up to 1000 (an entity with markup, or an unterminated construct).
    fn depth(xml: &str) -> Option<usize> {
        depth_bytes(xml.as_bytes())
    }

    fn depth_bytes(xml: &[u8]) -> Option<usize> {
        (0..1000).find(|&l| !xml_depth_over(xml, l))
    }

    fn nested(open: &str, close: &str, n: usize, inner: &str) -> String {
        let mut s = String::with_capacity(n * (open.len() + close.len()) + inner.len());
        for _ in 0..n {
            s.push_str(open);
        }
        s.push_str(inner);
        for _ in 0..n {
            s.push_str(close);
        }
        s
    }

    /// Runs `f` on a thread with a `stack` byte stack and returns its result.
    fn on_stack<T: Send + 'static>(stack: usize, f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(stack)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn counts_nesting_not_siblings() {
        assert_eq!(depth(""), Some(0));
        assert_eq!(depth("just text"), Some(0));
        assert_eq!(depth("<a/>"), Some(1));
        assert_eq!(depth("<a></a>"), Some(1));
        assert_eq!(depth("<a><b></b><b></b><b>x</b></a>"), Some(2));
        assert_eq!(depth("<a><b><c/></b><b/></a>"), Some(3));
        assert_eq!(
            depth("<?xml version=\"1.0\"?>\n<a>\n  <b>1 &gt; 0</b>\n</a>\n"),
            Some(2)
        );
    }

    #[test]
    fn a_self_closing_tag_is_a_level_but_does_not_stay_open() {
        assert_eq!(depth("<a><b/><b/><b/><b/></a>"), Some(2));
        assert_eq!(depth("<a><b x=\"1\" /><b/></a>"), Some(2));
    }

    #[test]
    fn comments_cdata_and_pis_do_not_change_the_count() {
        assert_eq!(depth("<a><!-- <b><c><d> --></a>"), Some(1));
        assert_eq!(depth("<a><!-- <b> -> - > --></a>"), Some(1));
        assert_eq!(depth("<a><![CDATA[<b><c> ]] ]> ]]></a>"), Some(1));
        assert_eq!(depth("<a><?pi <b><c> > ? ?></a>"), Some(1));
        assert_eq!(depth("<!-- <x><x> --><?p <y> ?><a/>"), Some(1));
        // `<!-->` does not end the comment it starts.
        assert_eq!(depth("<a><!--> <b><c> --></a>"), Some(1));
    }

    #[test]
    fn markup_inside_attribute_values_does_not_end_a_tag() {
        // `<a x="/>">` opens an element.
        assert_eq!(depth("<r><a x=\"/>\"><b/></a></r>"), Some(3));
        assert_eq!(depth("<r><a x=\">\"><b/></a></r>"), Some(3));
        assert_eq!(depth("<r><a x=\"<b><c>\"/></r>"), Some(2));
        assert_eq!(depth("<r><a x = \"/>\" y=\"2\"><b/></a></r>"), Some(3));
    }

    #[test]
    fn single_quoted_attribute_values() {
        assert_eq!(depth("<r><a x='/>'><b/></a></r>"), Some(3));
        assert_eq!(depth("<r><a x='\"'><b/></a></r>"), Some(3));
        assert_eq!(depth("<r><a x=\"'\" y='>'><b/></a></r>"), Some(3));
    }

    #[test]
    fn unquoted_attribute_values_are_ordinary() {
        assert_eq!(depth("<rss><a href=x><b/></a></rss>"), Some(3));
        assert_eq!(depth("<rss><a href=x/></rss>"), Some(2));
        assert_eq!(
            depth("<rss><a href=http://x.test/a?b=c><b/></a></rss>"),
            Some(3)
        );
    }

    #[test]
    fn a_quote_anywhere_in_a_tag_hides_the_tag_end_as_quick_xml_reads_it() {
        // quick-xml (feed-rs) treats any quote in a tag as opening a quoted run, so here `a`
        // ends at `d>` and the three `x` are elements. Reading only `="…"` as quoted would put
        // them inside a value and count 2.
        assert_eq!(depth("<r><a b\"c=\"d><x><x><x>\"></r>"), Some(5));
    }

    #[test]
    fn end_tags_close_and_stray_ones_do_not_underflow() {
        assert_eq!(depth("</x></y><a><b/></a>"), Some(2));
        assert_eq!(depth("<a></a></a></a><b><c/></b>"), Some(2));
    }

    #[test]
    fn an_entity_whose_value_holds_markup_is_over() {
        let double = "<!DOCTYPE d [<!ENTITY e \"<x><x></x></x>\">]><d>&e;</d>";
        let single = "<!DOCTYPE d [<!ENTITY e '<x/>'>]><d>&e;</d>";
        let parameter = "<!DOCTYPE d [<!ENTITY % p \"<!ENTITY e '<x/>'>\">]><d/>";
        for xml in [double, single, parameter] {
            assert_eq!(depth(xml), None, "{xml}");
        }
    }

    #[test]
    fn entities_for_characters_and_doctypes_without_a_subset_are_fine() {
        assert_eq!(
            depth(
                "<!DOCTYPE d [<!ENTITY lt2 \"&#60;\"><!ENTITY amp2 '&lt;'><!ENTITY gt2 \">\">]><d>&lt2;</d>"
            ),
            Some(1)
        );
        let rss091 = "<?xml version=\"1.0\"?>\n<!DOCTYPE rss PUBLIC \"-//Netscape Communications//DTD RSS 0.91//EN\" \"http://my.netscape.com/publish/formats/rss-0.91.dtd\">\n<rss version=\"0.91\"><channel><title>x &nbsp; y</title></channel></rss>";
        assert_eq!(depth(rss091), Some(3));
        assert_eq!(depth("<!doctype html><html><body/></html>"), Some(2));
        // A comment, PI or other declaration in the subset may hold quotes and `<`.
        assert_eq!(
            depth(
                "<!DOCTYPE d [<!-- don't <x> --><?p \"<y>\" ?><!ELEMENT d (#PCDATA)><!ATTLIST d a CDATA \"v\">]><d/>"
            ),
            Some(1)
        );
        // `]` and `>` inside a quoted value of the subset do not end it.
        assert_eq!(
            depth("<!DOCTYPE d [<!ENTITY e \"]>\">]><d><e/></d>"),
            Some(2)
        );
    }

    #[test]
    fn the_doctype_does_not_hide_elements_after_it() {
        let deep = nested("<x>", "</x>", 20, "");
        assert_eq!(
            depth(&format!("<!DOCTYPE d [<!ENTITY e \"v\">]>{deep}")),
            Some(20)
        );
        assert_eq!(
            depth(&format!("<!DOCTYPE d SYSTEM \"[>\">{deep}")),
            Some(20)
        );
        // quick-xml leaves a quote outside any declaration of the subset alone.
        assert_eq!(depth(&format!("<!DOCTYPE d [ \" ]>{deep}")), Some(20));
    }

    #[test]
    fn unterminated_constructs_are_over() {
        for xml in [
            "<a><!-- never closed <b>",
            "<a><![CDATA[ never closed",
            "<a><?pi never closed",
            "<a><b x=\"never closed></b></a>",
            "<a><b x='never closed></b></a>",
            "<!DOCTYPE d [<!ENTITY e \"never closed>]><d/>",
            "<!DOCTYPE d [<!ENTITY e \"v\">",
            "<!DOCTYPE d",
        ] {
            assert_eq!(depth(xml), None, "{xml}");
        }
    }

    #[test]
    fn exactly_max_depth_passes_and_one_more_fails() {
        let at = nested("<a>", "</a>", MAX_DEPTH, "text");
        assert!(!xml_depth_over(at.as_bytes(), MAX_DEPTH));
        let over = nested("<a>", "</a>", MAX_DEPTH + 1, "text");
        assert!(xml_depth_over(over.as_bytes(), MAX_DEPTH));
        let leaf_over = nested("<a>", "</a>", MAX_DEPTH, "<b/>");
        assert!(xml_depth_over(leaf_over.as_bytes(), MAX_DEPTH));
    }

    fn latin1_feed(levels: usize) -> Vec<u8> {
        let mut out = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><rss><channel><item><title>Caf\xe9</title><description>".to_vec();
        for _ in 0..levels {
            out.extend_from_slice(b"<media:group>\xe9");
        }
        for _ in 0..levels {
            out.extend_from_slice(b"</media:group>");
        }
        out.extend_from_slice(b"</description></item></channel></rss>");
        out
    }

    fn utf16(text: &str, little: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut push = |u: u16| {
            out.extend_from_slice(&if little {
                u.to_le_bytes()
            } else {
                u.to_be_bytes()
            })
        };
        if bom {
            push(0xFEFF);
        }
        text.encode_utf16().for_each(&mut push);
        out
    }

    fn utf16_feed(levels: usize) -> String {
        nested("<media:group>é", "</media:group>", levels, "Grüße — ✓").replace("é", "é😀")
    }

    #[test]
    fn a_deep_latin1_feed_is_refused_and_a_shallow_one_passes() {
        let deep = latin1_feed(5_000);
        assert!(
            std::str::from_utf8(&deep).is_err(),
            "the test feed is not UTF-8"
        );
        assert!(xml_depth_over(&deep, MAX_DEPTH));
        let shallow = latin1_feed(3);
        // rss, channel, item, description, then three groups.
        assert_eq!(depth_bytes(&shallow), Some(4 + 3));
    }

    #[test]
    fn a_deep_utf16_feed_is_refused_and_a_shallow_one_passes() {
        for (little, bom) in [(true, true), (false, true), (true, false), (false, false)] {
            let deep = utf16(&utf16_feed(5_000), little, bom);
            assert!(
                xml_depth_over(&deep, MAX_DEPTH),
                "little {little} bom {bom}"
            );
            let shallow = utf16(&utf16_feed(3), little, bom);
            assert_eq!(depth_bytes(&shallow), Some(3), "little {little} bom {bom}");
        }
        // In UTF-16 the zero bytes do not hide a `<`, and a `<` byte inside another unit is
        // not a `<`: U+3C3C is one character, not two tags.
        let mut tricky = utf16("<a>", true, true);
        tricky.extend_from_slice(&0x3C3Cu16.to_le_bytes());
        tricky.extend(utf16("</a>", true, false));
        assert_eq!(depth_bytes(&tricky), Some(1));
    }

    #[test]
    fn utf16_with_a_bom_and_an_unterminated_comment_is_over() {
        let xml = utf16("<a><!-- open", true, true);
        assert!(xml_depth_over(&xml, MAX_DEPTH));
    }

    #[test]
    fn invalid_utf8_is_not_a_reason_to_refuse() {
        assert_eq!(depth_bytes(b"<a>\xff\xfe\x80<b/></a>"), Some(2));
    }

    #[test]
    fn a_hundred_thousand_levels_are_refused_on_a_one_mib_stack() {
        let refused = on_stack(1 << 20, || {
            let deep = nested("<a>", "</a>", 100_000, "");
            let over = xml_depth_over(deep.as_bytes(), MAX_DEPTH);
            let parsed = parse_xml(&deep, roxmltree::ParsingOptions::default()).map(|_| ());
            (over, parsed)
        });
        assert_eq!(refused, (true, Err(XmlError::TooDeep)));
    }

    #[test]
    fn parse_xml_gives_the_two_messages_of_the_design() {
        let deep = nested("<a>", "</a>", MAX_DEPTH + 1, "");
        let e = parse_xml(&deep, Default::default()).unwrap_err();
        assert_eq!(e, XmlError::TooDeep);
        assert_eq!(
            e.to_string(),
            "this file is nested more than 256 levels deep"
        );

        let entity = "<!DOCTYPE d [<!ENTITY e \"<x/>\">]><d>&e;</d>";
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let e = parse_xml(entity, opts).unwrap_err();
        assert_eq!(e, XmlError::EntityMarkup);
        assert_eq!(
            e.to_string(),
            "this file defines an entity that contains markup"
        );
    }

    #[test]
    fn parse_xml_uses_the_callers_options_and_passes_parse_errors_through() {
        let with_dtd = "<!DOCTYPE d [<!ENTITY e \"v\">]><d>&e;</d>";
        assert!(matches!(
            parse_xml(with_dtd, Default::default()),
            Err(XmlError::Parse(roxmltree::Error::DtdDetected))
        ));
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = parse_xml(with_dtd, opts).unwrap();
        assert_eq!(doc.root_element().text(), Some("v"));

        assert!(matches!(
            parse_xml("<a>", Default::default()),
            Err(XmlError::Parse(_))
        ));
        // An unterminated comment is left to roxmltree, which names it.
        assert!(matches!(
            parse_xml("<a><!-- open", Default::default()),
            Err(XmlError::Parse(_))
        ));
        let at = nested("<a>", "</a>", MAX_DEPTH, "");
        let doc = on_stack(64 << 20, move || {
            parse_xml(&at, Default::default()).map(|d| d.descendants().count())
        });
        assert_eq!(doc, Ok(MAX_DEPTH + 1));
    }

    #[test]
    fn roxmltree_reads_markup_from_an_entity_and_keeps_character_references_as_text() {
        // Why the scan refuses `<` in an entity value: roxmltree parses it as elements.
        let opts = || roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let markup = "<!DOCTYPE d [<!ENTITY e \"<x><y/></x>\">]><d>&e;</d>";
        let doc = roxmltree::Document::parse_with_options(markup, opts()).unwrap();
        assert!(doc.descendants().any(|n| n.has_tag_name("y")));
        // And why `&#60;` and `&lt;` in a value are allowed: they stay text.
        for xml in [
            "<!DOCTYPE d [<!ENTITY e \"&#60;x/>\">]><d>&e;</d>",
            "<!DOCTYPE d [<!ENTITY e \"&#x3C;x/>\">]><d>&e;</d>",
            "<!DOCTYPE d [<!ENTITY e \"&lt;x/>\">]><d>&e;</d>",
        ] {
            let doc = parse_xml(xml, opts()).unwrap();
            assert!(!doc.descendants().any(|n| n.has_tag_name("x")), "{xml}");
            assert_eq!(doc.root_element().text(), Some("<x/>"), "{xml}");
        }
    }

    /// A small xorshift generator, so the test needs no dependency.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[self.below(items.len() as u64) as usize]
        }
    }

    const ATTRS: &[&str] = &[
        "",
        " x=\"/>\"",
        " x=\">\"",
        " x='/>' y=\"'\"",
        " x='\"'",
        " a = \"1\"",
        " h=\"&lt;b&gt;\"",
        " t=\"a &amp; b\"",
    ];
    const FILLER: &[&str] = &[
        "",
        "text",
        " 1 &gt; 0 > ",
        "<!-- <c><d> > -->",
        "<![CDATA[<c><d> ]] ]]>",
        "<?pi <c> > ?>",
        "<e/>",
        "<e x=\"/>\"/>",
        "<e></e>",
        "&ent;",
        "<f><g/></f>",
    ];

    /// A well-formed document whose deepest chain is `chain` elements, with random siblings,
    /// attributes, comments, CDATA and PIs. A sibling subtree may go one or two levels deeper
    /// than the chain.
    fn random_doc(rng: &mut Rng, chain: usize) -> String {
        let mut s = String::from(
            "<?xml version=\"1.0\"?><!DOCTYPE r [<!-- \"<x>\" --><!ENTITY ent \"a &#60; b\">]>",
        );
        let mut closes = Vec::new();
        for level in 0..chain {
            // Nothing but the root may stand outside the root.
            if level > 0 {
                s.push_str(rng.pick(FILLER));
            }
            let name = format!("n{}", rng.below(3));
            s.push('<');
            s.push_str(&name);
            s.push_str(rng.pick(ATTRS));
            s.push('>');
            s.push_str(rng.pick(FILLER));
            closes.push(name);
        }
        while let Some(name) = closes.pop() {
            s.push_str(rng.pick(FILLER));
            s.push_str("</");
            s.push_str(&name);
            s.push('>');
        }
        s
    }

    fn real_depth(xml: &str) -> usize {
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(xml, opts).unwrap();
        doc.descendants()
            .filter(|n| n.is_element())
            .map(|n| n.ancestors().filter(|a| a.is_element()).count())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn the_verdict_agrees_with_the_depth_roxmltree_builds() {
        let checked = on_stack(64 << 20, || {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let mut checked = 0;
            for round in 0..120 {
                let (chain, limits): (usize, Vec<usize>) = if round % 4 == 0 {
                    let c = MAX_DEPTH - 4 + rng.below(8) as usize;
                    (c, vec![MAX_DEPTH])
                } else {
                    let c = 1 + rng.below(24) as usize;
                    (c, (c.saturating_sub(3)..c + 4).collect())
                };
                let xml = random_doc(&mut rng, chain);
                let real = real_depth(&xml);
                for limit in limits {
                    assert_eq!(
                        xml_depth_over(xml.as_bytes(), limit),
                        real > limit,
                        "real depth {real}, limit {limit}: {xml}"
                    );
                    checked += 1;
                }
            }
            checked
        });
        assert!(checked > 300);
    }
}
