// SPDX-License-Identifier: GPL-3.0-only
//! The book's pictures and cover as kfxgen hands them to `generate_full_book` (plan Task 18):
//! against every model, and on hostile pictures.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use deckle_kfx::fixtures;
use deckle_kfxgen::book::{manifest_bytes, read_book};
use deckle_kfxgen::convert::{self, ConvertOptions, Error, Images};
use deckle_kfxgen::load::load_model;
use deckle_kfxgen::pyvalue::PyValue;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

use crate::convert_args::{dict, sources};
use crate::support::allocated_by;

const MAX_DIM: u32 = 1920;
const MAX_BYTES: usize = 1024 * 1024;

fn format_of(b: &[u8]) -> &'static str {
    if b.starts_with(b"\xff\xd8\xff") {
        "JPEG"
    } else if b.starts_with(b"\x89PNG") {
        "PNG"
    } else {
        "other"
    }
}

fn dims(b: &[u8]) -> (u32, u32) {
    image::ImageReader::new(Cursor::new(b))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap()
}

/// The colour components a JPEG's frame header states.
fn jpeg_components(b: &[u8]) -> Option<u8> {
    let mut i = 2;
    while i + 9 < b.len() {
        let marker = b[i + 1];
        if matches!(marker, 0xC0..=0xC3) {
            return Some(b[i + 9]);
        }
        i += 2 + usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
    }
    None
}

/// Where `ours` differs from kfxgen's `want` for a picture whose bytes in the EPUB are `raw`:
/// bytes where kfxgen kept the EPUB's, else format, size and JPEG components, an oversize
/// opaque PNG free to come out as either format (`DIVERGENCES.md`).
fn compare(what: &str, ours: &[u8], want: &[u8], raw: &[u8]) -> Vec<String> {
    if want == raw {
        return if ours == want {
            Vec::new()
        } else {
            vec![format!("{what}: not the EPUB's bytes")]
        };
    }
    let mut out = Vec::new();
    let oversize_png = format_of(raw) == "PNG" && {
        let (w, h) = dims(raw);
        w.max(h) > MAX_DIM || raw.len() > MAX_BYTES
    };
    if format_of(ours) != format_of(want) && !oversize_png {
        out.push(format!(
            "{what}: {} where kfxgen has {}",
            format_of(ours),
            format_of(want)
        ));
    }
    if dims(ours) != dims(want) {
        out.push(format!(
            "{what}: {:?} where kfxgen has {:?}",
            dims(ours),
            dims(want)
        ));
    }
    if format_of(ours) == "JPEG"
        && format_of(want) == "JPEG"
        && jpeg_components(ours) != jpeg_components(want)
    {
        out.push(format!("{what}: other JPEG components"));
    }
    out
}

