// SPDX-License-Identifier: GPL-3.0-only
//! EPUB → KFX without Kindle Previewer: a Rust port of kfxgen at commit `ad38ec8`
//! (`docs/superpowers/specs/2026-11-10-native-kfx-design.md`).
//!
//! Source-line citations in comments (`:1234`) are at kfxgen `4b81651`, an earlier pin,
//! unless the comment names another commit.

pub mod book;
pub mod convert;
pub mod css;
pub mod dom;
pub mod epub;
pub mod generate;
pub mod load;
pub mod pyvalue;
