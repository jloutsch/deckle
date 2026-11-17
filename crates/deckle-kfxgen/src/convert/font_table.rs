// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's `font_table.py` at `ad38ec8`: the weight and style readers a run's and a block's
//! emphasis use, and the fonts the book embeds as `generate_full_book` receives them
//! (`build_font_table`, `faces_from_rules`, `FontTable`; `_font_table_for` in `converter.py`).
//!
//! A WOFF is unpacked to the bytes fontTools' `TTFont.save(reorderTables=None)` writes for it
//! (`ttLib/ttFont.py` and `SFNTWriter` in `ttLib/sfnt.py`, fontTools 4.60.1, MIT, `NOTICE`):
//! the tables copied raw in fontTools' order, padded to 4 bytes, with its checksums. A WOFF2
//! face is refused: fontTools rebuilds its glyphs with its own compiler, which this port does
//! not have. kfxgen's `KFXGEN_MAX_FONT_BYTES` is not read: its default is [`MAX_FONT_BYTES`].
//!
//! Hostile input: a font is only read from the archive, through the manifest item a `src`
//! names (by href, else by file name, as kfxgen looks it up), so no path from the book reaches
//! the file system. Every offset and length a font states is checked against its bytes. A
//! WOFF's tables are inflated no further than the lengths they declare, which together stay
//! within [`MAX_FONT_BYTES`] for a font (kfxgen's `_check_woff`) and [`MAX_BOOK_FONT_BYTES`]
//! for the book, charged before inflating. A WOFF's directory holds at most 65,535 entries
//! (1.3 MB); one naming 4,096 tables or more, or a tag that is not ASCII, is skipped, as
//! fontTools cannot write it. Each file is worked out once however often the rules name it.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use deckle_kfx::epub_zip::{EpubReader, InspectError};
use flate2::{Decompress, FlushDecompress, Status};
use regex::Regex;
use sha1::{Digest, Sha1};

use super::{ConvertOptions, Error};
use crate::book::{self, Book};
use crate::css::Decls;
use crate::epub::package::unquote;
use crate::epub::{Item, data_is_bytes};
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `MAX_FONT_BYTES`: the most bytes a face embeds, and a WOFF's tables unpack to.
const MAX_FONT_BYTES: usize = 50 * 1024 * 1024;

/// The most bytes the book's WOFF tables may inflate to together, against a book of many
/// fonts each just under [`MAX_FONT_BYTES`]; past it the book is refused. kfxgen has no such
/// bound.
const MAX_BOOK_FONT_BYTES: u64 = 256 * 1024 * 1024;

/// `parse_weight`: a CSS font-weight as a number, 400 for none or one `int()` can't read.
/// Past an i64 it is `i64::MAX` or `i64::MIN` (`py::py_int`); Python's int has no bound.
pub fn parse_weight(v: Option<&PyValue>) -> i64 {
    match v {
        Some(PyValue::Str(s)) => match py::strip(s).to_lowercase().as_str() {
            "bold" => 700,
            "normal" => 400,
            s => py::py_int(s).unwrap_or(400),
        },
        // str() of an int reads back as it; of a float (always a point, an exponent, inf or
        // nan), a bool or None, never.
        Some(PyValue::Int(i)) => *i,
        _ => 400,
    }
}

/// `parse_style`: italic or oblique.
pub fn parse_style(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Str(s)) => {
            matches!(py::strip(s).to_lowercase().as_str(), "italic" | "oblique")
        }
        _ => false,
    }
}

