// SPDX-License-Identifier: GPL-3.0-only
//! Calibre's `DetectStructure` with the options an EPUB conversion runs it with (phase 2 plan,
//! Task 8): the `<div>` before each chapter heading, and the contents Calibre makes for a book
//! that has none. Ported from `ebooks/oeb/transforms/structure.py` (Calibre 9.18, GPL v3,
//! `NOTICE`), with `reader.py` `OEBReader._toc_from_spine` and `base.py`'s `TOC`; each rule
//! names its function. The options are the defaults of `conversion/plumber.py`: `chapter` (its
//! XPath, ported for its meaning only), `chapter_mark = 'pagebreak'`, no `level1_toc`,
//! `toc_threshold = 6`, `max_toc_links = 50`, no `toc_filter`, no duplicate links; EPUB input
//! sets `page_breaks_before` to `/`, which selects no element.
//!
//! Hostile input: every walk is a loop; a heading's text is never copied or searched per
//! heading, but read once for the whole file ([`Texts`]) and searched once.

use std::collections::HashMap;

use super::parse::{calibre_isspace, py_space};
use super::{Dom, IN_TAIL, IN_TEXT, NodeId, XHTML_NS};
use crate::epub::TocNode;
use crate::epub::package::{abshref, urlnormalize};

/// The style of the mark before a chapter (`detect_chapters`' `page_break_after`).
const MARK_STYLE: &str = "display: block; page-break-after: always";

/// The default `chapter` expression's words that must be followed by white space:
/// `(chapter|book|section|part)\s+`.
const SPACE_WORDS: [&str; 4] = ["chapter", "book", "section", "part"];
/// Its words that may also end the text: `(prolog|prologue|epilogue)(\s+|$)`.
const END_WORDS: [&str; 3] = ["prolog", "prologue", "epilogue"];

/// `toc_threshold`: with fewer entries than this, links are added.
const TOC_THRESHOLD: usize = 6;
/// `max_toc_links`.
const MAX_TOC_LINKS: usize = 50;

/// `structure.py` `DetectStructure.detect_chapters` with `chapter_mark = 'pagebreak'`: before
/// each chapter `dom` holds ([`Texts::chapters`]), a `<div style="display: block;
/// page-break-after: always">`, except before the first two when nothing comes before them in
/// `<body>` (`at_start`). A chapter that is the root gets none (lxml refuses it).
pub fn mark_chapters(dom: &mut Dom) {
    let found = Texts::new(dom).chapters(dom);
    let at: Vec<(NodeId, &str)> = found
        .iter()
        .enumerate()
        // `c[item] < 3 and at_start(elem)`: c counts this one.
        .filter(|&(i, &e)| !(i < 2 && at_start(dom, e)))
        .map(|(_, &e)| (e, "div"))
        .collect();
    for div in dom.insert_many_before(&at) {
        if dom.parent(div).is_none() {
            continue;
        }
        // `elem.makeelement(XHTML('div'), style=...)`: XHTML whatever the chapter's namespace.
        dom.set_namespace(div, Some(XHTML_NS));
        dom.set_attr(div, "style", Some(MARK_STYLE));
    }
}

/// The contents Calibre makes for a book with no contents file (`DetectStructure.__call__`
/// when `auto_generated_toc`), from `files`, the book's spine in order: (href from the book's
/// root, as Calibre's manifest writes it; whether the spine marks it linear; its tree).
///
/// One entry for each chapter, in spine and document order (`create_toc_from_chapters`),
/// each chapter given the id `calibre_toc_N` unless it has one (`elem_to_link`); with fewer
/// than six, the book's links follow (`create_toc_from_links`); with fewer than two in all and
/// more than two in the contents Calibre read from the spine (`OEBReader._toc_from_spine`),
/// those. An entry with no title is called `Unnamed`. Run [`mark_chapters`] on each file
/// first or after: the marks do not change what this finds.
pub fn generated_toc(files: &mut [(&str, bool, &mut Dom)]) -> Vec<TocNode> {
    let orig = spine_toc(files);
    let texts: Vec<Texts> = files.iter().map(|(_, _, dom)| Texts::new(dom)).collect();
    let mut toc: Vec<TocNode> = Vec::new();
    // `create_toc_from_chapters`: `counter = toc.next_play_order()`, 2 for a new TOC (its root
    // has play order 1).
    let mut counter: u64 = 2;
    for ((href, _, dom), texts) in files.iter_mut().zip(&texts) {
        for e in texts.chapters(dom) {
            let (title, href) = elem_to_link(dom, texts, href, e, counter);
            toc.push(entry(title, &href));
            counter += 1;
        }
    }
    if toc.len() < TOC_THRESHOLD {
        links(files, &texts, &mut toc);
    }
    if toc.len() < 2 && orig.len() > 2 {
        toc = orig;
    }
    // `DetectStructure.__call__`: `node.title = _('Unnamed')`.
    for node in &mut toc {
        if node.title.chars().all(py_space) {
            "Unnamed".clone_into(&mut node.title);
        }
    }
    toc
}

