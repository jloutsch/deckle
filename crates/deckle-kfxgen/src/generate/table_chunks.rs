// SPDX-License-Identifier: GPL-3.0-or-later
//! A table's chunks: `_emit_table_chunks` (`native_generator.py:5993`), its container and
//! cell entries, and where ids land in them.

use std::collections::BTreeSet;

use super::chapters::Block;
use super::chunks::{Chunk, InCell, Marker, Node};
use super::tables::{Border, Cell, ColumnWidths, Padding, Table, dedupe};
use super::writing::Bidi;

/// What a cell's `$157` is made from (:6063).
#[derive(Clone)]
pub(crate) struct CellInfo {
    pub header: bool,
    pub not_bold: bool,
    pub colspan: i64,
    pub rowspan: i64,
    pub valign: Option<String>,
    pub border: Option<Border>,
    pub width_set: bool,
    pub padding: Option<Padding>,
    pub background: Option<i64>,
    pub table_width: Option<f64>,
}

/// A table's `open` (:6015).
#[derive(Clone)]
pub(crate) struct TableOpen {
    pub zoom: bool,
    pub border: Option<Border>,
    pub column_widths: Option<ColumnWidths>,
    pub width: Option<f64>,
    pub bidi: Option<Bidi>,
}

/// `_table_has_zoom` (:2101): every table of 2 or more columns (#272).
fn has_zoom(t: &Table) -> bool {
    t.rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.colspan).sum::<i64>())
        .max()
        .unwrap_or(0)
        >= 2
}