/// `_font_table_for`: the book's `FontTable`, as the model holds it (`faces`, then
/// `_by_family`), from the `@font-face` rules of its spine files and the fonts of the EPUB at
/// `path`. Empty when `kfxgen_disable_font_embedding` is on. A WOFF2 face that would be chosen
/// is [`Error::Unsupported`].
pub fn font_table(book: &Book, path: &Path, opts: &ConvertOptions) -> Result<PyValue, Error> {
    if opts.disable_font_embedding || book.font_face.is_empty() {
        return Ok(table(Vec::new()));
    }
    let mut fonts = Fonts::new(&book.oeb.manifest, path)?;
    let rules = &book.font_face;
    // Faces some rule gives as TTF/OTF: a rule giving one of them only as WOFF adds nothing.
    let mut sfnt_faces = HashSet::new();
    for rule in rules {
        for href in src_urls(field(rule, "src")) {
            if let Some((_, Kind::Sfnt)) = fonts.src(&href)? {
                sfnt_faces.insert(face_key(rule));
                break;
            }
        }
    }
    let mut faces = Vec::new();
    let mut names = Names::default();
    let mut seen_hrefs = HashSet::new();
    for rule in rules {
        let family = normalize_family(field(rule, "font-family"));
        if family.is_empty() {
            continue;
        }
        let mut srcs = Vec::new();
        for href in src_urls(field(rule, "src")) {
            if let Some((i, kind)) = fonts.src(&href)? {
                srcs.push((href, i, kind));
            }
        }
        if !srcs.iter().any(|(.., k)| *k == Kind::Sfnt) && sfnt_faces.contains(&face_key(rule)) {
            continue;
        }
        // TTF/OTF first, in the author's order; a WOFF only when none fits.
        srcs.sort_by_key(|(.., k)| *k != Kind::Sfnt);
        let mut chosen = None;
        for (href, i, _) in srcs {
            if let Some(data) = fonts.embeds(i, &href)? {
                chosen = Some((href, data));
                break;
            }
        }
        let Some((href, data)) = chosen else {
            continue;
        };
        if !seen_hrefs.insert(href) {
            continue;
        }
        let weight = parse_weight(field(rule, "font-weight").map(str_value).as_ref());
        let italic = parse_style(field(rule, "font-style").map(str_value).as_ref());
        let emitted_family = names.emitted_name(&family, weight, italic);
        let location = format!("resource/font{}", faces.len());
        faces.push(face(family, weight, italic, data, emitted_family, location));
    }
    Ok(table(faces))
}

/// `rule_field`: the value of `key` in an `@font-face` rule, None when it has none.
fn field<'a>(rule: &'a Decls, key: &str) -> Option<&'a str> {
    rule.iter()
        .find(|(k, _)| k == key)
        .map(|(_, d)| d.value.as_str())
        .filter(|v| !v.is_empty())
}

fn str_value(s: &str) -> PyValue {
    PyValue::Str(s.to_owned())
}

/// `_face_key`.
fn face_key(rule: &Decls) -> (String, i64, bool) {
    (
        normalize_family(field(rule, "font-family")),
        parse_weight(field(rule, "font-weight").map(str_value).as_ref()),
        parse_style(field(rule, "font-style").map(str_value).as_ref()),
    )
}

/// `normalize_family`.
fn normalize_family(name: Option<&str>) -> String {
    let name = name.unwrap_or("");
    py::strip(py::strip(name).trim_matches(['"', '\''])).to_lowercase()
}

/// `_URL_RE`, with Python's `\s`.
static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"url\([\s\x1C-\x1F]*['"]?([^'")]+)['"]?[\s\x1C-\x1F]*\)"#)
        .expect("a valid pattern")
});

/// `extract_src_urls`: the `url()`s of a `src`, else the value itself.
fn src_urls(src: Option<&str>) -> Vec<String> {
    let Some(src) = src else {
        return Vec::new();
    };
    let urls: Vec<String> = URL
        .captures_iter(src)
        .map(|c| py::strip(&c[1]).to_owned())
        .collect();
    if urls.is_empty() {
        vec![py::strip(src).to_owned()]
    } else {
        urls
    }
}

static NOT_SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("[^a-z0-9]+").expect("a valid pattern"));

