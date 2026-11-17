// SPDX-License-Identifier: GPL-3.0-only
//! The book's embedded fonts as kfxgen hands them to `generate_full_book` (plan Task 19):
//! against every model, and on hostile fonts.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use deckle_kfx::fixtures;
use deckle_kfxgen::book::read_book;
use deckle_kfxgen::convert::{self, ConvertOptions, Error};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::convert_args::{dict, sources};
use crate::convert_support::model_diff;
use crate::support::{COST_LIMIT, allocated_by};

const MIB: usize = 1024 * 1024;

#[test]
fn font_tables_match_every_model() {
    let mut failures = Vec::new();
    let mut faces = 0;
    for name in sources() {
        let path = fixtures::kfxgen(&format!("sources/{name}.epub"));
        let book = read_book(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let model = load_model(&fixtures::kfxgen(&format!("models/{name}")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let want = dict(&model.args)["font_table"];
        match convert::font_table(&book, &path, &ConvertOptions::default()) {
            Ok(ours) => {
                failures.extend(
                    model_diff(&ours, want)
                        .into_iter()
                        .map(|l| format!("{name}: {l}")),
                );
                faces += ours
                    .get("faces")
                    .and_then(PyValue::as_list)
                    .map_or(0, <[_]>::len);
            }
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // `fonts` (a TTF) and `fonts_woff` (a WOFF unpacked as fontTools writes it).
    assert_eq!(faces, 2);
}

// ---------------------------------------------------------------------------------------
// Books made in the test
// ---------------------------------------------------------------------------------------

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// An EPUB at `dir/book.epub` whose page links `s.css` holding `css`, with `fonts` in its
/// manifest and archive (`(href, bytes)`, each of media type `font/ttf`) and `extra` in its
/// archive only.
fn font_book(dir: &Path, css: &str, fonts: &[(&str, &[u8])], extra: &[(&str, &[u8])]) -> PathBuf {
    let items: String = fonts
        .iter()
        .enumerate()
        .map(|(i, (href, _))| format!(r#"<item id="f{i}" href="{href}" media-type="font/ttf"/>"#))
        .collect();
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language></metadata><manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/><item id="s" href="s.css" media-type="text/css"/>{items}</manifest><spine><itemref idref="c"/></spine></package>"#
    );
    let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title><link rel="stylesheet" type="text/css" href="s.css"/></head><body><p>Hi.</p></body></html>"#;
    let mut entries: Vec<(&str, &[u8])> = vec![
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", CONTAINER),
        ("content.opf", opf.as_bytes()),
        ("c.xhtml", page.as_bytes()),
        ("s.css", css.as_bytes()),
    ];
    entries.extend(fonts.iter().chain(extra));
    let path = dir.join("book.epub");
    // `zip`'s writer, stored: its CRC is fast enough for books of 300 MiB of fonts.
    let mut zip = ZipWriter::new(std::fs::File::create(&path).unwrap());
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, data) in entries {
        zip.start_file(name, stored).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
    path
}

fn table_of(path: &Path) -> Result<PyValue, Error> {
    let book = read_book(path).unwrap();
    convert::font_table(&book, path, &ConvertOptions::default())
}

/// The faces' `(css_family, data)`.
fn faces(table: &PyValue) -> Vec<(String, Vec<u8>)> {
    table
        .get("faces")
        .and_then(PyValue::as_list)
        .unwrap()
        .iter()
        .map(|f| {
            (
                f.get("css_family")
                    .and_then(PyValue::as_str)
                    .unwrap()
                    .to_owned(),
                f.get("data").and_then(PyValue::as_bytes).unwrap().to_vec(),
            )
        })
        .collect()
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// A TrueType font of `tables` (tag, data), each where its directory says, with what
/// `_check_sfnt` asks for when `tables` is empty. `pad` zero bytes go after the last table.
fn sfnt(tables: &[(&[u8; 4], Vec<u8>)], pad: usize) -> Vec<u8> {
    let default: Vec<(&[u8; 4], Vec<u8>)>;
    let tables = if tables.is_empty() {
        default = [b"cmap", b"glyf", b"head", b"hhea", b"hmtx", b"maxp"]
            .into_iter()
            .enumerate()
            .map(|(i, t)| (t, vec![i as u8 + 1; 54]))
            .collect();
        &default[..]
    } else {
        tables
    };
    let mut out = vec![0, 1, 0, 0];
    out.extend((tables.len() as u16).to_be_bytes());
    out.extend([0; 6]);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in tables {
        out.extend(*tag);
        out.extend(checksum(data).to_be_bytes());
        out.extend((offset as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend(body);
    out.resize(out.len() + pad, 0);
    out
}

#[test]
fn woff2_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut woff2 = b"wOF2\x00\x01\x00\x00".to_vec();
    woff2.resize(64, 0);
    let css = r#"@font-face { font-family: "Two"; src: url(two.woff2) format("woff2") }"#;
    let path = font_book(dir.path(), css, &[("two.woff2", &woff2)], &[]);
    let err = table_of(&path).unwrap_err();
    assert!(
        matches!(&err, Error::Unsupported { feature: "WOFF2 fonts", file } if file == "two.woff2"),
        "{err:?}"
    );
    // A TTF given for the same face first is chosen, and the WOFF2 never read.
    let css = r#"@font-face { font-family: "Two"; src: url(two.woff2), url(one.ttf) }"#;
    let ttf = sfnt(&[], 0);
    let path = font_book(
        dir.path(),
        css,
        &[("two.woff2", &woff2), ("one.ttf", &ttf)],
        &[],
    );
    assert_eq!(faces(&table_of(&path).unwrap()), [("two".to_owned(), ttf)]);
}

#[test]
fn a_font_past_50_mib_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let big = sfnt(&[], 50 * MIB + 1 - sfnt(&[], 0).len());
    assert_eq!(big.len(), 50 * MIB + 1);
    let exact = sfnt(&[], 50 * MIB - sfnt(&[], 0).len());
    assert_eq!(exact.len(), 50 * MIB);
    let small = sfnt(&[], 4);
    let css = r#"@font-face { font-family: "Big"; src: url(big.ttf), url(small.ttf) }
        @font-face { font-family: "Only big"; src: url(big.ttf) }
        @font-face { font-family: "Exact"; src: url(exact.ttf) }"#;
    let path = font_book(
        dir.path(),
        css,
        &[
            ("big.ttf", &big),
            ("small.ttf", &small),
            ("exact.ttf", &exact),
        ],
        &[],
    );
    let got = faces(&table_of(&path).unwrap());
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], ("big".to_owned(), small));
    // Exactly at the cap is kept.
    assert!(got[1].0 == "exact" && got[1].1 == exact);
}

// ---------------------------------------------------------------------------------------
// WOFF: as fontTools writes it, and hostile
// ---------------------------------------------------------------------------------------

fn sha256(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// A table's bytes: `(i * 7 + sum(tag)) % 251`, as the script that pinned fontTools' output
/// made them.
fn table_data(tag: &[u8; 4], n: usize) -> Vec<u8> {
    let sum: usize = tag.iter().map(|&b| usize::from(b)).sum();
    (0..n).map(|i| ((i * 7 + sum) % 251) as u8).collect()
}

/// A table stored as it is.
fn raw(tag: &'static [u8; 4], n: usize) -> (&'static [u8; 4], Vec<u8>, Vec<u8>) {
    (tag, table_data(tag, n), table_data(tag, n))
}

/// A WOFF of `tables` (tag, original, stored: the original or a zlib stream of it), stored
/// one after another, each padded to 4 bytes, as its directory says.
fn woff(flavor: &[u8; 4], tables: &[(&[u8; 4], Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let n = tables.len();
    let start = 44 + 20 * n;
    let mut dir = Vec::new();
    let mut body = Vec::new();
    for (tag, orig, stored) in tables {
        dir.extend(*tag);
        dir.extend(((start + body.len()) as u32).to_be_bytes());
        dir.extend((stored.len() as u32).to_be_bytes());
        dir.extend((orig.len() as u32).to_be_bytes());
        dir.extend(checksum(orig).to_be_bytes());
        body.extend(stored);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    let total: usize = 12
        + 16 * n
        + tables
            .iter()
            .map(|t| t.1.len().next_multiple_of(4))
            .sum::<usize>();
    let mut out = b"wOFF".to_vec();
    out.extend(flavor);
    out.extend(((start + body.len()) as u32).to_be_bytes());
    out.extend((n as u16).to_be_bytes());
    out.extend([0; 2]);
    out.extend((total as u32).to_be_bytes());
    out.extend([0; 24]);
    out.extend(dir);
    out.extend(body);
    out
}

/// The `i`th directory entry's field at `field` (4 offset, 8 length, 12 original length).
fn set_entry(w: &mut [u8], i: usize, field: usize, v: u32) {
    let at = 44 + 20 * i + field;
    w[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

const TRUETYPE: &[u8; 4] = b"\x00\x01\x00\x00";

/// The tables `_check_sfnt` asks for, `head` 4 bytes long.
fn small_head() -> Vec<(&'static [u8; 4], Vec<u8>, Vec<u8>)> {
    vec![
        raw(b"cmap", 20),
        raw(b"glyf", 9),
        raw(b"hhea", 36),
        raw(b"hmtx", 5),
        raw(b"maxp", 6),
        raw(b"head", 4),
    ]
}

/// `zlib.compress(bytes(64) + b"glyf", 6)`, as Python wrote it.
const GLYF_ZLIB: &str = "789c6360a00ca4e754a60100047c01b3";

fn glyf() -> Vec<u8> {
    [vec![0; 64], b"glyf".to_vec()].concat()
}

/// A book with one family per font, named as its file, each font its only `src`.
fn book_of_fonts(dir: &Path, fonts: &[(String, Vec<u8>)]) -> PathBuf {
    let css: String = fonts
        .iter()
        .map(|(name, _)| format!(r#"@font-face {{ font-family: "{name}"; src: url({name}) }}"#))
        .collect();
    let files: Vec<(&str, &[u8])> = fonts
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    font_book(dir, &css, &files, &[])
}

/// The sha256 of each face's bytes, by family.
fn face_hashes(table: &PyValue) -> Vec<(String, String)> {
    faces(table)
        .into_iter()
        .map(|(f, d)| (f, sha256(&d)))
        .collect()
}

// The hashes below are of the bytes fontTools 4.60.1 wrote for each WOFF, through kfxgen's
// `sfnt_from_woff` (meta and private fields blanked, `TTFont(...).save(reorderTables=None)`).

#[test]
fn woffs_unpack_as_fonttools_writes_them() {
    // OpenType order, `DSIG` last, dependencies first (`OS/2` after `head`, `name` after
    // `ltag`, `gvar` after `fvar` and `glyf`), a tag given twice (the last wins), one table
    // compressed.
    let mut otf: Vec<(&[u8; 4], Vec<u8>, Vec<u8>)> = [
        (b"zzzz", 3),
        (b"name", 13),
        (b"DSIG", 8),
        (b"OS/2", 10),
        (b"head", 54),
        (b"hhea", 36),
        (b"maxp", 6),
        (b"CFF ", 37),
        (b"cmap", 20),
        (b"hmtx", 5),
        (b"post", 32),
        (b"fvar", 16),
        (b"gvar", 9),
        (b"ltag", 4),
        (b"GSUB", 7),
        (b"name", 11),
    ]
    .into_iter()
    .map(|(t, n)| raw(t, n))
    .collect();
    otf.push((b"glyf", glyf(), unhex(GLYF_ZLIB)));
    let otf = woff(b"OTTO", &otf);
    // A 4-byte `head`: fontTools writes its checksum adjustment past it.
    let small = woff(TRUETYPE, &small_head());
    // One table's entry pointing at another's bytes.
    let mut overlap = small.clone();
    let cmap_at = u32::from_be_bytes(overlap[48..52].try_into().unwrap());
    set_entry(&mut overlap, 3, 4, cmap_at);
    // A compressed table with bytes after its zlib stream, which zlib leaves unread.
    let mut tables = small_head();
    tables[1] = (
        b"glyf",
        glyf(),
        [unhex(GLYF_ZLIB), b"junk".to_vec()].concat(),
    );
    let trailing = woff(TRUETYPE, &tables);
    // An empty table stated to lie past the file's end.
    let mut tables = small_head();
    tables.push((b"zero", Vec::new(), Vec::new()));
    let mut zero_past_end = woff(TRUETYPE, &tables);
    set_entry(&mut zero_past_end, 6, 4, 0xFFFF_FFF0);
    // 4,095 tables: the most fontTools can write.
    let mut tables = small_head();
    let tags: Vec<[u8; 4]> = (0..4095 - tables.len())
        .map(|i| format!("T{i:03x}").into_bytes().try_into().unwrap())
        .collect();
    tables.extend(tags.iter().map(|t| (t, Vec::new(), Vec::new())));
    let most = woff(TRUETYPE, &tables);
    // Every table class with dependencies in fontTools 4.60.1, and odd ASCII tags (the
    // review's `ttf_deps`).
    let deps: [&[u8; 4]; 39] = [
        b"head",
        b"hhea",
        b"maxp",
        b"loca",
        b"glyf",
        b"fvar",
        b"name",
        b"ltag",
        b"avar",
        b"cvar",
        b"cvt ",
        b"trak",
        b"vhea",
        b"vmtx",
        b"CBLC",
        b"CBDT",
        b"EBLC",
        b"EBDT",
        b"Gloc",
        b"Glat",
        b"TSI0",
        b"TSI1",
        b"TSI2",
        b"TSI3",
        b"GVAR",
        b"gvar",
        b"OS/2",
        b"cmap",
        b"hmtx",
        b"post",
        b"DSIG",
        b"CFF2",
        b"kern",
        b"LTSH",
        b"prep",
        b"\0\0\0\0",
        b"a b ",
        b"~~~~",
        b"\x7f\x7f\x7f\x7f",
    ];
    let tables: Vec<_> = deps
        .iter()
        .rev()
        .enumerate()
        .map(|(i, t)| (*t, table_data(t, 3 + i), table_data(t, 3 + i)))
        .collect();
    let ttf_deps = woff(TRUETYPE, &tables);

    let fonts = [
        (
            "otf",
            otf,
            "e1ecaaad6e45757fa6ca983ae734e94c757a2b3fca13d7b0e587e3cc7bcabd1e",
            "4c1dc46aab7743de0f5943709b1019089015c11246e428ff176857d47f32a15a",
        ),
        (
            "small",
            small,
            "2b874467b0421699439b8a37d2b87ece09695c8712e0be987ba9c81f61e22a88",
            "5c61988bd968fba00792bf848108c7fc5614c98d49fa350c5cfe7fe07cfbdbd3",
        ),
        (
            "overlap",
            overlap,
            "3e572c94eb7b95171e75f9787668af69f8980b2220c00a9f10148f740164fc39",
            "5f5ce506a62a59f8e44ca612e0de317dee93dac1f745c1fa9b3e8c3982acd3bc",
        ),
        (
            "trailing",
            trailing,
            "55633c47f2d9ed6e5750d4823778377fe71ed45ef94db237e0723dcac99611a4",
            "b0dccc882f90817346c587180457a9f18f85337aa9c32ea032b7bbbeeca9d965",
        ),
        (
            "zero",
            zero_past_end,
            "a58aa0ede5e2c3f87fa1f99b54eb7c13482836b01cafc610542036b8d7422dc6",
            "b464fa11bf06a7b0a5b59350456f2b3bc95a4195da933d7201f266926cdffe6a",
        ),
        (
            "ttf_deps",
            ttf_deps,
            "38508a280092ccb186f2d4bccf201c04d9db85b45a8ad882b04815249f8a0a65",
            "861a1cd794e65e855f480bf95138107894c2ab15f110bf9dcae7f23fffad55fc",
        ),
        (
            "most",
            most,
            "1396f1145d3e71a3d1bc37014ba06be55453dc8b7bbe49b620b57242d8d7c44b",
            "065ce221188a1159bee988c4d8a7817b3f1193403fe0762debc66f3dfb28bc78",
        ),
    ];
    for (name, w, input, _) in &fonts {
        // The WOFF fontTools was given.
        assert_eq!(sha256(w), *input, "{name}");
    }
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<(String, Vec<u8>)> = fonts
        .iter()
        .map(|(n, w, ..)| (format!("{n}.woff"), w.clone()))
        .collect();
    let got = face_hashes(&table_of(&book_of_fonts(dir.path(), &files)).unwrap());
    let want: Vec<(String, String)> = fonts
        .iter()
        .map(|(n, _, _, out)| (format!("{n}.woff"), (*out).to_owned()))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn malformed_fonts_are_skipped_without_panicking() {
    let mut fonts: Vec<(String, Vec<u8>)> = Vec::new();
    let good_woff = woff(TRUETYPE, &small_head());
    let good_ttf = sfnt(&[], 0);
    // Cut short anywhere: the last table's bytes are missing.
    for n in 0..good_woff.len() {
        fonts.push((format!("cut{n}.woff"), good_woff[..n].to_vec()));
    }
    // The TTF's last 2 bytes are padding.
    for n in 0..good_ttf.len() - 2 {
        fonts.push((format!("cut{n}.ttf"), good_ttf[..n].to_vec()));
    }
    let mut edited = |name: &str, base: &[u8], f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = base.to_vec();
        f(&mut b);
        fonts.push((name.to_owned(), b));
    };
    edited("no_tables.woff", &woff(TRUETYPE, &[]), &|_| {});
    edited("bad_flavor.woff", &good_woff, &|w| {
        w[4..8].copy_from_slice(b"wOFF")
    });
    edited("offset_max.woff", &good_woff, &|w| {
        set_entry(w, 0, 4, u32::MAX)
    });
    edited("offset_wraps.woff", &good_woff, &|w| {
        set_entry(w, 0, 4, u32::MAX - 3)
    });
    // A zlib stream longer than the table it inflates to, exactly: kfxgen refuses it unread.
    let mut tables = small_head();
    let maxp = table_data(b"maxp", 6);
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&maxp).unwrap();
    let stream = z.finish().unwrap();
    assert!(stream.len() > maxp.len());
    tables[4] = (b"maxp", maxp, stream);
    edited("longer_compressed.woff", &woff(TRUETYPE, &tables), &|_| {});
    edited("tables_max.woff", &good_woff, &|w| w[12..14].fill(0xFF));
    let mut tables = small_head();
    tables[1] = (b"glyf", glyf(), unhex(GLYF_ZLIB));
    let compressed = woff(TRUETYPE, &tables);
    let glyf_at = u32::from_be_bytes(compressed[68..72].try_into().unwrap()) as usize;
    // The stream without its checksum: it inflates to the table's length, but never ends.
    edited("no_adler.woff", &compressed, &|w| set_entry(w, 1, 8, 12));
    edited("bad_adler.woff", &compressed, &|w| w[glyf_at + 15] ^= 1);
    edited("bad_header.woff", &compressed, &|w| w[glyf_at] = 0x79);
    edited("inflates_short.woff", &compressed, &|w| {
        set_entry(w, 1, 12, 69)
    });
    let mut tables = small_head();
    tables.push(raw(b"zzzz", 2));
    edited("not_ascii.woff", &woff(TRUETYPE, &tables), &|w| {
        w[44 + 20 * 6 + 3] = 0x80
    });
    edited("tables_past_end.ttf", &good_ttf, &|t| t[4..6].fill(0xFF));
    edited("offset_max.ttf", &good_ttf, &|t| t[20..24].fill(0xFF));
    edited("length_max.ttf", &good_ttf, &|t| t[24..28].fill(0xFF));
    edited("no_cmap.ttf", &good_ttf, &|t| {
        t[12..16].copy_from_slice(b"cmaq")
    });
    let mut tables = small_head();
    let tags: Vec<[u8; 4]> = (0..4096 - tables.len())
        .map(|i| format!("T{i:03x}").into_bytes().try_into().unwrap())
        .collect();
    tables.extend(tags.iter().map(|t| (t, Vec::new(), Vec::new())));
    fonts.push(("too_many.woff".to_owned(), woff(TRUETYPE, &tables)));
    fonts.push(("good.woff".to_owned(), good_woff));
    fonts.push(("good.ttf".to_owned(), good_ttf));
    // A collection's own header is not read: kfxgen embeds it as it is.
    fonts.push(("collection.ttc".to_owned(), b"ttcf junk".to_vec()));

    let dir = tempfile::tempdir().unwrap();
    let table = table_of(&book_of_fonts(dir.path(), &fonts)).unwrap();
    let kept: Vec<String> = faces(&table).into_iter().map(|(f, _)| f).collect();
    assert_eq!(kept, ["good.woff", "good.ttf", "collection.ttc"]);
}

#[test]
fn a_woff_claiming_more_than_it_holds_is_skipped_without_allocating_it() {
    let mut tables = small_head();
    tables[1] = (b"glyf", glyf(), unhex(GLYF_ZLIB));
    let compressed = woff(TRUETYPE, &tables);
    let mut fonts = Vec::new();
    // A table of 4 GiB, and 40 MiB from a 16-byte stream.
    let mut w = compressed.clone();
    set_entry(&mut w, 1, 12, u32::MAX);
    fonts.push(("huge.woff".to_owned(), w));
    let mut w = compressed.clone();
    set_entry(&mut w, 1, 12, 40 << 20);
    fonts.push(("claims_40_mib.woff".to_owned(), w));
    // A 4 GiB font in all.
    let mut w = compressed.clone();
    w[16..20].fill(0xFF);
    fonts.push(("total.woff".to_owned(), w));
    // 50 MiB of zeros in a 50 KB stream, for a table of 1,000 bytes.
    let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
    z.write_all(&vec![0; 50 * MIB]).unwrap();
    let bomb = z.finish().unwrap();
    assert!(bomb.len() < 100_000);
    let mut tables = small_head();
    tables[1] = (b"glyf", vec![0; 1000], bomb);
    fonts.push(("bomb.woff".to_owned(), woff(TRUETYPE, &tables)));

    let dir = tempfile::tempdir().unwrap();
    let path = book_of_fonts(dir.path(), &fonts);
    let book = read_book(&path).unwrap();
    let (table, bytes) =
        allocated_by(|| convert::font_table(&book, &path, &ConvertOptions::default()).unwrap());
    assert!(faces(&table).is_empty());
    // Each file read twice (its kind, then its bytes), and a little for each table.
    assert!(bytes < 2 * MIB as u64, "{bytes} bytes allocated");
}

/// A WOFF of 65,535 table entries (the most a WOFF holds) of one tag, each inflating one
/// shared stream to 760 bytes: 49.8 MB, just within the 50 MiB a font may declare. The most
/// work one font can ask for; it is then skipped (it lacks the tables every font has).
fn heaviest_woff() -> Vec<u8> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
    z.write_all(&[0; 760]).unwrap();
    let stream = z.finish().unwrap();
    let n = 65_535usize;
    let start = 44 + 20 * n;
    let mut w = b"wOFF".to_vec();
    w.extend(TRUETYPE);
    w.extend(((start + stream.len()) as u32).to_be_bytes());
    w.extend((n as u16).to_be_bytes());
    w.extend([0; 2]);
    w.extend(0u32.to_be_bytes());
    w.extend([0; 24]);
    for _ in 0..n {
        w.extend(b"glyf");
        w.extend((start as u32).to_be_bytes());
        w.extend((stream.len() as u32).to_be_bytes());
        w.extend(760u32.to_be_bytes());
        w.extend(0u32.to_be_bytes());
    }
    w.extend(stream);
    w
}

#[test]
fn a_book_past_its_font_budget_is_refused_and_its_work_is_bounded() {
    let heavy = heaviest_woff();
    // Five fit the book's 256 MiB, the sixth does not.
    let fonts: Vec<(String, Vec<u8>)> = (0..6)
        .map(|i| (format!("heavy{i}.woff"), heavy.clone()))
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = book_of_fonts(dir.path(), &fonts);
    let book = read_book(&path).unwrap();
    let start = Instant::now();
    let err = convert::font_table(&book, &path, &ConvertOptions::default()).unwrap_err();
    let took = start.elapsed();
    assert!(
        matches!(&err, Error::TooLarge { what } if what.starts_with("heavy5.woff: ")),
        "{err:?}"
    );
    eprintln!("five heaviest WOFFs, then refused: {took:?}");
    assert!(took < COST_LIMIT, "{took:?}");
    // Each file is worked out once, however often the rules name it.
    let css: String = (0..200)
        .map(|_| r#"@font-face { font-family: "Heavy"; src: url(heavy0.woff) }"#)
        .collect();
    let path = font_book(dir.path(), &css, &[("heavy0.woff", &heavy)], &[]);
    let start = Instant::now();
    assert!(faces(&table_of(&path).unwrap()).is_empty());
    eprintln!("one heaviest WOFF named 200 times: {:?}", start.elapsed());
}

#[test]
fn a_font_outside_the_archive_is_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let book_dir = dir.path().join("books");
    std::fs::create_dir(&book_dir).unwrap();
    // On the disk beside the book, where each href below would reach if it were a path.
    let on_disk = sfnt(&[], 8);
    std::fs::write(dir.path().join("disk.ttf"), &on_disk).unwrap();
    std::fs::write(dir.path().join("in.ttf"), &on_disk).unwrap();
    let abs = dir.path().to_str().unwrap();
    let in_archive = sfnt(&[], 12);
    let hrefs = [
        "../disk.ttf".to_owned(),
        "../../disk.ttf".to_owned(),
        "..%2Fdisk.ttf".to_owned(),
        "%2e%2e/disk.ttf".to_owned(),
        format!("{abs}/disk.ttf"),
        format!("file://{abs}/disk.ttf"),
        format!("file:{abs}/disk.ttf"),
        // Found by its file name in the manifest: the archive's copy.
        format!("file://{abs}/in.ttf"),
    ];
    let mut css: String = hrefs
        .iter()
        .enumerate()
        .map(|(i, h)| format!(r#"@font-face {{ font-family: "F{i}"; src: url("{h}") }}"#))
        .collect();
    // Named by its href too, or Calibre trims it from the manifest as unused.
    css += r#"@font-face { font-family: "Keep"; src: url(in.ttf) }"#;
    let path = font_book(&book_dir, &css, &[("in.ttf", &in_archive)], &[]);
    let got = faces(&table_of(&path).unwrap());
    assert_eq!(
        got,
        [
            ("f7".to_owned(), in_archive.clone()),
            ("keep".to_owned(), in_archive)
        ]
    );
}

#[test]
fn emitted_names_and_tables_are_kfxgens() {
    let ttf = sfnt(&[], 0);
    // Each its own href, each found by its file name.
    let rules = [
        ("Fixture", "normal", "normal"),
        ("Fixture", "400", "normal"),
        ("fixture", "bold", "italic"),
        ("명조", "normal", "normal"),
        ("Ünï Sans", "normal", "normal"),
        ("A", "normal", "oblique"),
        ("A", "normal", "normal"),
        ("A", "normal", "normal"),
        ("A", "normal", "normal"),
        ("--", "normal", "normal"),
        (" 'Quoted' ", "normal", "normal"),
    ];
    let mut css: String = rules
        .iter()
        .enumerate()
        .map(|(i, (f, w, s))| {
            format!(
                r#"@font-face {{ font-family: "{f}"; font-weight: {w}; font-style: {s}; src: url(d{i}/f.ttf) }}"#
            )
        })
        .collect();
    // Named by its href too, or Calibre trims it from the manifest as unused.
    css += r#"@font-face { font-family: "Keep"; src: url(f.ttf) }"#;
    let dir = tempfile::tempdir().unwrap();
    let path = font_book(dir.path(), &css, &[("f.ttf", &ttf)], &[]);
    let table = table_of(&path).unwrap();
    let names: Vec<(&str, i64, bool, &str, &str)> = table
        .get("faces")
        .and_then(PyValue::as_list)
        .unwrap()
        .iter()
        .map(|f| {
            (
                f.get("css_family").and_then(PyValue::as_str).unwrap(),
                f.get("weight").and_then(PyValue::as_int).unwrap(),
                matches!(f.get("italic"), Some(PyValue::Bool(true))),
                f.get("emitted_family").and_then(PyValue::as_str).unwrap(),
                f.get("location").and_then(PyValue::as_str).unwrap(),
            )
        })
        .collect();
    // From kfxgen's `emitted_name` run on the same families. A non-ASCII family's `_slug` hashes
    // `normalize_family(name)` (stripped of whitespace, then of `"` and `'`, then of whitespace,
    // then `str.lower()`) as UTF-8: "명조" is `eb aa 85 ec a1 b0`, sha1 `47a9f002…`, and the
    // slug is `font-` and the hash, as `[^a-z0-9]+` leaves nothing; "Ünï Sans" is
    // `c3 bc 6e c3 af 20 73 61 6e 73` ("ünï sans"), sha1 `815b0374…`, after the ASCII left
    // ("n-sans").
    assert_eq!(
        names,
        [
            ("fixture", 400, false, "fixture-400", "resource/font0"),
            ("fixture", 400, false, "fixture-400-1", "resource/font1"),
            ("fixture", 700, true, "fixture-700i", "resource/font2"),
            ("명조", 400, false, "font-47a9f002-400", "resource/font3"),
            (
                "ünï sans",
                400,
                false,
                "n-sans-815b0374-400",
                "resource/font4"
            ),
            ("a", 400, true, "a-400i", "resource/font5"),
            ("a", 400, false, "a-400", "resource/font6"),
            ("a", 400, false, "a-400-1", "resource/font7"),
            ("a", 400, false, "a-400-2", "resource/font8"),
            ("--", 400, false, "font-400", "resource/font9"),
            ("'quoted'", 400, false, "quoted-400", "resource/font10"),
            ("keep", 400, false, "keep-400", "resource/font11"),
        ]
    );
    let by_family = table.get("_by_family").unwrap();
    let PyValue::Dict(by_family) = by_family else {
        panic!("{by_family:?}")
    };
    let counts: Vec<(&str, usize)> = by_family
        .iter()
        .map(|(k, v)| (k.as_str().unwrap(), v.as_list().unwrap().len()))
        .collect();
    assert_eq!(
        counts,
        [
            ("fixture", 3),
            ("명조", 1),
            ("ünï sans", 1),
            ("a", 4),
            ("--", 1),
            ("'quoted'", 1),
            ("keep", 1)
        ]
    );
}

#[test]
fn font_embedding_turned_off_gives_no_faces() {
    let dir = tempfile::tempdir().unwrap();
    let css = r#"@font-face { font-family: "Two"; src: url(two.woff2) }"#;
    let path = font_book(
        dir.path(),
        css,
        &[("two.woff2", b"wOF2\x00\x01\x00\x00")],
        &[],
    );
    let book = read_book(&path).unwrap();
    let opts = ConvertOptions {
        disable_font_embedding: true,
        ..ConvertOptions::default()
    };
    assert!(faces(&convert::font_table(&book, &path, &opts).unwrap()).is_empty());
}

#[test]
fn a_woff_given_beside_a_ttf_face_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ttf = sfnt(&[], 0);
    let w = woff(TRUETYPE, &small_head());
    // The first WOFF-only rule repeats a face a TTF rule gives; the bold one is another face. A
    // rule with no family gives none.
    let css = r#"@font-face { font-family: "F"; src: url(f.woff) }
        @font-face { font-weight: bold; src: url(f.ttf) }
        @font-face { font-family: "F"; src: url(f.ttf) }
        @font-face { font-family: "F"; font-weight: bold; src: url(f.woff), url(f.woff) }"#;
    let path = font_book(dir.path(), css, &[("f.woff", &w), ("f.ttf", &ttf)], &[]);
    let got = faces(&table_of(&path).unwrap());
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].1, ttf);
    assert_eq!(
        sha256(&got[1].1),
        "5c61988bd968fba00792bf848108c7fc5614c98d49fa350c5cfe7fe07cfbdbd3"
    );
}

#[test]
fn a_large_compressed_table_unpacks_as_stored_raw() {
    // Over the 64 KiB the output buffer starts at, so it grows.
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 7 + i / 1000) as u8).collect();
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&big).unwrap();
    let stream = z.finish().unwrap();
    assert!(stream.len() < big.len());
    let mut tables = small_head();
    tables[1] = (b"glyf", big.clone(), big.clone());
    let stored = woff(TRUETYPE, &tables);
    tables[1] = (b"glyf", big.clone(), stream);
    let compressed = woff(TRUETYPE, &tables);
    let dir = tempfile::tempdir().unwrap();
    let path = book_of_fonts(
        dir.path(),
        &[
            ("stored.woff".to_owned(), stored),
            ("compressed.woff".to_owned(), compressed),
        ],
    );
    let got = faces(&table_of(&path).unwrap());
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].1, got[1].1);
    assert!(got[0].1.windows(big.len()).any(|w| w == big));
}

/// A TrueType font of `len` bytes: the tables `_check_sfnt` asks for, then zeros.
fn ttf_of(len: usize) -> Vec<u8> {
    sfnt(&[], len - sfnt(&[], 0).len())
}

#[test]
fn a_book_whose_fonts_pass_256_mib_together_is_refused() {
    let ttf = ttf_of(50 * MIB);
    let names: Vec<String> = (0..6).map(|i| format!("f{i}.ttf")).collect();
    let dir = tempfile::tempdir().unwrap();
    // Five files of 50 MiB fit. Each is named three times: twice by its href (one face, as
    // kfxgen keeps one face an href) and once by another href found by its file name (another
    // face, sharing the bytes). Each file is charged once: 250 MiB.
    let css: String = names[..5]
        .iter()
        .map(|n| {
            format!(
                r#"@font-face {{ font-family: "{n}"; src: url({n}) }}
                @font-face {{ font-family: "{n}"; font-weight: bold; src: url({n}) }}
                @font-face {{ font-family: "{n}"; font-style: italic; src: url(d/{n}) }}"#
            )
        })
        .collect();
    let files: Vec<(&str, &[u8])> = names[..5].iter().map(|n| (n.as_str(), &ttf[..])).collect();
    let path = font_book(dir.path(), &css, &files, &[]);
    let got = faces(&table_of(&path).unwrap());
    assert_eq!(got.len(), 10);
    drop(got);
    // A sixth passes the budget, and is the one named.
    let css: String = names
        .iter()
        .map(|n| format!(r#"@font-face {{ font-family: "{n}"; src: url({n}) }}"#))
        .collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &ttf[..])).collect();
    let path = font_book(dir.path(), &css, &files, &[]);
    drop(ttf);
    let book = read_book(&path).unwrap();
    let (got, bytes) =
        allocated_by(|| convert::font_table(&book, &path, &ConvertOptions::default()));
    let err = got.unwrap_err();
    assert!(
        matches!(&err, Error::TooLarge { what } if what.starts_with("f5.ttf: ")),
        "{err:?}"
    );
    eprintln!(
        "six 50 MiB TTFs, the sixth refused: {} MiB allocated",
        bytes >> 20
    );
    // Each file read once (its buffer grows to 64 MiB) and the five kept copied once: 634 MiB,
    // about 314 MiB of it live at once. Read twice and all six kept, it was 1,068 MiB.
    assert!(bytes < 700 << 20, "{bytes} bytes allocated");
}

#[test]
fn non_font_files_named_as_src_are_read_for_their_magic_only() {
    let junk = vec![0u8; 20 * MIB];
    let names = ["a.bin", "b.bin", "c.bin"];
    let css: String = names
        .iter()
        .map(|n| format!(r#"@font-face {{ font-family: "{n}"; src: url({n}) }}"#))
        .collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (*n, &junk[..])).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = font_book(dir.path(), &css, &files, &[]);
    let book = read_book(&path).unwrap();
    let (table, bytes) =
        allocated_by(|| convert::font_table(&book, &path, &ConvertOptions::default()).unwrap());
    assert!(faces(&table).is_empty());
    assert!(bytes < MIB as u64, "{bytes} bytes allocated");
}
