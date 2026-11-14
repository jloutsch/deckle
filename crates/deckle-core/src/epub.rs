// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal EPUB container access shared by the pipeline stages; the reader itself lives in
//! `deckle-kfx` so `deckle-kfxgen` can use it without `deckle-core`.

pub use deckle_kfx::epub_zip::*;