/// `_slug`: the family's name in `[a-z0-9-]`, with 8 hex digits of its sha1 when it is not
/// ASCII.
fn slug(name: &str) -> String {
    let norm = normalize_family(Some(name));
    let mut out = NOT_SLUG
        .replace_all(&norm, "-")
        .trim_matches('-')
        .to_owned();
    if !norm.is_ascii() {
        let hash: String = Sha1::digest(norm.as_bytes())[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        out = if out.is_empty() {
            format!("font-{hash}")
        } else {
            format!("{out}-{hash}")
        };
    }
    if out.is_empty() {
        "font".to_owned()
    } else {
        out
    }
}

/// The names `emitted_name` has given.
#[derive(Default)]
struct Names {
    taken: HashSet<String>,
    /// For each base, the suffix of the name last given: every earlier one is taken, so the
    /// search for the next starts there rather than at the base (one step a face, not one a
    /// face before it).
    last: HashMap<String, usize>,
}

impl Names {
    /// `emitted_name`: `<slug>-<weight>`, `i` when italic, then `-1`, `-2`… until unused.
    fn emitted_name(&mut self, family: &str, weight: i64, italic: bool) -> String {
        let base = format!("{}-{weight}{}", slug(family), if italic { "i" } else { "" });
        let mut n = self.last.get(&base).copied().unwrap_or(0);
        let name = loop {
            let name = if n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            if !self.taken.contains(&name) {
                break name;
            }
            n += 1;
        };
        self.last.insert(base, n);
        self.taken.insert(name.clone());
        name
    }
}

/// A `Face`'s state.
fn face(
    family: String,
    weight: i64,
    italic: bool,
    data: Arc<[u8]>,
    emitted_family: String,
    location: String,
) -> PyValue {
    obj(
        "kfxgen.font_table.Face",
        vec![
            ("css_family", PyValue::Str(family)),
            ("weight", PyValue::Int(weight)),
            ("italic", PyValue::Bool(italic)),
            ("data", PyValue::Bytes(data)),
            ("emitted_family", PyValue::Str(emitted_family)),
            ("location", PyValue::Str(location)),
        ],
    )
}

/// `FontTable(faces)`: the faces, and those of each family in order of first appearance.
fn table(faces: Vec<PyValue>) -> PyValue {
    let mut by_family: Vec<(&str, Vec<PyValue>)> = Vec::new();
    for f in &faces {
        let family = f
            .get("css_family")
            .and_then(PyValue::as_str)
            .unwrap_or_default();
        match by_family.iter_mut().find(|(k, _)| *k == family) {
            Some((_, list)) => list.push(f.clone()),
            None => by_family.push((family, vec![f.clone()])),
        }
    }
    let by_family = by_family
        .into_iter()
        .map(|(k, v)| (PyValue::Str(k.to_owned()), PyValue::List(v)))
        .collect();
    obj(
        "kfxgen.font_table.FontTable",
        vec![
            ("faces", PyValue::List(faces)),
            ("_by_family", PyValue::Dict(by_family)),
        ],
    )
}

fn obj(class: &str, state: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Obj {
        class: class.to_owned(),
        state: Box::new(PyValue::Dict(
            state
                .into_iter()
                .map(|(k, v)| (PyValue::Str(k.to_owned()), v))
                .collect(),
        )),
    }
}

/// A font file a `src` names: `is_ttf_otf`, or `is_woff` (WOFF or WOFF2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sfnt,
    Woff,
}

/// The book's font files: `build_manifest_lookup` over the archive, and what each file
/// embeds as, worked out once.
struct Fonts<'a> {
    zip: EpubReader,
    items: &'a [Item],
    by_href: HashMap<&'a str, usize>,
    /// By file name, the last item of a name winning. Calibre's manifest is a set, so which
    /// item kfxgen finds when two share a name varies from run to run; here it is the last in
    /// the OPF.
    by_base: HashMap<&'a str, usize>,
    kinds: HashMap<usize, Option<Kind>>,
    embeds: HashMap<usize, Option<Arc<[u8]>>>,
    /// The bytes WOFF tables were allowed to inflate to so far, against
    /// [`MAX_BOOK_FONT_BYTES`].
    inflated: u64,
}

