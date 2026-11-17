// SPDX-License-Identifier: GPL-3.0-only
//! The book-level arguments of `generate_full_book` (`converter.py` `convert_oeb_to_kfx`,
//! `extract_metadata`, `_book_landmarks_for`, `_book_page_list`): everything but the chapters,
//! pictures, fonts and notes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use super::href::{key_doc, unquote};
use super::options::ConvertOptions;
use super::package::{CalibreOpf, NavLink};
use super::toc::spine_in_order;
use crate::book::Book;
use crate::generate::py;
use crate::pyvalue::PyValue;

/// The thirteen book-level arguments, in this order: `title`, `author`, `authors`,
/// `description`, `language`, `publisher`, `issue_date`, `rtl`, `writing_mode`, `landmarks`,
/// `page_list`, `approximate_pages`, `resolved_image_refs`. A value kfxgen leaves out
/// (`authors` with no creator, `description`, `issue_date`) is `None`, as `generate_full_book`
/// receives it.
pub fn book_args(book: &Book, opf: &CalibreOpf, opts: &ConvertOptions) -> Vec<(String, PyValue)> {
    let m = &book.oeb.metadata;
    let str_ = |s: &str| PyValue::Str(s.to_owned());
    let opt = |s: Option<&String>| s.map_or(PyValue::None, |s| str_(s));

    let language = m
        .language
        .first()
        .map_or_else(|| "en".to_owned(), |l| kfx_language(l));
    // `_opf_raw_dates`: Calibre's own date only when its OPF can't be read, which the port's
    // always can.
    let issue_date = if m.date.is_empty() {
        None
    } else {
        issue_date(opf.dates.first().map_or("", String::as_str))
    };
    let rtl = book.oeb.page_progression_direction.as_deref() == Some("rtl");
    let writing_mode = if fixed_layout(book) {
        "horizontal-tb"
    } else {
        book_writing_mode(&language, rtl)
    };
    let names = spine_names(book, opf);
    let tuple = |items: &[&str]| PyValue::Tuple(items.iter().map(|s| str_(s)).collect());

    let mut landmarks = book_landmarks(opf, &names);
    if opts.open_at_cover {
        landmarks.retain(|(kind, _, _)| *kind != "start");
    }
    let landmarks = landmarks
        .iter()
        .map(|(kind, label, key)| tuple(&[kind, label, key]))
        .collect();
    let page_list = book_page_list(opf, &names)
        .iter()
        .map(|(label, key)| tuple(&[label, key]))
        .collect();

    vec![
        (
            "title".into(),
            str_(m.title.first().map_or("Untitled", String::as_str)),
        ),
        (
            "author".into(),
            str_(&if m.creator.is_empty() {
                "Unknown".to_owned()
            } else {
                m.creator.join(", ")
            }),
        ),
        (
            "authors".into(),
            if m.creator.is_empty() {
                PyValue::None
            } else {
                PyValue::List(m.creator.iter().map(|c| str_(c)).collect())
            },
        ),
        ("description".into(), opt(m.description.first())),
        ("language".into(), str_(&language)),
        (
            "publisher".into(),
            str_(m.publisher.first().map_or("kfxgen", String::as_str)),
        ),
        ("issue_date".into(), opt(issue_date.as_ref())),
        ("rtl".into(), PyValue::Bool(rtl)),
        ("writing_mode".into(), str_(writing_mode)),
        ("landmarks".into(), PyValue::List(landmarks)),
        ("page_list".into(), PyValue::List(page_list)),
        (
            "approximate_pages".into(),
            PyValue::Bool(!opts.disable_approximate_pages),
        ),
        ("resolved_image_refs".into(), PyValue::Bool(true)),
    ]
}

/// `_FULL_DATE_RE`; `\d` is any decimal digit, as in Python.
static FULL_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}").expect("a valid pattern"));

/// `extract_metadata`'s `issue_date` from the first date text of Calibre's OPF: its first ten
/// characters when, stripped, it starts as a full date, else None (a year alone is not one).
fn issue_date(first: &str) -> Option<String> {
    let first = py::strip(first);
    FULL_DATE
        .is_match(first)
        .then(|| py::take(first, 10).to_owned())
}

/// `_iso639_1` (calibre `lang_as_iso639_1`): the two-letter form of a language code, the
/// inverse of phase 2's table through Calibre's `canonicalize_lang`.
fn iso639_1(code: &str) -> Option<&'static str> {
    let three = crate::epub::package::canonicalize_lang(code)?;
    crate::epub::package::ISO639_2TO3
        .iter()
        .find(|(_, t)| *t == three)
        .map(|(two, _)| *two)
}

