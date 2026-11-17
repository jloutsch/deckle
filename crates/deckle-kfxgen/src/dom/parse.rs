// SPDX-License-Identifier: GPL-3.0-only
//! Bytes to an element tree, as `ebooks/oeb/parse_utils.py` `parse_html` reads a document up to
//! its parse: decoded (`oeb/base.py` `OEBBook.decode`, `ebooks/chardet.py` `xml_to_unicode`), its
//! encoding declarations stripped, the text before `<html` cut off, Calibre's preprocessor run
//! (`ebooks/conversion/preprocess.py` `HTMLPreProcessor`), then read as XML and, failing that,
//! with its HTML entities replaced, then as HTML.

use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use deckle_limits::roxmltree::{Document, Node, ParsingOptions};
use deckle_limits::{MAX_DEPTH, MAX_DOM_NODES, MAX_TAG_ATTRIBUTES, XmlError};
use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, QualName, local_name, ns};
use regex::Regex;
use regex::bytes::Regex as BytesRegex;

use super::{Attr, Dom, Error, NodeId, XML_NS, clean};

const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// `parse_html`: `bytes` decoded, parsed and cleaned as Calibre's conversion pipeline reads a
/// spine document. A file that is not XML is read as HTML. [`Error`] when its entities expand
/// past [`deckle_limits::MAX_ENTITY_TEXT`], or read as HTML it nests deeper than `MAX_DEPTH` or
/// builds far more elements than its size.
pub fn parse(bytes: &[u8]) -> Result<Dom, Error> {
    let text = decode(bytes);
    let text = strip_encoding_declarations(&text);

    // "Remove DOCTYPE declaration as it messes up parsing"
    let idx = text.find("<html").or_else(|| text.find("<HTML"));
    let (pre, mut data) = match idx {
        Some(i) => (&text[..i], text[i..].to_owned()),
        None => ("", text.clone()),
    };
    let mut has_html4_doctype = false;
    if pre.contains("<!DOCTYPE") {
        has_html4_doctype = HTML4_DOCTYPE.is_match(pre);
        data = replace_user_entities(pre, &data)?;
    }

    let data = preprocess(&data).replace('\0', "");
    let raw = clean_word_doc(&data);

    // "Try with more & more drastic measures to parse"
    let dom = match xml_tree(&raw, pre)? {
        Some(dom) => dom,
        None => {
            let raw = replace_entities(&raw, true);
            match xml_tree(&raw, pre)? {
                Some(dom) => dom,
                None => html_tree(&raw)?,
            }
        }
    };
    Ok(clean::clean(dom, has_html4_doctype))
}

// ---------------------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------------------

/// A strict decoder: None when the bytes are not in its encoding.
type Decoder = fn(&[u8]) -> Option<String>;

/// `OEBBook.decode` with no input encoding (EPUB input sets none): a byte-order mark, else
/// UTF-8, else `xml_to_unicode`. Line ends become `\n`.
pub(crate) fn decode(bytes: &[u8]) -> String {
    let fix = |s: String| s.replace("\r\n", "\n").replace('\r', "\n");
    let mut data = bytes;
    let bom: Option<(Decoder, usize)> = match data {
        [0, 0, 0xfe, 0xff, ..] => Some((|d| utf32(d, u32::from_be_bytes), 4)),
        [0xff, 0xfe, 0, 0, ..] => Some((|d| utf32(d, u32::from_le_bytes), 4)),
        [0xff, 0xfe, ..] => Some((|d| utf16(d, u16::from_le_bytes, false), 2)),
        [0xfe, 0xff, ..] => Some((|d| utf16(d, u16::from_be_bytes, false), 2)),
        [0xef, 0xbb, 0xbf, ..] => Some((|d| String::from_utf8(d.to_vec()).ok(), 3)),
        _ => None,
    };
    if let Some((dec, len)) = bom {
        data = &data[len..];
        if let Some(s) = dec(data) {
            return fix(s);
        }
    }
    if let Ok(s) = std::str::from_utf8(data) {
        return fix(s.to_owned());
    }
    fix(xml_to_unicode(data))
}