/// `TOC.add(title, href)`: the href is stored `urlnormalize`d.
fn entry(title: String, href: &str) -> TocNode {
    TocNode {
        title,
        href: if href.is_empty() {
            String::new()
        } else {
            urlnormalize(href)
        },
        children: Vec::new(),
    }
}

/// `DetectStructure.elem_to_link` with no title attribute: the chapter's text, else its
/// `title`, else its `alt`, as one line of at most 1,000 characters; its id, set to
/// `calibre_toc_<counter>` when it has none. (title, href).
fn elem_to_link(
    dom: &mut Dom,
    texts: &Texts,
    href: &str,
    e: NodeId,
    counter: u64,
) -> (String, String) {
    let mut title = one_line(texts.of(e), 1_000);
    if title.is_empty() {
        // `elem.get('title', '')`, then `alt`, each tested before it is stripped.
        let attr = [dom.attr(e, "title"), dom.attr(e, "alt")]
            .into_iter()
            .flatten()
            .find(|v| !v.is_empty())
            .unwrap_or("");
        title = one_line(attr, 1_000);
    }
    let id = match dom.attr(e, "id") {
        Some(id) => id.to_owned(),
        None => {
            let id = format!("calibre_toc_{counter}");
            dom.set_attr(e, "id", Some(&id));
            id
        }
    };
    (title, format!("{href}#{id}"))
}

/// `re.sub(r'\s+', ' ', text.strip())[:limit].strip()`, reading no further than it needs.
fn one_line(text: &str, limit: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    let mut space = false;
    for c in text.chars() {
        if py_space(c) {
            space = n > 0;
            continue;
        }
        if space {
            if n == limit {
                break;
            }
            out.push(' ');
            n += 1;
            space = false;
        }
        if n == limit {
            break;
        }
        out.push(c);
        n += 1;
    }
    out.truncate(out.trim_end_matches(' ').len());
    out
}

/// `DetectStructure.create_toc_from_links`: each `<a href>` of the spine, in order, that names
/// a place in the book not yet listed, by a text not yet listed (case aside), titled by its
/// first 100 characters; at most [`MAX_TOC_LINKS`].
fn links(files: &[(&str, bool, &mut Dom)], texts: &[Texts], toc: &mut Vec<TocNode>) {
    let mut num = 0;
    for ((item, _, dom), texts) in files.iter().zip(texts) {
        for a in dom.descendants() {
            if dom.tag(a) != "a" || dom.namespace(a) != Some(XHTML_NS) {
                continue;
            }
            let Some(raw) = dom.attr(a, "href") else {
                continue;
            };
            // `urlparse(href)`; a ValueError skips the link.
            let Some(url) = UrlParts::parse(raw) else {
                continue;
            };
            if !(url.scheme.is_empty() || url.scheme == "file") {
                continue;
            }
            let mut href = abshref(item, &url.path);
            if !url.fragment.is_empty() {
                href.push('#');
                href.push_str(&url.fragment);
            }
            // `toc.has_href(href)`: the stored hrefs are normalized, this one is not.
            if toc.iter().any(|n| n.href == href) {
                continue;
            }
            let text = python_strip(first_chars(texts.of(a), 100));
            // `toc.has_text(text)`.
            let lower = text.to_lowercase();
            if toc
                .iter()
                .any(|n| !n.title.is_empty() && n.title.to_lowercase() == lower)
            {
                continue;
            }
            // `TOC.__init__`'s `urlnormalize` raises for an href `urlparse` refuses.
            if UrlParts::parse(&href).is_none() {
                continue;
            }
            toc.push(entry(text.to_owned(), &href));
            num += 1;
            if num >= MAX_TOC_LINKS {
                return;
            }
        }
    }
}