/// `_kfx_language`: the language as Kindle Previewer writes it: the part before the first `-`
/// or `_` in its two-letter form when it has three letters and one exists, the rest kept up to
/// a line break (Python's `.`); `en` when that part is empty or `und`.
pub(crate) fn kfx_language(lang: &str) -> String {
    let cut = lang.find(['-', '_']).unwrap_or(lang.len());
    let (primary, rest) = lang.split_at(cut);
    let rest = &rest[..rest.find('\n').unwrap_or(rest.len())];
    // Python's `lower()`; `und` matches only ASCII forms either way.
    if primary.is_empty() || primary.to_lowercase() == "und" {
        return "en".to_owned();
    }
    let primary = if py::len(primary) == 3 {
        iso639_1(primary).unwrap_or(primary)
    } else {
        primary
    };
    format!("{primary}{rest}")
}

/// `_book_writing_mode`: `vertical-rl` for a book whose pages turn right to left and whose
/// language (before any `-` or `_`) is `ja` in any case, else `horizontal-tb`.
fn book_writing_mode(language: &str, rtl: bool) -> &'static str {
    let base = language.split(['-', '_']).next().unwrap_or("");
    if rtl && base.to_lowercase() == "ja" {
        "vertical-rl"
    } else {
        "horizontal-tb"
    }
}

/// `_PIXEL_VIEWPORT` with `re.I`: `width` (its `i` also `İ` or `ı`, as Python folds it) after
/// the start or a space, comma or semicolon, then `=` and a digit. `\s` is Python's: Unicode
/// white space and U+001C to U+001F.
static PIXEL_VIEWPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s\x1C-\x1F,;])[wW][iIİı][dD][tT][hH][\s\x1C-\x1F]*=[\s\x1C-\x1F]*\d")
        .expect("a valid pattern")
});

/// `_fixed_layout`: whether most of the spine's pages carry a `<meta name="viewport">` sized
/// in pixels.
fn fixed_layout(book: &Book) -> bool {
    let pages = book.oeb.spine.len();
    let sized = book
        .files
        .iter()
        .filter(|f| {
            let dom = &f.dom;
            dom.descendants().any(|m| {
                dom.tag(m) == "meta"
                    && dom.attr(m, "name").unwrap_or("").to_lowercase() == "viewport"
                    && PIXEL_VIEWPORT.is_match(dom.attr(m, "content").unwrap_or(""))
            })
        })
        .count();
    pages > 0 && sized * 2 > pages
}

/// The names `_respelled` matches keys against: `unquote(href)` of each file of
/// `_spine_in_order(oeb_book, [])`.
fn spine_names(book: &Book, opf: &CalibreOpf) -> BTreeSet<String> {
    spine_in_order(book, opf, &[])
        .into_iter()
        .map(|i| unquote(&book.oeb.spine[i].href))
        .collect()
}

/// `_respelled`: each key whose file part names a spine file only up to letter case, in the
/// spine's spelling, when exactly one file fits.
fn respell(key: &str, names: &BTreeSet<String>, loose: &BTreeMap<String, Vec<&str>>) -> String {
    let (path, rest) = match key.find('#') {
        Some(i) => key.split_at(i),
        None => (key, ""),
    };
    match loose.get(&path.to_lowercase()).map(Vec::as_slice) {
        Some([only]) if !names.contains(path) => format!("{only}{rest}"),
        _ => key.to_owned(),
    }
}

/// The names by their lower-case spelling, for [`respell`].
fn loose_names(names: &BTreeSet<String>) -> BTreeMap<String, Vec<&str>> {
    let mut loose: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for n in names {
        loose.entry(n.to_lowercase()).or_default().push(n);
    }
    loose
}

/// `_NAV_LANDMARKS` and `_GUIDE_LANDMARKS`.
fn nav_landmark(t: &str) -> Option<&'static str> {
    match t {
        "toc" => Some("toc"),
        "bodymatter" => Some("start"),
        _ => None,
    }
}

fn guide_landmark(t: &str) -> Option<&'static str> {
    match t {
        "toc" => Some("toc"),
        "text" => Some("start"),
        _ => None,
    }
}

