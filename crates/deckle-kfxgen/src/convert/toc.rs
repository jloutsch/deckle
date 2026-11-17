// SPDX-License-Identifier: GPL-3.0-only
//! The book's contents entries and the order of its files, as kfxgen's chapter assembly takes
//! them (`converter.py` `_extract_toc_with_hrefs`, `_with_ncx_empty_labels`, `_spine_in_order`,
//! `_respell_toc_hrefs`).
//!
//! Entries find their file by file name alone, as kfxgen's do: the first file of that name
//! wins. Every href here is only compared or used as a key, never opened. Each step is one pass
//! over the entries and files, through maps, and the contents tree is walked with a stack.

use std::collections::{HashMap, HashSet};

use super::href::{normalize, unquote};
use super::inline::words;
use super::package::{CalibreOpf, Ncx, file_name};
use crate::book::Book;
use crate::epub::SpineRef;
use crate::epub::package::{dirname, normpath, posix_join};
use crate::generate::py;

/// A contents entry: `{"title", "href", "level"}`, level 0 at the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub title: String,
    pub href: String,
    pub level: usize,
}

/// `_extract_toc_with_hrefs`: Calibre's contents depth first, an entry for each node with a
/// title (a node without one is left out, its children still a level deeper), then the NCX's
/// entries with a blank label put back ([`with_ncx_empty_labels`]) when that gives more.
pub fn toc_entries(book: &Book, opf: &CalibreOpf) -> Vec<TocEntry> {
    let mut entries = Vec::new();
    let mut stack: Vec<_> = book.oeb.toc.iter().rev().map(|n| (n, 0)).collect();
    while let Some((node, level)) = stack.pop() {
        if !node.title.is_empty() {
            entries.push(TocEntry {
                title: node.title.clone(),
                href: node.href.clone(),
                level,
            });
        }
        stack.extend(node.children.iter().rev().map(|c| (c, level + 1)));
    }
    let kept = with_ncx_empty_labels(&entries, opf.ncx.as_ref());
    if kept.len() > entries.len() {
        kept
    } else {
        entries
    }
}

/// `_with_ncx_empty_labels`: when Calibre's entries are the NCX's entries with a label, in
/// order and with the same words, the NCX's entries whose label is blank put back where it
/// has them, their label as written and their href from Calibre's OPF folder (the root). An
/// entry with no label at all is put back below the top level only, and one with no href not
/// at all. Else `entries` unchanged.
fn with_ncx_empty_labels(entries: &[TocEntry], ncx: Option<&Ncx>) -> Vec<TocEntry> {
    let Some(ncx) = ncx else {
        return entries.to_vec();
    };
    let blank = |label: &str| label.chars().all(py::is_space);
    if !ncx.points.iter().any(|(label, _, _)| blank(label)) {
        return entries.to_vec();
    }
    let ncx_dir = match dirname(&ncx.path) {
        "" => ".",
        d => d,
    };
    let mut out = Vec::new();
    let mut i = 0;
    for (label, src, depth) in &ncx.points {
        if !blank(label) {
            if entries
                .get(i)
                .is_none_or(|e| words(&e.title) != words(label))
            {
                return entries.to_vec();
            }
            out.push(entries[i].clone());
            i += 1;
        } else if !src.is_empty() && (!label.is_empty() || *depth > 0) {
            let (path, frag) = match src.split_once('#') {
                Some((p, f)) => (p, Some(f)),
                None => (src.as_str(), None),
            };
            let mut href = normpath(&posix_join(ncx_dir, path));
            if let Some(f) = frag {
                href.push('#');
                href.push_str(f);
            }
            out.push(TocEntry {
                title: label.clone(),
                href,
                level: *depth,
            });
        }
    }
    if i == entries.len() {
        out
    } else {
        entries.to_vec()
    }
}

/// `_spine_in_order`: Calibre's spine (indexes into [`Book::files`]) with the files Calibre
/// added in contents order. The added ones are the non-linear files at its end whose file name
/// the spine of Calibre's OPF does not list; they go after the rest by the first entry naming
/// their file name (none after all), then by file name, and one spelled as a file already
/// kept up to letter case and percent-encoding is left out.
///
/// Two added files of one file name sort alike in kfxgen, which then keeps Calibre's set order,
/// different from run to run; here they go by href (DIVERGENCES.md, "Added files of one
/// name").
pub fn spine_in_order(book: &Book, opf: &CalibreOpf, entries: &[TocEntry]) -> Vec<usize> {
    order(&book.oeb.spine, &opf.listed, entries)
}