/// The first `n` characters of `s`.
fn first_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// Python's `str.strip()`.
fn python_strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// `reader.py` `OEBReader._toc_from_spine`, the contents Calibre reads for a book with no
/// contents file before `DetectStructure` replaces them: each linear file's `<title>`, or, when
/// two titles are the same, its first heading's text. Titles and spine are zipped as Calibre
/// zips them: a non-linear file has no title but still takes its place in the spine.
fn spine_toc(files: &[(&str, bool, &mut Dom)]) -> Vec<TocNode> {
    let mut titles: Vec<String> = Vec::new();
    let mut headers: Vec<String> = Vec::new();
    for (_, linear, dom) in files {
        if !linear {
            continue;
        }
        let title = collapse(&title_text(dom));
        if !title.is_empty() {
            titles.push(title);
        }
        let header = ["h1", "h2", "h3", "h4", "h5", "strong"]
            .iter()
            .map(|tag| collapse(&first_of_kind_text(dom, tag)))
            .find(|h| !h.is_empty());
        headers.push(header.unwrap_or_else(|| "(unlabeled)".to_owned()));
    }
    let distinct: std::collections::BTreeSet<&String> = titles.iter().collect();
    let used = if titles.len() > distinct.len() {
        headers
    } else {
        titles
    };
    used.into_iter()
        .zip(files.iter())
        .filter(|(_, (_, linear, _))| *linear)
        .map(|(title, (href, ..))| entry(title, href))
        .collect()
}

/// `COLLAPSE_RE.sub(' ', text.strip())`: runs of ASCII space, tab, CR, LF and VT as one space.
fn collapse(text: &str) -> String {
    let mut out = String::new();
    let mut run = false;
    for c in python_strip(text).chars() {
        if matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0b') {
            run = true;
            continue;
        }
        if run {
            out.push(' ');
            run = false;
        }
        out.push(c);
    }
    out
}

fn is_xhtml(dom: &Dom, n: NodeId, tag: &str) -> bool {
    dom.tag(n) == tag && dom.namespace(n) == Some(XHTML_NS)
}

fn xhtml_child(dom: &Dom, n: NodeId, tag: &str) -> Vec<NodeId> {
    dom.children(n)
        .iter()
        .copied()
        .filter(|&c| is_xhtml(dom, c, tag))
        .collect()
}

/// `xpath(html, '/h:html/h:head/h:title/text()')` joined: the text nodes directly in each
/// `<title>`.
fn title_text(dom: &Dom) -> String {
    let root = dom.root();
    let mut out = String::new();
    if !is_xhtml(dom, root, "html") {
        return out;
    }
    for head in xhtml_child(dom, root, "head") {
        for title in xhtml_child(dom, head, "title") {
            out.push_str(dom.text(title));
            for &c in dom.children(title) {
                out.push_str(dom.tail(c));
            }
        }
    }
    out
}

/// `xpath(html, '/h:html/h:body//h:<tag>[position()=1]/text()')` joined: the direct text of
/// each `<tag>` under `<body>` that is the first of its name among its siblings, in document
/// order.
fn first_of_kind_text(dom: &Dom, tag: &str) -> String {
    let root = dom.root();
    let mut out = String::new();
    if !is_xhtml(dom, root, "html") {
        return out;
    }
    for body in xhtml_child(dom, root, "body") {
        let under = || {
            let mut stack = vec![body];
            std::iter::from_fn(move || {
                let n = stack.pop()?;
                stack.extend(dom.children(n).iter().rev());
                Some(n)
            })
        };
        let mut first = vec![false; dom.node_count()];
        for p in under() {
            if let Some(&c) = dom.children(p).iter().find(|&&c| is_xhtml(dom, c, tag)) {
                first[c.index()] = true;
            }
        }
        for n in under().filter(|&n| n != body && first[n.index()]) {
            out.push_str(dom.text(n));
            for &c in dom.children(n) {
                out.push_str(dom.tail(c));
            }
        }
    }
    out
}

