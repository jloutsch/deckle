// SPDX-License-Identifier: GPL-3.0-only
//! The book's own landmarks and page list, where their links land (`generate_full_book`,
//! `native_generator.py:5016`), else kfxlib's approximate pages.
use super::inline::{IdMaps, id_maps};
use super::text::ChapterData;
use super::{GenerateError, model_err};
use crate::pyvalue::{PyValue, truthy};

/// A landmark of `$389`'s `$236`: its kind's symbol, label and place.
pub(crate) struct Landmark {
    pub symbol: &'static str,
    pub label: String,
    pub position: i64,
    pub offset: i64,
}

/// What `$389` lists besides the contents.
pub(crate) struct BookNav {
    pub landmarks: Vec<Landmark>,
    /// (label, position, offset).
    pub pages: Vec<(String, i64, i64)>,
    pub page_list_name: &'static str,
}

/// `lands` (:5018): an id's chunk and offset, else for a whole file its first chunk.
fn lands(
    (key_to_chunk, file_to_chunk): &IdMaps,
    data: &ChapterData,
    key: &str,
) -> Option<(i64, i64)> {
    // kfxgen tries a file only for a key with no `#`; no file name holds one.
    let (idx, offset) = match key_to_chunk.get(key) {
        Some(&found) => found,
        None => (*file_to_chunk.get(key)?, 0),
    };
    // Only a `close` has no position, and it holds no ids.
    Some((data.chunk_positions[idx].unwrap_or_default(), offset))
}

/// `landmarks` or `page_list`: `arity`-tuples of strings; None or empty is none.
fn entries<'a>(
    args: &'a PyValue,
    key: &str,
    arity: usize,
) -> Result<Vec<Vec<&'a str>>, GenerateError> {
    let what = || model_err(format!("args.{key}: a list of {arity}-tuples of strings"));
    let Some(list) = args.get(key).filter(|v| v.is_truthy()) else {
        return Ok(Vec::new());
    };
    list.as_list()
        .ok_or_else(what)?
        .iter()
        .map(|entry| {
            entry
                .as_list()
                .filter(|items| items.len() == arity)
                .and_then(|items| items.iter().map(PyValue::as_str).collect())
                .ok_or_else(what)
        })
        .collect()
}

/// `approximate_pages` are kfxlib's (position, offset) pages, used when the book has none of
/// its own that land.
pub(crate) fn read(
    args: &PyValue,
    data: &ChapterData,
    approximate_pages: &[(i64, i64)],
) -> Result<BookNav, GenerateError> {
    let maps = id_maps(data.chunks.iter().map(|c| &c.anchors));
    // One that lands nowhere is left out (#579, :5025).
    let mut marks: Vec<(&str, &str, i64, i64)> = Vec::new();
    for e in entries(args, "landmarks", 3)? {
        if let Some((position, offset)) = lands(&maps, data, e[2]) {
            marks.push((e[0], e[1], position, offset));
        }
    }
    // The first start landmark that lands, as Previewer keeps it (#609, :5030).
    let mut started = false;
    marks.retain(|m| m.0 != "start" || !std::mem::replace(&mut started, true));
    let landmarks = marks
        .into_iter()
        .map(|(kind, label, position, offset)| {
            let symbol = match kind {
                "toc" => "$212",
                "start" => "$396",
                // kfxgen raises `KeyError` (:3198).
                _ => return Err(model_err("args.landmarks: kinds \"toc\" or \"start\"")),
            };
            Ok(Landmark {
                symbol,
                label: label.to_owned(),
                position,
                offset,
            })
        })
        .collect::<Result<_, _>>()?;

    // The book's print page breaks, the same way (#578, :5033).
    let mut pages: Vec<(String, i64, i64)> = Vec::new();
    for e in entries(args, "page_list", 2)? {
        if let Some((position, offset)) = lands(&maps, data, e[1]) {
            pages.push((e[0].to_owned(), position, offset));
        }
    }
    let mut page_list_name = "nP";
    if pages.is_empty() && truthy(args.get("approximate_pages")) {
        // None of its own: approximate pages, as kfxlib adds them (#586, :5040).
        pages = (1..)
            .zip(approximate_pages)
            .map(|(n, &(p, off))| (n.to_string(), p, off))
            .collect();
        page_list_name = "APPROXIMATE_PAGE_LIST";
    }
    Ok(BookNav {
        landmarks,
        pages,
        page_list_name,
    })
}
