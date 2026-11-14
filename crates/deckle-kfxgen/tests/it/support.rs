// SPDX-License-Identifier: GPL-3.0-only
//! Helpers the CSS tests share: the cascade fixture books, each spine file's stylesheets in
//! Calibre's order, and the dumps `read_book` is compared with ([`check_book`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use deckle_kfx::epub_zip::{EpubReader, percent_decode};
use deckle_kfx::fixtures;
use deckle_kfxgen::book::{self, file_sheets};
use deckle_kfxgen::css::{Decls, Sheet};
use deckle_kfxgen::dom::{Dom, NodeId};
use deckle_kfxgen::epub::read_package;
use serde_json::Value;

/// Every book under `fixtures/kfxgen/cascade/`.
/// How long a cost test may take. Debug builds run several times slower and share the machine
/// with other test threads; 30 s still fails a quadratic loop (the ones found took 19 to 106 s).
pub const COST_LIMIT: std::time::Duration =
    std::time::Duration::from_secs(if cfg!(debug_assertions) { 30 } else { 5 });

pub const BOOKS: [&str; 13] = [
    "css_quirks",
    "directions",
    "fake_margins",
    "fonts",
    "frames",
    "lists",
    "malformed",
    "navigation",
    "no_toc",
    "notes",
    "package_quirks",
    "rtl",
    "vertical",
];

/// A cascade fixture book: its EPUB and its dump.
pub struct Book {
    pub zip: EpubReader,
    /// The manifest's stylesheets, as paths from the EPUB root.
    pub css_items: std::collections::HashSet<String>,
    /// Spine href → `NNN`, the dump's name for the file (`files.json`).
    pub files: BTreeMap<String, String>,
    /// `fixtures/kfxgen/cascade/<book>`.
    pub dir: PathBuf,
}

impl Book {
    pub fn open(name: &str) -> Book {
        let dir = fixtures::kfxgen(&format!("cascade/{name}"));
        let files =
            serde_json::from_slice(&std::fs::read(dir.join("files.json")).unwrap()).unwrap();
        let mut zip = EpubReader::open(&fixtures::kfxgen(&format!("sources/{name}.epub"))).unwrap();
        let pkg = read_package(&mut zip).unwrap();
        let css_items = book::stylesheets(&pkg);
        Book {
            zip,
            css_items,
            files,
            dir,
        }
    }

    /// A spine file's bytes.
    pub fn read(&mut self, href: &str) -> Vec<u8> {
        self.zip
            .read_bytes(href)
            .or_else(|_| self.zip.read_bytes(&percent_decode(href)))
            .unwrap()
    }

    /// The stylesheets of the spine file `href`, whose tree is `dom` ([`file_sheets`]).
    pub fn sheets(&mut self, href: &str, dom: &Dom) -> Vec<Sheet> {
        file_sheets(&mut self.zip, &self.css_items, href, dom).unwrap()
    }
}

/// The `@font-face` rules of `sheets`, in order.
pub fn font_faces(sheets: &[Sheet]) -> Vec<Decls> {
    sheets.iter().flat_map(|s| s.font_face.clone()).collect()
}

/// A dump's JSON file.
pub fn read_json(path: &Path) -> Value {
    serde_json::from_slice(
        &std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap()
}

/// An element path as the dumps write it.
pub fn path_of(v: &Value) -> Vec<u32> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_u64().unwrap() as u32)
        .collect()
}

/// The element at `path` (element-child indexes from the root), if there is one.
pub fn find(dom: &Dom, path: &[u32]) -> Option<NodeId> {
    let mut n = dom.root();
    for &i in path {
        n = *dom.children(n).get(i as usize)?;
    }
    Some(n)
}

/// Refuses dumps made with another Calibre, or outside UTC: `dir/TOOLS` must name Calibre
/// 9.18.0 and `TZ: UTC` (`tools/ref/kfxgen_models.sh`, `record_tools` and
/// `record_cascade_run`).
pub fn check_tools(dir: &Path) -> Result<(), String> {
    let file = dir.join("TOOLS");
    let tools = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    if !tools
        .lines()
        .any(|l| l == "calibre: ebook-convert (calibre 9.18.0)")
    {
        return Err(format!(
            "{}: not made with calibre 9.18.0:\n{tools}",
            file.display()
        ));
    }
    if !tools.lines().any(|l| l == "TZ: UTC") {
        return Err(format!("{}: not made in UTC:\n{tools}", file.display()));
    }
    Ok(())
}