/// `structure.py` `at_start`: nothing but blank text comes before `elem` in the outermost
/// `<body>` around it (none: true), no picture (`img`, `svg`), and no comment with text.
fn at_start(dom: &Dom, elem: NodeId) -> bool {
    // `ancestor-or-self::h:body`, the first in document order.
    let mut body = None;
    let mut ancestors = Vec::new();
    let mut at = Some(elem);
    while let Some(n) = at {
        if is_xhtml(dom, n, "body") {
            body = Some(n);
        }
        if n != elem {
            ancestors.push(n);
        }
        at = dom.parent(n);
    }
    let Some(body) = body else {
        return true;
    };
    let notes = |n: NodeId| dom.el(n).notes;
    let mut stack = vec![body];
    while let Some(x) = stack.pop() {
        if x == elem {
            return true;
        }
        // `x.tag.rpartition('}')[-1] in {'img', 'svg'}`: any namespace.
        if matches!(dom.tag(x), "img" | "svg") {
            return false;
        }
        let text_blank = calibre_isspace(dom.text(x)) && notes(x) & IN_TEXT == 0;
        let tail_blank = calibre_isspace(dom.tail(x)) && notes(x) & IN_TAIL == 0;
        if !(text_blank && (ancestors.contains(&x) || tail_blank)) {
            return false;
        }
        stack.extend(dom.children(x).iter().rev());
    }
    false
}

/// A file's text in document order, read once: every element's text and tail, and where each
/// element's XPath string value (its text and all under it, not its tail) lies in it.
struct Texts {
    text: String,
    /// Byte range of each element's string value, by [`NodeId`] index; (0, 0) for one not in
    /// the tree.
    range: Vec<(usize, usize)>,
}

impl Texts {
    fn new(dom: &Dom) -> Texts {
        let mut text = String::new();
        let mut range = vec![(0, 0); dom.node_count()];
        let root = dom.root();
        let mut stack = vec![(root, true)];
        while let Some((n, entering)) = stack.pop() {
            if entering {
                range[n.index()].0 = text.len();
                text.push_str(dom.text(n));
                stack.push((n, false));
                stack.extend(dom.children(n).iter().rev().map(|&c| (c, true)));
            } else {
                range[n.index()].1 = text.len();
                if n != root {
                    text.push_str(dom.tail(n));
                }
            }
        }
        Texts { text, range }
    }

    /// The string value of `n`.
    fn of(&self, n: NodeId) -> &str {
        let (s, e) = self.range[n.index()];
        &self.text[s..e]
    }

    /// The default `chapter` expression of `plumber.py`, in document order:
    /// `//*[((name()='h1' or name()='h2') and re:test(., '\s*((chapter|book|section|part)\s+)|((prolog|prologue|epilogue)(\s+|$))', 'i')) or @class = 'chapter']`.
    /// `name()` is the qualified name, so `H1` is not a heading (the tree keeps no prefixes:
    /// any namespace counts, `DIVERGENCES.md`); `re:test` is lxml's, Python's `re.search`
    /// with `re.I`; `@class` is compared whole.
    fn chapters(&self, dom: &Dom) -> Vec<NodeId> {
        let headings: Vec<NodeId> = dom
            .descendants()
            .filter(|&n| matches!(dom.tag(n), "h1" | "h2"))
            .collect();
        let named = self.headings_named(&headings);
        let mut named = named.into_iter().peekable();
        dom.descendants()
            .filter(|&n| {
                let heading = named.next_if(|&h| h == n).is_some();
                heading || dom.attr(n, "class") == Some("chapter")
            })
            .collect()
    }