impl<'a> Fonts<'a> {
    fn new(items: &'a [Item], path: &Path) -> Result<Self, Error> {
        let mut by_href = HashMap::new();
        let mut by_base = HashMap::new();
        for (i, item) in items.iter().enumerate() {
            // Calibre holds the other files parsed, not as bytes.
            if item.href.is_empty() || !data_is_bytes(&item.media_type.to_lowercase()) {
                continue;
            }
            by_href.insert(item.href.as_str(), i);
            by_base.insert(base_name(&item.href), i);
        }
        Ok(Fonts {
            zip: EpubReader::open(path).map_err(read_error)?,
            items,
            by_href,
            by_base,
            kinds: HashMap::new(),
            embeds: HashMap::new(),
            inflated: 0,
        })
    }

    /// The manifest item `href` names: by href, else by file name.
    fn lookup(&self, href: &str) -> Option<usize> {
        if href.is_empty() {
            return None;
        }
        (self
            .by_href
            .get(href)
            .or_else(|| self.by_base.get(base_name(href))))
        .copied()
    }

    /// The item's bytes from the archive, None when the archive lacks them.
    fn bytes(&mut self, i: usize) -> Result<Option<Vec<u8>>, Error> {
        match self.zip.read_bytes(&unquote(&self.items[i].href)) {
            Ok(data) => Ok(Some(data)),
            Err(InspectError::MissingFile(_)) => Ok(None),
            Err(e) => Err(read_error(e)),
        }
    }

    /// The manifest item `href` names and its kind, None when it names none or one that is
    /// no font.
    fn src(&mut self, href: &str) -> Result<Option<(usize, Kind)>, Error> {
        let Some(i) = self.lookup(href) else {
            return Ok(None);
        };
        if let Some(k) = self.kinds.get(&i) {
            return Ok(k.map(|k| (i, k)));
        }
        let magic = self.bytes(i)?.and_then(|d| d.get(..4).map(<[u8]>::to_vec));
        let kind = match magic.as_deref() {
            Some(b"\x00\x01\x00\x00" | b"OTTO" | b"true" | b"ttcf") => Some(Kind::Sfnt),
            Some(b"wOFF" | b"wOF2") => Some(Kind::Woff),
            _ => None,
        };
        self.kinds.insert(i, kind);
        Ok(kind.map(|k| (i, k)))
    }

    /// The TrueType/OpenType bytes the font of item `i` (named by `href`) embeds as, None when
    /// kfxgen skips it: over [`MAX_FONT_BYTES`], a WOFF that does not unpack, or a table
    /// directory `_check_sfnt` refuses. A WOFF2 within the cap is refused.
    fn embeds(&mut self, i: usize, href: &str) -> Result<Option<Arc<[u8]>>, Error> {
        if let Some(e) = self.embeds.get(&i) {
            return Ok(e.clone());
        }
        let data = self.bytes(i)?.unwrap_or_default();
        let data = if data.len() > MAX_FONT_BYTES {
            None
        } else if data.starts_with(b"wOF2") {
            return Err(Error::Unsupported {
                feature: "WOFF2 fonts",
                file: href.to_owned(),
            });
        } else if data.starts_with(b"wOFF") {
            self.sfnt_from_woff(&data, href)?
                .filter(|d| d.len() <= MAX_FONT_BYTES)
        } else {
            Some(data)
        };
        let out = data.filter(|d| check_sfnt(d)).map(Arc::from);
        self.embeds.insert(i, out.clone());
        Ok(out)
    }