#[test]
fn picture_sets_match_every_model() {
    let mut failures = Vec::new();
    let (mut as_in_the_book, mut made) = (0, 0);
    for name in sources() {
        let path = fixtures::kfxgen(&format!("sources/{name}.epub"));
        let book = read_book(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let model = load_model(&fixtures::kfxgen(&format!("models/{name}")))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let args = dict(&model.args);
        let ours = match convert::images(&book, &path, &ConvertOptions::default()) {
            Ok(i) => i,
            Err(e) => {
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        let PyValue::Dict(want) = args["images"] else {
            panic!("{name}: images is not a dict")
        };
        let want: BTreeMap<&str, &[u8]> = want
            .iter()
            .map(|(k, v)| (k.as_str().unwrap(), v.as_bytes().unwrap()))
            .collect();
        if !want.keys().eq(ours.images.keys()) {
            failures.push(format!(
                "{name}: pictures {:?} where kfxgen has {:?}",
                ours.images.keys().collect::<Vec<_>>(),
                want.keys().collect::<Vec<_>>()
            ));
            continue;
        }
        for (href, want) in want {
            let raw = manifest_bytes(&path, href).unwrap();
            if want == raw {
                as_in_the_book += 1;
            } else {
                made += 1;
            }
            failures.extend(compare(
                &format!("{name}: {href}"),
                &ours.images[href],
                want,
                &raw,
            ));
        }
        match (&ours.cover, args["cover_image"].as_bytes()) {
            (None, None) => {}
            (Some(ours), Some(want)) => {
                // The cover is one of the book's files, or made from one.
                let raw = (book.oeb.manifest.iter())
                    .filter_map(|i| manifest_bytes(&path, &i.href).ok())
                    .find(|b| b == want)
                    .unwrap_or_default();
                failures.extend(compare(&format!("{name}: the cover"), ours, want, &raw));
            }
            (ours, want) => failures.push(format!(
                "{name}: cover {} where kfxgen has {}",
                ours.is_some(),
                want.is_some()
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // pictures_more's GIF, CMYK JPEG and oversize PNG are the three kfxgen made.
    assert_eq!((as_in_the_book, made), (58, 3));
}

const CONTAINER: &[u8] = br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// An EPUB at `path` whose OPF has `meta` in its metadata and `items` in its manifest, whose
/// spine is `c.xhtml` with `body`, and which holds `files`.
fn write_epub(path: &Path, meta: &str, items: &str, body: &str, files: &[(&str, &[u8])]) {
    let opf = format!(
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier><dc:title>T</dc:title><dc:language>en</dc:language>{meta}</metadata><manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>{items}</manifest><spine><itemref idref="c"/></spine></package>"#
    );
    let page = format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head><body><p>Hi.</p>{body}</body></html>"#
    );
    let mut entries: Vec<(&str, &[u8], u64)> = vec![
        ("mimetype", b"application/epub+zip", 20),
        ("META-INF/container.xml", CONTAINER, CONTAINER.len() as u64),
        ("content.opf", opf.as_bytes(), opf.len() as u64),
        ("c.xhtml", page.as_bytes(), page.len() as u64),
    ];
    entries.extend(files.iter().map(|&(n, d)| (n, d, d.len() as u64)));
    std::fs::write(path, fixtures::stored_zip(&entries)).unwrap();
}

/// A book of `files`, each a manifest image of its file name and shown in the text, the one
/// named `cover` also the cover.
fn book_of(dir: &Path, cover: Option<&str>, files: &[(&str, &[u8])]) -> PathBuf {
    let mut items = String::new();
    let mut body = String::new();
    for (i, (href, _)) in files.iter().enumerate() {
        let mt = match href.rsplit('.').next() {
            Some("jpg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("svg") => "image/svg+xml",
            _ => "image/png",
        };
        let id = if cover == Some(href) {
            "cov".to_owned()
        } else {
            format!("i{i}")
        };
        items += &format!(r#"<item id="{id}" href="{href}" media-type="{mt}"/>"#);
        body += &format!(r#"<p><img src="{href}" alt=""/></p>"#);
    }
    let meta = cover.map_or(String::new(), |_| {
        r#"<meta name="cover" content="cov"/>"#.to_owned()
    });
    let path = dir.join("book.epub");
    write_epub(&path, &meta, &items, &body, files);
    path
}

fn images_of(path: &Path, opts: &ConvertOptions) -> Result<Images, Error> {
    let book = read_book(path).unwrap();
    convert::images(&book, path, opts)
}

fn encode(img: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, format).unwrap();
    out.into_inner()
}

/// A `w` × `h` picture in shades of grey and colour, so no encoder makes it nothing.
fn picture(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
    }))
}

#[test]
fn the_cover_shares_bytes_with_its_copy() {
    let dir = tempfile::tempdir().unwrap();
    // Over the 1,920 px cap, so kfxgen re-encodes it, and the cover and its copy with it.
    let raw = encode(&picture(2000, 10), ImageFormat::Png);
    let path = book_of(dir.path(), Some("cover.png"), &[("cover.png", &raw)]);
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    let cover = got.cover.expect("a cover");
    assert_ne!(cover, raw);
    assert_eq!(dims(&cover), (1800, 9));
    assert_eq!(got.images["cover.png"], cover);
}

#[test]
fn a_picture_outside_the_archive_is_never_opened() {
    let dir = tempfile::tempdir().unwrap();
    let book_dir = dir.path().join("books");
    std::fs::create_dir(&book_dir).unwrap();
    // On the disk beside the book, where each href below would reach if it were a path.
    let on_disk = encode(&picture(3, 3), ImageFormat::Png);
    std::fs::write(dir.path().join("cover.png"), &on_disk).unwrap();
    let abs = dir.path().join("cover.png");
    let abs = abs.to_str().unwrap();
    // In the archive under a name with `..`: read from the archive.
    let in_archive = encode(&picture(12, 12), ImageFormat::Png);
    let inside = encode(&picture(4, 4), ImageFormat::Png);
    let hrefs = [
        "../cover.png".to_owned(),
        "..%2Fcover.png".to_owned(),
        "%2e%2e/cover.png".to_owned(),
        abs.to_owned(),
        format!("file://{abs}"),
        format!("file:{abs}"),
        "in.png".to_owned(),
    ];
    let mut items = String::new();
    let mut body = String::new();
    for (i, href) in hrefs.iter().enumerate() {
        items += &format!(r#"<item id="i{i}" href="{href}" media-type="image/png"/>"#);
        body += &format!(r#"<p><img src="{href}" alt=""/></p>"#);
    }
    let path = book_dir.join("book.epub");
    let meta = format!(r#"<meta name="cover" content="{abs}"/>"#);
    write_epub(
        &path,
        &meta,
        &items,
        &body,
        &[("../cover.png", &in_archive), ("in.png", &inside)],
    );
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    let kept: Vec<(&str, &Vec<u8>)> = got.images.iter().map(|(k, v)| (k.as_str(), v)).collect();
    assert_eq!(kept, [("../cover.png", &in_archive), ("in.png", &inside)]);
    // The first manifest picture with "cover" in its href, from the archive.
    assert_eq!(got.cover, Some(in_archive));
}

/// `bytes` with the PNG chunk at `at` (its length field) given a correct CRC.
fn fix_png_crc(bytes: &mut [u8], at: usize) {
    let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let mut crc = flate2::Crc::new();
    crc.update(&bytes[at + 4..at + 8 + len]);
    bytes[at + 8 + len..at + 12 + len].copy_from_slice(&crc.sum().to_be_bytes());
}

#[test]
fn a_100_megapixel_picture_is_refused_before_decoding() {
    let small = picture(8, 8);
    // Each a tiny file whose header claims a picture over 100 megapixels.
    let mut png = encode(&small, ImageFormat::Png);
    png[16..20].copy_from_slice(&10_001u32.to_be_bytes());
    png[20..24].copy_from_slice(&10_000u32.to_be_bytes());
    fix_png_crc(&mut png, 8);
    let mut jpeg = encode(&small, ImageFormat::Jpeg);
    let sof = (2..jpeg.len())
        .find(|&i| jpeg[i] == 0xFF && jpeg[i + 1] == 0xC0)
        .unwrap();
    jpeg[sof + 5..sof + 7].copy_from_slice(&1_600u16.to_be_bytes());
    jpeg[sof + 7..sof + 9].copy_from_slice(&65_535u16.to_be_bytes());
    let mut gif = encode(&small, ImageFormat::Gif);
    gif[6..8].copy_from_slice(&11_000u16.to_le_bytes());
    gif[8..10].copy_from_slice(&10_000u16.to_le_bytes());
    for (name, bytes) in [("p.png", png), ("p.jpg", jpeg), ("p.gif", gif)] {
        let dir = tempfile::tempdir().unwrap();
        let path = book_of(dir.path(), None, &[(name, &bytes)]);
        let book = read_book(&path).unwrap();
        let (got, allocated) =
            allocated_by(|| convert::images(&book, &path, &ConvertOptions::default()));
        match got {
            Err(Error::TooLarge { what }) => assert!(what.contains(name), "{what}"),
            other => panic!("{name}: {:?}", other.map(|i| i.images.len())),
        }
        // A decode would take 300 MB or more.
        assert!(allocated < 8 << 20, "{name}: {allocated} bytes");
    }
}

#[test]
fn an_oversize_picture_is_fitted_as_calibre_and_qt_do() {
    let dir = tempfile::tempdir().unwrap();
    let shapes = [(1921, 7), (2000, 10), (7, 1921)];
    let files: Vec<(String, Vec<u8>)> = shapes
        .iter()
        .map(|&(w, h)| {
            (
                format!("{w}x{h}.png"),
                encode(&picture(w, h), ImageFormat::Png),
            )
        })
        .collect();
    let thin = encode(&picture(1, 2000), ImageFormat::Png);
    let mut refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    refs.push(("1x2000.png", &thin));
    let path = book_of(dir.path(), None, &refs);
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    // `fit_image` floors the short side; Qt then fits the picture into that box.
    assert_eq!(dims(&got.images["1921x7.png"]), (1646, 6));
    assert_eq!(dims(&got.images["2000x10.png"]), (1800, 9));
    assert_eq!(dims(&got.images["7x1921.png"]), (6, 1920));
    // Fitted to no width at all, Qt gives no picture, and kfxgen keeps the original.
    assert_eq!(got.images["1x2000.png"], thin);
}

#[test]
fn unreadable_pictures_are_left_out_with_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let png = encode(&picture(30, 30), ImageFormat::Png);
    let jpeg = encode(&picture(30, 30), ImageFormat::Jpeg);
    let mut garbage_jpeg = b"\xff\xd8\xff\xe0".to_vec();
    garbage_jpeg.extend([0x5A; 200]);
    let mut bad_gif = b"GIF89a\x10\x00\x10\x00\x80\x00\x00".to_vec();
    bad_gif.extend([0xFF; 40]);
    // A GIF of no width, and a frame header of four components (CMYK) over nothing.
    let mut empty_gif = bad_gif.clone();
    empty_gif[6] = 0;
    let mut cmyk = b"\xff\xd8\xff\xc0\x00\x14\x08\x00\x10\x00\x10\x04".to_vec();
    cmyk.extend([0x11; 40]);
    let mut no_frame = gif_with_frame(4, 4, 4, 4);
    no_frame.truncate(13 + 6 + 8);
    no_frame.push(0x3B);
    let cut_frame = gif_with_frame(4, 4, 4, 4)[..13 + 6 + 8 + 7].to_vec();
    let files: [(&str, &[u8]); 10] = [
        ("cut.png", &png[..60]),
        ("cut.jpg", &jpeg[..jpeg.len() / 3]),
        ("garbage.jpg", &garbage_jpeg),
        ("bad.gif", &bad_gif),
        ("empty.gif", &empty_gif),
        ("cmyk.jpg", &cmyk),
        ("no-frame.gif", &no_frame),
        ("cut-frame.gif", &cut_frame),
        ("cover.png", &png[..png.len() - 20]),
        ("text.png", b"just some words, not a picture at all"),
    ];
    let path = book_of(dir.path(), Some("cover.png"), &files);
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    for (name, _) in files {
        // A JPEG cut short may still decode, as Qt fills it in; nothing else does.
        if name != "cut.jpg" {
            assert!(!got.images.contains_key(name), "{name}");
            assert!(
                got.warnings.iter().any(|w| w.contains(name)),
                "{name}: {:?}",
                got.warnings
            );
        }
    }
    for why in [
        "CMYK picture kept as it is",
        "GIF has unreadable dimensions",
    ] {
        assert!(got.warnings.iter().any(|w| w.contains(why)), "{why}");
    }
    assert!(got.cover.is_none());
}

#[test]
fn a_book_past_its_pixel_budget_is_refused_from_the_headers() {
    // 101 files of a few dozen bytes, each a grey PNG whose header claims 100 megapixels:
    // the first 100 fill the book's 10 gigapixels (each charged, then failing to decode its
    // missing data), the 101st passes them before it is decoded.
    let mut png = encode(
        &DynamicImage::ImageLuma8(image::GrayImage::new(8, 8)),
        ImageFormat::Png,
    );
    png[16..20].copy_from_slice(&10_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&10_000u32.to_be_bytes());
    fix_png_crc(&mut png, 8);
    let names: Vec<String> = (0..101).map(|i| format!("p{i:03}.png")).collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), png.as_slice())).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = book_of(dir.path(), None, &files);
    let book = read_book(&path).unwrap();
    let (got, allocated) =
        allocated_by(|| convert::images(&book, &path, &ConvertOptions::default()));
    match got {
        Err(Error::TooLarge { what }) => {
            assert!(what.starts_with("p100.png:"), "{what}");
            assert!(what.contains("10000 megapixels together"), "{what}");
        }
        other => panic!("{:?}", other.map(|i| i.images.len())),
    }
    // Each of the 100 failed decodes reserves its 100 MB picture; the 101st is not decoded.
    assert!(
        allocated < 100 * 100_000_000 + (64 << 20),
        "{allocated} bytes"
    );
}

/// A GIF whose screen is `sw` × `sh` and whose one frame states `fw` × `fh`, after a global
/// colour table and a graphic control extension, with a few bytes of data.
fn gif_with_frame(sw: u16, sh: u16, fw: u16, fh: u16) -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend(sw.to_le_bytes());
    g.extend(sh.to_le_bytes());
    g.extend([0x80, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF]);
    g.extend([0x21, 0xF9, 0x04, 0, 0, 0, 0, 0x00]);
    g.extend([0x2C, 0, 0, 0, 0]);
    g.extend(fw.to_le_bytes());
    g.extend(fh.to_le_bytes());
    g.extend([0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B]);
    g
}

#[test]
fn a_gif_frame_larger_than_its_screen_is_charged_its_own_size() {
    // A 1 × 1 screen over a frame of 110 megapixels: refused from the headers.
    let dir = tempfile::tempdir().unwrap();
    let gif = gif_with_frame(1, 1, 11_000, 10_000);
    let path = book_of(dir.path(), None, &[("p.gif", &gif)]);
    let book = read_book(&path).unwrap();
    let (got, allocated) =
        allocated_by(|| convert::images(&book, &path, &ConvertOptions::default()));
    match got {
        Err(Error::TooLarge { what }) => assert!(what.contains("100 megapixels"), "{what}"),
        other => panic!("{:?}", other.map(|i| i.images.len())),
    }
    assert!(allocated < 8 << 20, "{allocated} bytes");
    // Frames of 100 megapixels each fill the book's budget: the 101st is refused. An LZW code
    // size past 12 stops each decode before its frame is allocated.
    let mut gif = gif_with_frame(1, 1, 10_000, 10_000);
    gif[37] = 0x0F;
    let names: Vec<String> = (0..101).map(|i| format!("p{i:03}.gif")).collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), gif.as_slice())).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = book_of(dir.path(), None, &files);
    match images_of(&path, &ConvertOptions::default()) {
        Err(Error::TooLarge { what }) => assert!(what.starts_with("p100.gif:"), "{what}"),
        other => panic!("{:?}", other.map(|i| i.images.len())),
    }
}

