// SPDX-License-Identifier: GPL-3.0-only
//! What one stylesheet and its imports may hold ([`MAX_STYLE_RULES`], [`MAX_STYLE_DECLS`]),
//! charged as the text is read, so a sheet past either limit is refused before the rest of it
//! is held in memory.

use std::cell::Cell;

use deckle_limits::{MAX_STYLE_DECLS, MAX_STYLE_RULES};

use super::Error;
use super::normalize::expanded_len;

/// Bytes of a stylesheet, declaration or selector that count as one more declaration.
pub(crate) const TEXT_UNIT: usize = 64;

/// The counts charged so far against [`MAX_STYLE_RULES`] and [`MAX_STYLE_DECLS`]. Shared by the
/// parsers of one [`super::parse_sheet`] call, so each charge is made where the item is read.
#[derive(Default)]
pub(crate) struct Budget {
    rules: Cell<usize>,
    decls: Cell<usize>,
}

impl Budget {
    /// Adds `rules` rules and `decls` declaration units; `false` once either count is past its
    /// limit (and from then on).
    pub(crate) fn charge(&self, rules: usize, decls: usize) -> bool {
        self.rules.set(self.rules.get().saturating_add(rules));
        self.decls.set(self.decls.get().saturating_add(decls));
        !self.over()
    }

    /// Charges a stylesheet's text, before it is read.
    pub(crate) fn text(&self, len: usize) -> bool {
        self.charge(0, len / TEXT_UNIT)
    }

    pub(crate) fn over(&self) -> bool {
        self.rules.get() > MAX_STYLE_RULES || self.decls.get() > MAX_STYLE_DECLS
    }

    pub(crate) fn error(&self) -> Error {
        Error::TooLarge(if self.decls.get() > MAX_STYLE_DECLS {
            MAX_STYLE_DECLS
        } else {
            MAX_STYLE_RULES
        })
    }
}

/// The units of a declaration `name: value`, `len` the bytes of its value: one for each
/// declaration it becomes when normalized, each once more for every [`TEXT_UNIT`] bytes, so
/// the normalized rules are bounded as well as the parsed ones.
pub(crate) fn decl_units(name: &str, len: usize) -> usize {
    expanded_len(name).saturating_mul(1 + name.len().saturating_add(len) / TEXT_UNIT)
}

#[cfg(test)]
mod tests {
    use cssparser::{Parser, ParserInput};

    use super::*;
    use crate::css::value::declarations;

    /// Each declaration is charged as it is read, before its value is written, and reading
    /// stops where the budget runs out, not after the block is held.
    #[test]
    fn declarations_stop_where_the_budget_runs_out() {
        let budget = Budget::default();
        assert!(budget.charge(0, MAX_STYLE_DECLS - 10));
        let text: String = (0..1000).map(|i| format!("a{i}:url(x);")).collect();
        let mut input = ParserInput::new(&text);
        let mut p = Parser::new(&mut input);
        let written = Cell::new(0);
        let decls = declarations(
            &mut p,
            Some(&budget),
            Some(&|u| {
                written.set(written.get() + 1);
                u.to_owned()
            }),
        );
        assert_eq!(written.get(), 10);
        assert!(decls.is_empty());
        assert!(budget.over());
        assert!(matches!(budget.error(), Error::TooLarge(MAX_STYLE_DECLS)));
    }
}
