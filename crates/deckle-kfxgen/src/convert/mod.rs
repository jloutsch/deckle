// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's converter (`plugin/kfxgen/converter.py`), from the tree Calibre hands it to
//! `generate_full_book`'s arguments (phase 3).
//!
//! The submodules are private; what the tests in `tests/it/` and later `convert` need is
//! re-exported here, as `css` and `dom` do.

mod args;
mod href;
pub(crate) mod marks;
mod options;
mod package;

pub use args::book_args;
pub use href::{
    external_target, is_unsafe, key_doc, normalize, resolve_doc_path, resolve_link_target, unquote,
};
pub use options::ConvertOptions;
pub use package::{CalibreOpf, Nav, NavLink, Ncx};
pub(crate) use package::{item_path, kfxgen_paths};