/// `chardet.py` `xml_to_unicode` (through `detect_xml_encoding`): a byte-order mark, else the
/// first encoding a declaration pattern finds, decoded with Python's `'replace'`.
///
/// With no declaration Calibre takes uchardet's guess (`force_encoding`, `detect`), or UTF-8 when
/// it has none. The port has no detector and guesses windows-1252, what uchardet answers for
/// most Western text that is not UTF-8 (DIVERGENCES.md, "Encodings").
fn xml_to_unicode(raw: &[u8]) -> String {
    if let Some(rest) = raw.strip_prefix(b"\xef\xbb\xbf") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = raw.strip_prefix(b"\xff\xfe") {
        return utf16(rest, u16::from_le_bytes, true).unwrap_or_default();
    }
    if let Some(rest) = raw.strip_prefix(b"\xfe\xff") {
        return utf16(rest, u16::from_be_bytes, true).unwrap_or_default();
    }
    let declared = ENCODING_PATS_BYTES
        .iter()
        .find_map(|p| p.captures(raw))
        .map(|c| String::from_utf8_lossy(&c[1]).into_owned());
    let Some(declared) = declared else {
        return cp1252(raw);
    };
    match python_codec(&declared) {
        Some(Codec::Utf8) => String::from_utf8_lossy(raw).into_owned(),
        Some(Codec::Latin1) => raw.iter().map(|&b| char::from(b)).collect(),
        Some(Codec::Ascii) => raw
            .iter()
            .map(|&b| if b < 0x80 { char::from(b) } else { '\u{fffd}' })
            .collect(),
        Some(Codec::Utf16Le) => utf16(raw, u16::from_le_bytes, true).unwrap_or_default(),
        Some(Codec::Utf16Be) => utf16(raw, u16::from_be_bytes, true).unwrap_or_default(),
        // Python's codec of that name, as near as `encoding_rs` has it (the WHATWG encodings);
        // a name it does not know is decoded as UTF-8, as `detect_xml_encoding` does when
        // `codecs.lookup` fails.
        None => match encoding_rs::Encoding::for_label(declared.trim().as_bytes()) {
            Some(e) if e == encoding_rs::WINDOWS_1252 => cp1252(raw),
            Some(e) if e != encoding_rs::REPLACEMENT => {
                e.decode_without_bom_handling(raw).0.into_owned()
            }
            _ => String::from_utf8_lossy(raw).into_owned(),
        },
    }
}

/// Python's `cp1252` with `'replace'`: the five bytes it leaves undefined (0x81, 0x8D, 0x8F,
/// 0x90, 0x9D) become U+FFFD, where the WHATWG encoding maps them to C1 controls.
fn cp1252(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    for chunk in raw.split_inclusive(|b| matches!(b, 0x81 | 0x8d | 0x8f | 0x90 | 0x9d)) {
        let (body, undefined) = match chunk.split_last() {
            Some((0x81 | 0x8d | 0x8f | 0x90 | 0x9d, rest)) => (rest, true),
            _ => (chunk, false),
        };
        out.push_str(
            &encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(body)
                .0,
        );
        if undefined {
            out.push('\u{fffd}');
        }
    }
    out
}

enum Codec {
    Utf8,
    Latin1,
    Ascii,
    Utf16Le,
    Utf16Be,
}

/// The codecs the port decodes itself, by Python's names and aliases (`encodings/aliases.py`),
/// the label normalized as `encodings.normalize_encoding` does. Latin-1 is Python's, not the
/// WHATWG's (which reads the label as windows-1252).
fn python_codec(label: &str) -> Option<Codec> {
    let mut norm = String::new();
    let mut punct = false;
    for c in label.trim().to_lowercase().chars() {
        if c.is_alphanumeric() || c == '.' {
            if punct && !norm.is_empty() {
                norm.push('_');
            }
            if c.is_ascii() {
                norm.push(c);
            }
            punct = false;
        } else {
            punct = true;
        }
    }
    match norm.as_str() {
        "utf_8" | "u8" | "utf" | "utf8" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" => {
            Some(Codec::Utf8)
        }
        "latin_1" | "8859" | "cp819" | "csisolatin1" | "ibm819" | "iso8859" | "iso8859_1"
        | "iso_8859_1" | "iso_8859_1_1987" | "iso_ir_100" | "l1" | "latin" | "latin1" => {
            Some(Codec::Latin1)
        }
        "ascii" | "646" | "ansi_x3.4_1968" | "ansi_x3_4_1968" | "ansi_x3.4_1986" | "cp367"
        | "csascii" | "ibm367" | "iso646_us" | "iso_646.irv_1991" | "iso_ir_6" | "us"
        | "us_ascii" => Some(Codec::Ascii),
        // Python's `utf-16` with no byte-order mark reads the machine's order: little-endian
        // on every machine Calibre ships for.
        "utf_16" | "u16" | "utf16" | "utf_16le" | "utf_16_le" | "unicodelittleunmarked" => {
            Some(Codec::Utf16Le)
        }
        "utf_16be" | "utf_16_be" | "unicodebigunmarked" => Some(Codec::Utf16Be),
        _ => None,
    }
}

/// UTF-16 in the order `read` gives; with `lossy`, bad units become U+FFFD as Python's
/// `'replace'` does, else None.
fn utf16(data: &[u8], read: fn([u8; 2]) -> u16, lossy: bool) -> Option<String> {
    let units = data.chunks(2).map(|c| match c {
        [a, b] => read([*a, *b]),
        // An odd byte at the end: never a whole unit.
        _ => 0xdc00,
    });
    if lossy {
        Some(
            char::decode_utf16(units)
                .map(|r| r.unwrap_or('\u{fffd}'))
                .collect(),
        )
    } else {
        char::decode_utf16(units).collect::<Result<_, _>>().ok()
    }
}

