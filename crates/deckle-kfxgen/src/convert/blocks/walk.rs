// SPDX-License-Identifier: GPL-3.0-only
//! The tree walk of `extract_blocks_from_html`: its closures `_walk`, `_walk_flow`,
//! `_walk_element`, `_walk_container` (with `_flush_inline` and `follow`), `_holds_blocks`,
//! `_is_block_at_body`, `_discard_listing`, `_emit_image_block`, `_emit_svg_blocks`,
//! `_walk_split_row`, `_inline_picture_parts`, and the module helpers they call
//! (`_is_nav_listing`, `_is_front_matter`, `_page_breaks`, `_clear_side`,
//! `_pictures_among_words`, `_lone_block_picture`, `_is_spacer`, `_anchors_follow_rows`,
//! `_row_align`, `_dedupe_keep_order`), at kfxgen `ad38ec8`. Lists are `lists.rs`, tables
//! `tables.rs`.
//!
//! kfxgen recurses once per nested element (`_walk` → `_walk_flow` → `_walk_element` →
//! `_walk_container` → `_walk`, and `_discard_listing`); here each open call is a [`Frame`] on
//! a stack, and an element more than `MAX_DEPTH` below the `<body>` stops the file with
//! [`Error::TooLarge`]. `read_book` refuses any element deeper than `MAX_DEPTH` below the root,
//! so no tree it gives reaches that.

use std::collections::{HashMap, HashSet};

use deckle_limits::MAX_DEPTH;

use super::boxes::Seen;
use super::lists::Marker;
use super::lists::{Item, Lists, Native};
use super::spacing::Noted;
use super::tables::{CellEnd, TableJob, Tables, add_ids, is_cell, is_non_text};
use super::{Block, BlockId, ExtractOptions, Field, dedupe, dict_set, s, strs, unsupported};
use crate::convert::block_bidi::{BlockBidi, start_end_align};
use crate::convert::block_style::compute_block_style;
use crate::convert::initial::initial;
use crate::convert::inline::{
    Walk, epub_switch_branch, epub_types, is_body_block, is_non_rendered, keyword, own_anchor_ids,
    subtree_anchor_ids,
};
use crate::convert::inline_style::{
    FLAG_PRE, Flag, Flags, Segment, make_anchor_mark, normalize_runs_with_anchors,
};
use crate::convert::pictures::{
    DATA_URI, SVG_IMAGE_SIZE, css_background_image, displays_as_block, img_size_hint, img_token,
    is_data_uri, svg_draws_shapes_only, svg_image_refs, svg_text_paragraphs,
};
use crate::convert::resolver::{Resolved, Resolver};
use crate::convert::{Error, ItemError, Raised};
use crate::dom::{Dom, NodeId, XHTML_NS};
use crate::generate::page_scale::em_percent;
use crate::generate::py;
use crate::pyvalue::PyValue;