/// `_emit_table_chunks` (:5993): a table's `open`, its row groups, rows and cells, a `close`
/// for each. A row with no cells isn't written; its ids go on the next row written, or the
/// last. The table's own ids go on its first row, a link target as its `$278` isn't.
pub(crate) fn emit_table_chunks(block: &Block, t: &Table, chunks: &mut Vec<Chunk>) {
    let cell_keys = |c: &Cell| {
        c.anchors
            .keys
            .iter()
            .chain(c.paragraphs.iter().flat_map(|p| &p.anchors.keys))
            .cloned()
            .collect::<Vec<_>>()
    };
    let inner: BTreeSet<String> = t
        .rows
        .iter()
        .flat_map(|r| {
            r.anchor_keys
                .iter()
                .cloned()
                .chain(r.cells.iter().flat_map(cell_keys))
        })
        .collect();
    let own: Vec<String> = t
        .anchor_keys
        .iter()
        .filter(|k| !inner.contains(*k))
        .cloned()
        .collect();
    let open_at = chunks.len();
    chunks.push(Chunk::marker(Marker::Open(Node::Table(TableOpen {
        zoom: has_zoom(t),
        border: t.border.clone(),
        column_widths: t.column_widths.clone(),
        width: t.width,
        bidi: t.bidi.clone(),
    }))));
    let mut group = None;
    let mut last_row = None;
    // Ids of rows with no cells, skipped.
    let mut carried: Vec<String> = Vec::new();
    for row in &t.rows {
        if row.cells.is_empty() {
            carried.extend(row.anchor_keys.iter().cloned());
            continue;
        }
        if group != Some(row.group) {
            if group.is_some() {
                chunks.push(Chunk::marker(Marker::Close));
            }
            group = Some(row.group);
            chunks.push(Chunk::marker(Marker::Open(Node::Group(
                row.group,
                row.group_border.clone(),
                row.group_bidi.clone(),
            ))));
        }
        let leading = if last_row.is_none() { &own[..] } else { &[] };
        let keys = dedupe(
            leading
                .iter()
                .chain(&carried)
                .chain(&row.anchor_keys)
                .cloned(),
        );
        carried.clear();
        last_row = Some(chunks.len());
        let mut open = Chunk::marker(Marker::Open(Node::Row(
            row.border.clone(),
            row.bidi.clone(),
        )));
        open.anchors.attach(&keys);
        chunks.push(open);
        for cell in &row.cells {
            let info = CellInfo {
                header: cell.header,
                not_bold: cell.not_bold,
                colspan: cell.colspan,
                rowspan: cell.rowspan,
                valign: cell.valign.clone(),
                border: cell.border.clone(),
                width_set: cell.width_set,
                padding: cell.padding.clone(),
                background: cell.background,
                table_width: t.width,
            };
            if !cell.paragraphs.is_empty() {
                // A container styled as a cell, a text entry per block (#261).
                let header_bold = info.header && !info.not_bold;
                chunks.push(Chunk {
                    style: cell.style.clone(),
                    ..Chunk::marker(Marker::Open(Node::Cell(info)))
                });
                for p in &cell.paragraphs {
                    chunks.push(Chunk {
                        spans: p.spans.clone(),
                        anchors: p.anchors.clone(),
                        in_cell: Some(InCell::Paragraph { header_bold }),
                        ..Chunk::text(p.text.clone())
                    });
                }
                chunks.push(Chunk::marker(Marker::Close));
                continue;
            }
            if cell
                .text
                .trim_matches([' ', '\t', '\r', '\n', '\u{a0}'])
                .is_empty()
                && cell.anchors.keys.is_empty()
            {
                // An empty cell is a cell with nothing in it (#480); one holding an id keeps
                // its entry, for a link to land on.
                chunks.push(Chunk {
                    style: cell.style.clone(),
                    ..Chunk::marker(Marker::Open(Node::Cell(info)))
                });
                chunks.push(Chunk::marker(Marker::Close));
                continue;
            }
            let empty = cell.text.is_empty();
            chunks.push(Chunk {
                spans: if empty {
                    Vec::new()
                } else {
                    cell.spans.clone()
                },
                anchors: cell.anchors.clone(),
                in_cell: Some(InCell::Text(info)),
                style: cell.style.clone(),
                ..Chunk::text(if empty {
                    " ".to_owned()
                } else {
                    cell.text.clone()
                })
            });
        }
        chunks.push(Chunk::marker(Marker::Close));
    }
    if group.is_some() {
        chunks.push(Chunk::marker(Marker::Close));
    }
    // Backstop: a key on the block no part of the table declares (:6218).
    let stray = block
        .anchors
        .keys
        .iter()
        .filter(|k| !inner.contains(*k) && !t.anchor_keys.contains(k))
        .cloned();
    let leftover = dedupe(carried.into_iter().chain(stray));
    match last_row {
        None => chunks[open_at]
            .anchors
            .attach(&dedupe(own.into_iter().chain(leftover))),
        Some(i) => chunks[i].anchors.attach(&leftover),
    }
    chunks.push(Chunk::marker(Marker::Close));
}

/// `_attach_keys` (:5899): the chunk at `index`, or the first row of the table it opens:
/// rows, not the `$278`, are link targets (#219); or the first entry in the box it opens
/// (#238).
pub(crate) fn link_target(chunks: &[Chunk], index: usize) -> usize {
    match chunks[index].marker {
        Some(Marker::Open(Node::Table(_))) => {}
        Some(Marker::Open(Node::Box(_))) => {
            return (index..chunks.len())
                .find(|&i| chunks[i].is_content())
                .expect("a box is opened only around an entry");
        }
        _ => return index,
    }
    let mut depth = 0;
    for (i, c) in chunks.iter().enumerate().skip(index) {
        match c.marker {
            Some(Marker::Open(_)) => depth += 1,
            Some(Marker::Close) => depth -= 1,
            None => {}
        }
        if depth == 0 {
            break;
        }
        if matches!(c.marker, Some(Marker::Open(Node::Row(..)))) {
            return i;
        }
    }
    index
}

/// A table `open` with the zoom button, which `build_fragment_259` writes (:4315).
pub(crate) fn has_zoom_button(chunk: &Chunk) -> bool {
    matches!(&chunk.marker, Some(Marker::Open(Node::Table(t))) if t.zoom)
}
