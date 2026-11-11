// SPDX-License-Identifier: GPL-3.0-or-later
//! The book's metadata, `$490` (`build_fragment_490`, `native_generator.py:2859`).
use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::Value;

use super::{Generator, st};

pub(crate) struct BookMetadata<'a> {
    pub title: &'a str,
    pub asin: &'a str,
    pub container_id: &'a str,
    pub language: &'a str,
    pub issue_date: Option<&'a str>,
    pub publisher: &'a str,
    /// `authors or [author]`.
    pub authors: Vec<&'a str>,
    pub description: Option<&'a str>,
    /// The cover's `$164`.
    pub cover_image: Option<&'a str>,
}

/// The symbols `build_fragment_490` creates (:2878); its values are strings, so a book keeps
/// none of them.
const SYMBOLS: [&str; 22] = [
    "title",
    "author",
    "ASIN",
    "asset_id",
    "book_id",
    "content_id",
    "cover_image",
    "description",
    "is_sample",
    "issue_date",
    "language",
    "override_kindle_font",
    "publisher",
    "kindle_audit_metadata",
    "kindle_ebook_metadata",
    "kindle_title_metadata",
    "kindle_capability_metadata",
    "file_creator",
    "creator_version",
    "selection",
    "nested_span",
    "KPR",
];

fn entry(key: &str, value: Value) -> Value {
    st([("$492", Value::string(key)), ("$307", value)])
}

fn category(name: &str, entries: Vec<Value>) -> Value {
    st([
        ("$495", Value::string(name)),
        ("$258", Value::List(entries)),
    ])
}

/// `build_fragment_490` with no embedded fonts.
pub(crate) fn build_fragment_490(g: &mut Generator, m: &BookMetadata) -> Fragment {
    for name in SYMBOLS {
        g.local(name);
    }
    let text = Value::string;
    let audit = category(
        "kindle_audit_metadata",
        vec![
            entry("file_creator", text("KPR")),
            entry("creator_version", text("3.98.0")),
        ],
    );
    let ebook = category(
        "kindle_ebook_metadata",
        vec![
            entry("selection", text("enabled")),
            entry("nested_span", text("enabled")),
        ],
    );
    let mut title_meta = vec![
        entry("ASIN", text(m.asin)),
        entry("asset_id", text(m.container_id)),
    ];
    title_meta.extend(m.authors.iter().map(|a| entry("author", text(a))));
    title_meta.extend([
        entry("book_id", text(m.asin)),
        entry("cde_content_type", text("PDOC")),
        entry("content_id", text(m.asin)),
    ]);
    if let Some(d) = m.description {
        title_meta.push(entry("description", text(d)));
    }
    title_meta.push(entry("is_sample", Value::Bool(false)));
    if let Some(date) = m.issue_date {
        title_meta.push(entry("issue_date", text(date)));
    }
    title_meta.extend([
        entry("language", text(m.language)),
        // bool(self.font_table.faces): no embedded fonts
        entry("override_kindle_font", Value::Bool(false)),
        entry("publisher", text(m.publisher)),
        entry("title", text(m.title)),
    ]);
    if let Some(cover) = m.cover_image {
        title_meta.push(entry("cover_image", text(cover)));
    }
    let value = st([(
        "$491",
        Value::List(vec![
            audit,
            ebook,
            category("kindle_title_metadata", title_meta),
            category("kindle_capability_metadata", Vec::new()),
        ]),
    )]);
    Fragment::root("$490", value)
}