    /// `sfnt_from_woff` for a WOFF: the bytes fontTools writes for it, None when kfxgen's
    /// `_check_woff` or fontTools refuses it. kfxgen blanks the metadata and private blocks'
    /// fields first, so neither is read.
    fn sfnt_from_woff(&mut self, data: &[u8], href: &str) -> Result<Option<Vec<u8>>, Error> {
        // `_check_woff`: the directory within the file, the declared total and each table's
        // within the cap, and no table longer compressed than not.
        if data.len() < WOFF_HEADER_LEN {
            return Ok(None);
        }
        let num_tables = usize::from(be16(data, 12));
        if be32(data, 16) as usize > MAX_FONT_BYTES {
            return Ok(None);
        }
        let mut entries = Vec::with_capacity(num_tables);
        let mut declared = 0usize;
        for i in 0..num_tables {
            let pos = WOFF_HEADER_LEN + i * WOFF_ENTRY_LEN;
            if pos + WOFF_ENTRY_LEN > data.len() {
                return Ok(None);
            }
            let tag: [u8; 4] = data[pos..pos + 4].try_into().expect("4 bytes");
            let (offset, length, orig) = (
                be32(data, pos + 4) as usize,
                be32(data, pos + 8) as usize,
                be32(data, pos + 12) as usize,
            );
            declared = declared.saturating_add(orig);
            if declared > MAX_FONT_BYTES || length > orig {
                return Ok(None);
            }
            entries.push((tag, offset, length, orig));
        }
        // fontTools reads only these, and its `SFNTWriter` cannot pack a `searchRange` past
        // 65,535 (4,096 tables) or a tag that is not ASCII.
        if !matches!(&data[4..8], b"\x00\x01\x00\x00" | b"OTTO" | b"true") {
            return Ok(None);
        }
        let mut last = BTreeMap::new();
        for (i, (tag, ..)) in entries.iter().enumerate() {
            last.insert(*tag, i);
        }
        if last.len() >= 4096 || last.keys().any(|t| !t.is_ascii()) {
            return Ok(None);
        }
        // Charged before inflating: `declared` bounds what the tables inflate to.
        self.inflated += declared as u64;
        if self.inflated > MAX_BOOK_FONT_BYTES {
            return Err(Error::TooLarge {
                what: format!(
                    "{href}: this book's WOFF fonts unpack to more than {} MiB together",
                    MAX_BOOK_FONT_BYTES / (1024 * 1024)
                ),
            });
        }
        // `_check_woff` inflates every compressed table, from what of it the file holds, no
        // further than one byte past its length; fontTools then reads the table a tag names
        // last, whole, and inflates it to its end.
        let mut inflated = Vec::with_capacity(entries.len());
        for &(_, offset, length, orig) in &entries {
            if length == orig {
                inflated.push(None);
                continue;
            }
            let start = offset.min(data.len());
            let end = offset.saturating_add(length).min(data.len());
            match inflate(&data[start..end], orig) {
                Some((out, ended)) if out.len() == orig => inflated.push(Some((out, ended))),
                _ => return Ok(None),
            }
        }
        let mut tables = BTreeMap::new();
        for (tag, i) in last {
            let (_, offset, length, _) = entries[i];
            // Python reads nothing past the end, so an empty table may lie there.
            let raw = match data.get(offset..offset.saturating_add(length)) {
                Some(raw) => raw,
                None if length == 0 => &[][..],
                None => return Ok(None),
            };
            match &inflated[i] {
                None => tables.insert(tag, raw),
                Some((out, true)) => tables.insert(tag, out.as_slice()),
                Some((_, false)) => return Ok(None),
            };
        }
        Ok(Some(write_sfnt(&data[4..8], &tables)))
    }
}

fn read_error(e: InspectError) -> Error {
    Error::Book(book::Error::from(e))
}

/// `href.rsplit("/", 1)[-1]`.
fn base_name(href: &str) -> &str {
    href.rsplit('/').next().unwrap_or(href)
}

/// Bytes before a WOFF's table directory, and in each of its entries.
const WOFF_HEADER_LEN: usize = 44;
const WOFF_ENTRY_LEN: usize = 20;

