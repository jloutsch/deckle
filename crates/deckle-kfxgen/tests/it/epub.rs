// SPDX-License-Identifier: GPL-3.0-only
//! The package as Calibre presents it to kfxgen (phase 2 plan Task 3): `oeb_view` against the
//! `cascade/<book>/oeb.json` dumps, Calibre 9.18's own view recorded inside a kfxgen run.

use std::io::Write as _;

use deckle_kfx::epub_zip::EpubReader;
use deckle_kfx::fixtures;
use deckle_kfxgen::epub::{Error, Oeb, Package, TocNode, oeb_view, read_package};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Every book under `fixtures/kfxgen/cascade/`.
const BOOKS: [&str; 12] = [
    "css_quirks",
    "directions",
    "fake_margins",
    "fonts",
    "frames",
    "lists",
    "navigation",
    "no_toc",
    "notes",
    "package_quirks",
    "rtl",
    "vertical",
];

/// Manifest items in a dump that the port does not make yet, by book: (book, ids, why).
/// Each listed id must be in the dump, so the entry goes when the port catches up.
const NOT_YET_MADE: &[(&str, &[&str], &str)] = &[(
    "css_quirks",
    &["page_css1", "page_css2"],
    "Calibre writes one page_styles*.css per distinct set of @page and @font-face rules among \
     the files it flattens (flatcss.py collect_global_css). Telling them apart needs the \
     book's CSS, which the css module reads (Tasks 5 to 7); oeb_view always writes one.",
)];

fn open(path: &std::path::Path) -> (Package, EpubReader) {
    let mut zip = EpubReader::open(path).unwrap();
    let pkg = read_package(&mut zip).unwrap();
    (pkg, zip)
}

fn toc_json(nodes: &[TocNode]) -> Value {
    Value::Array(
        nodes
            .iter()
            .map(|n| json!({"title": n.title, "href": n.href, "children": toc_json(&n.children)}))
            .collect(),
    )
}

/// `oeb` in `book_dump.py`'s `_oeb_view` shape, without `raw`.
fn oeb_json(oeb: &Oeb) -> Value {
    let mut manifest: Vec<Value> = oeb
        .manifest
        .iter()
        .map(|i| json!({"id": i.id, "href": i.href, "media_type": i.media_type}))
        .collect();
    manifest.sort_by(|a, b| {
        (a["href"].as_str(), a["id"].as_str()).cmp(&(b["href"].as_str(), b["id"].as_str()))
    });
    let mut guide: Vec<Value> = oeb
        .guide
        .iter()
        .map(|g| json!({"type": g.kind, "href": g.href}))
        .collect();
    guide.sort_by(|a, b| {
        (a["type"].as_str(), a["href"].as_str()).cmp(&(b["type"].as_str(), b["href"].as_str()))
    });
    let m = &oeb.metadata;
    json!({
        "spine": oeb.spine.iter().map(|s| json!({"href": s.href, "linear": s.linear})).collect::<Vec<_>>(),
        "page_progression_direction": oeb.page_progression_direction,
        "manifest": manifest,
        "toc": toc_json(&oeb.toc),
        "auto_generated_toc": oeb.auto_generated_toc,
        "guide": guide,
        "metadata": {
            "title": m.title, "creator": m.creator, "description": m.description,
            "language": m.language, "publisher": m.publisher, "date": m.date, "cover": m.cover,
        },
    })
}

#[test]
fn the_dumps_are_from_calibre_9_18() {
    let tools = std::fs::read_to_string(fixtures::kfxgen("cascade/TOOLS")).unwrap();
    assert!(tools.contains("(calibre 9.18.0)"), "{tools}");
    assert!(tools.contains("TZ: UTC"), "{tools}");
    let mut found: Vec<String> = std::fs::read_dir(fixtures::kfxgen("cascade"))
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    assert_eq!(found, BOOKS);
}

