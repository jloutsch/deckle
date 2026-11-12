// SPDX-License-Identifier: GPL-3.0-only
//! EPUB → KFX without Kindle Previewer: a Rust port of kfxgen at commit `0bc705d`
//! (`docs/superpowers/specs/2026-11-10-native-kfx-design.md`).
//!
//! Source-line citations in comments (`:1234`) are at kfxgen `4b81651`, the previous pin,
//! unless the comment names another commit.

pub mod generate;
pub mod load;
pub mod pyvalue;
