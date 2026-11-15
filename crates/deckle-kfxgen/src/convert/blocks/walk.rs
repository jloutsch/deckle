// SPDX-License-Identifier: GPL-3.0-only
//! The tree walk of `extract_blocks_from_html`: its closures `_walk`, `_walk_flow`,
//! `_walk_element`, `_walk_container` (with `_flush_inline`), `_holds_blocks`,
//! `_is_block_at_body`, `_discard_listing`, `_emit_image_block`, `_inline_picture_parts`, and
//! the module helpers they call (`_is_nav_listing`, `_is_front_matter`, `_page_breaks`,
//! `_clear_side`, `_pictures_among_words`, `_lone_block_picture`, `_is_spacer`,
//! `_anchors_follow_rows`, `_dedupe_keep_order`), at kfxgen `ad38ec8`.
//!
//! kfxgen recurses once per nested element (`_walk` → `_walk_flow` → `_walk_element` →
//! `_walk_container` → `_walk`, and `_discard_listing`); here each open call is a [`Frame`] on
//! a stack, and an element more than `MAX_DEPTH` below the `<body>` stops the file with
//! [`Error::TooLarge`]. `read_book` refuses any element deeper than `MAX_DEPTH` below the root,
//! so no tree it gives reaches that.
//!
//! Branches later tasks port stop the file with [`Error::Unsupported`] naming the task.

use std::collections::{HashMap, HashSet};

use deckle_limits::MAX_DEPTH;

use super::{Block, BlockId, ExtractOptions, Field, unsupported};
use crate::convert::block_bidi::{BlockBidi, dict_set, start_end_align};
use crate::convert::block_style::compute_block_style;
use crate::convert::initial::initial;
use crate::convert::inline::{
    Walk, epub_switch_branch, epub_types, is_body_block, is_non_rendered, keyword, own_anchor_ids,
    subtree_anchor_ids, text_of,
};
use crate::convert::inline_style::{
    FLAG_PRE, Flag, Flags, Segment, css_rgb, is_transparent, make_anchor_mark,
    normalize_runs_with_anchors, parse_css_length,
};
use crate::convert::pictures::{css_background_image, displays_as_block, float_side};
use crate::convert::resolver::{Resolved, Resolver};
use crate::convert::{Error, ItemError};
use crate::dom::{Dom, NodeId, XHTML_NS};
use crate::generate::images::{IMG_TOKEN, WHOLE_TOKEN};
use crate::generate::page_scale::em_percent;
use crate::generate::py;
use crate::pyvalue::PyValue;

const MATHML_NS: &str = "http://www.w3.org/1998/Math/MathML";
/// `_SOURCE_MARKED`, `_SOURCE_TOC`, `_SOURCE_FRONT`: kfxgen's marks of the book's own classes.
const SOURCE_MARKED: &str = "data-kfxgen-source";
const SOURCE_TOC: &str = "data-kfxgen-toc";
const SOURCE_FRONT: &str = "data-kfxgen-front";
const HORIZONTAL: &str = "data-kfxgen-horizontal";
const VERTICAL: &str = "data-kfxgen-vertical";

/// `block_tags`' local names (it holds each bare and in the XHTML namespace).
fn is_block_name(local: &str) -> bool {
    matches!(
        local,
        "p" | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "pre"
            | "li"
            | "ol"
            | "ul"
            | "dl"
            | "dt"
            | "dd"
            | "section"
            | "article"
            | "figure"
            | "table"
            | "caption"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "hr"
    ) || (is_body_block(local) && !matches!(local, "img" | "svg"))
}

/// `elem.tag in block_tags`: the qualified tag, so an element of another namespace is not one.
fn is_block_tag(dom: &Dom, n: NodeId) -> bool {
    !dom.is_comment(n)
        && matches!(dom.namespace(n), None | Some(XHTML_NS))
        && is_block_name(dom.tag(n))
}

/// `_HEADING_TAGS`.
fn heading_level(local: &str) -> Option<i64> {
    Some(match local {
        "h1" => 1,
        "h2" => 2,
        "h3" => 3,
        "h4" => 4,
        "h5" => 5,
        "h6" => 6,
        _ => return None,
    })
}

/// `_FRONT_MATTER`.
fn is_front_matter_token(t: &str) -> bool {
    matches!(
        t,
        "dedication" | "epigraph" | "copyright" | "copyright-page" | "colophon"
    )
}

/// `_BOX_TAGS` and `_SIZED_BOX_TAGS`.
fn is_box_tag(local: &str) -> bool {
    matches!(
        local,
        "div" | "aside" | "section" | "blockquote" | "article" | "p" | "pre"
    )
}

/// Whether `s` holds anything but Python's white space (`s.strip()` is not empty).
fn has_words(s: &str) -> bool {
    !py::strip(s).is_empty()
}

fn s(v: &str) -> PyValue {
    PyValue::Str(v.to_owned())
}

/// `_dedupe_keep_order(items)`.
fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for it in items {
        if seen.insert(it.clone()) {
            out.push(it);
        }
    }
    out
}

/// What the walk leaves for the steps after it (spacing, trailing ids, anchor keys).
pub(super) struct Walked {
    pub(super) blocks: Vec<Block>,
    pub(super) pending_ids: Vec<String>,
    /// `rule_ids`: an `<hr>`'s own ids (Task 14 fills it).
    pub(super) rule_ids: HashSet<String>,
    pub(super) listing_ids: HashSet<String>,
    pub(super) nav_listing_at: Vec<(usize, Vec<String>)>,
    pub(super) body: NodeId,
}

