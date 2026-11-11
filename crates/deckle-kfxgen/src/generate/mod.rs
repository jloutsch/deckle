// SPDX-License-Identifier: GPL-3.0-or-later
//! kfxgen's generator, `NativeKFXGenerator.generate_full_book` (`native_generator.py:4549`):
//! a model's book to the bytes of a KFX, byte for byte as kfxgen writes them.
//!
//! What isn't ported yet is [`GenerateError::Unsupported`], so a book needing it fails rather
//! than coming out different from kfxgen's.
mod block_styles;
mod boxes;
mod chapters;
mod chunks;
mod fonts;
mod fragments;
mod frames;
mod images;
mod inline;
mod inline_picture_entries;
mod inline_pictures;
mod metadata;
mod nav;
mod notes;
mod pages;
mod positions;
mod py;
mod skeleton;
mod standard_symbols;
mod styles;
mod table_chunks;
mod table_styles;
mod tables;
mod text;
mod title;
mod writing;

use deckle_kfx::container::ContainerError;
use deckle_kfx::fragment::{Fragment, FragmentList};
use deckle_kfx::ion::{Decimal, Struct, Sym, Value};
use deckle_kfx::symtab::SymbolTable;

use crate::load::Model;
use standard_symbols::{STANDARD_SYMBOLS, YJ_SYMBOLS_MAX_ID};

#[derive(Debug)]
pub struct GenerateOutput {
    pub bytes: Vec<u8>,
    /// The fragments of the KFX in `bytes`, as `deckle_kfx::container::read_kfx` reads them.
    pub fragments: FragmentList,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("not ported from kfxgen yet: {0}")]
    Unsupported(&'static str),
    /// Where in the model, and what was expected there.
    #[error("model: {0}")]
    Model(String),
    #[error(transparent)]
    Container(#[from] ContainerError),
}

pub fn generate(model: &Model) -> Result<GenerateOutput, GenerateError> {
    skeleton::generate_full_book(&model.args)
}

/// kfxgen's `NativeKFXGenerator` state: the fragments built so far and the symbol table their
/// local symbols are created in.
pub struct Generator {
    pub(crate) fragments: FragmentList,
    pub(crate) symtab: SymbolTable,
    /// `self.font_table`: the book's embedded faces.
    pub(crate) fonts: fonts::FontTable,
    /// `self._math_strings` and `self._wrote_tcy`: 縦中横's texts and whether one was written.
    pub(crate) tcy: writing::TcyNotes,
}

impl Generator {
    /// `StandardSymbolTable()`: `YJ_symbols`, then the preloaded local symbols.
    fn new() -> Self {
        let mut symtab = SymbolTable::with_yj_max_id(YJ_SYMBOLS_MAX_ID);
        for name in STANDARD_SYMBOLS {
            symtab.create_local(name);
        }
        Generator {
            fragments: FragmentList::new(),
            symtab,
            fonts: fonts::FontTable::default(),
            tcy: writing::TcyNotes::default(),
        }
    }

    /// `self.symtab.create_local_symbol(name)`.
    fn local(&mut self, name: &str) {
        self.symtab.create_local(name);
    }

    fn push(&mut self, fragment: Fragment) {
        self.fragments.push(fragment);
    }
}

/// An Ion struct, its fields in order: kfxgen's `IonStruct(k1, v1, k2, v2, ...)`.
fn st<const N: usize>(fields: [(&str, Value); N]) -> Value {
    let mut s = Struct::new();
    for (key, value) in fields {
        s.insert(key, value);
    }
    Value::Struct(s)
}

fn sym(name: &str) -> Value {
    Value::sym(name)
}

/// `IonAnnotation([IS(annotation)], value)`.
fn annotated(annotation: &str, value: Value) -> Value {
    Value::Annotated {
        annots: vec![Sym::new(annotation)],
        value: Box::new(value),
    }
}

/// A measure: `IonStruct(IS("$307"), value, IS("$306"), IS(unit))`.
fn measure(value: Decimal, unit: &str) -> Value {
    st([("$307", Value::Decimal(value)), ("$306", sym(unit))])
}

/// `IonStruct(IS("$155"), position, IS("$143"), offset)`: a place in the text.
fn place(position: i64, offset: i64) -> Value {
    st([("$155", Value::Int(position)), ("$143", Value::Int(offset))])
}

fn model_err(what: impl Into<String>) -> GenerateError {
    GenerateError::Model(what.into())
}