/// `_book_landmarks`: the contents and start landmarks as (kind, label, key), from the nav's
/// landmarks when it has any links there, else from the guide; only the last contents
/// landmark kept, every start kept.
fn book_landmarks(
    opf: &CalibreOpf,
    names: &BTreeSet<String>,
) -> Vec<(&'static str, String, String)> {
    let links: &[NavLink] = opf.nav.as_ref().map_or(&[], |n| n.landmarks.as_slice());
    let nav_base = opf.nav_path.as_deref().unwrap_or("");
    let found: Vec<(Option<&'static str>, String, String)> = if !links.is_empty() {
        links
            .iter()
            .map(|a| {
                let kind = a
                    .epub_type
                    .as_deref()
                    // Python's `split()`.
                    .and_then(|t| t.split(py::is_space).find_map(nav_landmark));
                let label = py::strip(&a.text).to_owned();
                (
                    kind,
                    label,
                    key_doc(nav_base, a.href.as_deref().unwrap_or("")),
                )
            })
            .collect()
    } else {
        opf.guide
            .iter()
            .map(|(kind, title, href)| {
                let kind = guide_landmark(&py::strip(kind).to_lowercase());
                (kind, py::strip(title).to_owned(), key_doc("_", href))
            })
            .collect()
    };
    let found: Vec<(&'static str, String, String)> = found
        .into_iter()
        .filter_map(|(kind, label, key)| Some((kind?, label, key)).filter(|m| !m.2.is_empty()))
        .collect();
    let last_toc = found.iter().rposition(|m| m.0 == "toc");
    let loose = loose_names(names);
    found
        .into_iter()
        .enumerate()
        .filter(|(n, m)| m.0 != "toc" || Some(*n) == last_toc)
        .map(|(_, (kind, label, key))| (kind, label, respell(&key, names, &loose)))
        .collect()
}

/// `_PAGE_LABEL_WRAP_RE` (DOTALL): the label inside one pair of `[]`, `{}` or `()`, spaces
/// inside them left out; else all of it.
static PAGE_LABEL_WRAP: LazyLock<Regex> = LazyLock::new(|| {
    let s = r"[\s\x1C-\x1F]*";
    Regex::new(&format!(
        r"(?s)^(?:\[{s}(.*?){s}\]|\{{{s}(.*?){s}\}}|\({s}(.*?){s}\)|(.*))$"
    ))
    .expect("a valid pattern")
});

/// `_PAGE_LABEL_PREFIX_RE` (`re.I`): "page", "pg" or "p", a dot, spaces.
static PAGE_LABEL_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[pP][aA][gG][eE]|[pP][gG]|[pP])\.?[\s\x1C-\x1F]*").expect("a valid pattern")
});

/// `_PAGE_LABEL_RE` (`re.I`), matched whole: decimal digits, or the letters of a roman
/// numeral (`i` also `İ` or `ı`, as Python folds it).
static PAGE_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\d+|[ivxlcdmIVXLCDMİı]+)$").expect("a valid pattern"));

/// `_page_label`: a page label stripped, out of its brackets, without "Page", "Pg" or "p.".
fn page_label(text: &str) -> String {
    let text = py::strip(text);
    let caps = PAGE_LABEL_WRAP
        .captures(text)
        .expect("the last branch matches anything");
    let inner = (1..=4).find_map(|i| caps.get(i)).map_or("", |m| m.as_str());
    let inner = py::strip(inner);
    let unprefixed = match PAGE_LABEL_PREFIX.find(inner) {
        Some(m) => &inner[m.end()..],
        None => inner,
    };
    py::strip(unprefixed).to_owned()
}