    /// The `headings` (in document order) whose string value the expression's pattern finds.
    ///
    /// A heading's string value is a slice of [`Texts::text`], so the words are looked for
    /// once in the text of the outermost headings, not once per heading: a word followed by
    /// white space is found in a heading when it starts inside it and its white space ends
    /// inside it; a word of [`END_WORDS`] also when it ends where the heading's text ends.
    fn headings_named(&self, headings: &[NodeId]) -> Vec<NodeId> {
        // The outermost headings' ranges: nested ones lie inside them.
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for &h in headings {
            let (s, e) = self.range[h.index()];
            match spans.last_mut() {
                Some(last) if s < last.1 => last.1 = last.1.max(e),
                _ => spans.push((s, e)),
            }
        }
        // (start, end of the white space after it), by start.
        let mut spaced: Vec<(usize, usize)> = Vec::new();
        // End → the latest start of an END_WORDS word ending there.
        let mut ends: HashMap<usize, usize> = HashMap::new();
        for &(s, e) in &spans {
            let text = &self.text[s..e];
            for (i, c) in text.char_indices() {
                // Every word starts with one of these: most characters are passed over here.
                if !matches!(
                    c.to_ascii_lowercase(),
                    'c' | 'b' | 's' | 'p' | 'e' | '\u{17f}'
                ) {
                    continue;
                }
                for (word, end_word) in SPACE_WORDS
                    .iter()
                    .map(|w| (w, false))
                    .chain(END_WORDS.iter().map(|w| (w, true)))
                {
                    let Some(len) = fold_prefix(&text[i..], word) else {
                        continue;
                    };
                    let after = i + len;
                    if let Some(c) = text[after..].chars().next().filter(|&c| py_space(c)) {
                        spaced.push((s + i, s + after + c.len_utf8()));
                    }
                    if end_word {
                        ends.insert(s + after, s + i);
                    }
                }
            }
        }
        // Each heading, latest start first: the soonest a word starting in it is done.
        let mut found = vec![false; headings.len()];
        let mut order: Vec<usize> = (0..headings.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.range[headings[i].index()].0));
        let mut next = spaced.len();
        let mut soonest = usize::MAX;
        for i in order {
            let (s, e) = self.range[headings[i].index()];
            while next > 0 && spaced[next - 1].0 >= s {
                next -= 1;
                soonest = soonest.min(spaced[next].1);
            }
            found[i] = soonest <= e || ends.get(&e).is_some_and(|&start| start >= s);
        }
        headings
            .iter()
            .zip(found)
            .filter(|(_, f)| *f)
            .map(|(&h, _)| h)
            .collect()
    }
}

/// The length in bytes of the start of `text` that matches `word` (ASCII lower case) as
/// Python's `re.I` matches it: ASCII case aside, `ı` and `İ` for `i`, `ſ` for `s` and the
/// Kelvin sign for `k` (probed in calibre 9.18's Python).
fn fold_prefix(text: &str, word: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for w in word.chars() {
        let (_, c) = chars.next()?;
        let same = c.to_ascii_lowercase() == w
            || match w {
                'i' => matches!(c, '\u{130}' | '\u{131}'),
                's' => c == '\u{17f}',
                'k' => c == '\u{212a}',
                _ => false,
            };
        if !same {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(i, _)| i))
}

/// The parts of `urllib.parse.urlparse(href)` `create_toc_from_links` reads; `None` where it
/// raises ValueError (calibre 9.18's Python 3.14, probed).
struct UrlParts {
    scheme: String,
    path: String,
    fragment: String,
}

impl UrlParts {
    fn parse(href: &str) -> Option<UrlParts> {
        // `urlsplit`: tab, CR and LF go wherever they are, then leading C0 controls and
        // spaces.
        let cleaned: String = href
            .chars()
            .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
            .collect();
        let mut url = cleaned.trim_start_matches(|c: char| c <= ' ');
        let mut scheme = String::new();
        if let Some(i) = url.find(':')
            && i > 0
            && url.as_bytes()[0].is_ascii_alphabetic()
            && url[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            scheme = url[..i].to_ascii_lowercase();
            url = &url[i + 1..];
        }
        if let Some(rest) = url.strip_prefix("//") {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let netloc = &rest[..end];
            // "Invalid IPv6 URL". What is in the brackets goes unchecked (DIVERGENCES.md,
            // "Links to a bracketed host in generated contents").
            if netloc.contains('[') != netloc.contains(']') {
                return None;
            }
            url = &rest[end..];
        }
        let (url, fragment) = url.split_once('#').unwrap_or((url, ""));
        let (url, _query) = url.split_once('?').unwrap_or((url, ""));
        // `urlparse`'s `_splitparams`, for the schemes that take them ('' and 'file' here).
        let path = if matches!(scheme.as_str(), "" | "file") {
            let from = url.rfind('/').unwrap_or(0);
            match url[from..].find(';') {
                Some(j) => &url[..from + j],
                None => url,
            }
        } else {
            url
        };
        Some(UrlParts {
            scheme,
            path: path.to_owned(),
            fragment: fragment.to_owned(),
        })
    }
}