fn utf32(data: &[u8], read: fn([u8; 4]) -> u32) -> Option<String> {
    data.chunks(4)
        .map(|c| match c {
            [a, b, c, d] => char::from_u32(read([*a, *b, *c, *d])),
            _ => None,
        })
        .collect()
}

/// `chardet.py` `_encoding_pats`: the XML declaration, an HTML 5 charset, an HTML 4 pragma.
const ENCODING_PATS: [&str; 3] = [
    r#"<\?[^<>]+encoding\s*=\s*['"](.*?)['"][^<>]*>"#,
    r#"<meta\s+charset=['"]([-_a-z0-9]+)['"][^<>]*>(?:\s*</meta>){0,1}"#,
    r#"<meta\s+?[^<>]*?content\s*=\s*['"][^'"]*?charset=([-_a-z0-9]+)[^'"]*?['"][^<>]*>(?:\s*</meta>){0,1}"#,
];

static ENCODING_PATS_BYTES: LazyLock<Vec<BytesRegex>> = LazyLock::new(|| {
    ENCODING_PATS
        .iter()
        .map(|p| BytesRegex::new(&format!("(?i-u){p}")).expect("valid pattern"))
        .collect()
});

static ENCODING_PATS_STR: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    ENCODING_PATS
        .iter()
        .map(|p| Regex::new(&format!("(?i){p}")).expect("valid pattern"))
        .collect()
});

/// `chardet.py` `strip_encoding_declarations`: every declaration in the first 50 KiB
/// characters, removed.
fn strip_encoding_declarations(raw: &str) -> String {
    let split = raw
        .char_indices()
        .nth(50 * 1024)
        .map_or(raw.len(), |(i, _)| i);
    let mut prefix = raw[..split].to_owned();
    for pat in ENCODING_PATS_STR.iter() {
        if let Cow::Owned(s) = pat.replace_all(&prefix, "") {
            prefix = s;
        }
    }
    prefix + &raw[split..]
}

// ---------------------------------------------------------------------------------------
// The text before parsing
// ---------------------------------------------------------------------------------------

static HTML4_DOCTYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!DOCTYPE\s+[^>]+HTML\s+4.0[^.]+>").expect("valid pattern"));
static USER_ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!ENTITY\s+(\S+)\s+([^>]+)").expect("valid pattern"));
static HTML5_DOCTYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<!DOCTYPE\s+html\s*>").expect("valid pattern"));

/// `parse_html`: the entities the DOCTYPE before `<html` defines, replaced in `data`.
/// [`Error::EntityText`] when the text they produce would pass
/// [`deckle_limits::MAX_ENTITY_TEXT`]: a short value referenced many times multiplies.
fn replace_user_entities(pre: &str, data: &str) -> Result<String, Error> {
    let mut entities: BTreeMap<&str, &str> = BTreeMap::new();
    for m in USER_ENTITY.captures_iter(pre) {
        let (Some(name), Some(val)) = (m.get(1), m.get(2)) else {
            continue;
        };
        let val = val.as_str();
        let val = val
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(val);
        entities.insert(name.as_str(), val);
    }
    if entities.is_empty() {
        return Ok(data.to_owned());
    }
    let alternation: Vec<String> = entities.keys().map(|k| regex::escape(k)).collect();
    let Ok(pat) = regex::RegexBuilder::new(&format!("&({});", alternation.join("|")))
        .size_limit(1 << 22)
        .build()
    else {
        return Ok(data.to_owned());
    };
    let value = |c: &regex::Captures| entities.get(&c[1]).copied().unwrap_or_default();
    let mut produced: u64 = 0;
    for c in pat.captures_iter(data) {
        produced = produced.saturating_add(value(&c).len() as u64);
        if produced > deckle_limits::MAX_ENTITY_TEXT {
            return Err(Error::EntityText(deckle_limits::TooLarge {
                limit: deckle_limits::MAX_ENTITY_TEXT,
            }));
        }
    }
    Ok(pat
        .replace_all(data, |c: &regex::Captures| value(c).to_owned())
        .into_owned())
}

/// `preprocess.py` `LIGATURES`.
const LIGATURES: [(char, &str); 7] = [
    ('ﬀ', "ff"),
    ('ﬁ', "fi"),
    ('ﬂ', "fl"),
    ('ﬃ', "ffi"),
    ('ﬄ', "ffl"),
    ('ﬅ', "ft"),
    ('ﬆ', "st"),
];

static HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<head[^>]*>(.*?)</head>").expect("valid pattern"));
static SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<span.*?</span>").expect("valid pattern"));
static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(\S+?);").expect("valid pattern"));
static MS_IF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</{0,1}!\[(end){0,1}if\]{0,1}>").expect("valid pattern"));

/// Python's `str.isspace`, which `\s` follows in a `str` pattern.
pub(crate) fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// `structure.py` `isspace`: empty, or blank once its no-break spaces are taken out, but not
/// made only of them.
pub(crate) fn calibre_isspace(s: &str) -> bool {
    s.is_empty() || {
        let mut rest = s.chars().filter(|&c| c != '\u{a0}').peekable();
        rest.peek().is_some() && rest.all(py_space)
    }
}