fn order(spine: &[SpineRef], listed: &HashSet<String>, entries: &[TocEntry]) -> Vec<usize> {
    let mut toc: HashMap<String, usize> = HashMap::new();
    for (n, e) in entries.iter().enumerate() {
        toc.entry(file_name(&e.href)).or_insert(n);
    }
    let tail = spine.len() - spine.iter().rev().take_while(|s| !s.linear).count();
    let mut own: Vec<usize> = (0..tail).collect();
    let mut added = Vec::new();
    for (i, s) in spine.iter().enumerate().skip(tail) {
        let name = file_name(&s.href);
        if listed.contains(&name) {
            own.push(i);
        } else {
            added.push((toc.get(&name).copied().unwrap_or(toc.len()), name, i));
        }
    }
    let folded = |i: usize| unquote(spine[i].href.split('#').next().unwrap_or("")).to_lowercase();
    let mut seen: HashSet<String> = own.iter().map(|&i| folded(i)).collect();
    added.sort_by(|a, b| (a.0, &a.1, &spine[a.2].href).cmp(&(b.0, &b.1, &spine[b.2].href)));
    for (_, _, i) in added {
        if seen.insert(folded(i)) {
            own.push(i);
        }
    }
    own
}

/// `_respell_toc_hrefs`: each entry whose file name is no spine file's but names exactly one
/// spine href up to letter case and percent-encoding gets that href, its fragment kept.
/// `spine` holds indexes into [`Book::files`].
pub fn respell_toc_hrefs(entries: &mut [TocEntry], book: &Book, spine: &[usize]) {
    let hrefs: Vec<&str> = spine.iter().map(|&i| book.files[i].href.as_str()).collect();
    respell(entries, &hrefs);
}