/// A progressive JPEG of `w` × `h` and three components: its headers and a first scan of a few
/// bytes (the reviewer's crafted file, about 200 bytes).
fn progressive_jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00];
    j.extend([1u8; 64]);
    j.extend([0xFF, 0xC2, 0x00, 0x11, 8]);
    j.extend(h.to_be_bytes());
    j.extend(w.to_be_bytes());
    j.push(3);
    for c in 1..=3 {
        j.extend([c, 0x11, 0x00]);
    }
    j.extend([0xFF, 0xC4, 0x00, 0x14, 0x00, 0x01]);
    j.extend([0u8; 16]);
    j.extend([
        0xFF, 0xDA, 0x00, 0x0C, 3, 1, 0, 2, 0, 3, 0, 0x00, 0x00, 0x00,
    ]);
    j.extend([0u8; 64]);
    j.extend([0xFF, 0xD9]);
    j
}

#[test]
fn a_large_progressive_jpeg_is_refused_before_its_planes_are_allocated() {
    // 100 megapixels: zune-jpeg's coefficient planes alone would take 600 MB, outside
    // `image`'s count.
    let dir = tempfile::tempdir().unwrap();
    let path = book_of(
        dir.path(),
        None,
        &[("p.jpg", &progressive_jpeg(10_000, 10_000))],
    );
    let book = read_book(&path).unwrap();
    let (got, allocated) =
        allocated_by(|| convert::images(&book, &path, &ConvertOptions::default()));
    match got {
        Err(Error::TooLarge { what }) => assert!(what.contains("p.jpg"), "{what}"),
        other => panic!("{:?}", other.map(|i| i.images.len())),
    }
    assert!(allocated < 8 << 20, "{allocated} bytes");
    // 16 megapixels still converts, fitted into 1,920 px.
    let dir = tempfile::tempdir().unwrap();
    let path = book_of(
        dir.path(),
        None,
        &[("p.jpg", &progressive_jpeg(4_000, 4_000))],
    );
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    assert_eq!(dims(&got.images["p.jpg"]), (1920, 1920));
}

