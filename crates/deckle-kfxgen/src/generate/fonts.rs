// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedded fonts (#15): the `FontTable` kfxgen is given (`font_table.py`), each face written
//! as its `$418` bytes and a `$262` `@font-face` (`native_generator.py:4816`), and `_family`
//! (:7233), the family a paragraph's, cell's or run's `$157` names in `$11`.
use std::sync::Arc;

use deckle_kfx::fragment::Fragment;
use deckle_kfx::ion::Value;

use super::{GenerateError, Generator, model_err, st, sym};
use crate::pyvalue::PyValue;

/// `font_table.BOLD_WEIGHT_THRESHOLD`: a face this heavy is bold.
const BOLD_WEIGHT_THRESHOLD: i64 = 600;

/// `inline_style._MONOSPACE_NAMES` (:461).
const MONOSPACE_NAMES: [&str; 20] = [
    "courier",
    "courier new",
    "consolas",
    "menlo",
    "monaco",
    "lucida console",
    "lucida sans typewriter",
    "andale mono",
    "dejavu sans mono",
    "liberation mono",
    "source code pro",
    "inconsolata",
    "fira mono",
    "fira code",
    "ubuntu mono",
    "roboto mono",
    "noto sans mono",
    "droid sans mono",
    "sf mono",
    "pt mono",
];

/// A `font_table.Face`.
struct Face {
    css_family: String,
    weight: i64,
    italic: bool,
    data: Arc<[u8]>,
    /// The `$11` naming it in `$262` and `$157`.
    emitted_family: String,
    /// Its `$418`'s id.
    location: String,
}

impl Face {
    fn read(v: &PyValue, at: &str) -> Result<Self, GenerateError> {
        if !matches!(v, PyValue::Obj { .. }) {
            return Err(model_err(format!("{at}: a Face")));
        }
        let field = |key: &str| v.get(key);
        // kfxgen makes symbols of the names, and kfxlib refuses an empty one.
        let text = |key: &str| {
            field(key)
                .and_then(PyValue::as_str)
                .filter(|s| key == "css_family" || !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| model_err(format!("{at}.{key}: a string, not empty")))
        };
        Ok(Face {
            css_family: text("css_family")?,
            weight: field("weight")
                .and_then(PyValue::as_int)
                .ok_or_else(|| model_err(format!("{at}.weight: an int")))?,
            italic: match field("italic") {
                Some(PyValue::Bool(b)) => *b,
                _ => return Err(model_err(format!("{at}.italic: a bool"))),
            },
            data: match field("data") {
                Some(PyValue::Bytes(b)) => b.clone(),
                _ => return Err(model_err(format!("{at}.data: bytes"))),
            },
            emitted_family: text("emitted_family")?,
            location: text("location")?,
        })
    }

    fn bold(&self) -> bool {
        self.weight >= BOLD_WEIGHT_THRESHOLD
    }
}

/// `font_table.FontTable`: the book's faces, in order.
#[derive(Default)]
pub(crate) struct FontTable {
    faces: Vec<Face>,
}

impl FontTable {
    /// `generate_full_book`'s `font_table` (:4672): None is a table of no faces.
    pub(crate) fn read(v: Option<&PyValue>) -> Result<Self, GenerateError> {
        let Some(table) = v.filter(|v| !matches!(v, PyValue::None)) else {
            return Ok(FontTable::default());
        };
        let faces = match table {
            PyValue::Obj { .. } => table.get("faces").and_then(PyValue::as_list),
            _ => None,
        }
        .ok_or_else(|| model_err("args.font_table: a FontTable of a faces list"))?;
        let faces = faces
            .iter()
            .enumerate()
            .map(|(i, f)| Face::read(f, &format!("args.font_table.faces[{i}]")))
            .collect::<Result<_, _>>()?;
        Ok(FontTable { faces })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// One `$418` and one `$262` per face (:4819), after the pictures' resources.
    pub(crate) fn build_fragments(&self, g: &mut Generator) {
        for face in &self.faces {
            // `build_fragment_418` (:2820).
            g.local(&face.location);
            g.push(Fragment::new(
                "$418",
                Some(&face.location),
                Value::Blob(face.data.to_vec()),
            ));
            // `build_fragment_262` (:2830): a root fragment, the family and location plain
            // strings; weight and style written only when not the default.
            g.local(&face.emitted_family);
            g.local(&face.location);
            let Value::Struct(mut value) = st([
                ("$11", Value::string(&face.emitted_family)),
                ("$165", Value::string(&face.location)),
            ]) else {
                unreachable!()
            };
            if face.bold() {
                value.insert("$13", sym("$361"));
            }
            if face.italic {
                value.insert("$12", sym("$382"));
            }
            g.push(Fragment::new("$262", Some("$262"), Value::Struct(value)));
        }
    }

    /// The `$418` locations `$419` lists after the pictures' (:5158).
    pub(crate) fn locations(&self) -> impl Iterator<Item = &String> {
        self.faces.iter().map(|f| &f.location)
    }

    /// `FontTable.match`: the face of the first listed family the book embeds, of the
    /// weight and slant asked for, else its regular face, else its first (#50).
    fn match_face(&self, families: &[String], bold: bool, italic: bool) -> Option<&str> {
        // `_by_family`: the faces of each family, in order.
        let fam_faces: Vec<&Face> = families.iter().find_map(|fam| {
            let faces: Vec<&Face> = self.faces.iter().filter(|f| &f.css_family == fam).collect();
            (!faces.is_empty()).then_some(faces)
        })?;
        let exact = fam_faces
            .iter()
            .find(|f| f.bold() == bold && f.italic == italic);
        let face = exact
            .or_else(|| fam_faces.iter().find(|f| !f.bold() && !f.italic))
            .unwrap_or(&fam_faces[0]);
        Some(&face.emitted_family)
    }

    /// `_family` (:7233): an embedded face wins; with none, monospace text is written
    /// "monospace", as Kindle Previewer writes it (#242).
    pub(crate) fn family(&self, families: &[String], bold: bool, italic: bool) -> Option<String> {
        match self.match_face(families, bold, italic) {
            Some(fam) => Some(fam.to_owned()),
            None => is_monospace(families).then(|| "monospace".to_owned()),
        }
    }
}

/// `inline_style.is_monospace` (:487): the generic `monospace` listed, or a known monospace
/// face first.
fn is_monospace(families: &[String]) -> bool {
    families.iter().any(|f| f == "monospace")
        || families
            .first()
            .is_some_and(|f| MONOSPACE_NAMES.contains(&f.as_str()))
}
