// SPDX-License-Identifier: GPL-3.0-only
//! kfxgen's `font_table.py`: the weight and style readers a run's and a block's emphasis use.
use crate::generate::py;
use crate::pyvalue::PyValue;

/// `parse_weight`: a CSS font-weight as a number, 400 for none or one `int()` can't read.
/// Past an i64 it is `i64::MAX` or `i64::MIN` (`py::py_int`); Python's int has no bound.
pub fn parse_weight(v: Option<&PyValue>) -> i64 {
    match v {
        Some(PyValue::Str(s)) => match py::strip(s).to_lowercase().as_str() {
            "bold" => 700,
            "normal" => 400,
            s => py::py_int(s).unwrap_or(400),
        },
        // str() of an int reads back as it; of a float (always a point, an exponent, inf or
        // nan), a bool or None, never.
        Some(PyValue::Int(i)) => *i,
        _ => 400,
    }
}

/// `parse_style`: italic or oblique.
pub fn parse_style(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Str(s)) => {
            matches!(py::strip(s).to_lowercase().as_str(), "italic" | "oblique")
        }
        _ => false,
    }
}
