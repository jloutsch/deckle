// SPDX-License-Identifier: GPL-3.0-only
//! The plugin options kfxgen's converter reads (`opts.kfxgen_*`, `plugin/__init__.py`), each
//! `False` unless the user turns it on.

/// kfxgen's conversion options; `Default` is the plugin's defaults, all off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConvertOptions {
    /// `kfxgen_disable_native_tables`: tables as text, not KFX tables.
    pub disable_native_tables: bool,
    /// `kfxgen_disable_font_embedding`: no fonts from the book.
    pub disable_font_embedding: bool,
    /// `kfxgen_open_at_cover`: no start landmark, so the book opens at its cover.
    pub open_at_cover: bool,
    /// `kfxgen_disable_approximate_pages`: no page numbers made up for a book without a page
    /// list.
    pub disable_approximate_pages: bool,
    /// `kfxgen_embed_original_images`: pictures as the book has them, not optimized.
    pub embed_original_images: bool,
}
