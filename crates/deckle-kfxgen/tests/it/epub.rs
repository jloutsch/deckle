// SPDX-License-Identifier: GPL-3.0-only
//! The package as Calibre presents it to kfxgen (phase 2 plan Task 3): the book view against the
//! `cascade/<book>/oeb.json` dumps, Calibre 9.18's own view recorded inside a kfxgen run
//! ([`check_oeb`], run on `read_book` by `book.rs` and `corpus_cascade.rs`), and `oeb_view` on
//! hand-made packages.

use std::io::Write as _;
use std::time::Instant;

use deckle_kfx::epub_zip::EpubReader;
use deckle_kfx::fixtures;
use deckle_kfxgen::epub::{Error, Oeb, Package, TocNode, oeb_view, read_package};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::support::{BOOKS, Dump, Tally, read_json};

/// Manifest items in a dump that the port does not make yet, by book: (book, ids, the
/// DIVERGENCES.md section that says why). Each listed id must be in the dump, so the entry
/// goes when the port catches up.
const NOT_YET_MADE: &[(&str, &[&str], &str)] = &[(
    "css_quirks",
    &["page_css1", "page_css2"],
    "One page_styles.css until the CSS is read",
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
    crate::support::check_tools(&fixtures::kfxgen("cascade")).unwrap();
    let tools = std::fs::read_to_string(fixtures::kfxgen("cascade/TOOLS")).unwrap();
    assert!(tools.contains("(calibre 9.18.0)"), "{tools}");
    let mut found: Vec<String> = std::fs::read_dir(fixtures::kfxgen("cascade"))
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    assert_eq!(found, BOOKS);
}

/// [`NOT_YET_MADE`] as (book, the key [`check_oeb`] records when it uses an entry).
pub(crate) fn allowed() -> Vec<(String, String)> {
    NOT_YET_MADE
        .iter()
        .flat_map(|&(book, ids, section)| {
            ids.iter()
                .map(move |id| (book.to_owned(), format!("oeb {book} {id} ({section})")))
        })
        .collect()
}

/// The book view `read_book` gives against the dump's `oeb.json` (Task 3), and the OPF's
/// bytes against `raw.sha256`.
pub(crate) fn check_oeb(got: &deckle_kfxgen::book::Book, dump: &Dump, tally: &mut Tally) {
    let divergences =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/DIVERGENCES.md")).unwrap();
    let book = dump.name.as_str();
    let want_all = read_json(&dump.dir.join("oeb.json"));
    let (pkg, oeb) = (&got.package, &got.oeb);

    // `raw`: the EPUB's own OPF, its sha256 null where Calibre wrote over it.
    if want_all["raw"]["opfname"] != json!(pkg.opf_path) {
        tally.failures.push(format!(
            "{book}: opf path {} against {}",
            pkg.opf_path, want_all["raw"]["opfname"]
        ));
    }
    if let Some(want) = want_all["raw"]["sha256"].as_str() {
        let sha: String = Sha256::digest(pkg.opf_text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if sha != want {
            tally
                .failures
                .push(format!("{book}: opf_text is not the OPF's bytes"));
        }
    }

    let mut want = want_all.clone();
    want.as_object_mut().unwrap().remove("raw");
    let mut got = oeb_json(oeb);
    for &(b, ids, section) in NOT_YET_MADE {
        assert!(
            divergences.contains(&format!("\n## {section}\n")),
            "DIVERGENCES.md has no section {section:?}"
        );
        if b != book {
            continue;
        }
        let items = want["manifest"].as_array_mut().unwrap();
        for id in ids {
            let before = items.len();
            items.retain(|i| i["id"] != json!(id));
            if items.len() + 1 == before {
                tally
                    .allowed_used
                    .insert(format!("oeb {book} {id} ({section})"));
            } else {
                tally
                    .failures
                    .push(format!("{book}: allow-listed {id} is not in the dump"));
            }
        }
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
        tally.failures.push(format!(
            "{book}: book view\n want {}\n  got {}",
            serde_json::to_string(&want).unwrap(),
            serde_json::to_string(&got).unwrap()
        ));
    }
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

/// An EPUB 3 package of `manifest`, `spine` and `meta`, with the nav `nav.xhtml` listed.
fn opf3(manifest: &str, spine: &str, meta: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">x</dc:identifier>
<dc:language>en</dc:language>{meta}</metadata>
<manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
{manifest}</manifest><spine>{spine}</spine></package>"#
    )
}

fn nav_doc(body: &str) -> String {
    format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>N</title></head><body>{body}</body></html>"#
    )
}

#[test]
fn nested_landmark_navs_give_each_link_once() {
    // `epub_input.py` `landmarks_to_guide` walks every landmarks nav, so a link inside 200
    // nested navs is met 200 times; `seen` drops the repeats. 10,000 links of custom types
    // give 10,000 guide entries (`guide.py` `Clean` keeps `other.calibre-epub-type-…`).
    let links: String = (0..10_000)
        .map(|i| format!(r##"<a epub:type="t{i}" href="c.xhtml#a{i}">L{i}</a>"##))
        .collect();
    let nav = nav_doc(&format!(
        "{}{links}{}",
        r#"<nav epub:type="landmarks">"#.repeat(200),
        "</nav>".repeat(200)
    ));
    let opf = opf3(
        r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="c"/>"#,
        "<dc:title>T</dc:title>",
    );
    let f = epub_with(&opf, &[("c.xhtml", CHAPTER), ("nav.xhtml", &nav)]);
    // roxmltree parses recursively: in a debug build 200 nested elements need more than a
    // test thread's 2 MB of stack (it overflows between 128 and 160 levels; a release build
    // parses 256 levels in 2 MB).
    let path = f.path().to_owned();
    let (oeb, took) = std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let start = Instant::now();
            let (pkg, mut zip) = open(&path);
            let oeb = oeb_view(&pkg, &mut zip).unwrap();
            (oeb, start.elapsed())
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(oeb.guide.len(), 10_000);
    let mut kinds: Vec<&str> = oeb.guide.iter().map(|g| g.kind.as_str()).collect();
    kinds.dedup();
    assert_eq!(kinds.len(), 10_000);
    assert!(
        oeb.guide
            .iter()
            .all(|g| g.href == format!("c.xhtml#a{}", &g.kind["other.calibre-epub-type-t".len()..]))
    );
    // With the nested navs walked again and `seen` a list, it took 118 s in a debug build.
    assert!(took < crate::support::COST_LIMIT, "took {took:?}");
}

#[test]
fn a_large_package_reads_in_linear_time() {
    // 20,000 manifest items (8,000 of them sharing the id `dup`), 20,000 itemrefs, 20,000
    // guide references (half in the wrong case) and 20,000 creators with refines.
    const N: usize = 20_000;
    let id = |i: usize| {
        if i < 8_000 {
            "dup".to_owned()
        } else {
            format!("i{i}")
        }
    };
    let mut manifest = String::new();
    let mut spine = String::new();
    let mut guide = String::new();
    let mut meta = String::from("<dc:title>T</dc:title>");
    let doc = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body/></html>"#;
    let names: Vec<String> = (0..N).map(|i| format!("f{i}.xhtml")).collect();
    for i in 0..N {
        manifest.push_str(&format!(
            r#"<item id="{}" href="f{i}.xhtml" media-type="application/xhtml+xml"/>"#,
            id(i)
        ));
        spine.push_str(&format!(r#"<itemref idref="{}"/>"#, id(i)));
        let href = if i % 2 == 0 {
            format!("f{i}.xhtml")
        } else {
            format!("F{i}.XHTML")
        };
        guide.push_str(&format!(r#"<reference type="t{i}" href="{href}"/>"#));
        meta.push_str(&format!(
            r##"<dc:creator id="a{i}">Writer {i}</dc:creator><meta refines="#a{i}" property="role" scheme="marc:relators">aut</meta>"##
        ));
    }
    let opf = opf3(&manifest, &spine, &meta)
        .replace("</spine>", &format!("</spine><guide>{guide}</guide>"));
    let mut files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), doc)).collect();
    let nav = nav_doc("");
    files.push(("nav.xhtml", &nav));
    let f = epub_with(&opf, &files);
    let start = Instant::now();
    let (pkg, mut zip) = open(f.path());
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    let took = start.elapsed();
    // Each spine idref once: `dup` names the first of its items.
    assert_eq!(oeb.spine.len(), 1 + (N - 8_000));
    assert_eq!(oeb.metadata.creator.len(), N);
    // About 1.9 s in a debug build, and linear in N; it took 208 s before the package was
    // read through indexes.
    assert!(took < crate::support::COST_LIMIT, "took {took:?}");
}

#[test]
fn opf3_refines_give_roles_and_titles() {
    // `opf3.py`: `find_main_title` takes the title refined as `main`, `find_subtitle` the one
    // whose title-type holds `subtitle`, and `read_title` joins them with ": ". `read_authors`
    // keeps creators whose refined role is `aut` (no scheme, or `marc:relators`), so the
    // illustrator goes; `uniq` drops a repeat only when its name, file-as and display-seq
    // all match, so the second "B. Writer", with another file-as, stays; `display-seq` orders
    // them.
    let meta = r##"<dc:title id="t1">Sub</dc:title><dc:title id="t2">Main</dc:title>
<meta refines="#t1" property="title-type">subtitle</meta>
<meta refines="#t2" property="title-type">main</meta>
<dc:creator id="c1">A. Writer</dc:creator>
<meta refines="#c1" property="role" scheme="marc:relators">aut</meta>
<meta refines="#c1" property="display-seq">2</meta>
<dc:creator id="c2">I. Lustrator</dc:creator>
<meta refines="#c2" property="role" scheme="marc:relators">ill</meta>
<dc:creator id="c3">B. Writer</dc:creator>
<meta refines="#c3" property="role">aut</meta>
<meta refines="#c3" property="file-as">Writer, B.</meta>
<meta refines="#c3" property="display-seq">1</meta>
<dc:creator id="c4">B. Writer</dc:creator>
<meta refines="#c4" property="role">aut</meta>
<meta refines="#c4" property="file-as">Writer, Bee</meta>
<meta refines="#c4" property="display-seq">1</meta>
<dc:creator id="c5">A. Writer</dc:creator>
<meta refines="#c5" property="role" scheme="marc:relators">aut</meta>
<meta refines="#c5" property="display-seq">2</meta>"##;
    let opf = opf3(
        r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="c"/>"#,
        meta,
    );
    let f = epub_with(&opf, &[("c.xhtml", CHAPTER), ("nav.xhtml", &nav_doc(""))]);
    let (pkg, mut zip) = open(f.path());
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    assert_eq!(oeb.metadata.title, ["Main: Sub"]);
    assert_eq!(
        oeb.metadata.creator,
        ["B. Writer", "B. Writer", "A. Writer"]
    );
}

#[test]
fn nav_landmarks_become_the_guide() {
    // `epub_input.py` `landmarks_to_guide`: each landmark's first epub:type through
    // `guide_type_for_epubtype` (bodymatter → text, titlepage → title-page, others under
    // `other.calibre-epub-type-`), the first of each type, none for `cover` (left to
    // `rationalize_cover3`) or a missing file. They replace the OPF's references of the same
    // type; `guide.py` `Clean` then drops the OPF's unknown `other.x`.
    let opf = opf3(
        r#"<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>
<item id="d" href="d.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="c"/><itemref idref="d"/>"#,
        "<dc:title>T</dc:title>",
    )
    .replace(
        "</spine>",
        r#"</spine><guide><reference type="text" href="d.xhtml"/><reference type="other.x" href="d.xhtml"/></guide>"#,
    );
    let nav = nav_doc(
        r##"<nav epub:type="landmarks"><ol>
<li><a epub:type="bodymatter chapter" href="c.xhtml#start">Start</a></li>
<li><a epub:type="bodymatter" href="d.xhtml">Again</a></li>
<li><a epub:type="titlepage" href="d.xhtml">Title</a></li>
<li><a epub:type="cover" href="c.xhtml">Cover</a></li>
<li><a epub:type="loi" href="missing.xhtml">Pictures</a></li>
<li><a epub:type="appendix" href="d.xhtml#app">Appendix</a></li>
</ol></nav>"##,
    );
    let f = epub_with(
        &opf,
        &[
            ("c.xhtml", CHAPTER),
            ("d.xhtml", CHAPTER),
            ("nav.xhtml", &nav),
        ],
    );
    let (pkg, mut zip) = open(f.path());
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    let guide: Vec<(&str, &str)> = oeb
        .guide
        .iter()
        .map(|g| (g.kind.as_str(), g.href.as_str()))
        .collect();
    assert_eq!(
        guide,
        [
            ("other.calibre-epub-type-appendix", "d.xhtml#app"),
            ("text", "c.xhtml#start"),
            ("title-page", "d.xhtml"),
        ]
    );
}

#[test]
fn an_html_contents_page_gives_the_contents() {
    // `reader.py` `_toc_from_html`, with no NCX: the links under the guide's `toc` fragment,
    // or under its nearest ancestor that has links, one entry per target in the order first
    // met, the texts of links to the same target joined by a space. A link outside the
    // manifest is left out.
    let contents = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>C</title></head><body>
<p><a href="a.xhtml">Not in the list</a></p>
<div><h1 id="list">Contents</h1>
<p><a href="a.xhtml">One</a></p><p><a href="b.xhtml#x">Two</a></p>
<p><a href="a.xhtml">  First  chapter </a></p><p><a href="gone.xhtml">Gone</a></p></div>
</body></html>"##;
    let opf = opf2(
        r#"<item id="toc" href="toc.xhtml" media-type="application/xhtml+xml"/>
<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>
<item id="b" href="b.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="toc"/><itemref idref="a"/><itemref idref="b"/>"#,
        "",
    )
    .replace(
        "</spine>",
        r##"</spine><guide><reference type="toc" href="toc.xhtml#list"/></guide>"##,
    );
    let f = epub_with(
        &opf,
        &[
            ("toc.xhtml", contents),
            ("a.xhtml", CHAPTER),
            ("b.xhtml", CHAPTER),
        ],
    );
    let (pkg, mut zip) = open(f.path());
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    assert!(!oeb.auto_generated_toc);
    let toc: Vec<(&str, &str)> = oeb
        .toc
        .iter()
        .map(|n| (n.title.as_str(), n.href.as_str()))
        .collect();
    assert_eq!(
        toc,
        [("One First chapter", "a.xhtml"), ("Two", "b.xhtml#x")]
    );
}

#[test]
fn a_chapter_that_is_not_well_formed_still_has_its_links() {
    // 300 unclosed `<br>`s: `parse_xml` refuses the chapter (300 levels once the end tags are
    // recovered), and it is read as HTML, as Calibre reads it, so its link to `c.xhtml` adds
    // that file to the spine.
    let chapter = format!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>T</title></head><body><p>{}<a href="c.xhtml">c</a></p></body></html>"#,
        "line<br>".repeat(300)
    );
    let opf = opf2(
        r#"<item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>
<item id="b" href="b.xhtml" media-type="application/xhtml+xml"/>
<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>"#,
        r#"<itemref idref="a"/><itemref idref="b"/>"#,
        "",
    );
    let f = epub_with(
        &opf,
        &[
            ("a.xhtml", &chapter),
            ("b.xhtml", CHAPTER),
            ("c.xhtml", CHAPTER),
        ],
    );
    let (pkg, mut zip) = open(f.path());
    let oeb = oeb_view(&pkg, &mut zip).unwrap();
    let spine: Vec<(&str, bool)> = oeb
        .spine
        .iter()
        .map(|s| (s.href.as_str(), s.linear))
        .collect();
    assert_eq!(
        spine,
        [("a.xhtml", true), ("b.xhtml", true), ("c.xhtml", false)]
    );
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