/// `_book_page_list`: the print page breaks as (label, key), from the nav's page list, else
/// the NCX's; none at all when any cleaned label is neither all digits nor all roman letters.
fn book_page_list(opf: &CalibreOpf, names: &BTreeSet<String>) -> Vec<(String, String)> {
    let nav_base = opf.nav_path.as_deref().unwrap_or("");
    let mut found: Vec<(String, String)> = opf
        .nav
        .as_ref()
        .map_or(&[][..], |n| n.page_list.as_slice())
        .iter()
        .map(|a| {
            (
                py::strip(&a.text).to_owned(),
                key_doc(nav_base, a.href.as_deref().unwrap_or("")),
            )
        })
        .collect();
    if found.is_empty()
        && let Some(ncx) = &opf.ncx
    {
        found = ncx
            .page_targets
            .iter()
            .map(|(label, src)| (py::strip(label).to_owned(), key_doc(&ncx.path, src)))
            .collect();
    }
    let found: Vec<(String, String)> = found
        .into_iter()
        .filter(|(label, key)| !label.is_empty() && !key.is_empty())
        .map(|(label, key)| (page_label(&label), key))
        .collect();
    if found.iter().any(|(label, _)| !PAGE_LABEL.is_match(label)) {
        return Vec::new();
    }
    let loose = loose_names(names);
    found
        .into_iter()
        .map(|(label, key)| {
            let key = respell(&key, names, &loose);
            (label, key)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_as_previewer_writes_them() {
        assert_eq!(kfx_language("eng"), "en");
        assert_eq!(kfx_language("ENG"), "en");
        assert_eq!(kfx_language("jpn"), "ja");
        assert_eq!(kfx_language("haw"), "haw");
        assert_eq!(kfx_language("und"), "en");
        assert_eq!(kfx_language("UND-x"), "en");
        assert_eq!(kfx_language(""), "en");
        assert_eq!(kfx_language("-GB"), "en");
        assert_eq!(kfx_language("en-GB"), "en-GB");
        assert_eq!(kfx_language("pt_BR"), "pt_BR");
        assert_eq!(kfx_language("por-BR"), "pt-BR");
        // Python's `.` stops at a line break; `[^-_]*` does not.
        assert_eq!(kfx_language("en-GB\nx"), "en-GB");
        assert_eq!(kfx_language("e\nn"), "e\nn");
        // Calibre's `canonicalize_lang` strips first: " en" is "eng", so "en".
        assert_eq!(kfx_language(" en"), "en");
    }

    #[test]
    fn page_labels_lose_brackets_and_page_words() {
        for (raw, want) in [
            ("12", "12"),
            (" [Pg 12] ", "12"),
            ("{vii}", "vii"),
            ("( p. 3 )", "3"),
            ("Page 4", "4"),
            ("pg.5", "5"),
            ("[1] and [2]", "1] and [2"),
            ("[]", ""),
            ("page", ""),
            ("x\ny", "x\ny"),
        ] {
            assert_eq!(page_label(raw), want, "{raw:?}");
        }
        assert!(PAGE_LABEL.is_match("xIv"));
        assert!(PAGE_LABEL.is_match("ıv"));
        assert!(PAGE_LABEL.is_match("١٢"));
        assert!(!PAGE_LABEL.is_match("12a"));
        assert!(!PAGE_LABEL.is_match(""));
    }

    #[test]
    fn full_dates_only() {
        assert_eq!(
            issue_date(" 2021-03-04T00:00:00 "),
            Some("2021-03-04".into())
        );
        assert_eq!(issue_date("2021"), None);
        assert_eq!(issue_date("2021-3-4"), None);
        assert_eq!(issue_date(""), None);
        assert_eq!(issue_date("٢٠٢١-٠٣-٠٤x"), Some("٢٠٢١-٠٣-٠٤".into()));
    }

    #[test]
    fn landmarks_keep_the_last_contents_and_every_start() {
        let g = |t: &str, title: &str, h: &str| (t.to_owned(), title.to_owned(), h.to_owned());
        let opf = CalibreOpf {
            guide: vec![
                g(" TOC ", "First", "a.xhtml"),
                g("text", " Go ", "B.XHTML#x"),
                g("cover", "Cover", "c.xhtml"),
                g("toc", "Second", "c.xhtml"),
                g("text", "Again", "../out.xhtml"),
                g("text", "Also", "d.xhtml"),
            ],
            ..CalibreOpf::default()
        };
        let names: BTreeSet<String> = ["a.xhtml", "b.xhtml", "c.xhtml", "d.xhtml"]
            .map(str::to_owned)
            .into();
        let got = book_landmarks(&opf, &names);
        let want = [
            ("start", "Go", "b.xhtml#x"),
            ("toc", "Second", "c.xhtml"),
            ("start", "Also", "d.xhtml"),
        ];
        assert_eq!(
            got,
            want.map(|(k, l, h)| (k, l.to_owned(), h.to_owned()))
                .to_vec()
        );
    }

    #[test]
    fn pixel_viewports() {
        for (content, sized) in [
            ("width=1200, height=1600", true),
            ("height=1600,WIDTH = 12", true),
            ("wİdth=1", true),
            ("width=device-width", false),
            ("min-width=10", false),
        ] {
            assert_eq!(PIXEL_VIEWPORT.is_match(content), sized, "{content:?}");
        }
    }
}