/// `_walk_container(elem)` part-way through its children.
struct Container {
    elem: NodeId,
    /// `is_block=_is_block_at_body`: the `<body>` holding loose text.
    at_body: bool,
    /// `ws`: the white-space flags its own text and tails take.
    ws: Flags,
    /// `among_words`: the pictures that run with its loose words.
    among: HashSet<NodeId>,
    /// `inline_parts`.
    parts: Vec<Segment>,
    /// Its next child, as an index into its children.
    next: usize,
    /// The child just walked as a block, whose tail comes next.
    walked: Option<NodeId>,
    depth: usize,
}

/// One call of kfxgen's walk still open: what it does once the call it made returns.
enum Frame {
    /// `_walk(elem)` after `_walk_flow(elem)`: front matter, a page break after it, and the
    /// break and clear noted, placed.
    WalkEnd {
        elem: NodeId,
        start: usize,
        after: bool,
    },
    /// `_walk_element`'s container branch after `_walk_container(elem)`.
    ContainerEnd {
        elem: NodeId,
        start: usize,
    },
    Container(Container),
    /// `_discard_listing(elem)` part-way through its children.
    Discard {
        elem: NodeId,
        next: usize,
        depth: usize,
    },
    /// `_walk_element`'s listing branch after `_discard_listing`.
    ListingEnd,
}

/// Memo slots per node.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bit {
    Unknown,
    Yes,
    No,
}

struct Walker<'o, 'w, 'a, 's> {
    w: &'w mut Walk<'a, 's>,
    dom: &'a Dom,
    resolver: &'a Resolver<'a>,
    opts: &'o ExtractOptions<'o>,
    blocks: Vec<Block>,
    next_id: usize,
    pending_ids: Vec<String>,
    listing_ids: HashSet<String>,
    /// How many of `pending_ids`, from the front, are in `listing_ids` already: kfxgen adds
    /// all of them after each listing, which is quadratic over many listings in a row.
    listed: usize,
    nav_listing_at: Vec<(usize, Vec<String>)>,
    /// `break_at` and `clear_at`: indexes into `blocks`, kept as Python's plain integers (a
    /// later splice of `blocks` leaves them as they are, as kfxgen's does). Each holds at
    /// most one entry in kfxgen (it appends only to an empty list).
    break_at: Option<usize>,
    clear_at: Option<(usize, &'static str)>,
    bidi: BlockBidi,
    /// Whether a node's subtree, itself included, holds an element of `block_tags`.
    holds_block_tag: Vec<Bit>,
    stack: Vec<Frame>,
}

/// The walk of one file: everything `extract_blocks_from_html` does before `_space_between`.
pub(super) fn walk(w: &mut Walk, opts: &ExtractOptions) -> Result<Walked, ItemError> {
    let file = w.file();
    let dom = &file.dom;
    let resolver = w.resolver();
    let body = find_body(dom);
    let mut walker = Walker {
        w,
        dom,
        resolver,
        opts,
        blocks: Vec::new(),
        next_id: 0,
        pending_ids: Vec::new(),
        listing_ids: HashSet::new(),
        listed: 0,
        nav_listing_at: Vec::new(),
        break_at: None,
        clear_at: None,
        bidi: BlockBidi::new(dom),
        holds_block_tag: vec![Bit::Unknown; dom.node_count()],
        stack: Vec::new(),
    };
    walker.walk_body(body)?;
    // Each listing's index counts the space blocks before it, which `_space_between` takes
    // out.
    if !walker.nav_listing_at.is_empty() {
        let spaces: Vec<usize> = walker
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind() == Some("space"))
            .map(|(i, _)| i)
            .collect();
        for (at, _) in &mut walker.nav_listing_at {
            *at -= spaces.partition_point(|&i| i < *at);
        }
    }
    Ok(Walked {
        blocks: walker.blocks,
        pending_ids: walker.pending_ids,
        rule_ids: HashSet::new(),
        listing_ids: walker.listing_ids,
        nav_listing_at: walker.nav_listing_at,
        body,
    })
}

/// `element.find(".//{xhtml}body")`, then `.//body`, else the root: the first in document
/// order below the root.
fn find_body(dom: &Dom) -> NodeId {
    let root = dom.root();
    let below = || dom.descendants().filter(move |&n| n != root);
    below()
        .find(|&n| dom.qualified(n) == (Some(XHTML_NS), "body"))
        .or_else(|| below().find(|&n| dom.qualified(n) == (None, "body")))
        .unwrap_or(root)
}

impl<'o, 'w, 'a, 's> Walker<'o, 'w, 'a, 's> {
    fn too_deep(&self) -> ItemError {
        ItemError::Error(Error::TooLarge {
            what: format!(
                "{}: an element nested more than {MAX_DEPTH} levels below the body",
                self.w.file().href
            ),
        })
    }

    fn unsupported(&self, feature: &'static str) -> ItemError {
        unsupported(self.w, feature)
    }

    fn new_block(&mut self, fields: Vec<(&'static str, Field)>) -> Block {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        Block { id, fields }
    }

    /// `ids = pending_ids[:]; pending_ids.clear()`.
    fn take_pending(&mut self) -> Vec<String> {
        self.listed = 0;
        std::mem::take(&mut self.pending_ids)
    }

    fn resolve(&self, n: NodeId) -> Result<Option<Resolved<'a>>, ItemError> {
        Ok(self.resolver.resolve(n)?)
    }

    /// The body: an SVG document's pictures, the body's loose text walked as a `<div>`'s, or
    /// each child walked.
    fn walk_body(&mut self, body: NodeId) -> Result<(), ItemError> {
        let dom = self.dom;
        if dom.tag(body) == "svg" {
            // `_emit_svg_blocks(body)`: the page is an SVG file.
            return Err(self.unsupported("SVG blocks (Task 17)"));
        }
        let loose =
            has_words(dom.text(body)) || dom.children(body).iter().any(|&c| has_words(dom.tail(c)));
        if loose {
            let c = self.container(body, true, 0)?;
            self.stack.push(Frame::Container(c));
            return self.run();
        }
        for &child in dom.children(body) {
            self.enter(child, 1)?;
            self.run()?;
        }
        Ok(())
    }