#[test]
fn every_cascade_fixture_oeb_matches_calibre() {
    let mut failures = Vec::new();
    for book in BOOKS {
        let dump: Value = serde_json::from_slice(
            &std::fs::read(fixtures::kfxgen(&format!("cascade/{book}/oeb.json"))).unwrap(),
        )
        .unwrap();
        let (pkg, mut zip) = open(&fixtures::kfxgen(&format!("sources/{book}.epub")));
        let oeb = oeb_view(&pkg, &mut zip).unwrap();

        // `raw`: the EPUB's own OPF, its sha256 null where Calibre wrote over it.
        assert_eq!(
            dump["raw"]["opfname"],
            json!(pkg.opf_path),
            "{book}: opf path"
        );
        if let Some(want) = dump["raw"]["sha256"].as_str() {
            let got: String = Sha256::digest(pkg.opf_text.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(got, want, "{book}: opf_text is not the OPF's bytes");
        }

        let mut want = dump.clone();
        want.as_object_mut().unwrap().remove("raw");
        let mut got = oeb_json(&oeb);
        for &(b, ids, _why) in NOT_YET_MADE {
            if b != book {
                continue;
            }
            let items = want["manifest"].as_array_mut().unwrap();
            for id in ids {
                let before = items.len();
                items.retain(|i| i["id"] != json!(id));
                assert_eq!(
                    items.len() + 1,
                    before,
                    "{book}: allow-listed {id} is not in the dump"
                );
            }
        }
        // Task 8 makes the contents Calibre generates; until then only the flag is compared.
        if oeb.auto_generated_toc {
            want["toc"] = json!([]);
        }
        // A year-only dc:date takes the month of the run in Calibre, January in the port
        // (DIVERGENCES.md, "The month of a year-only date").
        if pkg
            .metadata
            .date
            .first()
            .is_some_and(|d| d.trim().len() == 4)
        {
            for v in [&mut want, &mut got] {
                for d in v["metadata"]["date"].as_array_mut().unwrap() {
                    let s = d.as_str().unwrap();
                    *d = json!(format!("{}MM{}", &s[..5], &s[7..]));
                }
            }
        }
        if got != want {
            failures.push(format!(
                "{book}:\n want {}\n  got {}",
                serde_json::to_string(&want).unwrap(),
                serde_json::to_string(&got).unwrap()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// An EPUB of `opf` (at `content.opf`) and `files`, written to a temp file.
fn epub_with(opf: &str, files: &[(&str, &str)]) -> tempfile::NamedTempFile {
    const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
    let mut entries: Vec<(&str, &[u8], u64)> = vec![
        ("mimetype", b"application/epub+zip", 20),
        (
            "META-INF/container.xml",
            CONTAINER.as_bytes(),
            CONTAINER.len() as u64,
        ),
        ("content.opf", opf.as_bytes(), opf.len() as u64),
    ];
    for (name, data) in files {
        entries.push((name, data.as_bytes(), data.len() as u64));
    }
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(&fixtures::stored_zip(&entries)).unwrap();
    f
}

const CHAPTER: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head><body><p>Hi.</p></body></html>"#;

fn opf2(manifest: &str, spine: &str, extra_meta: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier>
<dc:title>T</dc:title><dc:language>en</dc:language>{extra_meta}</metadata>
<manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
    )
}

#[test]
fn no_contents_file_gives_generated_flag() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(&fixtures::stored_epub(&[])).unwrap();
    let (pkg, mut zip) = open(f.path());
    assert_eq!(pkg.nav_text, None);
    assert_eq!(pkg.ncx_text, None);
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    assert!(oeb.auto_generated_toc);
    assert!(oeb.toc.is_empty());
    assert_eq!(oeb.spine.len(), 1);
    assert_eq!(oeb.spine[0].href, "c.xhtml");
}

#[test]
fn spine_item_missing_from_manifest_is_skipped() {
    let opf = opf2(
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>
<item id="b" href="b.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/><itemref idref="ghost"/><itemref idref="b"/>"#,
        "",
    );
    let f = epub_with(&opf, &[("a.xhtml", CHAPTER), ("b.xhtml", CHAPTER)]);
    let (pkg, mut zip) = open(f.path());
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    let spine: Vec<(&str, bool)> = oeb
        .spine
        .iter()
        .map(|s| (s.href.as_str(), s.linear))
        .collect();
    assert_eq!(spine, [("a.xhtml", true), ("b.xhtml", true)]);
}

#[test]
fn missing_cover_is_not_an_error() {
    // The cover image is named in the metadata and the manifest but not in the archive.
    let opf = opf2(
        r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>
<item id="cover-img" href="cover.jpg" media-type="image/jpeg"/>"#,
        r#"<itemref idref="c"/>"#,
        r#"<meta name="cover" content="cover-img"/>"#,
    );
    let f = epub_with(&opf, &[("c.xhtml", CHAPTER)]);
    let (pkg, mut zip) = open(f.path());
    assert_eq!(pkg.metadata.cover, ["cover-img"]);
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    assert!(oeb.metadata.cover.is_empty());
    assert!(oeb.guide.is_empty());
    assert!(oeb.manifest.iter().all(|i| i.id != "cover-img"));
    assert_eq!(oeb.spine.len(), 1);
}

#[test]
fn opf_nested_past_256_levels_is_refused() {
    let deep = format!("{}{}", "<x>".repeat(300), "</x>".repeat(300));
    let opf = opf2(
        r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="c"/>"#,
        &deep,
    );
    let f = epub_with(&opf, &[("c.xhtml", CHAPTER)]);
    let mut zip = EpubReader::open(f.path()).unwrap();
    match read_package(&mut zip) {
        Err(Error::Xml { file, .. }) => assert_eq!(file, "content.opf"),
        other => panic!("expected Error::Xml, got {:?}", other.map(|p| p.opf_path)),
    }
}
