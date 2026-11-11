// SPDX-License-Identifier: GPL-3.0-or-later
//! kfxgen 0bc705d's `page_scale.py` (#754): the page width Kindle Previewer 4.0.3 measures a
//! percentage against, 384pt (a 512px column) in a horizontal book and 480pt (640px) in a
//! vertical one (P247j, #247). kfxgen's `convert` sets it around `generate_full_book`; the port
//! sets it from the `writing_mode` argument, as `convert` chooses it (DIVERGENCES.md).
use std::cell::Cell;

pub(crate) const HORIZONTAL_PT: f64 = 384.0;
pub(crate) const VERTICAL_PT: f64 = 480.0;

thread_local! {
    /// `_PAGE_PT`, a context variable in kfxgen.
    static PAGE_PT: Cell<f64> = const { Cell::new(HORIZONTAL_PT) };
}

/// `page_pt`: the page width in pt.
pub(crate) fn page_pt() -> f64 {
    PAGE_PT.with(Cell::get)
}

/// `column_px`: the page width in CSS px, 512 or 640.
pub(crate) fn column_px() -> f64 {
    page_pt() * 4.0 / 3.0
}

/// `em_percent`: one em, 12pt, as a % of the page: 3.125, or 2.5 in a vertical book.
pub(crate) fn em_percent() -> f64 {
    1200.0 / page_pt()
}

/// `page_width`: percentages are measured against `pt` until the guard is dropped.
pub(crate) fn page_width(pt: f64) -> PageWidth {
    PageWidth(PAGE_PT.with(|p| p.replace(pt)))
}

/// Puts back the width it replaced, as `page_width`'s `_PAGE_PT.reset(token)`.
pub(crate) struct PageWidth(f64);

impl Drop for PageWidth {
    fn drop(&mut self) {
        PAGE_PT.with(|p| p.set(self.0));
    }
}