/// The big-endian u16 at `at`; the caller has checked it is within `data`.
fn be16(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn be32(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(data[at..at + 4].try_into().expect("4 bytes"))
}

/// A zlib stream inflated no further than just past `orig` bytes, and whether it reached its
/// end; None when zlib refuses it.
fn inflate(raw: &[u8], orig: usize) -> Option<(Vec<u8>, bool)> {
    let mut z = Decompress::new(true);
    let mut out = Vec::new();
    loop {
        if out.len() == out.capacity() {
            // Grown as the output comes: a small stream claiming a large table costs little.
            let room = (orig + 1 - out.len()).min(out.len().max(64 * 1024));
            out.reserve_exact(room);
        }
        let before = (z.total_in(), out.len());
        let input = &raw[z.total_in() as usize..];
        match z.decompress_vec(input, &mut out, FlushDecompress::None) {
            Ok(Status::StreamEnd) => return Some((out, true)),
            Ok(_) if out.len() <= orig && (z.total_in(), out.len()) != before => {}
            Ok(_) => return Some((out, false)),
            Err(_) => return None,
        }
    }
}

/// `_check_sfnt`: a TrueType/OpenType table directory within the file, with the tables every
/// font has. A collection (`ttcf`) is not read.
fn check_sfnt(data: &[u8]) -> bool {
    if data.starts_with(b"ttcf") {
        return true;
    }
    if data.len() < 12 {
        return false;
    }
    let num_tables = usize::from(be16(data, 4));
    if 12 + 16 * num_tables > data.len() {
        return false;
    }
    let mut tags = HashSet::new();
    for i in 0..num_tables {
        let at = 12 + 16 * i;
        let (offset, length) = (be32(data, at + 8), be32(data, at + 12));
        if u64::from(offset) + u64::from(length) > data.len() as u64 {
            return false;
        }
        tags.insert(&data[at..at + 4]);
    }
    [b"cmap", b"head", b"hhea", b"hmtx", b"maxp"]
        .iter()
        .all(|t| tags.contains(&t[..]))
        && [b"glyf", b"CFF ", b"CFF2"]
            .iter()
            .any(|t| tags.contains(&t[..]))
}

/// fontTools' `TTFTableOrder` and `OTFTableOrder`.
const TTF_ORDER: [&[u8; 4]; 19] = [
    b"head", b"hhea", b"maxp", b"OS/2", b"hmtx", b"LTSH", b"VDMX", b"hdmx", b"cmap", b"fpgm",
    b"prep", b"cvt ", b"loca", b"glyf", b"kern", b"name", b"post", b"gasp", b"PCLT",
];
const OTF_ORDER: [&[u8; 4]; 8] = [
    b"head", b"hhea", b"maxp", b"OS/2", b"name", b"cmap", b"post", b"CFF ",
];

/// The `dependencies` of fontTools 4.60.1's table classes: the tables written before one.
fn dependencies(tag: &[u8; 4]) -> &'static [&'static [u8; 4]] {
    match tag {
        b"CBLC" => &[b"CBDT"],
        b"EBLC" => &[b"EBDT"],
        b"GVAR" | b"gvar" => &[b"fvar", b"glyf"],
        b"Gloc" => &[b"Glat"],
        b"OS/2" => &[b"head"],
        b"TSI0" => &[b"TSI1"],
        b"TSI2" => &[b"TSI3"],
        b"avar" | b"glyf" => &[b"fvar"],
        b"cvar" => &[b"cvt ", b"fvar"],
        b"fvar" | b"trak" => &[b"name"],
        b"head" => &[b"maxp", b"loca", b"CFF ", b"CFF2"],
        b"hhea" => &[b"hmtx", b"glyf", b"CFF ", b"CFF2"],
        b"vhea" => &[b"vmtx", b"glyf", b"CFF ", b"CFF2"],
        b"loca" | b"maxp" => &[b"glyf"],
        b"name" => &[b"ltag"],
        _ => &[],
    }
}