/// One book's cascade dump (`tools/ref/kfxgen_hook/book_dump.py`).
pub struct Dump {
    /// The book's name: a fixture's (`css_quirks`) or a corpus book's (`pg11`).
    pub name: String,
    /// The folder holding `oeb.json`, `files.json`, `dom/`, `styles/` and `rules/`.
    pub dir: PathBuf,
    /// Spine href → `NNN`.
    pub files: BTreeMap<String, String>,
    /// Calibre's user-agent rules as the run dumped them.
    pub ua_rules: PathBuf,
}

impl Dump {
    fn new(name: &str, dir: PathBuf, ua_rules: PathBuf) -> Dump {
        Dump {
            name: name.to_owned(),
            files: serde_json::from_value(read_json(&dir.join("files.json"))).unwrap(),
            dir,
            ua_rules,
        }
    }

    /// The dump of the fixture book `name`, under `fixtures/kfxgen/cascade/`, whose
    /// user-agent rules the fixture books share.
    pub fn fixture(name: &str) -> Dump {
        Dump::new(
            name,
            fixtures::kfxgen(&format!("cascade/{name}")),
            fixtures::kfxgen("cascade/ua_rules.json"),
        )
    }

    /// The dump of the corpus book `name` in the corpus folder `root` (`--corpus` writes
    /// `<book>/cascade`, with its own user-agent rules).
    pub fn corpus(root: &Path, name: &str) -> Dump {
        let dir = root.join(name).join("cascade");
        let ua = dir.join("ua_rules.json");
        Dump::new(name, dir, ua)
    }

    /// The dump's name for the spine file `href`.
    pub fn nnn(&self, href: &str) -> &str {
        self.files
            .get(href)
            .unwrap_or_else(|| panic!("{}: {href} is not in files.json", self.name))
    }

    /// `<kind>/<NNN>.json` of the spine file `href` (`styles` or `rules`).
    pub fn json(&self, kind: &str, href: &str) -> Value {
        read_json(&self.dir.join(format!("{kind}/{}.json", self.nnn(href))))
    }

    /// `dom/<NNN>.xhtml` of the spine file `href`.
    pub fn xhtml(&self, href: &str) -> String {
        std::fs::read_to_string(self.dir.join(format!("dom/{}.xhtml", self.nnn(href)))).unwrap()
    }
}

/// What the comparisons of one run found: failures, what was compared, and the allow-list
/// entries that were needed.
#[derive(Default)]
pub struct Tally {
    pub failures: Vec<String>,
    /// The books compared.
    pub books: BTreeSet<String>,
    pub files: usize,
    /// Book rules of the first cascade.
    pub rules: usize,
    pub font_faces: usize,
    /// Chapter marks in the dumps.
    pub marks: usize,
    /// Elements whose declared styles were compared.
    pub elements: usize,
    /// Computed values compared.
    pub values: usize,
    /// Allow-list entries used, by their section and key.
    pub allowed_used: BTreeSet<String>,
}

impl Tally {
    /// Fails with every mismatch, and with every allow-list entry for a compared book that
    /// nothing needed (so the entry goes when the port catches up). `allowed` lists every
    /// entry as (book, key) with the key [`Tally::allowed_used`] records.
    pub fn assert_clean(&self, allowed: &[(String, String)]) {
        assert!(
            self.failures.is_empty(),
            "{}\n{} mismatches",
            self.failures.join("\n"),
            self.failures.len()
        );
        for (book, key) in allowed {
            assert!(
                !self.books.contains(book) || self.allowed_used.contains(key),
                "{key} is allowed but matches: take it out"
            );
        }
    }
}

/// Every allow-list entry of the four comparisons, as (book, key).
pub fn allowed() -> Vec<(String, String)> {
    let mut out = crate::epub::allowed();
    out.extend(crate::css_flatten::allowed());
    out.extend(crate::css_style::allowed());
    out
}

/// `read_book`'s result for `book` against its dump at four levels: the book view (Task 3),
/// the element tree and first-pass rules (Task 8), the declared styles (Task 9) and the
/// computed values (Task 10).
pub fn check_book(got: &book::Book, dump: &Dump, tally: &mut Tally) {
    tally.books.insert(dump.name.clone());
    crate::epub::check_oeb(got, dump, tally);
    crate::css_cascade::check_tree_and_rules(got, dump, tally);
    crate::css_flatten::check_specified(got, dump, tally);
    crate::css_style::check_computed(got, dump, tally);
}