#[test]
fn a_picture_of_exactly_100_megapixels_is_not_refused() {
    // `MAX_IMAGE_PIXELS` is the most a picture may have: 10,000 × 10,000 is within it. Its
    // data is missing, so it is left out as undecodable, not refused.
    let mut png = encode(
        &DynamicImage::ImageLuma8(image::GrayImage::new(8, 8)),
        ImageFormat::Png,
    );
    png[16..20].copy_from_slice(&10_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&10_000u32.to_be_bytes());
    fix_png_crc(&mut png, 8);
    let dir = tempfile::tempdir().unwrap();
    let path = book_of(dir.path(), None, &[("p.png", &png)]);
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    assert!(got.images.is_empty());
}

/// The quantisation tables (`DQT` segments) of a JPEG, in order.
fn jpeg_tables(b: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= b.len() && b[i] == 0xFF && b[i + 1] != 0xDA {
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        if b[i + 1] == 0xDB {
            out.push(&b[i + 4..i + 2 + len]);
        }
        i += 2 + len;
    }
    out
}

#[test]
fn jpegs_are_written_at_quality_85() {
    let path = fixtures::kfxgen("sources/pictures_more.epub");
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    let ours = &got.images["OEBPS/cmyk.jpg"];
    let mut want = Vec::new();
    image::load_from_memory(ours)
        .unwrap()
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut want, 85,
        ))
        .unwrap();
    assert!(!jpeg_tables(ours).is_empty());
    assert_eq!(jpeg_tables(ours), jpeg_tables(&want));
}