    /// Resumes the open calls until none is left.
    fn run(&mut self) -> Result<(), ItemError> {
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::WalkEnd { elem, start, after } => self.walk_end(elem, start, after)?,
                Frame::ContainerEnd { elem, start } => self.container_end(elem, start)?,
                Frame::Container(c) => self.container_step(c)?,
                Frame::Discard { elem, next, depth } => {
                    if let Some(&child) = self.dom.children(elem).get(next) {
                        self.stack.push(Frame::Discard {
                            elem,
                            next: next + 1,
                            depth,
                        });
                        self.discard(child, depth + 1)?;
                    }
                }
                Frame::ListingEnd => {
                    // `listing_ids.update(pending_ids)`: those not added after an earlier
                    // listing.
                    let from = self.listed.min(self.pending_ids.len());
                    self.listing_ids
                        .extend(self.pending_ids[from..].iter().cloned());
                    self.listed = self.pending_ids.len();
                }
            }
        }
        Ok(())
    }

    /// `_walk(elem)` up to its `_walk_flow`; the rest is its [`Frame::WalkEnd`].
    fn enter(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep());
        }
        let css = if self.dom.is_comment(elem) {
            None
        } else {
            self.resolve(elem)?
        };
        let (before, after) = page_breaks(css.as_ref())?;
        if before && self.break_at.is_none() {
            self.break_at = Some(self.blocks.len());
        }
        // A <br> is inline and never walked here.
        if let Some(side) = clear_side(css.as_ref())?
            && self.clear_at.is_none()
        {
            self.clear_at = Some((self.blocks.len(), side));
        }
        drop(css);
        self.stack.push(Frame::WalkEnd {
            elem,
            start: self.blocks.len(),
            after,
        });
        self.walk_flow(elem, depth)
    }

    fn walk_end(&mut self, elem: NodeId, start: usize, after: bool) -> Result<(), ItemError> {
        if !self.dom.is_comment(elem) && is_front_matter(self.dom, elem) {
            // Kept if a contents rebuild replaces its chapter (#393).
            for b in self.blocks.iter_mut().skip(start) {
                b.set("front_matter", PyValue::Bool(true));
            }
        }
        if after && self.break_at.is_none() {
            self.break_at = Some(self.blocks.len());
        }
        if let Some(at) = self.break_at
            && let Some(b) = self
                .blocks
                .iter_mut()
                .skip(at)
                .find(|b| b.kind() != Some("space"))
        {
            b.set("break_before", PyValue::Bool(true));
            self.break_at = None;
        }
        if let Some((at, side)) = self.clear_at
            && let Some(b) = self
                .blocks
                .iter_mut()
                .skip(at)
                .find(|b| b.kind() != Some("space"))
        {
            if !b.contains("clear") {
                b.set("clear", s(side));
            }
            self.clear_at = None;
        }
        Ok(())
    }

    /// `_walk_flow(elem)`: a list item's marker path is Task 13's.
    fn walk_flow(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
        if !self.dom.is_comment(elem) && self.dom.tag(elem) == "li" && !self.w.hidden(elem)? {
            return Err(self.unsupported("lists (Task 13)"));
        }
        // Tables counted in `tables_seen`/`notes_seen` are written as rows (Tasks 16, 17):
        // every `<table>` stops at `_walk_element` until then.
        self.walk_element(elem, depth)
    }

    /// `_walk_element(elem)`, up to its container branch's `_walk_container`.
    fn walk_element(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
        let dom = self.dom;
        if dom.is_comment(elem) {
            // Markup between blocks, not a paragraph (#252).
            return Ok(());
        }
        if self.w.hidden(elem)? {
            // Its ids name the next block shown (#232).
            self.pending_ids.extend(subtree_anchor_ids(dom, elem));
            return Ok(());
        }
        if let Some((ids, default)) = epub_switch_branch(dom, elem) {
            self.pending_ids.extend(ids);
            if let Some(default) = default {
                self.enter(default, depth + 1)?;
            }
            return Ok(());
        }
        if !self.opts.keep_nav_listings && is_nav_listing(dom, elem) {
            let mut ids = subtree_anchor_ids(dom, elem);
            ids.sort();
            ids.dedup();
            self.nav_listing_at.push((self.blocks.len(), ids));
            self.stack.push(Frame::ListingEnd);
            return self.discard(elem, depth);
        }
        let css = self.resolve(elem)?;
        let background = css_background_image(dom, elem, css.as_ref(), &self.w.file().href)?;
        if background.is_some_and(|b| !b.is_empty()) {
            return Err(self.unsupported("background pictures (Task 17)"));
        }
        drop(css);
        let local = dom.tag(elem);
        if local == "hr" {
            return Err(self.unsupported("rules (Task 14)"));
        }
        if local == "table" {
            return Err(self.unsupported(if self.opts.native_tables {
                "tables (Task 16)"
            } else {
                "tables written as rows (Task 17)"
            }));
        }
        // An equation walked as a block is a paragraph of its own (#235). A cell walked as a
        // paragraph (`leaf_cells`) is Task 16's.
        let is_block = is_block_tag(dom, elem) || local == "math";
        let mut has_block_child = false;
        for &child in dom.children(elem) {
            if self.holds_blocks(child) {
                has_block_child = true;
                break;
            }
        }
        if self.opts.native_tables
            && local == "tr"
            && !has_block_child
            && dom
                .subtree(elem)
                .skip(1)
                .any(|d| !dom.is_comment(d) && dom.tag(d) == "table")
        {
            return Err(self.unsupported("table rows holding tables (Task 17)"));
        }
        if is_block && !has_block_child {
            return self.leaf(elem);
        }
        if local == "img" {
            return self.image_block(elem);
        }
        if local == "svg" {
            return Err(self.unsupported("SVG blocks (Task 17)"));
        }
        self.pending_ids
            .extend(own_anchor_ids(dom, elem).map(str::to_owned));
        let start = self.blocks.len();
        self.stack.push(Frame::ContainerEnd { elem, start });
        let c = self.container(elem, false, depth)?;
        self.stack.push(Frame::Container(c));
        Ok(())
    }

    /// The leaf-block branch of `_walk_element`: `elem` holds no block, so its inline content
    /// is one paragraph.
    fn leaf(&mut self, elem: NodeId) -> Result<(), ItemError> {
        let dom = self.dom;
        let segments = self.w.inline(elem)?;
        let (text, spans, marks) = normalize_runs_with_anchors(&segments);
        drop(segments);
        // `ids = pending_ids[:] + _subtree_anchor_ids(elem)`, the pending ids moved only when
        // a block takes them (copying them for every empty block is quadratic).
        let own = subtree_anchor_ids(dom, elem);
        // Only non-breaking spaces is a spacer, not a paragraph (#230, #244).
        if !has_words(&text) {
            self.pending_ids.extend(own);
            if is_spacer(dom, elem) {
                return Err(self.unsupported("spacers (Task 12)"));
            }
            return Ok(());
        }
        let mut ids = self.take_pending();
        ids.extend(own);
        // `_take_marker`: no list marker is pending until Task 13 ports lists.
        let local = dom.tag(elem);
        let css = self.resolve(elem)?;
        let mut spans = spans;
        let mut bstyle = PyValue::None;
        if let Some(css) = &css {
            bstyle = compute_block_style(css)?;
            if local == "tr" {
                // `_row_align`.
                return Err(self.unsupported("table rows (Task 17)"));
            }
            if local == "p" {
                let (with, dropped) = initial(dom, self.resolver, elem, Some(css), &text, spans)?;
                spans = with;
                if let (Some(n), PyValue::Dict(style)) = (dropped, &mut bstyle) {
                    dict_set(
                        style,
                        "drop_cap",
                        PyValue::Int(i64::try_from(n).unwrap_or(i64::MAX)),
                    );
                }
            }
        }
        // A `<math>` display equation never gets here: the inline walk refuses it.
        let bstyle = self
            .bidi
            .with_bidi(dom, self.resolver, bstyle, elem, true)?;
        let bstyle = start_end_align(bstyle, &keyword(css.as_ref(), "text-align")?);
        let block_ids = dedupe(ids);
        let preformatted = self
            .w
            .white_space_flags(elem, css.as_ref(), &Flags::default())?
            .contains(&FLAG_PRE);
        drop(css);
        let level = heading_level(local);
        let mut fields: Vec<(&'static str, Field)> = vec![
            ("text", Field::Py(PyValue::Str(text))),
            (
                "spans",
                Field::Py(PyValue::List(spans.iter().map(|s| s.to_py()).collect())),
            ),
            ("block_style", Field::Py(bstyle)),
            ("preformatted", Field::Py(PyValue::Bool(preformatted))),
            ("anchor_ids", Field::Py(strs(&block_ids))),
            ("heading", Field::Py(PyValue::Bool(level.is_some()))),
        ];
        if let Some(level) = level {
            fields.push(("heading_level", Field::Py(PyValue::Int(level))));
        }
        fields.push(("row", Field::Py(PyValue::Bool(local == "tr"))));
        if self.lone_block_picture(elem)? {
            fields.push(("picture_block", Field::Py(PyValue::Bool(true))));
        }
        if self.pictures_apart(elem)? {
            fields.push(("pictures_apart", Field::Py(PyValue::Bool(true))));
        }
        if local == "li"
            || dom
                .ancestors(elem)
                .any(|a| !dom.is_comment(a) && dom.tag(a) == "li")
        {
            fields.push(("in_list", Field::Py(PyValue::Bool(true))));
        }
        // `_label_texts`: a block holding an equation, hidden or not (a shown one is refused
        // by the inline walk already).
        if dom.subtree(elem).any(|d| {
            !dom.is_comment(d) && matches!(dom.qualified(d), (None | Some(MATHML_NS), "math"))
        }) {
            return Err(self.unsupported("math"));
        }
        fields.push(("anchor_offsets", Field::Py(offsets(&block_ids, &marks))));
        let block = self.new_block(fields);
        self.blocks.push(block);
        let at = self.blocks.len() - 1;
        if self.would_box(elem, at, true)? {
            return Err(self.unsupported("boxes (Task 15)"));
        }
        // `_note_margins(elem, at, _note_padding(elem, at))`: Task 12.
        Ok(())
    }

    /// `_emit_image_block(elem)` for an `<img>`.
    fn image_block(&mut self, elem: NodeId) -> Result<(), ItemError> {
        let dom = self.dom;
        let mut ids = self.take_pending();
        ids.extend(own_anchor_ids(dom, elem).map(str::to_owned));
        let block_ids = dedupe(ids);
        let token = self.w.picture_token(elem, dom.parent(elem))?;
        let mut fields: Vec<(&'static str, Field)> = vec![
            ("text", Field::Py(PyValue::Str(token))),
            ("spans", Field::Py(PyValue::List(Vec::new()))),
            ("block_style", Field::Py(PyValue::None)),
            ("anchor_ids", Field::Py(strs(&block_ids))),
            ("anchor_offsets", Field::Py(offsets(&block_ids, &[]))),
        ];
        let css = self.resolve(elem)?;
        let align = match &css {
            Some(css) => block_align(css)?,
            None => PyValue::None,
        };
        if align.is_truthy() {
            // The text-align it inherits from its block (#247, S755).
            fields.push(("picture_align", Field::Py(align)));
        }
        if displays_as_block(css.as_ref())? {
            fields.push(("picture_block", Field::Py(PyValue::Bool(true))));
        }
        let block = self.new_block(fields);
        self.blocks.push(block);
        Ok(())
    }

    /// `_inline_picture_parts(img, parent)`: an `<img>` among loose words, as the inline walk
    /// writes a picture.
    fn inline_picture_parts(
        &mut self,
        img: NodeId,
        parent: NodeId,
        parts: &mut Vec<Segment>,
    ) -> Result<(), ItemError> {
        let dom = self.dom;
        if self.w.hidden(img)? {
            parts.extend(
                subtree_anchor_ids(dom, img)
                    .iter()
                    .map(|a| make_anchor_mark(a)),
            );
            return Ok(());
        }
        parts.extend(own_anchor_ids(dom, img).map(make_anchor_mark));
        let token = self.w.picture_token(img, Some(parent))?;
        parts.push(Segment {
            text: token,
            flags: Flags::default(),
        });
        Ok(())
    }

    /// `_discard_listing(elem)`: a contents listing's text left out, its pictures and ids
    /// kept. Its children are a [`Frame::Discard`].
    fn discard(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
        let dom = self.dom;
        if depth > MAX_DEPTH {
            return Err(self.too_deep());
        }
        // A comment: no attributes, not hidden, no ids, no children.
        if dom.is_comment(elem) || is_non_rendered(dom, elem) {
            return Ok(());
        }
        if self.w.hidden(elem)? {
            self.pending_ids.extend(subtree_anchor_ids(dom, elem));
            return Ok(());
        }
        match dom.tag(elem) {
            "img" => return self.image_block(elem),
            "svg" => return Err(self.unsupported("SVG blocks (Task 17)")),
            _ => {}
        }
        self.pending_ids
            .extend(own_anchor_ids(dom, elem).map(str::to_owned));
        self.stack.push(Frame::Discard {
            elem,
            next: 0,
            depth,
        });
        Ok(())
    }

    /// The start of `_walk_container(elem, is_block)`: up to its children.
    fn container(
        &mut self,
        elem: NodeId,
        at_body: bool,
        depth: usize,
    ) -> Result<Container, ItemError> {
        let dom = self.dom;
        let css = self.resolve(elem)?;
        let ws: Flags = self
            .w
            .white_space_flags(elem, css.as_ref(), &Flags::default())?
            .into_iter()
            .map(|f| Flag::Str(f.to_owned()))
            .collect();
        drop(css);
        let among = if at_body {
            HashSet::new()
        } else {
            self.pictures_among_words(elem)?
        };
        let mut parts = Vec::new();
        if !dom.text(elem).is_empty() {
            parts.push(Segment {
                text: dom.text(elem).to_owned(),
                flags: ws.clone(),
            });
        }
        if anchors_follow_rows(dom, elem) {
            // An anchor after a row moves onto that row (#221).
            return Err(self.unsupported("anchors that follow rows (Task 17)"));
        }
        Ok(Container {
            elem,
            at_body,
            ws,
            among,
            parts,
            next: 0,
            walked: None,
            depth,
        })
    }

    /// `_walk_container`'s loop from where it stopped, until the next child walked as a block
    /// (then it waits on the stack) or its end.
    fn container_step(&mut self, mut c: Container) -> Result<(), ItemError> {
        let dom = self.dom;
        if let Some(child) = c.walked.take() {
            self.tail(&mut c, child);
        }
        while let Some(&child) = dom.children(c.elem).get(c.next) {
            c.next += 1;
            if c.among.contains(&child) {
                let mut parts = std::mem::take(&mut c.parts);
                self.inline_picture_parts(child, c.elem, &mut parts)?;
                c.parts = parts;
            } else if if c.at_body {
                self.is_block_at_body(child)?
            } else {
                self.holds_blocks(child)
                    || (!dom.is_comment(child) && matches!(dom.tag(child), "img" | "svg"))
            } {
                self.flush(&mut c)?;
                let depth = c.depth + 1;
                c.walked = Some(child);
                self.stack.push(Frame::Container(c));
                return self.enter(child, depth);
            } else {
                // A hidden child gives only its ids (#256).
                let segs = self.w.inline_child(child, &c.ws)?;
                c.parts.extend(segs);
            }
            self.tail(&mut c, child);
        }
        self.flush(&mut c)
    }

    fn tail(&self, c: &mut Container, child: NodeId) {
        let tail = self.dom.tail(child);
        if !tail.is_empty() {
            c.parts.push(Segment {
                text: tail.to_owned(),
                flags: c.ws.clone(),
            });
        }
    }

    /// `_flush_inline()`: the run of inline content so far, one paragraph.
    fn flush(&mut self, c: &mut Container) -> Result<(), ItemError> {
        if c.parts.is_empty() {
            return Ok(());
        }
        let parts = std::mem::take(&mut c.parts);
        let (text, spans, marks) = normalize_runs_with_anchors(&parts);
        drop(parts);
        if !has_words(&text) {
            // An anchor with no visible text carries forward.
            self.pending_ids.extend(marks.into_iter().map(|(id, _)| id));
            return Ok(());
        }
        // `_take_marker`: no list marker is pending until Task 13.
        let mut ids = self.take_pending();
        ids.extend(marks.iter().map(|(id, _)| id.clone()));
        let block_ids = dedupe(ids);
        // The container's text-align, and only that (P757).
        let css = self.resolve(c.elem)?;
        let align = match &css {
            Some(css) => block_align(css)?,
            None => PyValue::None,
        };
        let aligned = align.is_truthy();
        let style = if aligned {
            PyValue::Dict(vec![(s("align"), align)])
        } else {
            PyValue::None
        };
        let mut style = self
            .bidi
            .with_bidi(self.dom, self.resolver, style, c.elem, false)?;
        if aligned {
            style = start_end_align(style, &keyword(css.as_ref(), "text-align")?);
        }
        let fields = vec![
            ("text", Field::Py(PyValue::Str(text))),
            (
                "spans",
                Field::Py(PyValue::List(spans.iter().map(|s| s.to_py()).collect())),
            ),
            ("block_style", Field::Py(style)),
            ("anchor_ids", Field::Py(strs(&block_ids))),
            ("anchor_offsets", Field::Py(offsets(&block_ids, &marks))),
        ];
        let block = self.new_block(fields);
        self.blocks.push(block);
        Ok(())
    }

    /// The end of `_walk_element`'s container branch.
    fn container_end(&mut self, elem: NodeId, start: usize) -> Result<(), ItemError> {
        // A cell walked as a paragraph (`leaf_cells`) is Task 16's.
        if matches!(self.dom.tag(elem), "ul" | "ol") {
            // `_native_list(elem)` and `_make_list`.
            return Err(self.unsupported("lists (Task 13)"));
        }
        if self.would_box(elem, start, false)? {
            return Err(self.unsupported("boxes (Task 15)"));
        }
        // `_carry_container_margins(elem, start)`, `_note_margins(elem, start,
        // _note_padding(elem, start))`: Task 12.
        Ok(())
    }

    /// Whether `_make_box(elem, start, leaf)` would draw a box round `blocks[start:]`: its
    /// tests up to the box, in kfxgen's order (the box itself is Task 15's).
    fn would_box(&mut self, elem: NodeId, start: usize, leaf: bool) -> Result<bool, ItemError> {
        let dom = self.dom;
        let tag = dom.tag(elem);
        let turned = dom.attr(elem, HORIZONTAL).is_some() || dom.attr(elem, VERTICAL).is_some();
        if !(is_box_tag(tag) || tag == "figure") && !turned {
            return Ok(false);
        }
        let css = self.resolve(elem)?;
        let border = css_border_drawn(css.as_ref())?;
        // `_box_background(css)`: the resolved style's background values are the element's
        // own (`st.get`), which the resolver's fill reads.
        let fill = match &css {
            Some(_) => self
                .resolver
                .box_background(elem)?
                .map_err(ItemError::Raised)?,
            None => None,
        };
        let (border, fill) = if is_box_tag(tag) {
            (border, fill)
        } else {
            (false, None)
        };
        let kids = &self.blocks[start.min(self.blocks.len())..];
        let mut width = box_width(css.as_ref())?;
        let side = float_side(css.as_ref())?;
        if width.is_some() && !kids.iter().any(|b| IMG_TOKEN.is_match(text_field(b))) {
            width = None;
        }
        if width.is_some()
            && side.is_some()
            && kids.len() == 1
            && let Some(m) = WHOLE_TOKEN.captures(text_field(&kids[0]))
            && m.get(4).is_some_and(|g| !g.as_str().is_empty())
        {
            return Ok(false);
        }
        if !border && fill.is_none() && width.is_none() && !turned {
            return Ok(false);
        }
        if !kids
            .iter()
            .any(|b| matches!(b.kind(), Some("table" | "box")) || has_words(text_field(b)))
        {
            return Ok(false);
        }
        // A leaf's style loses its side margins here; no TOC entry can name a block past a
        // leaf box's only one.
        if !leaf
            && kids
                .iter()
                .skip(1)
                .flat_map(|b| {
                    b.get("anchor_ids")
                        .and_then(PyValue::as_list)
                        .unwrap_or(&[])
                })
                .any(|a| {
                    a.as_str()
                        .is_some_and(|a| self.opts.toc_targets.contains(a))
                })
        {
            return Ok(false);
        }
        Ok(true)
    }

    /// `_holds_blocks(elem)`: one of `block_tags`, or an `<epub:switch>` whose default holds
    /// one.
    fn holds_blocks(&mut self, n: NodeId) -> bool {
        if is_block_tag(self.dom, n) {
            return true;
        }
        match epub_switch_branch(self.dom, n) {
            Some((_, Some(default))) => self.block_below(default),
            _ => false,
        }
    }

    /// `any(d.tag in block_tags for d in n.iter())`, kept per node: worked out for a whole
    /// subtree at once, children first, so every node is looked at once per file.
    fn block_below(&mut self, n: NodeId) -> bool {
        let dom = self.dom;
        if self.holds_block_tag[n.index()] == Bit::Unknown {
            // (node, children done)
            let mut stack = vec![(n, false)];
            while let Some((m, done)) = stack.pop() {
                if self.holds_block_tag[m.index()] != Bit::Unknown {
                    continue;
                }
                if done {
                    let below = is_block_tag(dom, m)
                        || dom
                            .children(m)
                            .iter()
                            .any(|c| self.holds_block_tag[c.index()] == Bit::Yes);
                    self.holds_block_tag[m.index()] = if below { Bit::Yes } else { Bit::No };
                } else {
                    stack.push((m, true));
                    stack.extend(dom.children(m).iter().map(|&c| (c, false)));
                }
            }
        }
        self.holds_block_tag[n.index()] == Bit::Yes
    }

    /// `_is_block_at_body(child)`.
    fn is_block_at_body(&mut self, child: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        if dom.is_comment(child) {
            return Ok(true);
        }
        if is_body_block(dom.tag(child)) {
            return Ok(true);
        }
        let css = self.resolve(child)?;
        let display = keyword(css.as_ref(), "display")?;
        Ok(["block", "list-item", "table", "flex", "grid"]
            .iter()
            .any(|p| display.starts_with(p))
            || is_nav_listing(dom, child)
            || self.block_below(child))
    }

    /// `_pictures_among_words(elem, _holds_blocks, set_apart)`: the `<img>` children that
    /// share a run of loose content with words, or with another picture none set apart.
    fn pictures_among_words(&mut self, elem: NodeId) -> Result<HashSet<NodeId>, ItemError> {
        let dom = self.dom;
        let mut found = HashSet::new();
        let mut run: Vec<NodeId> = Vec::new();
        let mut words = has_words(dom.text(elem));
        for &child in dom.children(elem) {
            if dom.is_comment(child) {
                words = words || has_words(dom.tail(child));
                continue;
            }
            if self.holds_blocks(child) || dom.tag(child) == "svg" {
                if words || self.side_by_side(&run)? {
                    found.extend(run.iter().copied());
                }
                run.clear();
                words = false;
            } else if dom.tag(child) == "img" {
                run.push(child);
            } else if has_text(dom, child) {
                words = true;
            }
            if has_words(dom.tail(child)) {
                words = true;
            }
        }
        if words || self.side_by_side(&run)? {
            found.extend(run);
        }
        Ok(found)
    }

    /// `_side_by_side(pictures, set_apart)`: two or more, none set to display as a block.
    fn side_by_side(&self, pictures: &[NodeId]) -> Result<bool, ItemError> {
        if pictures.len() < 2 {
            return Ok(false);
        }
        for &p in pictures {
            if displays_as_block(self.resolve(p)?.as_ref())? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `_lone_block_picture(elem, style_resolver)`.
    fn lone_block_picture(&self, elem: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        if has_words(dom.text(elem)) {
            return Ok(false);
        }
        let mut kids = dom.children(elem).iter().filter(|&&c| !dom.is_comment(c));
        let (Some(&kid), None) = (kids.next(), kids.next()) else {
            return Ok(false);
        };
        Ok(dom.tag(kid) == "img"
            && !has_words(dom.tail(kid))
            && displays_as_block(self.resolve(kid)?.as_ref())?)
    }

    /// The `pictures_apart` test: a picture in the block set to display as a block.
    fn pictures_apart(&self, elem: NodeId) -> Result<bool, ItemError> {
        let dom = self.dom;
        for d in dom.subtree(elem).skip(1) {
            if !dom.is_comment(d)
                && dom.tag(d) == "img"
                && displays_as_block(self.resolve(d)?.as_ref())?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// A block's `text`, "" for none.
fn text_field(b: &Block) -> &str {
    b.get("text").and_then(PyValue::as_str).unwrap_or("")
}

/// A list of strings as the model holds it.
fn strs(items: &[String]) -> PyValue {
    PyValue::List(items.iter().map(|i| s(i)).collect())
}

/// `{aid: mark_offsets.get(aid, 0) for aid in ids}`.
fn offsets(ids: &[String], marks: &[(String, usize)]) -> PyValue {
    let at: HashMap<&str, usize> = marks.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    PyValue::Dict(
        ids.iter()
            .map(|id| {
                let n = at.get(id.as_str()).copied().unwrap_or(0);
                (s(id), PyValue::Int(i64::try_from(n).unwrap_or(i64::MAX)))
            })
            .collect(),
    )
}

/// `compute_block_style(css)["align"]`.
fn block_align(css: &Resolved) -> Result<PyValue, ItemError> {
    let style = compute_block_style(css)?;
    Ok(style.get("align").cloned().unwrap_or(PyValue::None))
}

/// `_page_breaks(css)`: whether `css` breaks the page before its element, and after it.
fn page_breaks(css: Option<&Resolved>) -> Result<(bool, bool), ItemError> {
    let breaks = |side: &str| -> Result<bool, ItemError> {
        for prop in [format!("page-break-{side}"), format!("break-{side}")] {
            if matches!(
                keyword(css, &prop)?.as_str(),
                "always" | "page" | "left" | "right" | "recto" | "verso"
            ) {
                return Ok(true);
            }
        }
        Ok(false)
    };
    Ok((breaks("before")?, breaks("after")?))
}

/// `_clear_side(css)`.
fn clear_side(css: Option<&Resolved>) -> Result<Option<&'static str>, ItemError> {
    Ok(match keyword(css, "clear")?.as_str() {
        "left" => Some("left"),
        "right" => Some("right"),
        "both" => Some("both"),
        _ => None,
    })
}

/// Whether the file's root carries `data-kfxgen-source` (truthy).
fn source_marked(dom: &Dom) -> bool {
    dom.attr(dom.root(), SOURCE_MARKED)
        .is_some_and(|v| !v.is_empty())
}

/// `(elem.get("role") or "").strip().lower()`, compared with an ASCII word (no character
/// outside ASCII lowers into one).
fn role_is(dom: &Dom, n: NodeId, word: &str) -> bool {
    py::strip(dom.attr(n, "role").unwrap_or("")).eq_ignore_ascii_case(word)
}

/// `_is_nav_listing(elem)`: a contents listing the book printed itself.
fn is_nav_listing(dom: &Dom, n: NodeId) -> bool {
    if source_marked(dom) {
        if dom.attr(n, SOURCE_TOC).is_some_and(|v| !v.is_empty()) {
            return true;
        }
    } else if dom
        .attr(n, "class")
        .unwrap_or("")
        .split(py::is_space)
        .any(|t| t == "toc")
    {
        return true;
    }
    epub_types(dom, n).iter().any(|t| t == "toc") || role_is(dom, n, "doc-toc")
}

/// `_is_front_matter(elem)`: a dedication, epigraph, copyright page, colophon, or list of
/// illustrations or tables, as the book marks it.
fn is_front_matter(dom: &Dom, n: NodeId) -> bool {
    if source_marked(dom) {
        if dom.attr(n, SOURCE_FRONT).is_some_and(|v| !v.is_empty()) {
            return true;
        }
    } else if dom
        .attr(n, "class")
        .unwrap_or("")
        .to_lowercase()
        .split(py::is_space)
        .any(is_front_matter_token)
    {
        return true;
    }
    if epub_types(dom, n)
        .iter()
        .any(|t| is_front_matter_token(t) || matches!(t.as_str(), "lot" | "loi" | "lov"))
    {
        return true;
    }
    ["doc-dedication", "doc-epigraph", "doc-colophon"]
        .iter()
        .any(|r| role_is(dom, n, r))
}

/// `"".join(n.itertext()).strip()` is not empty: a non-space character in `n`'s text or the
/// text and tail of anything under it (a comment's tail, not its text). Nothing is copied.
fn has_text(dom: &Dom, n: NodeId) -> bool {
    dom.subtree(n).any(|m| {
        if m == n {
            has_words(dom.text(m))
        } else if dom.is_comment(m) {
            has_words(dom.tail(m))
        } else {
            has_words(dom.text(m)) || has_words(dom.tail(m))
        }
    })
}

/// `_is_spacer(elem)`: only no-break spaces or a `<br>`, no picture.
fn is_spacer(dom: &Dom, elem: NodeId) -> bool {
    if has_text(dom, elem) {
        // A no-break space is Python white space, so `replace` then `strip` is `strip`.
        return false;
    }
    let mut nbsp = false;
    let mut br = false;
    for d in dom.subtree(elem) {
        let texts: [&str; 2] = if d == elem {
            [dom.text(d), ""]
        } else if dom.is_comment(d) {
            [dom.tail(d), ""]
        } else {
            [dom.text(d), dom.tail(d)]
        };
        nbsp = nbsp || texts.iter().any(|t| t.contains('\u{a0}'));
        if !dom.is_comment(d) {
            match dom.tag(d) {
                "img" | "svg" => return false,
                "br" => br = true,
                _ => {}
            }
        }
    }
    nbsp || br
}

/// `_is_empty_anchor(elem)`: an `<a>` that only names a place.
fn is_empty_anchor(dom: &Dom, n: NodeId) -> bool {
    !dom.is_comment(n)
        && dom.tag(n) == "a"
        && own_anchor_ids(dom, n).next().is_some()
        && !has_words(dom.text(n))
        && dom.children(n).is_empty()
}

/// `_anchors_follow_rows(elem)`: a row group whose link targets sit after the row each names.
fn anchors_follow_rows(dom: &Dom, elem: NodeId) -> bool {
    if !matches!(dom.tag(elem), "table" | "thead" | "tbody" | "tfoot") {
        return false;
    }
    let kinds: String = dom
        .children(elem)
        .iter()
        .filter_map(|&c| {
            if is_empty_anchor(dom, c) {
                Some('A')
            } else if !dom.is_comment(c) && dom.tag(c) == "tr" {
                Some('R')
            } else {
                None
            }
        })
        .collect();
    kinds.starts_with('R') && kinds.ends_with('A') && kinds.trim_end_matches('A').contains('A')
}

/// `_css_border(css) is not None`: a side drawn. Its colour is read as kfxgen's
/// `_border_colour` reads it, which can raise.
fn css_border_drawn(css: Option<&Resolved>) -> Result<bool, ItemError> {
    let Some(css) = css else {
        return Ok(false);
    };
    let Some(PyValue::Tuple(sides)) = css.get("border-sides")? else {
        return Ok(false);
    };
    let mut drawn = false;
    for side in sides.iter().take(4) {
        let PyValue::Tuple(parts) = side else {
            continue;
        };
        let get = |i: usize| parts.get(i).unwrap_or(&PyValue::None);
        let kind = text_of(Some(get(0)));
        let kind = if kind.is_empty() {
            "none".to_owned()
        } else {
            kind.to_lowercase()
        };
        let colour = text_of(Some(get(2)));
        if matches!(kind.as_str(), "none" | "hidden") || is_transparent(&colour)? {
            continue;
        }
        let pt = match get(1) {
            PyValue::Float(f) => Some(*f),
            PyValue::Int(i) => Some(*i as f64),
            PyValue::Str(v) => py::py_float(v),
            _ => None,
        };
        let pt = match pt {
            Some(pt) => pt,
            None => match text_of(Some(get(1))).to_lowercase().as_str() {
                "thin" => 0.72,
                "medium" => 3.0 * 0.72,
                "thick" => 5.0 * 0.72,
                _ => continue,
            },
        };
        // False for NaN, as Python's: such a side is drawn.
        if pt <= 0.0 {
            continue;
        }
        css_rgb(&colour)?;
        drawn = true;
    }
    Ok(drawn)
}

/// `_unit_percent(unit)`.
fn unit_percent(unit: &str) -> f64 {
    let em = em_percent();
    match unit {
        "$308" | "$505" => em,
        "$319" => em * 0.45 / 12.0,
        "$318" => em / 12.0,
        "$316" => em * 72.0 / 25.4 / 12.0,
        _ => 1.0,
    }
}

/// `_box_width(css)`: a wrapper's CSS width as a share of the page, in %, or None.
fn box_width(css: Option<&Resolved>) -> Result<Option<f64>, ItemError> {
    let raw = match css {
        Some(css) => text_of(css.get("width")?).into_owned(),
        None => String::new(),
    };
    let Some((mag, unit)) = parse_css_length(&raw, false)? else {
        return Ok(None);
    };
    // `float(margin[0])` of a magnitude `parse_css_length` wrote: always a number.
    let share = py::py_float(&mag).unwrap_or(0.0) * unit_percent(unit);
    Ok((share > 0.0 && share < 100.0).then_some(share))
}