/// The order `TTFont._save` writes tables in: `sortedTagList` (sorted, `DSIG` last, then the
/// recommended order first), each table's dependencies before it (`_writeTable`).
fn write_order(tables: &BTreeMap<[u8; 4], &[u8]>) -> Vec<[u8; 4]> {
    let mut rest: Vec<[u8; 4]> = tables.keys().copied().collect();
    if let Some(i) = rest.iter().position(|t| t == b"DSIG") {
        let dsig = rest.remove(i);
        rest.push(dsig);
    }
    let order: &[&[u8; 4]] = if tables.contains_key(b"CFF ") {
        &OTF_ORDER
    } else {
        &TTF_ORDER
    };
    let mut sorted = Vec::with_capacity(rest.len());
    for tag in order {
        if let Some(i) = rest.iter().position(|t| t == *tag) {
            sorted.push(rest.remove(i));
        }
    }
    sorted.extend(rest);

    fn visit(
        tag: [u8; 4],
        tables: &BTreeMap<[u8; 4], &[u8]>,
        done: &mut HashSet<[u8; 4]>,
        out: &mut Vec<[u8; 4]>,
    ) {
        if done.contains(&tag) {
            return;
        }
        for dep in dependencies(&tag) {
            if !done.contains(*dep) {
                if tables.contains_key(*dep) {
                    visit(**dep, tables, done, out);
                } else {
                    done.insert(**dep);
                }
            }
        }
        done.insert(tag);
        out.push(tag);
    }
    let mut done = HashSet::new();
    let mut out = Vec::with_capacity(sorted.len());
    for tag in sorted {
        visit(tag, tables, &mut done, &mut out);
    }
    out
}

/// `calcChecksum`: the sum of the big-endian words, the last padded with zeros.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// `SFNTWriter` (no flavour) given `tables` (fewer than 4,096) in [`write_order`]: the
/// directory, then each table padded to 4 bytes; `head`'s checksum taken with its
/// `checkSumAdjustment` zeroed, which then gets the whole font's (`writeMasterChecksum`).
fn write_sfnt(version: &[u8], tables: &BTreeMap<[u8; 4], &[u8]>) -> Vec<u8> {
    let n = tables.len();
    let mut out = vec![0u8; 12 + 16 * n];
    let mut directory = BTreeMap::new();
    for tag in write_order(tables) {
        let data = tables[&tag];
        let sum = if &tag == b"head" {
            // `data[:8] + b"\0\0\0\0" + data[12:]`
            let mut head = data[..data.len().min(8)].to_vec();
            head.extend([0; 4]);
            head.extend(data.get(12..).unwrap_or_default());
            checksum(&head)
        } else {
            checksum(data)
        };
        directory.insert(tag, (sum, out.len() as u32, data.len() as u32));
        out.extend(data);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    // `getSearchRange(n, 16)`.
    let exponent = if n == 0 { 0 } else { n.ilog2() };
    let search_range = (1usize << exponent) * 16;
    let mut dir = version.to_vec();
    dir.extend((n as u16).to_be_bytes());
    dir.extend((search_range as u16).to_be_bytes());
    dir.extend((exponent as u16).to_be_bytes());
    dir.extend(((n * 16).saturating_sub(search_range) as u16).to_be_bytes());
    for (tag, (sum, offset, length)) in &directory {
        dir.extend(tag);
        dir.extend(sum.to_be_bytes());
        dir.extend(offset.to_be_bytes());
        dir.extend(length.to_be_bytes());
    }
    if let Some((_, head_offset, _)) = directory.get(b"head") {
        let total = directory
            .values()
            .fold(checksum(&dir), |s, (sum, ..)| s.wrapping_add(*sum));
        let at = *head_offset as usize + 8;
        // A `head` shorter than 12 bytes: fontTools writes past it all the same.
        if out.len() < at + 4 {
            out.resize(at + 4, 0);
        }
        out[at..at + 4].copy_from_slice(&0xB1B0_AFBAu32.wrapping_sub(total).to_be_bytes());
    }
    out[..dir.len()].copy_from_slice(&dir);
    out
}

#[cfg(test)]
mod tests {
    use super::Names;

    /// As kfxgen's `emitted_name` gave them: a name another base took is passed over.
    #[test]
    fn a_name_another_base_took_is_passed_over() {
        let mut names = Names::default();
        let got: Vec<String> = [("a-400", 1), ("a", 400), ("a", 400), ("a", 400)]
            .into_iter()
            .map(|(f, w)| names.emitted_name(f, w, false))
            .collect();
        assert_eq!(got, ["a-400-1", "a-400", "a-400-2", "a-400-3"]);
    }
}