#[test]
fn a_picture_needing_more_than_512_mib_is_refused_before_decoding() {
    // 81 megapixels, within the pixel limit, but 16-bit RGBA: 648 MB decoded.
    let mut png = encode(&picture(8, 8), ImageFormat::Png);
    png[16..20].copy_from_slice(&9_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&9_000u32.to_be_bytes());
    png[24..26].copy_from_slice(&[16, 6]);
    fix_png_crc(&mut png, 8);
    let dir = tempfile::tempdir().unwrap();
    let path = book_of(dir.path(), None, &[("p.png", &png)]);
    let book = read_book(&path).unwrap();
    let (got, allocated) =
        allocated_by(|| convert::images(&book, &path, &ConvertOptions::default()));
    match got {
        Err(Error::TooLarge { what }) => assert!(what.contains("p.png"), "{what}"),
        other => panic!("{:?}", other.map(|i| i.images.len())),
    }
    assert!(allocated < 8 << 20, "{allocated} bytes");
}

#[test]
fn svg_pictures_are_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>"#;
    let path = book_of(dir.path(), None, &[("p.svg", svg)]);
    match images_of(&path, &ConvertOptions::default()) {
        Err(Error::Unsupported { feature, file }) => {
            assert_eq!((feature, file.as_str()), ("SVG pictures", "p.svg"));
        }
        other => panic!("{:?}", other.map(|i| i.images.len())),
    }
    // One drawn with shapes in a page, even where nothing shows it.
    let path = dir.path().join("inline.epub");
    let body = r#"<div style="display: none"><svg xmlns="http://www.w3.org/2000/svg"><rect width="4" height="4"/></svg></div>"#;
    write_epub(&path, "", "", body, &[]);
    match images_of(&path, &ConvertOptions::default()) {
        Err(Error::Unsupported { feature, file }) => {
            assert_eq!((feature, file.as_str()), ("SVG pictures", "c.xhtml"));
        }
        other => panic!("{:?}", other.map(|i| i.images.len())),
    }
}