/// `HTMLPreProcessor.__call__` for a book that is not Baen's, Book Designer's or calibre's
/// pdftohtml output, with no search and replace (`html_preprocess_rules`): ligatures split,
/// runs of 10,000 spaces dropped, spans out of the head, entities replaced, Word's
/// `<![if …]>` dropped.
fn preprocess(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    for c in html.chars() {
        if c == '\0' {
            continue;
        }
        match LIGATURES.iter().find(|(l, _)| *l == c) {
            Some((_, s)) => out.push_str(s),
            None => out.push(c),
        }
    }
    let out = drop_long_space_runs(&out);
    let out = HEAD.replace_all(&out, |c: &regex::Captures| {
        let inner = c[1].trim_matches(py_space);
        format!("<head>\n{}\n</head>", SPAN.replace_all(inner, ""))
    });
    let out = ENTITY.replace_all(&out, |c: &regex::Captures| replace_entities(&c[0], true));
    MS_IF.replace_all(&out, "").into_owned()
}

/// `re.compile(r'\s{10000,}')` replaced with nothing.
fn drop_long_space_runs(s: &str) -> Cow<'_, str> {
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut run_start = None;
    let mut run_len = 0usize;
    let mut flush = |start: usize, end: usize, len: usize, out: &mut Option<String>| {
        if len >= 10_000 {
            let o = out.get_or_insert_with(|| String::with_capacity(s.len()));
            o.push_str(&s[copied..start]);
            copied = end;
        }
    };
    for (i, c) in s.char_indices() {
        if py_space(c) {
            run_start.get_or_insert(i);
            run_len += 1;
        } else if let Some(start) = run_start.take() {
            flush(start, i, run_len, &mut out);
            run_len = 0;
        }
    }
    if let Some(start) = run_start {
        flush(start, s.len(), run_len, &mut out);
    }
    match out {
        Some(mut o) => {
            o.push_str(&s[copied..]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(s),
    }
}

/// `calibre/__init__.py` `xml_replace_entities` (`keep_xml` true) and `replace_entities`: the
/// C function `replace_all_entities` (`ebooks/html_entities.c`). An entity is `&`, then ASCII
/// letters and digits (or `#` first), then `;`; a named one is looked up in the HTML 5 table,
/// a numeric one read as a code point. With `keep_xml`, one that stands for `<`, `>`, `&`, `"`
/// or `'` is kept.
fn replace_entities(s: &str, keep_xml: bool) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let Some(off) = b[i..].iter().position(|&c| c == b'&') else {
            out.extend_from_slice(&b[i..]);
            break;
        };
        out.extend_from_slice(&b[i..i + off]);
        let start = i + off;
        // `process_entity`
        let mut pos = start + 1;
        let mut done = false;
        while pos < b.len() {
            let ch = b[pos];
            pos += 1;
            if ch.is_ascii_alphanumeric() || (ch == b'#' && pos == start + 2) {
                continue;
            }
            if ch == b';' {
                add_entity(&b[start..pos], &mut out, keep_xml);
                done = true;
            }
            break;
        }
        if !done {
            out.extend_from_slice(&b[start..pos]);
        }
        i = pos;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// `add_entity` and `convert_entity`: `entity` (with its `&` and `;`) converted, or copied.
fn add_entity(entity: &[u8], out: &mut Vec<u8>, keep_xml: bool) {
    let unsafe_xml = |c: u32| matches!(c, 0x3c | 0x3e | 0x26 | 0x22 | 0x27);
    let converted: Option<String> = (|| {
        if entity.len() > 64 || entity.len() < 3 {
            return None;
        }
        let inner = &entity[1..entity.len() - 1];
        if inner[0] == b'#' {
            if inner.len() < 2 {
                return None;
            }
            let (digits, radix) = match inner[1] {
                b'x' | b'X' if inner.len() > 2 => (&inner[2..], 16),
                b'x' | b'X' => return None,
                _ => (&inner[1..], 10),
            };
            let mut cp: u32 = 0;
            for &d in digits {
                let v = char::from(d).to_digit(radix)?;
                cp = cp.wrapping_mul(radix).wrapping_add(v);
            }
            if keep_xml && unsafe_xml(cp) {
                return None;
            }
            return Some(match cp {
                0 | 0x11_0000.. => String::new(),
                // `encode_utf8` writes a surrogate's bytes, which are not UTF-8.
                0xd800..=0xdfff => "\u{fffd}".to_owned(),
                _ => char::from_u32(cp).map(String::from).unwrap_or_default(),
            });
        }
        let name = std::str::from_utf8(inner).ok()?;
        let &(a, b) = html5ever::data::NAMED_ENTITIES.get(&format!("{name};"))?;
        let mut v: String = char::from_u32(a).into_iter().collect();
        if b != 0 {
            v.extend(char::from_u32(b));
        }
        if keep_xml && v.chars().count() == 1 && unsafe_xml(a) {
            return None;
        }
        Some(v)
    })();
    match converted {
        Some(v) => out.extend_from_slice(v.as_bytes()),
        None => out.extend_from_slice(entity),
    }
}

static MS_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"xmlns:(\S+?)=".*?microsoft.*?""#).expect("valid pattern"));

/// `parse_utils.py` `clean_word_doc`: empty elements in a Microsoft namespace removed. Calibre's
/// patterns match the same prefix and name at both ends with back references; the `regex`
/// crate has none, so the ends are compared after the match.
fn clean_word_doc(data: &str) -> String {
    let prefixes: Vec<String> = MS_PREFIX
        .captures_iter(data)
        .map(|c| regex::escape(&c[1]))
        .collect();
    if prefixes.is_empty() {
        return data.to_owned();
    }
    let alt = prefixes.join("|");
    let build = |p: String| {
        regex::RegexBuilder::new(&p)
            .size_limit(1 << 22)
            .build()
            .ok()
    };
    let (Some(pair), Some(single)) = (
        build(format!(
            r"(?s)<({alt}):([a-zA-Z0-9]+)[^>/]*?></({alt}):([a-zA-Z0-9]+)>"
        )),
        build(format!(r"<({alt}):([a-zA-Z0-9]+)[^>/]*?/>")),
    ) else {
        return data.to_owned();
    };
    let data = pair.replace_all(data, |c: &regex::Captures| {
        if c[1] == c[3] && c[2] == c[4] {
            String::new()
        } else {
            c[0].to_owned()
        }
    });
    single.replace_all(&data, "").into_owned()
}

// ---------------------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------------------

/// `safe_xml_fromstring(data, recover=False)` then `check_for_html5`: the tree, or None when
/// `data` is not XML `parse_xml` reads, or is un-namespaced HTML 5 with an `<svg>`.
/// [`Error::TooLarge`] past [`MAX_DOM_NODES`] nodes: read as HTML it would stop there too.
fn xml_tree(data: &str, pre: &str) -> Result<Option<Dom>, Error> {
    let options = ParsingOptions {
        allow_dtd: true,
        // Past it the file is refused: read as HTML it would stop at the same number.
        nodes_limit: u32::try_from(MAX_DOM_NODES).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let doc = match deckle_limits::parse_xml_limiting_attributes(data, options, MAX_TAG_ATTRIBUTES)
    {
        Ok(doc) => doc,
        Err(XmlError::Parse(deckle_limits::roxmltree::Error::NodesLimitReached)) => {
            return Err(Error::TooManyNodes(MAX_DOM_NODES));
        }
        // Past it the file is refused: read as HTML the tag would cost as much.
        Err(XmlError::TooManyAttributes(max)) => return Err(Error::TooManyAttributes(max)),
        Err(_) => return Ok(None),
    };
    let dom = from_xml(&doc);
    let html5 = HTML5_DOCTYPE.is_match(pre)
        && dom
            .descendants()
            .any(|n| dom.tag(n) == "svg" && dom.namespace(n).is_none());
    Ok((!html5).then_some(dom))
}

/// The elements and text of `doc`, as lxml holds them, uncleaned.
pub(crate) fn from_xml(doc: &Document) -> Dom {
    let root = doc.root_element();
    let mut dom = Dom::new(root.tag_name().name(), root.tag_name().namespace());
    let r = dom.root;
    let attrs = xml_attrs(&mut dom, &root);
    dom.el_mut(r).attrs = attrs;
    // roxmltree's node index → the element made for it.
    let mut made: Vec<Option<NodeId>> = Vec::new();
    let set = |made: &mut Vec<Option<NodeId>>, n: Node, id: NodeId| {
        let i = n.id().get_usize();
        if made.len() <= i {
            made.resize(i + 1, None);
        }
        made[i] = Some(id);
    };
    set(&mut made, root, r);
    for n in root.descendants().skip(1) {
        let Some(parent) = n
            .parent()
            .and_then(|p| made.get(p.id().get_usize()).copied().flatten())
        else {
            continue;
        };
        if n.is_element() {
            let attrs = xml_attrs(&mut dom, &n);
            let id = dom.alloc(n.tag_name().name(), n.tag_name().namespace(), attrs);
            dom.attach(parent, id);
            set(&mut made, n, id);
        } else if let Some(t) = n.text().filter(|_| n.is_text()) {
            dom.append_text(parent, t);
        } else if n.is_comment() || n.is_pi() {
            // lxml's `text` of a processing instruction is its value.
            let text = match n.pi() {
                Some(pi) => pi.value.unwrap_or(""),
                None => n.text().unwrap_or(""),
            };
            let id = dom.alloc_comment(text);
            dom.attach(parent, id);
        }
    }
    dom
}

fn xml_attrs(dom: &mut Dom, n: &Node) -> Vec<Attr> {
    n.attributes()
        .map(|a| {
            let name = match a.namespace() {
                Some(XML_NS) => format!("xml:{}", a.name()),
                Some(uri) => match n.lookup_prefix(uri) {
                    Some(p) if !p.is_empty() => format!("{p}:{}", a.name()),
                    _ => a.name().to_owned(),
                },
                None => a.name().to_owned(),
            };
            dom.new_attr(&name, a.namespace(), a.value().to_owned())
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// HTML
// ---------------------------------------------------------------------------------------

/// `html5_parse`: `data`, its characters XML does not allow dropped (`clean_xml_chars`), read
/// with `html5ever` where Calibre uses `html5-parser` (DIVERGENCES.md, "XHTML that is not
/// well-formed").
fn html_tree(data: &str) -> Result<Dom, Error> {
    let data: String = data.chars().filter(|&c| xml_char(c)).collect();
    // Every node the input accounts for, and room for the elements the HTML algorithm adds,
    // up to the nodes a file's tree may hold.
    let budget = (data.len() + 1024).min(MAX_DOM_NODES);
    let sink = Sink::new(budget);
    let opts = html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut parser = html5ever::parse_document(sink, opts);
    // Fed in pieces, so a parse that has gone past a limit stops within one piece: `html5ever`
    // cannot be stopped from the sink, and one piece can rebuild the formatting elements at
    // most a few times.
    let mut at = 0;
    while at < data.len() {
        let mut end = (at + 64).min(data.len());
        while !data.is_char_boundary(end) {
            end += 1;
        }
        parser.process(StrTendril::from_slice(&data[at..end]));
        at = end;
        if let Some(e) = parser.tokenizer.sink.sink.stopped.take() {
            return Err(e);
        }
    }
    let sink = parser.finish();
    if let Some(e) = sink.stopped.take() {
        return Err(e);
    }
    sink.into_dom()
}

/// `speedup.c` `clean_xml_chars`: the characters it keeps.
fn xml_char(c: char) -> bool {
    matches!(c as u32,
        0x20..=0x7e | 0x9 | 0xa | 0xd | 0x85 | 0xa0..=0xd7ff | 0xe000..=0xfdcf
        | 0xfdf0..=0xfffd | 0x10000..)
}

enum Kind {
    Document,
    Element {
        attrs: Vec<Attribute>,
        mathml_annotation_xml_integration_point: bool,
    },
    Text(String),
    /// A comment or processing instruction, and its text (a processing instruction's value).
    Comment(String),
}

struct SinkNode {
    name: QualName,
    kind: Kind,
    parent: Option<usize>,
    first: Option<usize>,
    last: Option<usize>,
    prev: Option<usize>,
    next: Option<usize>,
}

/// An `html5ever` tree sink with linked children, so moving a node costs the same whatever its
/// siblings. The DOCTYPE is not kept.
///
/// Past its budget an element is only a name in `over` (its handle `OVER + index`), which
/// keeps `html5ever`'s stack and formatting list consistent while it finishes the piece of
/// input it is in; nothing is attached to it.
struct Sink {
    nodes: RefCell<Vec<SinkNode>>,
    over: RefCell<Vec<QualName>>,
    /// Nodes and attributes made so far: an element costs one and one per attribute.
    used: Cell<usize>,
    budget: usize,
    stopped: Cell<Option<Error>>,
}

const DOCUMENT: usize = 0;
/// The first handle of an element made past the budget.
const OVER: usize = usize::MAX / 2;

impl Sink {
    fn new(budget: usize) -> Sink {
        let node = |kind| SinkNode {
            name: QualName::new(None, ns!(), local_name!("")),
            kind,
            parent: None,
            first: None,
            last: None,
            prev: None,
            next: None,
        };
        Sink {
            nodes: RefCell::new(vec![node(Kind::Document)]),
            over: RefCell::new(Vec::new()),
            used: Cell::new(1),
            budget,
            stopped: Cell::new(None),
        }
    }

    fn stop(&self, e: Error) {
        let prev = self.stopped.take();
        self.stopped.set(prev.or(Some(e)));
    }

    /// A handle that is a node of the sink's tree (not past the budget).
    fn kept(h: usize) -> bool {
        h < OVER
    }

    /// A new node, or a name past the budget. It costs one, and one per attribute: the HTML
    /// algorithm copies a formatting element with all its attributes each time it rebuilds it.
    fn push(&self, name: QualName, kind: Kind) -> usize {
        let cost = 1 + match &kind {
            Kind::Element { attrs, .. } => attrs.len(),
            _ => 0,
        };
        let used = self.used.get().saturating_add(cost);
        if used > self.budget {
            self.stop(Error::TooLarge(self.budget));
            let mut over = self.over.borrow_mut();
            over.push(name);
            return OVER + over.len() - 1;
        }
        self.used.set(used);
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(SinkNode {
            name,
            kind,
            parent: None,
            first: None,
            last: None,
            prev: None,
            next: None,
        });
        nodes.len() - 1
    }

    fn parent_of(&self, h: usize) -> Option<usize> {
        if Self::kept(h) {
            self.nodes.borrow()[h].parent
        } else {
            None
        }
    }

    fn detach(nodes: &mut [SinkNode], n: usize) {
        let Some(p) = nodes[n].parent.take() else {
            return;
        };
        let (prev, next) = (nodes[n].prev.take(), nodes[n].next.take());
        match prev {
            Some(s) => nodes[s].next = next,
            None => nodes[p].first = next,
        }
        match next {
            Some(s) => nodes[s].prev = prev,
            None => nodes[p].last = prev,
        }
    }

    /// Links the parentless `n` into `parent`, before `before` or last.
    fn link(nodes: &mut [SinkNode], parent: usize, n: usize, before: Option<usize>) {
        let prev = match before {
            Some(b) => nodes[b].prev,
            None => nodes[parent].last,
        };
        nodes[n].parent = Some(parent);
        nodes[n].prev = prev;
        nodes[n].next = before;
        match prev {
            Some(s) => nodes[s].next = Some(n),
            None => nodes[parent].first = Some(n),
        }
        match before {
            Some(b) => nodes[b].prev = Some(n),
            None => nodes[parent].last = Some(n),
        }
    }

    /// Puts `child` into `parent` before `before` (or last), joining text to a text node it
    /// follows.
    fn insert(&self, parent: usize, child: NodeOrText<usize>, before: Option<usize>) {
        if !Self::kept(parent) {
            return;
        }
        match child {
            NodeOrText::AppendNode(n) => {
                if !Self::kept(n) {
                    return;
                }
                let mut nodes = self.nodes.borrow_mut();
                Self::detach(&mut nodes, n);
                Self::link(&mut nodes, parent, n, before);
            }
            NodeOrText::AppendText(t) => {
                {
                    let mut nodes = self.nodes.borrow_mut();
                    let prev = match before {
                        Some(b) => nodes[b].prev,
                        None => nodes[parent].last,
                    };
                    if let Some(p) = prev
                        && let Kind::Text(s) = &mut nodes[p].kind
                    {
                        s.push_str(&t);
                        return;
                    }
                }
                let n = self.push(
                    QualName::new(None, ns!(), local_name!("")),
                    Kind::Text(t.to_string()),
                );
                if Self::kept(n) {
                    Self::link(&mut self.nodes.borrow_mut(), parent, n, before);
                }
            }
        }
    }

    /// How deep `n` sits under the document, counted up to `MAX_DEPTH + 1`.
    fn depth(&self, n: usize) -> usize {
        if !Self::kept(n) {
            return 0;
        }
        let nodes = self.nodes.borrow();
        let mut d = 0;
        let mut at = n;
        while let Some(p) = nodes[at].parent {
            d += 1;
            if d > MAX_DEPTH {
                break;
            }
            at = p;
        }
        d
    }

    /// The tree as a [`Dom`], from the document's `<html>`; [`Error::TooDeep`] past
    /// `MAX_DEPTH`.
    fn into_dom(self) -> Result<Dom, Error> {
        let nodes = self.nodes.into_inner();
        let children = |n: usize| std::iter::successors(nodes[n].first, |&c| nodes[c].next);
        let Some(html) =
            children(DOCUMENT).find(|&c| matches!(nodes[c].kind, Kind::Element { .. }))
        else {
            return Ok(Dom::new("html", Some(super::XHTML_NS)));
        };
        let declared = declared_prefixes(&nodes[html].kind);
        let (local, ns) = html_name(&nodes[html].name);
        let mut dom = Dom::new(&local, ns);
        let r = dom.root;
        let attrs = html_attrs(&mut dom, &nodes[html].kind, &declared);
        dom.el_mut(r).attrs = attrs;
        let mut stack = vec![(html, r, 0usize)];
        while let Some((s, d, depth)) = stack.pop() {
            for c in children(s) {
                match &nodes[c].kind {
                    Kind::Text(t) => dom.append_text(d, t),
                    Kind::Element { .. } => {
                        if depth + 1 > MAX_DEPTH {
                            return Err(Error::TooDeep);
                        }
                        let (local, ns) = html_name(&nodes[c].name);
                        let attrs = html_attrs(&mut dom, &nodes[c].kind, &declared);
                        let id = dom.alloc(&local, ns, attrs);
                        dom.attach(d, id);
                        stack.push((c, id, depth + 1));
                    }
                    Kind::Comment(t) => {
                        let id = dom.alloc_comment(t);
                        dom.attach(d, id);
                    }
                    Kind::Document => {}
                }
            }
        }
        Ok(dom)
    }
}

/// An element's local name, made a valid XML name (`html5-parser`'s `sanitize_names`), and its
/// namespace.
fn html_name(name: &QualName) -> (String, Option<&str>) {
    let ns: &str = &name.ns;
    (xml_name(&name.local), (!ns.is_empty()).then_some(ns))
}

/// `name` with each character an XML name may not hold replaced by `_`.
fn xml_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, c) in name.chars().enumerate() {
        let ok = c.is_alphabetic()
            || c == '_'
            || c == ':'
            || (i > 0 && (c.is_alphanumeric() || c == '-' || c == '.'))
            || (!c.is_ascii() && i > 0);
        out.push(if ok { c } else { '_' });
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// The `xmlns:prefix` attributes of the root, which `html5ever` reads as plain attributes.
fn declared_prefixes(kind: &Kind) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Kind::Element { attrs, .. } = kind {
        for a in attrs {
            if let Some(p) = a.name.local.strip_prefix("xmlns:") {
                out.insert(p.to_owned(), a.value.to_string());
            }
        }
    }
    out.insert("xml".to_owned(), XML_NS.to_owned());
    out
}

/// An element's attributes by qualified name, namespace declarations left out, a prefix the
/// root declares resolved, a name seen before (after it is made a valid XML name) left out.
fn html_attrs(dom: &mut Dom, kind: &Kind, declared: &BTreeMap<String, String>) -> Vec<Attr> {
    let Kind::Element { attrs, .. } = kind else {
        return Vec::new();
    };
    let mut out: Vec<Attr> = Vec::with_capacity(attrs.len());
    let mut seen: HashSet<String> = HashSet::with_capacity(attrs.len());
    for a in attrs {
        let local: &str = &a.name.local;
        let ns: &str = &a.name.ns;
        if ns == XMLNS_NS || local == "xmlns" || local.starts_with("xmlns:") {
            continue;
        }
        let name = match &a.name.prefix {
            Some(p) => format!("{}:{}", &**p, xml_name(local)),
            None => xml_name(local),
        };
        let ns = if ns.is_empty() {
            name.split_once(':')
                .and_then(|(p, _)| declared.get(p))
                .cloned()
        } else {
            Some(ns.to_owned())
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        out.push(dom.new_attr(&name, ns.as_deref(), a.value.to_string()));
    }
    out
}

impl TreeSink for Sink {
    type Handle = usize;
    type Output = Self;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self {
        self
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> usize {
        DOCUMENT
    }

    fn elem_name<'a>(&'a self, target: &'a usize) -> Ref<'a, QualName> {
        match target.checked_sub(OVER) {
            Some(i) => Ref::map(self.over.borrow(), |o| &o[i]),
            None => Ref::map(self.nodes.borrow(), |n| &n[*target].name),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> usize {
        self.push(
            name,
            Kind::Element {
                attrs,
                mathml_annotation_xml_integration_point: flags
                    .mathml_annotation_xml_integration_point,
            },
        )
    }

    fn create_comment(&self, text: StrTendril) -> usize {
        self.push(
            QualName::new(None, ns!(), local_name!("")),
            Kind::Comment(text.to_string()),
        )
    }

    fn create_pi(&self, _target: StrTendril, data: StrTendril) -> usize {
        self.push(
            QualName::new(None, ns!(), local_name!("")),
            Kind::Comment(data.to_string()),
        )
    }

    fn append(&self, parent: &usize, child: NodeOrText<usize>) {
        // `parent`'s depth under the document is the child's under `<html>`.
        if let NodeOrText::AppendNode(_) = &child
            && self.depth(*parent) > MAX_DEPTH
        {
            self.stop(Error::TooDeep);
            return;
        }
        self.insert(*parent, child, None);
    }

    fn append_based_on_parent_node(
        &self,
        element: &usize,
        prev_element: &usize,
        child: NodeOrText<usize>,
    ) {
        if self.parent_of(*element).is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn get_template_contents(&self, target: &usize) -> usize {
        // `html5-parser` keeps a template's contents as its children.
        *target
    }

    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &usize, new_node: NodeOrText<usize>) {
        if let Some(parent) = self.parent_of(*sibling) {
            self.insert(parent, new_node, Some(*sibling));
        }
    }

    fn add_attrs_if_missing(&self, target: &usize, attrs: Vec<Attribute>) {
        if !Self::kept(*target) {
            return;
        }
        if let Kind::Element { attrs: have, .. } = &mut self.nodes.borrow_mut()[*target].kind {
            // Each new attribute is compared with those already there: a repeated `<html>` or
            // `<body>` tag costs what it scans, so many of them stop.
            let used = self
                .used
                .get()
                .saturating_add(have.len().saturating_mul(attrs.len()));
            if used > self.budget {
                self.stop(Error::TooLarge(self.budget));
                return;
            }
            self.used.set(used);
            for a in attrs {
                if !have.iter().any(|h| h.name == a.name) {
                    have.push(a);
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &usize) {
        if Self::kept(*target) {
            Self::detach(&mut self.nodes.borrow_mut(), *target);
        }
    }

    fn reparent_children(&self, node: &usize, new_parent: &usize) {
        if !Self::kept(*node) || !Self::kept(*new_parent) {
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        while let Some(c) = nodes[*node].first {
            Self::detach(&mut nodes, c);
            Self::link(&mut nodes, *new_parent, c, None);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &usize) -> bool {
        Self::kept(*handle)
            && matches!(
                self.nodes.borrow()[*handle].kind,
                Kind::Element {
                    mathml_annotation_xml_integration_point: true,
                    ..
                }
            )
    }
}
