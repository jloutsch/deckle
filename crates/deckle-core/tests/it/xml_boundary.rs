// SPDX-License-Identifier: GPL-3.0-or-later
//! Every parse of file content goes through `deckle_limits::parse_xml`, which refuses XML nested
//! too deep before `roxmltree` recurses (nesting-depth design, mechanism 1). This walks the
//! workspace's Rust sources and fails on a direct `Document::parse` outside that wrapper.
//! Test code (`tests/` folders and `#[cfg(test)] mod` blocks) parses Deckle's own output and may
//! call `roxmltree` directly.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.rs` file under `dir`, skipping build output, `node_modules` and `tests/` folders.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_dir() {
            if !matches!(name.as_ref(), "target" | "node_modules" | "tests") {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// The text before the file's `#[cfg(test)] mod …`, the code that ships.
fn shipped(text: &str) -> &str {
    let mut offset = 0;
    // Where a `#[cfg(test)]` line starts, until the next item shows what it marks.
    let mut marked = None;
    for line in text.split_inclusive('\n') {
        let item = line.trim();
        if let Some(start) = marked {
            if item.starts_with("mod ") || item.starts_with("pub mod ") {
                return &text[..start];
            }
            if !item.is_empty() {
                marked = None;
            }
        }
        if item == "#[cfg(test)]" {
            marked = Some(offset);
        }
        offset += line.len();
    }
    text
}

fn all_sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in ["crates", "apps/desktop/src-tauri", "data"] {
        let dir = root().join(dir);
        if dir.is_dir() {
            sources(&dir, &mut files);
        }
    }
    files
}

#[test]
fn the_walk_sees_the_source_tree() {
    let files = all_sources();
    let has = |end: &str| files.iter().any(|f| f.ends_with(end));
    assert!(has("crates/deckle-docx/src/package.rs"));
    assert!(has("crates/deckle-core/src/epub.rs"));
    assert!(!has("crates/deckle-core/tests/it/xml_boundary.rs"));
    let package =
        std::fs::read_to_string(root().join("crates/deckle-docx/src/package.rs")).unwrap();
    assert!(shipped(&package).contains("deckle_limits::parse_xml("));
}

#[test]
fn shipped_stops_at_the_test_module_only() {
    let text = "fn a() {}\n#[cfg(test)]\nfn helper() {}\nfn b() {}\n\n#[cfg(test)]\nmod tests {\n    fn c() {}\n}\n";
    assert_eq!(
        shipped(text),
        "fn a() {}\n#[cfg(test)]\nfn helper() {}\nfn b() {}\n\n"
    );
    assert_eq!(shipped("fn a() {}\n"), "fn a() {}\n");
}

#[test]
fn only_deckle_limits_calls_document_parse() {
    let offenders: Vec<String> = all_sources()
        .into_iter()
        .filter(|f| !f.components().any(|c| c.as_os_str() == "deckle-limits"))
        .filter(|f| {
            let text = std::fs::read_to_string(f).unwrap();
            shipped(&text).contains("Document::parse")
        })
        .map(|f| f.strip_prefix(root()).unwrap_or(&f).display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "parse file content with deckle_limits::parse_xml, not roxmltree::Document::parse: {offenders:?}"
    );
}
