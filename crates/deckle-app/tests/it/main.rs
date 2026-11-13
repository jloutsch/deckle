// SPDX-License-Identifier: GPL-3.0-or-later
//! All integration tests in one binary: one link instead of one per file.

mod dispatch;
mod press;
// Unix only (see the module).
#[cfg(unix)]
mod state_files;