#[test]
fn embedded_originals_are_only_checked() {
    let dir = tempfile::tempdir().unwrap();
    let big = encode(&picture(2000, 10), ImageFormat::Png);
    let png = encode(&picture(30, 30), ImageFormat::Png);
    let path = book_of(
        dir.path(),
        None,
        &[("big.png", &big), ("cut.png", &png[..60])],
    );
    let opts = ConvertOptions {
        embed_original_images: true,
        ..ConvertOptions::default()
    };
    let got = images_of(&path, &opts).unwrap();
    assert_eq!(got.images.get("big.png"), Some(&big));
    assert!(!got.images.contains_key("cut.png"));
}

#[test]
fn a_heavy_picture_is_re_encoded_at_its_size() {
    let dir = tempfile::tempdir().unwrap();
    // Noise: over 1 MiB as a PNG, within 1,920 px.
    let mut seed = 1u32;
    let noise = DynamicImage::ImageRgb8(RgbImage::from_fn(640, 640, |_, _| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let [a, b, c, _] = seed.to_be_bytes();
        Rgb([a, b, c])
    }));
    let raw = encode(&noise, ImageFormat::Png);
    assert!(raw.len() > MAX_BYTES);
    let path = book_of(dir.path(), None, &[("heavy.png", &raw)]);
    let got = images_of(&path, &ConvertOptions::default()).unwrap();
    let out = &got.images["heavy.png"];
    assert_eq!(dims(out), (640, 640));
    assert!(out.len() < raw.len());
}