const MATHML_NS: &str = "http://www.w3.org/1998/Math/MathML";
const WEIGHT_NOT_A_NUMBER: Raised = Raised {
    exception: "ValueError",
    note: "_walk_split_row: int() of a cell's font-weight of digits int() does not read",
};
/// `_SOURCE_MARKED`, `_SOURCE_TOC`, `_SOURCE_FRONT`: kfxgen's marks of the book's own classes.
const SOURCE_MARKED: &str = "data-kfxgen-source";
const SOURCE_TOC: &str = "data-kfxgen-toc";
const SOURCE_FRONT: &str = "data-kfxgen-front";
pub(super) const HORIZONTAL: &str = "data-kfxgen-horizontal";
pub(super) const VERTICAL: &str = "data-kfxgen-vertical";

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
pub(super) fn is_block_tag(dom: &Dom, n: NodeId) -> bool {
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

/// Whether `s` holds anything but Python's white space (`s.strip()` is not empty).
pub(super) fn has_words(s: &str) -> bool {
    !py::strip(s).is_empty()
}

/// What the walk leaves for the steps after it (spacing, trailing ids, anchor keys).
pub(super) struct Walked {
    pub(super) blocks: Vec<Block>,
    pub(super) pending_ids: Vec<String>,
    /// `rule_ids`: an `<hr>`'s own ids.
    pub(super) rule_ids: HashSet<String>,
    pub(super) listing_ids: HashSet<String>,
    pub(super) nav_listing_at: Vec<(usize, Vec<String>)>,
    pub(super) body: NodeId,
    pub(super) margins: HashMap<BlockId, [f64; 2]>,
    pub(super) paddings: HashMap<BlockId, [f64; 4]>,
    /// `tables_seen` and `notes_seen`: the first block of each table written otherwise than
    /// as one table block.
    pub(super) tables_seen: Vec<BlockId>,
    pub(super) notes_seen: Vec<BlockId>,
}

/// What `_walk_cell` sets aside before walking a cell and puts back after: the pending ids
/// (with how many of them are listed already), list markers, page break and clear.
pub(super) struct Aside {
    ids: Vec<String>,
    listed: usize,
    markers: Vec<Marker>,
    break_at: Option<usize>,
    clear_at: Option<(usize, &'static str)>,
}

/// `_walk_container(elem)` part-way through its children.
pub(super) struct Container {
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
    /// The child just walked as a block, whose tail comes next, and where its blocks began.
    walked: Option<NodeId>,
    walked_from: usize,
    /// `follow`: a row group whose empty anchors after a row name that row (#221).
    follow: bool,
    /// `row_block`, as its index in `blocks`, and the ids of the empty anchors after it not
    /// added to it yet (added together, in order, before anything else reads it).
    row: Option<usize>,
    row_ids: Vec<String>,
    depth: usize,
}

/// `_walk_split_row(row)` part-way through its cells.
pub(super) struct SplitRow {
    row: NodeId,
    /// The alignment its paragraphs take where they set none (`_row_align`).
    align: PyValue,
    start: usize,
    /// Its next child, as an index into its children.
    next: usize,
    /// The cell just walked and where its blocks began.
    cell: Option<(NodeId, usize)>,
    depth: usize,
}

/// One call of kfxgen's walk still open: what it does once the call it made returns.
pub(super) enum Frame {
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
    /// `_walk_list_item(li, native)` after `_walk_element(li)`.
    ListItemEnd {
        li: NodeId,
        list: NodeId,
        start: usize,
    },
    /// The `li` branch of `_walk_flow` after `_walk_element(li)`: an unused marker dropped.
    MarkerEnd {
        before: usize,
    },
    /// `_discard_listing(elem)` part-way through its children.
    Discard {
        elem: NodeId,
        next: usize,
        depth: usize,
    },
    /// `_walk_element`'s listing branch after `_discard_listing`.
    ListingEnd,
    /// The non-`li` branch of `_walk_flow` after `_walk_element(table)`: a table that made
    /// blocks but no table block is counted in `tables_seen` or `notes_seen`.
    TableSeen {
        elem: NodeId,
        start: usize,
    },
    /// A native table's block, after the fostered block or caption just walked.
    Table(Box<TableJob>),
    /// `_walk_cell(cell)` after its `_walk_element(cell)`, then the table it is building.
    CellEnd(Box<CellEnd>),
    SplitRow(SplitRow),
}

/// Memo slots per node.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bit {
    Unknown,
    Yes,
    No,
}

pub(super) struct Walker<'o, 'w, 'a, 's> {
    pub(super) w: &'w mut Walk<'a, 's>,
    pub(super) dom: &'a Dom,
    pub(super) resolver: &'a Resolver<'a>,
    pub(super) opts: &'o ExtractOptions<'o>,
    pub(super) blocks: Vec<Block>,
    next_id: usize,
    pub(super) pending_ids: Vec<String>,
    /// `rule_ids`: an `<hr>`'s own ids, which name the rule.
    pub(super) rule_ids: HashSet<String>,
    listing_ids: HashSet<String>,
    /// How many of `pending_ids`, from the front, are in `listing_ids` already: kfxgen adds
    /// all of them after each listing, which is quadratic over many listings in a row.
    listed: usize,
    pub(super) nav_listing_at: Vec<(usize, Vec<String>)>,
    /// `break_at` and `clear_at`: indexes into `blocks`, kept as Python's plain integers (a
    /// later splice of `blocks` leaves them as they are, as kfxgen's does). Each holds at
    /// most one entry in kfxgen (it appends only to an empty list).
    break_at: Option<usize>,
    clear_at: Option<(usize, &'static str)>,
    pub(super) bidi: BlockBidi,
    /// Whether a node's subtree, itself included, holds an element of `block_tags`; a
    /// `<table>`; text or an element of `_NON_TEXT_TAGS` (its own tail counted, see
    /// [`Walker::shows`]).
    holds_block_tag: Vec<Bit>,
    holds_table: Vec<Bit>,
    shows: Vec<Bit>,
    pub(super) stack: Vec<Frame>,
    /// `margins` and `paddings`, and what working them out reads more than once.
    pub(super) noted: Noted,
    /// `pending_markers`, `list_ordinals`, `native_lists`, `cell_depth`.
    pub(super) lists: Lists,
    /// What `_make_box` has read in each block's text.
    pub(super) seen: HashMap<BlockId, Seen>,
    /// `leaf_cells`, `native_made`, `notes_tables`, `tables_seen`, `notes_seen`, and what the
    /// table code reads more than once.
    pub(super) tables: Tables,
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
        rule_ids: HashSet::new(),
        listing_ids: HashSet::new(),
        listed: 0,
        nav_listing_at: Vec::new(),
        break_at: None,
        clear_at: None,
        bidi: BlockBidi::new(dom),
        holds_block_tag: vec![Bit::Unknown; dom.node_count()],
        holds_table: vec![Bit::Unknown; dom.node_count()],
        shows: vec![Bit::Unknown; dom.node_count()],
        stack: Vec::new(),
        noted: Noted::new(dom),
        lists: Lists::default(),
        seen: HashMap::new(),
        tables: Tables::default(),
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
        rule_ids: walker.rule_ids,
        listing_ids: walker.listing_ids,
        nav_listing_at: walker.nav_listing_at,
        body,
        margins: walker.noted.margins,
        paddings: walker.noted.paddings,
        tables_seen: walker.tables.seen,
        notes_seen: walker.tables.notes_seen,
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
    pub(super) fn too_deep(&self) -> ItemError {
        ItemError::Error(Error::TooLarge {
            what: format!(
                "{}: an element nested more than {MAX_DEPTH} levels below the body",
                self.w.file().href
            ),
        })
    }

    pub(super) fn unsupported(&self, feature: &'static str) -> ItemError {
        unsupported(self.w, feature)
    }

    pub(super) fn new_block(&mut self, fields: Vec<(&'static str, Field)>) -> Block {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        Block { id, fields }
    }

    /// `ids = pending_ids[:]; pending_ids.clear()`.
    pub(super) fn take_pending(&mut self) -> Vec<String> {
        self.listed = 0;
        std::mem::take(&mut self.pending_ids)
    }

    /// `_walk_cell`'s `saved_ids, saved_markers = pending_ids[:], pending_markers[:]`,
    /// `saved_break, saved_clear = break_at[:], clear_at[:]`, then each cleared.
    pub(super) fn set_aside(&mut self) -> Aside {
        Aside {
            listed: std::mem::take(&mut self.listed),
            ids: std::mem::take(&mut self.pending_ids),
            markers: std::mem::take(&mut self.lists.pending),
            break_at: self.break_at.take(),
            clear_at: self.clear_at.take(),
        }
    }

    /// `_walk_cell`'s `pending_ids[:] = saved_ids` and the rest put back.
    pub(super) fn put_back(&mut self, aside: Aside) {
        self.pending_ids = aside.ids;
        self.listed = aside.listed;
        self.lists.pending = aside.markers;
        self.break_at = aside.break_at;
        self.clear_at = aside.clear_at;
    }

    pub(super) fn resolve(&self, n: NodeId) -> Result<Option<Resolved<'a>>, ItemError> {
        Ok(self.resolver.resolve(n)?)
    }

    /// The body: an SVG document's pictures, the body's loose text walked as a `<div>`'s, or
    /// each child walked.
    fn walk_body(&mut self, body: NodeId) -> Result<(), ItemError> {
        let dom = self.dom;
        if dom.tag(body) == "svg" {
            // The page is an SVG file (#165).
            return self.svg_blocks(body);
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
                Frame::ListItemEnd { li, list, start } => self.list_item_end(li, list, start)?,
                Frame::MarkerEnd { before } => self.marker_end(before),
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
                Frame::TableSeen { elem, start } => self.table_seen(elem, start),
                Frame::Table(job) => self.table_run(job)?,
                Frame::CellEnd(end) => self.cell_end(*end)?,
                Frame::SplitRow(r) => self.split_row_step(r)?,
            }
        }
        Ok(())
    }

    /// `_walk(elem)` up to its `_walk_flow`; the rest is its [`Frame::WalkEnd`].
    pub(super) fn enter(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
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

    /// `_walk_flow(elem)`.
    fn walk_flow(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
        if !self.dom.is_comment(elem) && self.dom.tag(elem) == "li" && !self.w.hidden(elem)? {
            match self.list_item(elem)? {
                Item::Native { list } => self.stack.push(Frame::ListItemEnd {
                    li: elem,
                    list,
                    start: self.blocks.len(),
                }),
                Item::Marked { before } => self.stack.push(Frame::MarkerEnd { before }),
                Item::Unmarked => {}
            }
            return self.walk_element(elem, depth);
        }
        if !self.dom.is_comment(elem) && self.dom.tag(elem) == "table" {
            self.stack.push(Frame::TableSeen {
                elem,
                start: self.blocks.len(),
            });
        }
        self.walk_element(elem, depth)
    }

    /// `_walk_element(elem)`, up to its container branch's `_walk_container`.
    pub(super) fn walk_element(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
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
        if let Some(href) = background.filter(|b| !b.is_empty()) {
            // The element is the picture: no children, no text (#168).
            let size = img_size_hint(dom, elem, css.as_ref())?;
            drop(css);
            let token = img_token(&href, "", size.as_deref(), None, None);
            return self.image_block(elem, Some(token));
        }
        drop(css);
        let local = dom.tag(elem);
        if local == "hr" {
            return self.emit_rule(elem);
        }
        if local == "table" && self.opts.native_tables && self.table(elem, depth)? {
            return Ok(());
        }
        self.walk_rest(elem, depth)
    }

    /// The rest of `_walk_element(elem)`: a table written as rows goes on here.
    pub(super) fn walk_rest(&mut self, elem: NodeId, depth: usize) -> Result<(), ItemError> {
        let dom = self.dom;
        let local = dom.tag(elem);
        // An equation walked as a block is a paragraph of its own (#235); so is a cell
        // `_walk_cell` walks that holds no block (#565).
        let is_block =
            is_block_tag(dom, elem) || local == "math" || self.tables.leaf_cells.contains(&elem);
        let mut has_block_child = false;
        for &child in dom.children(elem) {
            if self.holds_blocks(child) {
                has_block_child = true;
                break;
            }
        }
        if self.opts.native_tables && local == "tr" && !has_block_child && self.table_below(elem) {
            return self.split_row(elem, depth);
        }
        if is_block && !has_block_child {
            return self.leaf(elem);
        }
        if local == "img" {
            return self.image_block(elem, None);
        }
        if local == "svg" {
            return self.svg_blocks(elem);
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
        let (mut text, mut spans, mut marks) = normalize_runs_with_anchors(&segments);
        drop(segments);
        // `ids = pending_ids[:] + _subtree_anchor_ids(elem)`, the pending ids moved only when
        // a block takes them (copying them for every empty block is quadratic).
        let own = subtree_anchor_ids(dom, elem);
        // Only non-breaking spaces is a spacer, not a paragraph (#230, #244).
        if !has_words(&text) {
            self.pending_ids.extend(own);
            if is_spacer(dom, elem) {
                self.spacer(elem)?;
            }
            return Ok(());
        }
        let mut ids = self.take_pending();
        ids.extend(own);
        self.take_marker(&mut text, &mut spans, &mut marks)?;
        let local = dom.tag(elem);
        let css = self.resolve(elem)?;
        let mut bstyle = PyValue::None;
        if let Some(css) = &css {
            bstyle = compute_block_style(css)?;
            if local == "tr"
                && let PyValue::Dict(style) = &mut bstyle
            {
                let own = style_align(style);
                let align = self.row_align(elem, own)?;
                dict_set(style, "align", align);
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
        if self.tables.leaf_cells.contains(&elem) {
            // The cell itself: its padding is the cell's, and a cell has no margins (#655).
            return Ok(());
        }
        // Text written straight in a bordered or filled block is a box round that one
        // paragraph; the box takes its margins and padding.
        if self.make_box(elem, at, true)? {
            return Ok(());
        }
        let padded = self.note_padding(elem, at)?;
        self.note_margins(elem, at, padded)
    }

    /// `_emit_image_block(elem, ...)`: an `<img>` (`token` None), or the picture `token` an
    /// element draws (its background, or an `<svg>`'s `<image>`).
    fn image_block(&mut self, elem: NodeId, token: Option<String>) -> Result<(), ItemError> {
        let dom = self.dom;
        let mut ids = self.take_pending();
        ids.extend(own_anchor_ids(dom, elem).map(str::to_owned));
        let block_ids = dedupe(ids);
        let token = match token {
            Some(t) => t,
            None => self.w.picture_token(elem, dom.parent(elem))?,
        };
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
            "img" => return self.image_block(elem, None),
            "svg" => return self.svg_blocks(elem),
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
        Ok(Container {
            elem,
            at_body,
            ws,
            among,
            parts,
            next: 0,
            walked: None,
            walked_from: 0,
            follow: anchors_follow_rows(dom, elem),
            row: None,
            row_ids: Vec::new(),
            depth,
        })
    }

    /// `_walk_container`'s loop from where it stopped, until the next child walked as a block
    /// (then it waits on the stack) or its end.
    fn container_step(&mut self, mut c: Container) -> Result<(), ItemError> {
        let dom = self.dom;
        if let Some(child) = c.walked.take() {
            if c.follow && dom.tag(child) == "tr" {
                self.add_row_ids(&mut c);
                c.row = (self.blocks.len() > c.walked_from).then_some(c.walked_from);
            }
            self.tail(&mut c, child);
        }
        while let Some(&child) = dom.children(c.elem).get(c.next) {
            c.next += 1;
            if c.follow && c.row.is_some() && is_empty_anchor(dom, child) {
                // An anchor after a row names that row, at its start (#221).
                c.row_ids
                    .extend(own_anchor_ids(dom, child).map(str::to_owned));
            } else if c.among.contains(&child) {
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
                c.walked_from = self.blocks.len();
                self.stack.push(Frame::Container(c));
                return self.enter(child, depth);
            } else {
                // A hidden child gives only its ids (#256).
                let segs = self.w.inline_child(child, &c.ws)?;
                c.parts.extend(segs);
            }
            self.tail(&mut c, child);
        }
        self.add_row_ids(&mut c);
        self.flush(&mut c)
    }

    /// The ids of the empty anchors after `c`'s row added to its first block, and to the
    /// table inside it when that block is a table (#263): each where it is missing, in order.
    fn add_row_ids(&mut self, c: &mut Container) {
        let ids = std::mem::take(&mut c.row_ids);
        let Some(b) = c.row.and_then(|at| self.blocks.get_mut(at)) else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        add_ids(b, &ids);
        if let Some(PyValue::Dict(tbl)) = b.get_mut("table")
            && let Some((_, PyValue::List(own))) = tbl
                .iter_mut()
                .find(|(k, _)| k.as_str() == Some("anchor_ids"))
        {
            let mut have: HashSet<String> = own
                .iter()
                .filter_map(|a| a.as_str().map(str::to_owned))
                .collect();
            for aid in ids {
                if have.insert(aid.clone()) {
                    own.push(PyValue::Str(aid));
                }
            }
        }
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
        let (mut text, mut spans, mut marks) = normalize_runs_with_anchors(&parts);
        drop(parts);
        if !has_words(&text) {
            // An anchor with no visible text carries forward.
            self.pending_ids.extend(marks.into_iter().map(|(id, _)| id));
            return Ok(());
        }
        self.take_marker(&mut text, &mut spans, &mut marks)?;
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
        if self.tables.leaf_cells.contains(&elem) {
            // The cell itself: its padding is the cell's, written on it, and a cell has no
            // margins (#655).
            return Ok(());
        }
        if matches!(self.dom.tag(elem), "ul" | "ol")
            && let Some(native) = self.native_list(elem)?
        {
            let plain = matches!(native, Native::Plain { .. });
            self.make_list(elem, start)?;
            if !plain {
                // An unmarked list's side margins are in its indent.
                self.carry_container_margins(elem, start)?;
            }
            return Ok(());
        }
        if self.make_box(elem, start, false)? {
            return Ok(());
        }
        self.carry_container_margins(elem, start)?;
        let padded = self.note_padding(elem, start)?;
        self.note_margins(elem, start, padded)
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

    /// `any(d.tag in block_tags for d in n.iter())`.
    fn block_below(&mut self, n: NodeId) -> bool {
        let dom = self.dom;
        any_below(dom, &mut self.holds_block_tag, n, |m| is_block_tag(dom, m))
    }

    /// `any(_local_tag(d.tag) == "table" for d in tr.iterdescendants())`.
    fn table_below(&mut self, tr: NodeId) -> bool {
        let dom = self.dom;
        let table = |m: NodeId| !dom.is_comment(m) && dom.tag(m) == "table";
        dom.children(tr)
            .iter()
            .any(|&c| any_below(dom, &mut self.holds_table, c, table))
    }

    /// `"".join(cell.itertext()).strip()`, or an element of `_NON_TEXT_TAGS` in `cell.iter()`.
    fn cell_shows(&mut self, cell: NodeId) -> bool {
        let dom = self.dom;
        // Below the cell a node's tail is part of the cell's text.
        let shows = |m: NodeId| {
            has_words(dom.tail(m))
                || (!dom.is_comment(m) && (has_words(dom.text(m)) || is_non_text(dom.tag(m))))
        };
        has_words(dom.text(cell))
            || is_non_text(dom.tag(cell))
            || dom
                .children(cell)
                .iter()
                .any(|&c| any_below(dom, &mut self.shows, c, shows))
    }

    /// `_row_align(tr, own, style_resolver)`: a row written as one paragraph takes its cells'
    /// alignment when every cell with something in it agrees, else its own (#224).
    fn row_align(&mut self, tr: NodeId, own: PyValue) -> Result<PyValue, ItemError> {
        let dom = self.dom;
        let mut aligns: Vec<PyValue> = Vec::new();
        for &cell in dom.children(tr) {
            if dom.is_comment(cell) || !is_cell(dom.tag(cell)) || !self.cell_shows(cell) {
                continue;
            }
            let align = match self.resolve(cell)? {
                Some(css) => block_align(&css)?,
                None => PyValue::None,
            };
            // The values are a few keywords and None.
            if !aligns.contains(&align) {
                aligns.push(align);
            }
        }
        Ok(match aligns.as_slice() {
            [one] if *one != PyValue::None => one.clone(),
            _ => own,
        })
    }

    /// `_walk_split_row(row)` up to its first cell: a row holding a table, each cell walked
    /// as the body is, so each table in it is a block of its own (#263, #681).
    fn split_row(&mut self, row: NodeId, depth: usize) -> Result<(), ItemError> {
        let mut align = PyValue::None;
        if let Some(css) = self.resolve(row)? {
            let own = block_align(&css)?;
            drop(css);
            align = self.row_align(row, own)?;
        }
        // The row's own ids name its first piece.
        self.pending_ids
            .extend(own_anchor_ids(self.dom, row).map(str::to_owned));
        let start = self.blocks.len();
        self.split_row_step(SplitRow {
            row,
            align,
            start,
            next: 0,
            cell: None,
            depth,
        })
    }

    /// `_walk_split_row`'s loop from where it stopped: the next cell's `_walk_container`, or
    /// the row's end.
    fn split_row_step(&mut self, mut r: SplitRow) -> Result<(), ItemError> {
        let dom = self.dom;
        if let Some((cell, first)) = r.cell.take() {
            self.header_bold(cell, first)?;
        }
        while let Some(&cell) = dom.children(r.row).get(r.next) {
            r.next += 1;
            if dom.is_comment(cell) || !is_cell(dom.tag(cell)) {
                continue;
            }
            self.pending_ids
                .extend(own_anchor_ids(dom, cell).map(str::to_owned));
            r.cell = Some((cell, self.blocks.len()));
            let c = self.container(cell, false, r.depth + 1)?;
            self.stack.push(Frame::SplitRow(r));
            self.stack.push(Frame::Container(c));
            return Ok(());
        }
        // A table's rows written as paragraphs keep their space (#219), and the row's
        // alignment where they set none of their own. A row nested in a cell is looked at
        // again by each row round it: each look is charged.
        self.step(self.blocks.len().saturating_sub(r.start))?;
        for b in self.blocks.iter_mut().skip(r.start) {
            if b.kind().is_some() {
                continue;
            }
            b.set("row", PyValue::Bool(true));
            let style = b.get("block_style");
            let set = style
                .and_then(|st| st.get("align"))
                .is_some_and(PyValue::is_truthy);
            if r.align.is_truthy() && !set {
                let mut pairs = match style {
                    Some(PyValue::Dict(pairs)) => pairs.clone(),
                    _ => Vec::new(),
                };
                dict_set(&mut pairs, "align", r.align.clone());
                b.set("block_style", PyValue::Dict(pairs));
            }
        }
        Ok(())
    }

    /// A header cell's bold, which its own text, written straight in it, has no paragraph
    /// style to carry (QA on #692): onto each paragraph from `first` on without a `bold`.
    fn header_bold(&mut self, cell: NodeId, first: usize) -> Result<(), ItemError> {
        let weight = keyword(self.resolve(cell)?.as_ref(), "font-weight")?;
        let bold = match weight.as_str() {
            "bold" | "bolder" => true,
            w if !w.is_empty() && w.chars().all(py::is_digit) => {
                // `int()` refuses the digits `isdigit` takes beyond category Nd.
                py::py_int(w).ok_or(WEIGHT_NOT_A_NUMBER)? >= 600
            }
            _ => false,
        };
        if !bold {
            return Ok(());
        }
        self.step(self.blocks.len().saturating_sub(first))?;
        for b in self.blocks.iter_mut().skip(first) {
            if b.kind().is_some() {
                continue;
            }
            let mut pairs = match b.get("block_style") {
                Some(PyValue::Dict(pairs)) => pairs.clone(),
                _ => Vec::new(),
            };
            if pairs.iter().any(|(k, _)| k.as_str() == Some("bold")) {
                continue;
            }
            pairs.push((s("bold"), PyValue::Bool(true)));
            b.set("block_style", PyValue::Dict(pairs));
        }
        Ok(())
    }

    /// `_emit_svg_blocks(svg)`: each `<image>` an `<svg>` draws a picture block of its own,
    /// then the words of each `<text>` a paragraph (#231). One drawn only with shapes, but for
    /// a page that is an SVG file, is refused as in text (kfxgen draws it as a picture, #709),
    /// and one with a `data:` picture, before anything is copied.
    fn svg_blocks(&mut self, svg: NodeId) -> Result<(), ItemError> {
        let dom = self.dom;
        if dom.parent(svg).is_some() && svg_draws_shapes_only(dom, svg) {
            return Err(self.unsupported("SVG pictures"));
        }
        let refs = svg_image_refs(dom, svg);
        if refs.iter().any(|href| is_data_uri(href)) {
            return Err(self.unsupported(DATA_URI));
        }
        let mut drew_picture = !refs.is_empty();
        for href in refs {
            self.resolver.charge(href.len())?;
            let token = img_token(href, "", Some(SVG_IMAGE_SIZE), None, None);
            self.image_block(svg, Some(token))?;
        }
        for (text, node) in svg_text_paragraphs(dom, svg) {
            let mut ids = Vec::new();
            if !drew_picture {
                // No picture took the `<svg>`'s own ids or those carried to it.
                ids = self.take_pending();
                ids.extend(own_anchor_ids(dom, svg).map(str::to_owned));
                drew_picture = true;
            }
            ids.extend(subtree_anchor_ids(dom, node));
            let ids = dedupe(ids);
            self.resolver.charge(text.len())?;
            let style = self
                .bidi
                .with_bidi(dom, self.resolver, PyValue::None, node, true)?;
            let block = self.new_block(vec![
                ("text", Field::Py(PyValue::Str(text))),
                ("spans", Field::Py(PyValue::List(Vec::new()))),
                ("block_style", Field::Py(style)),
                ("anchor_ids", Field::Py(strs(&ids))),
                ("anchor_offsets", Field::Py(offsets(&ids, &[]))),
            ]);
            self.blocks.push(block);
        }
        Ok(())
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

/// Whether `own` holds for `n` or anything below it, kept per node in `memo`: worked out for a
/// whole subtree at once, children first, so every node is looked at once per file.
fn any_below(dom: &Dom, memo: &mut [Bit], n: NodeId, own: impl Fn(NodeId) -> bool) -> bool {
    if memo[n.index()] == Bit::Unknown {
        // (node, children done)
        let mut stack = vec![(n, false)];
        while let Some((m, done)) = stack.pop() {
            if memo[m.index()] != Bit::Unknown {
                continue;
            }
            if done {
                let below = own(m) || dom.children(m).iter().any(|c| memo[c.index()] == Bit::Yes);
                memo[m.index()] = if below { Bit::Yes } else { Bit::No };
            } else {
                stack.push((m, true));
                stack.extend(dom.children(m).iter().map(|&c| (c, false)));
            }
        }
    }
    memo[n.index()] == Bit::Yes
}

/// `compute_block_style(...)["align"]` of a block style dict.
fn style_align(style: &[(PyValue, PyValue)]) -> PyValue {
    style
        .iter()
        .find(|(k, _)| k.as_str() == Some("align"))
        .map_or(PyValue::None, |(_, v)| v.clone())
}

/// A block's `text`, "" for none.
pub(super) fn text_field(b: &Block) -> &str {
    b.get("text").and_then(PyValue::as_str).unwrap_or("")
}

/// `{aid: mark_offsets.get(aid, 0) for aid in ids}`.
pub(super) fn offsets(ids: &[String], marks: &[(String, usize)]) -> PyValue {
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
pub(super) fn is_nav_listing(dom: &Dom, n: NodeId) -> bool {
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
pub(super) fn is_empty_anchor(dom: &Dom, n: NodeId) -> bool {
    !dom.is_comment(n)
        && dom.tag(n) == "a"
        && own_anchor_ids(dom, n).next().is_some()
        && !has_words(dom.text(n))
        && dom.children(n).is_empty()
}

/// `_anchors_follow_rows(elem)`: a row group whose link targets sit after the row each names.
pub(super) fn anchors_follow_rows(dom: &Dom, elem: NodeId) -> bool {
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

/// `_unit_percent(unit)`.
pub(super) fn unit_percent(unit: &str) -> f64 {
    let em = em_percent();
    match unit {
        "$308" | "$505" => em,
        "$319" => em * 0.45 / 12.0,
        "$318" => em / 12.0,
        "$316" => em * 72.0 / 25.4 / 12.0,
        _ => 1.0,
    }
}