fn respell(entries: &mut [TocEntry], hrefs: &[&str]) {
    let names: HashSet<String> = hrefs.iter().map(|h| normalize(h)).collect();
    let mut loose: HashMap<String, Vec<&str>> = HashMap::new();
    for &h in hrefs {
        let fits = loose
            .entry(normalize(&unquote(h)).to_lowercase())
            .or_default();
        if !fits.contains(&h) {
            // Two at most are kept: only "exactly one" is asked.
            if fits.len() < 2 {
                fits.push(h);
            }
        }
    }
    for entry in entries {
        let (path, rest) = match entry.href.find('#') {
            Some(at) => entry.href.split_at(at),
            None => (entry.href.as_str(), ""),
        };
        if names.contains(&normalize(path)) {
            continue;
        }
        if let Some([only]) = loose
            .get(&normalize(&unquote(path)).to_lowercase())
            .map(Vec::as_slice)
        {
            entry.href = format!("{only}{rest}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, href: &str, level: usize) -> TocEntry {
        TocEntry {
            title: title.to_owned(),
            href: href.to_owned(),
            level,
        }
    }

    fn spine(items: &[(&str, bool)]) -> Vec<SpineRef> {
        items
            .iter()
            .map(|&(h, linear)| SpineRef {
                href: h.to_owned(),
                linear,
            })
            .collect()
    }

    fn listed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    #[test]
    fn added_files_go_in_contents_order_then_by_name() {
        let s = spine(&[
            ("c1.xhtml", true),
            ("skip.xhtml", false),
            ("c2.xhtml", true),
            ("z.xhtml", false),
            ("own.xhtml", false),
            ("a.xhtml", false),
            ("m.xhtml", false),
        ]);
        let l = listed(&["c1.xhtml", "skip.xhtml", "c2.xhtml", "own.xhtml"]);
        // A non-linear file before a linear one stays where it is; `own.xhtml` is listed, so
        // it stays first of the tail; `m` and `z` have entries, `a` none; the first entry
        // naming a file decides.
        let entries = [
            entry("Z", "Text/z.xhtml#x", 0),
            entry("M", "m.xhtml", 0),
            entry("Z again", "z.xhtml", 0),
        ];
        assert_eq!(order(&s, &l, &entries), [0, 1, 2, 4, 3, 6, 5]);
        assert_eq!(order(&s, &l, &[]), [0, 1, 2, 4, 5, 6, 3]);
    }

    #[test]
    fn an_added_spelling_of_a_kept_file_is_left_out() {
        let s = spine(&[
            ("Text/c1.xhtml", true),
            ("Text/C1.XHTML", false),
            ("Text/b%20c.xhtml", false),
            ("Text/B C.xhtml", false),
        ]);
        let l = listed(&["c1.xhtml"]);
        // `B C.xhtml` sorts before `b c.xhtml`, so it is the spelling kept.
        assert_eq!(order(&s, &l, &[]), [0, 3]);
    }

    #[test]
    fn added_files_of_one_name_go_by_href() {
        let s = spine(&[
            ("c.xhtml", true),
            ("B/x.xhtml", false),
            ("a/x.xhtml", false),
            ("A/x.xhtml", false),
        ]);
        let entries = [entry("X", "B/x.xhtml", 0)];
        // With an entry or without, the three tie in kfxgen. By href `A/x.xhtml` comes first,
        // and of it and `a/x.xhtml`, one file up to case, it is the one kept, though the spine
        // has `a/x.xhtml` first.
        assert_eq!(order(&s, &listed(&["c.xhtml"]), &entries), [0, 3, 1]);
        assert_eq!(order(&s, &listed(&["c.xhtml"]), &[]), [0, 3, 1]);
    }

    /// What `_respell_toc_hrefs` does with hrefs a crafted contents can write.
    #[test]
    fn respelling_matches_by_file_name_and_refuses_unsafe_hrefs() {
        let hrefs = [
            "OEBPS/c1.xhtml",
            "OEBPS/b%20c.xhtml",
            "A/dup.xhtml",
            "B/DUP.xhtml",
        ];
        let mut entries = [
            // Other case: one fit, respelled, fragment kept.
            entry("", "OEBPS/C1.XHTML#f%3A1", 0),
            // Another folder: matched by file name alone, as kfxgen matches.
            entry("", "Other/c1.XHTML", 0),
            // Percent-encoding: decoded before matching.
            entry("", "b c.xhtml", 0),
            entry("", "c%31.xhtml", 0),
            // A `..` part, an absolute path, a scheme: `_normalize_href` gives "", no fit.
            entry("", "../evil/C1.xhtml", 0),
            entry("", "/OEBPS/C1.xhtml", 0),
            entry("", "http://x/C1.xhtml", 0),
            // Encoded `..`: refused once decoded too.
            entry("", "%2e%2e/C1.xhtml", 0),
            // Two fits up to case: left alone.
            entry("", "Dup.xhtml", 0),
            // Already a spine file's name: left alone, folder and all.
            entry("", "elsewhere/c1.xhtml", 0),
            // Nothing fits.
            entry("", "none.xhtml", 0),
        ];
        respell(&mut entries, &hrefs);
        let got: Vec<&str> = entries.iter().map(|e| e.href.as_str()).collect();
        assert_eq!(
            got,
            [
                "OEBPS/c1.xhtml#f%3A1",
                "OEBPS/c1.xhtml",
                "OEBPS/b%20c.xhtml",
                "OEBPS/c1.xhtml",
                "../evil/C1.xhtml",
                "/OEBPS/C1.xhtml",
                "http://x/C1.xhtml",
                "%2e%2e/C1.xhtml",
                "Dup.xhtml",
                "elsewhere/c1.xhtml",
                "none.xhtml",
            ]
        );
    }

    fn ncx(path: &str, points: &[(&str, &str, usize)]) -> Ncx {
        Ncx {
            path: path.to_owned(),
            page_targets: Vec::new(),
            points: points
                .iter()
                .map(|&(l, s, d)| (l.to_owned(), s.to_owned(), d))
                .collect(),
        }
    }

    #[test]
    fn blank_ncx_labels_are_put_back() {
        let entries = [
            entry("One", "OEBPS/c1.xhtml", 0),
            entry("Two  words", "OEBPS/c2.xhtml", 0),
        ];
        let n = ncx(
            "OEBPS/toc.ncx",
            &[
                ("One", "c1.xhtml", 0),
                (" ", "c1.xhtml#a", 1),
                // No label at the top level, and no href: left out.
                ("", "c1.xhtml#b", 0),
                ("\t", "", 1),
                ("Two\nwords", "c2.xhtml", 0),
                ("", "../x/c3.xhtml#c", 1),
                ("  ", "c3.xhtml", 2),
            ],
        );
        assert_eq!(
            with_ncx_empty_labels(&entries, Some(&n)),
            [
                entry("One", "OEBPS/c1.xhtml", 0),
                entry(" ", "OEBPS/c1.xhtml#a", 1),
                entry("Two  words", "OEBPS/c2.xhtml", 0),
                entry("", "x/c3.xhtml#c", 1),
                entry("  ", "OEBPS/c3.xhtml", 2),
            ]
        );
        // An NCX at the root, and a src of a fragment alone.
        let n = ncx("toc.ncx", &[("One", "c1.xhtml", 0), ("", "#a", 1)]);
        assert_eq!(
            with_ncx_empty_labels(&entries[..1], Some(&n))[1],
            entry("", ".#a", 1)
        );
        // Labels that are not Calibre's entries, or fewer of them: unchanged.
        for points in [
            &[("Uno", "c1.xhtml", 0), (" ", "c1.xhtml#a", 1)][..],
            &[("One", "c1.xhtml", 0), (" ", "c1.xhtml#a", 1)][..],
            &[
                ("One", "c1.xhtml", 0),
                ("Two words", "c2.xhtml", 0),
                ("Three", "c3.xhtml", 0),
                (" ", "c1.xhtml#a", 1),
            ][..],
        ] {
            let n = ncx("toc.ncx", points);
            assert_eq!(with_ncx_empty_labels(&entries, Some(&n)), entries);
        }
        assert_eq!(with_ncx_empty_labels(&entries, None), entries);
    }
}
