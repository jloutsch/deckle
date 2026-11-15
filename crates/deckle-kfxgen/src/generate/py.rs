// SPDX-License-Identifier: GPL-3.0-only
//! Python's string and number semantics where kfxgen's output depends on them. Strings are
//! indexed by code point (`char`), as Python indexes them. Character classes are Unicode 16.0:
//! the tables below from Python 3.14's `unicodedata` (16.0.0, as calibre's), categories from the
//! `regex` crate (`DIVERGENCES.md`).
use std::sync::LazyLock;

use deckle_kfx::ion::Decimal;
use regex::Regex;

// The tables from here to `BIDI_STRONG` are the output of
// `python3.14 -I tools/ref/py_unicode_tables.py`, which regenerates them all.

/// `str.isspace()`: every character it is true for.
const SPACE: [char; 29] = [
    '\u{9}', '\u{A}', '\u{B}', '\u{C}', '\u{D}', '\u{1C}', '\u{1D}', '\u{1E}', '\u{1F}', '\u{20}',
    '\u{85}', '\u{A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
    '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}', '\u{2029}',
    '\u{202F}', '\u{205F}', '\u{3000}',
];

/// `str.isdigit()` beyond category Nd: superscripts, circled digits and the like.
const DIGIT_NOT_ND: [(u32, u32); 20] = [
    (0xB2, 0xB3),
    (0xB9, 0xB9),
    (0x1369, 0x1371),
    (0x19DA, 0x19DA),
    (0x2070, 0x2070),
    (0x2074, 0x2079),
    (0x2080, 0x2089),
    (0x2460, 0x2468),
    (0x2474, 0x247C),
    (0x2488, 0x2490),
    (0x24EA, 0x24EA),
    (0x24F5, 0x24FD),
    (0x24FF, 0x24FF),
    (0x2776, 0x277E),
    (0x2780, 0x2788),
    (0x278A, 0x2792),
    (0x10A40, 0x10A43),
    (0x10E60, 0x10E68),
    (0x11052, 0x1105A),
    (0x1F100, 0x1F10A),
];

/// `unicodedata.east_asian_width(c) in ("W", "F")`.
#[rustfmt::skip]
const EAST_ASIAN_WIDE: [(u32, u32); 122] = [
    (0x1100, 0x115F), (0x231A, 0x231B), (0x2329, 0x232A), (0x23E9, 0x23EC), (0x23F0, 0x23F0),
    (0x23F3, 0x23F3), (0x25FD, 0x25FE), (0x2614, 0x2615), (0x2630, 0x2637), (0x2648, 0x2653),
    (0x267F, 0x267F), (0x268A, 0x268F), (0x2693, 0x2693), (0x26A1, 0x26A1), (0x26AA, 0x26AB),
    (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE), (0x26D4, 0x26D4), (0x26EA, 0x26EA),
    (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD), (0x2705, 0x2705),
    (0x270A, 0x270B), (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E), (0x2753, 0x2755),
    (0x2757, 0x2757), (0x2795, 0x2797), (0x27B0, 0x27B0), (0x27BF, 0x27BF), (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x2E80, 0x2E99), (0x2E9B, 0x2EF3), (0x2F00, 0x2FD5),
    (0x2FF0, 0x303E), (0x3041, 0x3096), (0x3099, 0x30FF), (0x3105, 0x312F), (0x3131, 0x318E),
    (0x3190, 0x31E5), (0x31EF, 0x321E), (0x3220, 0x3247), (0x3250, 0xA48C), (0xA490, 0xA4C6),
    (0xA960, 0xA97C), (0xAC00, 0xD7A3), (0xF900, 0xFAFF), (0xFE10, 0xFE19), (0xFE30, 0xFE52),
    (0xFE54, 0xFE66), (0xFE68, 0xFE6B), (0xFF01, 0xFF60), (0xFFE0, 0xFFE6), (0x16FE0, 0x16FE4),
    (0x16FF0, 0x16FF1), (0x17000, 0x187F7), (0x18800, 0x18CD5), (0x18CFF, 0x18D08),
    (0x1AFF0, 0x1AFF3), (0x1AFF5, 0x1AFFB), (0x1AFFD, 0x1AFFE), (0x1B000, 0x1B122),
    (0x1B132, 0x1B132), (0x1B150, 0x1B152), (0x1B155, 0x1B155), (0x1B164, 0x1B167),
    (0x1B170, 0x1B2FB), (0x1D300, 0x1D356), (0x1D360, 0x1D376), (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF), (0x1F18E, 0x1F18E), (0x1F191, 0x1F19A), (0x1F200, 0x1F202),
    (0x1F210, 0x1F23B), (0x1F240, 0x1F248), (0x1F250, 0x1F251), (0x1F260, 0x1F265),
    (0x1F300, 0x1F320), (0x1F32D, 0x1F335), (0x1F337, 0x1F37C), (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA), (0x1F3CF, 0x1F3D3), (0x1F3E0, 0x1F3F0), (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E), (0x1F440, 0x1F440), (0x1F442, 0x1F4FC), (0x1F4FF, 0x1F53D),
    (0x1F54B, 0x1F54E), (0x1F550, 0x1F567), (0x1F57A, 0x1F57A), (0x1F595, 0x1F596),
    (0x1F5A4, 0x1F5A4), (0x1F5FB, 0x1F64F), (0x1F680, 0x1F6C5), (0x1F6CC, 0x1F6CC),
    (0x1F6D0, 0x1F6D2), (0x1F6D5, 0x1F6D7), (0x1F6DC, 0x1F6DF), (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC), (0x1F7E0, 0x1F7EB), (0x1F7F0, 0x1F7F0), (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945), (0x1F947, 0x1F9FF), (0x1FA70, 0x1FA7C), (0x1FA80, 0x1FA89),
    (0x1FA8F, 0x1FAC6), (0x1FACE, 0x1FADC), (0x1FADF, 0x1FAE9), (0x1FAF0, 0x1FAF8),
    (0x20000, 0x2FFFD), (0x30000, 0x3FFFD),
];

/// The zero of each run of ten decimal digits (`unicodedata.decimal(c) == 0`).
#[rustfmt::skip]
const DECIMAL_ZEROS: [u32; 76] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66, 0xDE6,
    0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50,
    0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10, 0x104A0,
    0x10D30, 0x10D40, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x116D0, 0x116DA, 0x11730, 0x118E0, 0x11950, 0x11BF0, 0x11C50, 0x11D50, 0x11DA0,
    0x11F50, 0x16130, 0x16A60, 0x16AC0, 0x16B50, 0x16D70, 0x1CCF0, 0x1D7CE, 0x1D7D8, 0x1D7E2,
    0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E5F1, 0x1E950, 0x1FBF0,
];

/// `unicodedata.bidirectional(c)` where it is L, R or AL: (first, last, class).
#[rustfmt::skip]
const BIDI_STRONG: [(u32, u32, Strong); 912] = [
    (0x41, 0x5A, L), (0x61, 0x7A, L), (0xAA, 0xAA, L), (0xB5, 0xB5, L), (0xBA, 0xBA, L),
    (0xC0, 0xD6, L), (0xD8, 0xF6, L), (0xF8, 0x2B8, L), (0x2BB, 0x2C1, L), (0x2D0, 0x2D1, L),
    (0x2E0, 0x2E4, L), (0x2EE, 0x2EE, L), (0x370, 0x373, L), (0x376, 0x377, L), (0x37A, 0x37D, L),
    (0x37F, 0x37F, L), (0x386, 0x386, L), (0x388, 0x38A, L), (0x38C, 0x38C, L), (0x38E, 0x3A1, L),
    (0x3A3, 0x3F5, L), (0x3F7, 0x482, L), (0x48A, 0x52F, L), (0x531, 0x556, L), (0x559, 0x589, L),
    (0x5BE, 0x5BE, R), (0x5C0, 0x5C0, R), (0x5C3, 0x5C3, R), (0x5C6, 0x5C6, R), (0x5D0, 0x5EA, R),
    (0x5EF, 0x5F4, R), (0x608, 0x608, Al), (0x60B, 0x60B, Al), (0x60D, 0x60D, Al),
    (0x61B, 0x64A, Al), (0x66D, 0x66F, Al), (0x671, 0x6D5, Al), (0x6E5, 0x6E6, Al),
    (0x6EE, 0x6EF, Al), (0x6FA, 0x70D, Al), (0x70F, 0x710, Al), (0x712, 0x72F, Al),
    (0x74D, 0x7A5, Al), (0x7B1, 0x7B1, Al), (0x7C0, 0x7EA, R), (0x7F4, 0x7F5, R), (0x7FA, 0x7FA, R),
    (0x7FE, 0x815, R), (0x81A, 0x81A, R), (0x824, 0x824, R), (0x828, 0x828, R), (0x830, 0x83E, R),
    (0x840, 0x858, R), (0x85E, 0x85E, R), (0x860, 0x86A, Al), (0x870, 0x88E, Al),
    (0x8A0, 0x8C9, Al), (0x903, 0x939, L), (0x93B, 0x93B, L), (0x93D, 0x940, L), (0x949, 0x94C, L),
    (0x94E, 0x950, L), (0x958, 0x961, L), (0x964, 0x980, L), (0x982, 0x983, L), (0x985, 0x98C, L),
    (0x98F, 0x990, L), (0x993, 0x9A8, L), (0x9AA, 0x9B0, L), (0x9B2, 0x9B2, L), (0x9B6, 0x9B9, L),
    (0x9BD, 0x9C0, L), (0x9C7, 0x9C8, L), (0x9CB, 0x9CC, L), (0x9CE, 0x9CE, L), (0x9D7, 0x9D7, L),
    (0x9DC, 0x9DD, L), (0x9DF, 0x9E1, L), (0x9E6, 0x9F1, L), (0x9F4, 0x9FA, L), (0x9FC, 0x9FD, L),
    (0xA03, 0xA03, L), (0xA05, 0xA0A, L), (0xA0F, 0xA10, L), (0xA13, 0xA28, L), (0xA2A, 0xA30, L),
    (0xA32, 0xA33, L), (0xA35, 0xA36, L), (0xA38, 0xA39, L), (0xA3E, 0xA40, L), (0xA59, 0xA5C, L),
    (0xA5E, 0xA5E, L), (0xA66, 0xA6F, L), (0xA72, 0xA74, L), (0xA76, 0xA76, L), (0xA83, 0xA83, L),
    (0xA85, 0xA8D, L), (0xA8F, 0xA91, L), (0xA93, 0xAA8, L), (0xAAA, 0xAB0, L), (0xAB2, 0xAB3, L),
    (0xAB5, 0xAB9, L), (0xABD, 0xAC0, L), (0xAC9, 0xAC9, L), (0xACB, 0xACC, L), (0xAD0, 0xAD0, L),
    (0xAE0, 0xAE1, L), (0xAE6, 0xAF0, L), (0xAF9, 0xAF9, L), (0xB02, 0xB03, L), (0xB05, 0xB0C, L),
    (0xB0F, 0xB10, L), (0xB13, 0xB28, L), (0xB2A, 0xB30, L), (0xB32, 0xB33, L), (0xB35, 0xB39, L),
    (0xB3D, 0xB3E, L), (0xB40, 0xB40, L), (0xB47, 0xB48, L), (0xB4B, 0xB4C, L), (0xB57, 0xB57, L),
    (0xB5C, 0xB5D, L), (0xB5F, 0xB61, L), (0xB66, 0xB77, L), (0xB83, 0xB83, L), (0xB85, 0xB8A, L),
    (0xB8E, 0xB90, L), (0xB92, 0xB95, L), (0xB99, 0xB9A, L), (0xB9C, 0xB9C, L), (0xB9E, 0xB9F, L),
    (0xBA3, 0xBA4, L), (0xBA8, 0xBAA, L), (0xBAE, 0xBB9, L), (0xBBE, 0xBBF, L), (0xBC1, 0xBC2, L),
    (0xBC6, 0xBC8, L), (0xBCA, 0xBCC, L), (0xBD0, 0xBD0, L), (0xBD7, 0xBD7, L), (0xBE6, 0xBF2, L),
    (0xC01, 0xC03, L), (0xC05, 0xC0C, L), (0xC0E, 0xC10, L), (0xC12, 0xC28, L), (0xC2A, 0xC39, L),
    (0xC3D, 0xC3D, L), (0xC41, 0xC44, L), (0xC58, 0xC5A, L), (0xC5D, 0xC5D, L), (0xC60, 0xC61, L),
    (0xC66, 0xC6F, L), (0xC77, 0xC77, L), (0xC7F, 0xC80, L), (0xC82, 0xC8C, L), (0xC8E, 0xC90, L),
    (0xC92, 0xCA8, L), (0xCAA, 0xCB3, L), (0xCB5, 0xCB9, L), (0xCBD, 0xCC4, L), (0xCC6, 0xCC8, L),
    (0xCCA, 0xCCB, L), (0xCD5, 0xCD6, L), (0xCDD, 0xCDE, L), (0xCE0, 0xCE1, L), (0xCE6, 0xCEF, L),
    (0xCF1, 0xCF3, L), (0xD02, 0xD0C, L), (0xD0E, 0xD10, L), (0xD12, 0xD3A, L), (0xD3D, 0xD40, L),
    (0xD46, 0xD48, L), (0xD4A, 0xD4C, L), (0xD4E, 0xD4F, L), (0xD54, 0xD61, L), (0xD66, 0xD7F, L),
    (0xD82, 0xD83, L), (0xD85, 0xD96, L), (0xD9A, 0xDB1, L), (0xDB3, 0xDBB, L), (0xDBD, 0xDBD, L),
    (0xDC0, 0xDC6, L), (0xDCF, 0xDD1, L), (0xDD8, 0xDDF, L), (0xDE6, 0xDEF, L), (0xDF2, 0xDF4, L),
    (0xE01, 0xE30, L), (0xE32, 0xE33, L), (0xE40, 0xE46, L), (0xE4F, 0xE5B, L), (0xE81, 0xE82, L),
    (0xE84, 0xE84, L), (0xE86, 0xE8A, L), (0xE8C, 0xEA3, L), (0xEA5, 0xEA5, L), (0xEA7, 0xEB0, L),
    (0xEB2, 0xEB3, L), (0xEBD, 0xEBD, L), (0xEC0, 0xEC4, L), (0xEC6, 0xEC6, L), (0xED0, 0xED9, L),
    (0xEDC, 0xEDF, L), (0xF00, 0xF17, L), (0xF1A, 0xF34, L), (0xF36, 0xF36, L), (0xF38, 0xF38, L),
    (0xF3E, 0xF47, L), (0xF49, 0xF6C, L), (0xF7F, 0xF7F, L), (0xF85, 0xF85, L), (0xF88, 0xF8C, L),
    (0xFBE, 0xFC5, L), (0xFC7, 0xFCC, L), (0xFCE, 0xFDA, L), (0x1000, 0x102C, L),
    (0x1031, 0x1031, L), (0x1038, 0x1038, L), (0x103B, 0x103C, L), (0x103F, 0x1057, L),
    (0x105A, 0x105D, L), (0x1061, 0x1070, L), (0x1075, 0x1081, L), (0x1083, 0x1084, L),
    (0x1087, 0x108C, L), (0x108E, 0x109C, L), (0x109E, 0x10C5, L), (0x10C7, 0x10C7, L),
    (0x10CD, 0x10CD, L), (0x10D0, 0x1248, L), (0x124A, 0x124D, L), (0x1250, 0x1256, L),
    (0x1258, 0x1258, L), (0x125A, 0x125D, L), (0x1260, 0x1288, L), (0x128A, 0x128D, L),
    (0x1290, 0x12B0, L), (0x12B2, 0x12B5, L), (0x12B8, 0x12BE, L), (0x12C0, 0x12C0, L),
    (0x12C2, 0x12C5, L), (0x12C8, 0x12D6, L), (0x12D8, 0x1310, L), (0x1312, 0x1315, L),
    (0x1318, 0x135A, L), (0x1360, 0x137C, L), (0x1380, 0x138F, L), (0x13A0, 0x13F5, L),
    (0x13F8, 0x13FD, L), (0x1401, 0x167F, L), (0x1681, 0x169A, L), (0x16A0, 0x16F8, L),
    (0x1700, 0x1711, L), (0x1715, 0x1715, L), (0x171F, 0x1731, L), (0x1734, 0x1736, L),
    (0x1740, 0x1751, L), (0x1760, 0x176C, L), (0x176E, 0x1770, L), (0x1780, 0x17B3, L),
    (0x17B6, 0x17B6, L), (0x17BE, 0x17C5, L), (0x17C7, 0x17C8, L), (0x17D4, 0x17DA, L),
    (0x17DC, 0x17DC, L), (0x17E0, 0x17E9, L), (0x1810, 0x1819, L), (0x1820, 0x1878, L),
    (0x1880, 0x1884, L), (0x1887, 0x18A8, L), (0x18AA, 0x18AA, L), (0x18B0, 0x18F5, L),
    (0x1900, 0x191E, L), (0x1923, 0x1926, L), (0x1929, 0x192B, L), (0x1930, 0x1931, L),
    (0x1933, 0x1938, L), (0x1946, 0x196D, L), (0x1970, 0x1974, L), (0x1980, 0x19AB, L),
    (0x19B0, 0x19C9, L), (0x19D0, 0x19DA, L), (0x1A00, 0x1A16, L), (0x1A19, 0x1A1A, L),
    (0x1A1E, 0x1A55, L), (0x1A57, 0x1A57, L), (0x1A61, 0x1A61, L), (0x1A63, 0x1A64, L),
    (0x1A6D, 0x1A72, L), (0x1A80, 0x1A89, L), (0x1A90, 0x1A99, L), (0x1AA0, 0x1AAD, L),
    (0x1B04, 0x1B33, L), (0x1B35, 0x1B35, L), (0x1B3B, 0x1B3B, L), (0x1B3D, 0x1B41, L),
    (0x1B43, 0x1B4C, L), (0x1B4E, 0x1B6A, L), (0x1B74, 0x1B7F, L), (0x1B82, 0x1BA1, L),
    (0x1BA6, 0x1BA7, L), (0x1BAA, 0x1BAA, L), (0x1BAE, 0x1BE5, L), (0x1BE7, 0x1BE7, L),
    (0x1BEA, 0x1BEC, L), (0x1BEE, 0x1BEE, L), (0x1BF2, 0x1BF3, L), (0x1BFC, 0x1C2B, L),
    (0x1C34, 0x1C35, L), (0x1C3B, 0x1C49, L), (0x1C4D, 0x1C8A, L), (0x1C90, 0x1CBA, L),
    (0x1CBD, 0x1CC7, L), (0x1CD3, 0x1CD3, L), (0x1CE1, 0x1CE1, L), (0x1CE9, 0x1CEC, L),
    (0x1CEE, 0x1CF3, L), (0x1CF5, 0x1CF7, L), (0x1CFA, 0x1CFA, L), (0x1D00, 0x1DBF, L),
    (0x1E00, 0x1F15, L), (0x1F18, 0x1F1D, L), (0x1F20, 0x1F45, L), (0x1F48, 0x1F4D, L),
    (0x1F50, 0x1F57, L), (0x1F59, 0x1F59, L), (0x1F5B, 0x1F5B, L), (0x1F5D, 0x1F5D, L),
    (0x1F5F, 0x1F7D, L), (0x1F80, 0x1FB4, L), (0x1FB6, 0x1FBC, L), (0x1FBE, 0x1FBE, L),
    (0x1FC2, 0x1FC4, L), (0x1FC6, 0x1FCC, L), (0x1FD0, 0x1FD3, L), (0x1FD6, 0x1FDB, L),
    (0x1FE0, 0x1FEC, L), (0x1FF2, 0x1FF4, L), (0x1FF6, 0x1FFC, L), (0x200E, 0x200E, L),
    (0x200F, 0x200F, R), (0x2071, 0x2071, L), (0x207F, 0x207F, L), (0x2090, 0x209C, L),
    (0x2102, 0x2102, L), (0x2107, 0x2107, L), (0x210A, 0x2113, L), (0x2115, 0x2115, L),
    (0x2119, 0x211D, L), (0x2124, 0x2124, L), (0x2126, 0x2126, L), (0x2128, 0x2128, L),
    (0x212A, 0x212D, L), (0x212F, 0x2139, L), (0x213C, 0x213F, L), (0x2145, 0x2149, L),
    (0x214E, 0x214F, L), (0x2160, 0x2188, L), (0x2336, 0x237A, L), (0x2395, 0x2395, L),
    (0x249C, 0x24E9, L), (0x26AC, 0x26AC, L), (0x2800, 0x28FF, L), (0x2C00, 0x2CE4, L),
    (0x2CEB, 0x2CEE, L), (0x2CF2, 0x2CF3, L), (0x2D00, 0x2D25, L), (0x2D27, 0x2D27, L),
    (0x2D2D, 0x2D2D, L), (0x2D30, 0x2D67, L), (0x2D6F, 0x2D70, L), (0x2D80, 0x2D96, L),
    (0x2DA0, 0x2DA6, L), (0x2DA8, 0x2DAE, L), (0x2DB0, 0x2DB6, L), (0x2DB8, 0x2DBE, L),
    (0x2DC0, 0x2DC6, L), (0x2DC8, 0x2DCE, L), (0x2DD0, 0x2DD6, L), (0x2DD8, 0x2DDE, L),
    (0x3005, 0x3007, L), (0x3021, 0x3029, L), (0x302E, 0x302F, L), (0x3031, 0x3035, L),
    (0x3038, 0x303C, L), (0x3041, 0x3096, L), (0x309D, 0x309F, L), (0x30A1, 0x30FA, L),
    (0x30FC, 0x30FF, L), (0x3105, 0x312F, L), (0x3131, 0x318E, L), (0x3190, 0x31BF, L),
    (0x31F0, 0x321C, L), (0x3220, 0x324F, L), (0x3260, 0x327B, L), (0x327F, 0x32B0, L),
    (0x32C0, 0x32CB, L), (0x32D0, 0x3376, L), (0x337B, 0x33DD, L), (0x33E0, 0x33FE, L),
    (0x3400, 0x4DBF, L), (0x4E00, 0xA48C, L), (0xA4D0, 0xA60C, L), (0xA610, 0xA62B, L),
    (0xA640, 0xA66E, L), (0xA680, 0xA69D, L), (0xA6A0, 0xA6EF, L), (0xA6F2, 0xA6F7, L),
    (0xA722, 0xA787, L), (0xA789, 0xA7CD, L), (0xA7D0, 0xA7D1, L), (0xA7D3, 0xA7D3, L),
    (0xA7D5, 0xA7DC, L), (0xA7F2, 0xA801, L), (0xA803, 0xA805, L), (0xA807, 0xA80A, L),
    (0xA80C, 0xA824, L), (0xA827, 0xA827, L), (0xA830, 0xA837, L), (0xA840, 0xA873, L),
    (0xA880, 0xA8C3, L), (0xA8CE, 0xA8D9, L), (0xA8F2, 0xA8FE, L), (0xA900, 0xA925, L),
    (0xA92E, 0xA946, L), (0xA952, 0xA953, L), (0xA95F, 0xA97C, L), (0xA983, 0xA9B2, L),
    (0xA9B4, 0xA9B5, L), (0xA9BA, 0xA9BB, L), (0xA9BE, 0xA9CD, L), (0xA9CF, 0xA9D9, L),
    (0xA9DE, 0xA9E4, L), (0xA9E6, 0xA9FE, L), (0xAA00, 0xAA28, L), (0xAA2F, 0xAA30, L),
    (0xAA33, 0xAA34, L), (0xAA40, 0xAA42, L), (0xAA44, 0xAA4B, L), (0xAA4D, 0xAA4D, L),
    (0xAA50, 0xAA59, L), (0xAA5C, 0xAA7B, L), (0xAA7D, 0xAAAF, L), (0xAAB1, 0xAAB1, L),
    (0xAAB5, 0xAAB6, L), (0xAAB9, 0xAABD, L), (0xAAC0, 0xAAC0, L), (0xAAC2, 0xAAC2, L),
    (0xAADB, 0xAAEB, L), (0xAAEE, 0xAAF5, L), (0xAB01, 0xAB06, L), (0xAB09, 0xAB0E, L),
    (0xAB11, 0xAB16, L), (0xAB20, 0xAB26, L), (0xAB28, 0xAB2E, L), (0xAB30, 0xAB69, L),
    (0xAB70, 0xABE4, L), (0xABE6, 0xABE7, L), (0xABE9, 0xABEC, L), (0xABF0, 0xABF9, L),
    (0xAC00, 0xD7A3, L), (0xD7B0, 0xD7C6, L), (0xD7CB, 0xD7FB, L), (0xD800, 0xFA6D, L),
    (0xFA70, 0xFAD9, L), (0xFB00, 0xFB06, L), (0xFB13, 0xFB17, L), (0xFB1D, 0xFB1D, R),
    (0xFB1F, 0xFB28, R), (0xFB2A, 0xFB36, R), (0xFB38, 0xFB3C, R), (0xFB3E, 0xFB3E, R),
    (0xFB40, 0xFB41, R), (0xFB43, 0xFB44, R), (0xFB46, 0xFB4F, R), (0xFB50, 0xFBC2, Al),
    (0xFBD3, 0xFD3D, Al), (0xFD50, 0xFD8F, Al), (0xFD92, 0xFDC7, Al), (0xFDF0, 0xFDFC, Al),
    (0xFE70, 0xFE74, Al), (0xFE76, 0xFEFC, Al), (0xFF21, 0xFF3A, L), (0xFF41, 0xFF5A, L),
    (0xFF66, 0xFFBE, L), (0xFFC2, 0xFFC7, L), (0xFFCA, 0xFFCF, L), (0xFFD2, 0xFFD7, L),
    (0xFFDA, 0xFFDC, L), (0x10000, 0x1000B, L), (0x1000D, 0x10026, L), (0x10028, 0x1003A, L),
    (0x1003C, 0x1003D, L), (0x1003F, 0x1004D, L), (0x10050, 0x1005D, L), (0x10080, 0x100FA, L),
    (0x10100, 0x10100, L), (0x10102, 0x10102, L), (0x10107, 0x10133, L), (0x10137, 0x1013F, L),
    (0x1018D, 0x1018E, L), (0x101D0, 0x101FC, L), (0x10280, 0x1029C, L), (0x102A0, 0x102D0, L),
    (0x10300, 0x10323, L), (0x1032D, 0x1034A, L), (0x10350, 0x10375, L), (0x10380, 0x1039D, L),
    (0x1039F, 0x103C3, L), (0x103C8, 0x103D5, L), (0x10400, 0x1049D, L), (0x104A0, 0x104A9, L),
    (0x104B0, 0x104D3, L), (0x104D8, 0x104FB, L), (0x10500, 0x10527, L), (0x10530, 0x10563, L),
    (0x1056F, 0x1057A, L), (0x1057C, 0x1058A, L), (0x1058C, 0x10592, L), (0x10594, 0x10595, L),
    (0x10597, 0x105A1, L), (0x105A3, 0x105B1, L), (0x105B3, 0x105B9, L), (0x105BB, 0x105BC, L),
    (0x105C0, 0x105F3, L), (0x10600, 0x10736, L), (0x10740, 0x10755, L), (0x10760, 0x10767, L),
    (0x10780, 0x10785, L), (0x10787, 0x107B0, L), (0x107B2, 0x107BA, L), (0x10800, 0x10805, R),
    (0x10808, 0x10808, R), (0x1080A, 0x10835, R), (0x10837, 0x10838, R), (0x1083C, 0x1083C, R),
    (0x1083F, 0x10855, R), (0x10857, 0x1089E, R), (0x108A7, 0x108AF, R), (0x108E0, 0x108F2, R),
    (0x108F4, 0x108F5, R), (0x108FB, 0x1091B, R), (0x10920, 0x10939, R), (0x1093F, 0x1093F, R),
    (0x10980, 0x109B7, R), (0x109BC, 0x109CF, R), (0x109D2, 0x10A00, R), (0x10A10, 0x10A13, R),
    (0x10A15, 0x10A17, R), (0x10A19, 0x10A35, R), (0x10A40, 0x10A48, R), (0x10A50, 0x10A58, R),
    (0x10A60, 0x10A9F, R), (0x10AC0, 0x10AE4, R), (0x10AEB, 0x10AF6, R), (0x10B00, 0x10B35, R),
    (0x10B40, 0x10B55, R), (0x10B58, 0x10B72, R), (0x10B78, 0x10B91, R), (0x10B99, 0x10B9C, R),
    (0x10BA9, 0x10BAF, R), (0x10C00, 0x10C48, R), (0x10C80, 0x10CB2, R), (0x10CC0, 0x10CF2, R),
    (0x10CFA, 0x10CFF, R), (0x10D00, 0x10D23, Al), (0x10D4A, 0x10D65, R), (0x10D6F, 0x10D85, R),
    (0x10D8E, 0x10D8F, R), (0x10E80, 0x10EA9, R), (0x10EAD, 0x10EAD, R), (0x10EB0, 0x10EB1, R),
    (0x10EC2, 0x10EC4, Al), (0x10F00, 0x10F27, R), (0x10F30, 0x10F45, Al), (0x10F51, 0x10F59, Al),
    (0x10F70, 0x10F81, R), (0x10F86, 0x10F89, R), (0x10FB0, 0x10FCB, R), (0x10FE0, 0x10FF6, R),
    (0x11000, 0x11000, L), (0x11002, 0x11037, L), (0x11047, 0x1104D, L), (0x11066, 0x1106F, L),
    (0x11071, 0x11072, L), (0x11075, 0x11075, L), (0x11082, 0x110B2, L), (0x110B7, 0x110B8, L),
    (0x110BB, 0x110C1, L), (0x110CD, 0x110CD, L), (0x110D0, 0x110E8, L), (0x110F0, 0x110F9, L),
    (0x11103, 0x11126, L), (0x1112C, 0x1112C, L), (0x11136, 0x11147, L), (0x11150, 0x11172, L),
    (0x11174, 0x11176, L), (0x11182, 0x111B5, L), (0x111BF, 0x111C8, L), (0x111CD, 0x111CE, L),
    (0x111D0, 0x111DF, L), (0x111E1, 0x111F4, L), (0x11200, 0x11211, L), (0x11213, 0x1122E, L),
    (0x11232, 0x11233, L), (0x11235, 0x11235, L), (0x11238, 0x1123D, L), (0x1123F, 0x11240, L),
    (0x11280, 0x11286, L), (0x11288, 0x11288, L), (0x1128A, 0x1128D, L), (0x1128F, 0x1129D, L),
    (0x1129F, 0x112A9, L), (0x112B0, 0x112DE, L), (0x112E0, 0x112E2, L), (0x112F0, 0x112F9, L),
    (0x11302, 0x11303, L), (0x11305, 0x1130C, L), (0x1130F, 0x11310, L), (0x11313, 0x11328, L),
    (0x1132A, 0x11330, L), (0x11332, 0x11333, L), (0x11335, 0x11339, L), (0x1133D, 0x1133F, L),
    (0x11341, 0x11344, L), (0x11347, 0x11348, L), (0x1134B, 0x1134D, L), (0x11350, 0x11350, L),
    (0x11357, 0x11357, L), (0x1135D, 0x11363, L), (0x11380, 0x11389, L), (0x1138B, 0x1138B, L),
    (0x1138E, 0x1138E, L), (0x11390, 0x113B5, L), (0x113B7, 0x113BA, L), (0x113C2, 0x113C2, L),
    (0x113C5, 0x113C5, L), (0x113C7, 0x113CA, L), (0x113CC, 0x113CD, L), (0x113CF, 0x113CF, L),
    (0x113D1, 0x113D1, L), (0x113D3, 0x113D5, L), (0x113D7, 0x113D8, L), (0x11400, 0x11437, L),
    (0x11440, 0x11441, L), (0x11445, 0x11445, L), (0x11447, 0x1145B, L), (0x1145D, 0x1145D, L),
    (0x1145F, 0x11461, L), (0x11480, 0x114B2, L), (0x114B9, 0x114B9, L), (0x114BB, 0x114BE, L),
    (0x114C1, 0x114C1, L), (0x114C4, 0x114C7, L), (0x114D0, 0x114D9, L), (0x11580, 0x115B1, L),
    (0x115B8, 0x115BB, L), (0x115BE, 0x115BE, L), (0x115C1, 0x115DB, L), (0x11600, 0x11632, L),
    (0x1163B, 0x1163C, L), (0x1163E, 0x1163E, L), (0x11641, 0x11644, L), (0x11650, 0x11659, L),
    (0x11680, 0x116AA, L), (0x116AC, 0x116AC, L), (0x116AE, 0x116AF, L), (0x116B6, 0x116B6, L),
    (0x116B8, 0x116B9, L), (0x116C0, 0x116C9, L), (0x116D0, 0x116E3, L), (0x11700, 0x1171A, L),
    (0x1171E, 0x1171E, L), (0x11720, 0x11721, L), (0x11726, 0x11726, L), (0x11730, 0x11746, L),
    (0x11800, 0x1182E, L), (0x11838, 0x11838, L), (0x1183B, 0x1183B, L), (0x118A0, 0x118F2, L),
    (0x118FF, 0x11906, L), (0x11909, 0x11909, L), (0x1190C, 0x11913, L), (0x11915, 0x11916, L),
    (0x11918, 0x11935, L), (0x11937, 0x11938, L), (0x1193D, 0x1193D, L), (0x1193F, 0x11942, L),
    (0x11944, 0x11946, L), (0x11950, 0x11959, L), (0x119A0, 0x119A7, L), (0x119AA, 0x119D3, L),
    (0x119DC, 0x119DF, L), (0x119E1, 0x119E4, L), (0x11A00, 0x11A00, L), (0x11A07, 0x11A08, L),
    (0x11A0B, 0x11A32, L), (0x11A39, 0x11A3A, L), (0x11A3F, 0x11A46, L), (0x11A50, 0x11A50, L),
    (0x11A57, 0x11A58, L), (0x11A5C, 0x11A89, L), (0x11A97, 0x11A97, L), (0x11A9A, 0x11AA2, L),
    (0x11AB0, 0x11AF8, L), (0x11B00, 0x11B09, L), (0x11BC0, 0x11BE1, L), (0x11BF0, 0x11BF9, L),
    (0x11C00, 0x11C08, L), (0x11C0A, 0x11C2F, L), (0x11C3E, 0x11C45, L), (0x11C50, 0x11C6C, L),
    (0x11C70, 0x11C8F, L), (0x11CA9, 0x11CA9, L), (0x11CB1, 0x11CB1, L), (0x11CB4, 0x11CB4, L),
    (0x11D00, 0x11D06, L), (0x11D08, 0x11D09, L), (0x11D0B, 0x11D30, L), (0x11D46, 0x11D46, L),
    (0x11D50, 0x11D59, L), (0x11D60, 0x11D65, L), (0x11D67, 0x11D68, L), (0x11D6A, 0x11D8E, L),
    (0x11D93, 0x11D94, L), (0x11D96, 0x11D96, L), (0x11D98, 0x11D98, L), (0x11DA0, 0x11DA9, L),
    (0x11EE0, 0x11EF2, L), (0x11EF5, 0x11EF8, L), (0x11F02, 0x11F10, L), (0x11F12, 0x11F35, L),
    (0x11F3E, 0x11F3F, L), (0x11F41, 0x11F41, L), (0x11F43, 0x11F59, L), (0x11FB0, 0x11FB0, L),
    (0x11FC0, 0x11FD4, L), (0x11FFF, 0x12399, L), (0x12400, 0x1246E, L), (0x12470, 0x12474, L),
    (0x12480, 0x12543, L), (0x12F90, 0x12FF2, L), (0x13000, 0x1343F, L), (0x13441, 0x13446, L),
    (0x13460, 0x143FA, L), (0x14400, 0x14646, L), (0x16100, 0x1611D, L), (0x1612A, 0x1612C, L),
    (0x16130, 0x16139, L), (0x16800, 0x16A38, L), (0x16A40, 0x16A5E, L), (0x16A60, 0x16A69, L),
    (0x16A6E, 0x16ABE, L), (0x16AC0, 0x16AC9, L), (0x16AD0, 0x16AED, L), (0x16AF5, 0x16AF5, L),
    (0x16B00, 0x16B2F, L), (0x16B37, 0x16B45, L), (0x16B50, 0x16B59, L), (0x16B5B, 0x16B61, L),
    (0x16B63, 0x16B77, L), (0x16B7D, 0x16B8F, L), (0x16D40, 0x16D79, L), (0x16E40, 0x16E9A, L),
    (0x16F00, 0x16F4A, L), (0x16F50, 0x16F87, L), (0x16F93, 0x16F9F, L), (0x16FE0, 0x16FE1, L),
    (0x16FE3, 0x16FE3, L), (0x16FF0, 0x16FF1, L), (0x17000, 0x187F7, L), (0x18800, 0x18CD5, L),
    (0x18CFF, 0x18D08, L), (0x1AFF0, 0x1AFF3, L), (0x1AFF5, 0x1AFFB, L), (0x1AFFD, 0x1AFFE, L),
    (0x1B000, 0x1B122, L), (0x1B132, 0x1B132, L), (0x1B150, 0x1B152, L), (0x1B155, 0x1B155, L),
    (0x1B164, 0x1B167, L), (0x1B170, 0x1B2FB, L), (0x1BC00, 0x1BC6A, L), (0x1BC70, 0x1BC7C, L),
    (0x1BC80, 0x1BC88, L), (0x1BC90, 0x1BC99, L), (0x1BC9C, 0x1BC9C, L), (0x1BC9F, 0x1BC9F, L),
    (0x1CCD6, 0x1CCEF, L), (0x1CF50, 0x1CFC3, L), (0x1D000, 0x1D0F5, L), (0x1D100, 0x1D126, L),
    (0x1D129, 0x1D166, L), (0x1D16A, 0x1D172, L), (0x1D183, 0x1D184, L), (0x1D18C, 0x1D1A9, L),
    (0x1D1AE, 0x1D1E8, L), (0x1D2C0, 0x1D2D3, L), (0x1D2E0, 0x1D2F3, L), (0x1D360, 0x1D378, L),
    (0x1D400, 0x1D454, L), (0x1D456, 0x1D49C, L), (0x1D49E, 0x1D49F, L), (0x1D4A2, 0x1D4A2, L),
    (0x1D4A5, 0x1D4A6, L), (0x1D4A9, 0x1D4AC, L), (0x1D4AE, 0x1D4B9, L), (0x1D4BB, 0x1D4BB, L),
    (0x1D4BD, 0x1D4C3, L), (0x1D4C5, 0x1D505, L), (0x1D507, 0x1D50A, L), (0x1D50D, 0x1D514, L),
    (0x1D516, 0x1D51C, L), (0x1D51E, 0x1D539, L), (0x1D53B, 0x1D53E, L), (0x1D540, 0x1D544, L),
    (0x1D546, 0x1D546, L), (0x1D54A, 0x1D550, L), (0x1D552, 0x1D6A5, L), (0x1D6A8, 0x1D6C0, L),
    (0x1D6C2, 0x1D6DA, L), (0x1D6DC, 0x1D6FA, L), (0x1D6FC, 0x1D714, L), (0x1D716, 0x1D734, L),
    (0x1D736, 0x1D74E, L), (0x1D750, 0x1D76E, L), (0x1D770, 0x1D788, L), (0x1D78A, 0x1D7A8, L),
    (0x1D7AA, 0x1D7C2, L), (0x1D7C4, 0x1D7CB, L), (0x1D800, 0x1D9FF, L), (0x1DA37, 0x1DA3A, L),
    (0x1DA6D, 0x1DA74, L), (0x1DA76, 0x1DA83, L), (0x1DA85, 0x1DA8B, L), (0x1DF00, 0x1DF1E, L),
    (0x1DF25, 0x1DF2A, L), (0x1E030, 0x1E06D, L), (0x1E100, 0x1E12C, L), (0x1E137, 0x1E13D, L),
    (0x1E140, 0x1E149, L), (0x1E14E, 0x1E14F, L), (0x1E290, 0x1E2AD, L), (0x1E2C0, 0x1E2EB, L),
    (0x1E2F0, 0x1E2F9, L), (0x1E4D0, 0x1E4EB, L), (0x1E4F0, 0x1E4F9, L), (0x1E5D0, 0x1E5ED, L),
    (0x1E5F0, 0x1E5FA, L), (0x1E5FF, 0x1E5FF, L), (0x1E7E0, 0x1E7E6, L), (0x1E7E8, 0x1E7EB, L),
    (0x1E7ED, 0x1E7EE, L), (0x1E7F0, 0x1E7FE, L), (0x1E800, 0x1E8C4, R), (0x1E8C7, 0x1E8CF, R),
    (0x1E900, 0x1E943, R), (0x1E94B, 0x1E94B, R), (0x1E950, 0x1E959, R), (0x1E95E, 0x1E95F, R),
    (0x1EC71, 0x1ECB4, Al), (0x1ED01, 0x1ED3D, Al), (0x1EE00, 0x1EE03, Al), (0x1EE05, 0x1EE1F, Al),
    (0x1EE21, 0x1EE22, Al), (0x1EE24, 0x1EE24, Al), (0x1EE27, 0x1EE27, Al), (0x1EE29, 0x1EE32, Al),
    (0x1EE34, 0x1EE37, Al), (0x1EE39, 0x1EE39, Al), (0x1EE3B, 0x1EE3B, Al), (0x1EE42, 0x1EE42, Al),
    (0x1EE47, 0x1EE47, Al), (0x1EE49, 0x1EE49, Al), (0x1EE4B, 0x1EE4B, Al), (0x1EE4D, 0x1EE4F, Al),
    (0x1EE51, 0x1EE52, Al), (0x1EE54, 0x1EE54, Al), (0x1EE57, 0x1EE57, Al), (0x1EE59, 0x1EE59, Al),
    (0x1EE5B, 0x1EE5B, Al), (0x1EE5D, 0x1EE5D, Al), (0x1EE5F, 0x1EE5F, Al), (0x1EE61, 0x1EE62, Al),
    (0x1EE64, 0x1EE64, Al), (0x1EE67, 0x1EE6A, Al), (0x1EE6C, 0x1EE72, Al), (0x1EE74, 0x1EE77, Al),
    (0x1EE79, 0x1EE7C, Al), (0x1EE7E, 0x1EE7E, Al), (0x1EE80, 0x1EE89, Al), (0x1EE8B, 0x1EE9B, Al),
    (0x1EEA1, 0x1EEA3, Al), (0x1EEA5, 0x1EEA9, Al), (0x1EEAB, 0x1EEBB, Al), (0x1F110, 0x1F12E, L),
    (0x1F130, 0x1F169, L), (0x1F170, 0x1F1AC, L), (0x1F1E6, 0x1F202, L), (0x1F210, 0x1F23B, L),
    (0x1F240, 0x1F248, L), (0x1F250, 0x1F251, L), (0x20000, 0x2A6DF, L), (0x2A700, 0x2B739, L),
    (0x2B740, 0x2B81D, L), (0x2B820, 0x2CEA1, L), (0x2CEB0, 0x2EBE0, L), (0x2EBF0, 0x2EE5D, L),
    (0x2F800, 0x2FA1D, L), (0x30000, 0x3134A, L), (0x31350, 0x323AF, L), (0xF0000, 0xFFFFD, L),
    (0x100000, 0x10FFFD, L),
];

fn in_ranges(c: char, ranges: &[(u32, u32)]) -> bool {
    let c = u32::from(c);
    ranges.iter().any(|&(a, b)| (a..=b).contains(&c))
}

fn matches(re: &Regex, c: char) -> bool {
    re.is_match(c.encode_utf8(&mut [0; 4]))
}

/// Python's `isalpha` is exactly category L, and `isalnum` L or N.
static ALPHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").unwrap());
static ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}]$").unwrap());
static DIGIT_ND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Nd}$").unwrap());

pub(crate) fn is_space(c: char) -> bool {
    SPACE.contains(&c)
}

pub(crate) fn is_alpha(c: char) -> bool {
    matches(&ALPHA, c)
}

pub(crate) fn is_alnum(c: char) -> bool {
    matches(&ALNUM, c)
}

pub(crate) fn is_digit(c: char) -> bool {
    matches(&DIGIT_ND, c) || in_ranges(c, &DIGIT_NOT_ND)
}

pub(crate) fn is_east_asian_wide(c: char) -> bool {
    in_ranges(c, &EAST_ASIAN_WIDE)
}

/// `unicodedata.decimal(c, None)`: the value of a decimal digit (category Nd, what `\d`
/// matches and `int()` and `float()` read), else None.
pub(crate) fn digit_value(c: char) -> Option<u32> {
    let c = u32::from(c);
    let i = DECIMAL_ZEROS.partition_point(|&zero| zero <= c);
    let zero = DECIMAL_ZEROS[..i].last()?;
    Some(c - zero).filter(|&v| v < 10)
}

/// The strong classes of `unicodedata.bidirectional`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strong {
    L,
    R,
    Al,
}
use Strong::{Al, L, R};

/// `unicodedata.bidirectional(c)` when it is L, R or AL, else None.
pub(crate) fn bidi_strong(c: char) -> Option<Strong> {
    let c = u32::from(c);
    let i = BIDI_STRONG.partition_point(|&(first, _, _)| first <= c);
    BIDI_STRONG[..i]
        .last()
        .filter(|&&(_, last, _)| c <= last)
        .map(|&(_, _, class)| class)
}

/// The white space `int()` and `float()` take around a number: `isspace`, but for U+001C to
/// U+001F, which they read as ASCII and refuse.
fn is_number_space(c: char) -> bool {
    is_space(c) && !('\u{1c}'..='\u{1f}').contains(&c)
}

/// `int(s)`, base 10: white space around, one sign, decimal digits of any script with single
/// underscores between them, at most 4300 digits (`sys.int_info.default_max_str_digits`;
/// more raise). None where Python raises `ValueError`. Python's int has no bound; past an
/// i64 this gives `i64::MAX` or `i64::MIN`. Linear in `s`.
pub(crate) fn py_int(s: &str) -> Option<i64> {
    let s = s.trim_matches(is_number_space);
    let (negative, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let mut value: i64 = 0;
    let mut count = 0usize;
    let mut after_digit = false;
    for c in digits.chars() {
        if c == '_' {
            // Only between two digits.
            if !after_digit {
                return None;
            }
            after_digit = false;
            continue;
        }
        let d = i64::from(digit_value(c)?);
        count += 1;
        after_digit = true;
        value = value
            .saturating_mul(10)
            .saturating_add(if negative { -d } else { d });
    }
    (after_digit && count <= 4300).then_some(value)
}

/// `float(s)`: white space around (not U+001C to U+001F), decimal digits of any script, single
/// underscores between digits, a sign, a point, an exponent, `inf`, `infinity` and `nan` in any
/// case. None where Python raises `ValueError`. Linear in `s`.
pub(crate) fn py_float(s: &str) -> Option<f64> {
    let mut ascii = String::with_capacity(s.len());
    // CPython makes the digits ASCII first, then allows `_` only between two digits
    // (`_Py_string_to_number_with_underscores`).
    let mut prev = '\0';
    for c in s.trim_matches(is_number_space).chars() {
        let c = digit_value(c).map_or(c, |d| char::from(b'0' + d as u8));
        if c == '_' {
            if !prev.is_ascii_digit() {
                return None;
            }
        } else {
            if prev == '_' && !c.is_ascii_digit() {
                return None;
            }
            if !(c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-')) {
                return None;
            }
            ascii.push(c);
        }
        prev = c;
    }
    if prev == '_' {
        return None;
    }
    // Rust reads the forms Python's `float()` reads from ASCII, and no others.
    ascii.parse().ok()
}

/// `s.strip()`.
pub(crate) fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.lstrip()`.
pub(crate) fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `"".join(s.split())`.
pub(crate) fn without_spaces(s: &str) -> String {
    s.chars().filter(|&c| !is_space(c)).collect()
}

/// `len(s)`.
pub(crate) fn len(s: &str) -> usize {
    s.chars().count()
}

/// `s[n:]`.
pub(crate) fn skip(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or("", |(i, _)| &s[i..])
}

/// `s[:n]`.
pub(crate) fn take(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// `s[a:b]` and `s[a:]` for slices taken one after another: each is found from where the
/// last began, not from the start of `s`.
pub(crate) struct Slicer<'a> {
    s: &'a str,
    /// The start of the last slice, in characters and in bytes.
    at: (usize, usize),
}

impl<'a> Slicer<'a> {
    pub(crate) fn new(s: &'a str) -> Self {
        Slicer { s, at: (0, 0) }
    }

    /// `s[a:]`.
    pub(crate) fn from(&mut self, a: usize) -> &'a str {
        if a < self.at.0 {
            self.at = (0, 0);
        }
        let rest = skip(&self.s[self.at.1..], a - self.at.0);
        // Past the end, the next slice starts over or is empty too.
        self.at = (a, self.s.len() - rest.len());
        rest
    }

    /// `s[a:b]`.
    pub(crate) fn slice(&mut self, a: usize, b: usize) -> &'a str {
        take(self.from(a), b.saturating_sub(a))
    }
}

/// `round(x, digits)`: both round the float's exact value half to even.
pub(crate) fn round(x: f64, digits: usize) -> f64 {
    format!("{x:.digits$}").parse().unwrap_or(x)
}

/// `repr(x)`, which is `str(x)`, of a float: the shortest digits that read back, written out
/// from 1e-4 to below 1e16 (with `.0` when whole), else with an exponent of two digits or more.
pub(crate) fn repr_float(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    // Rust's `{:e}` and `{}` print the same shortest digits as `repr`.
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..16).contains(&exp) {
        let fixed = format!("{x}");
        if fixed.contains('.') {
            fixed
        } else {
            format!("{fixed}.0")
        }
    } else {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.abs())
    }
}

/// `Decimal(text)` for the decimal text kfxgen writes: `"0.5"`, `"2.0"`, `"1e-05"`.
pub(crate) fn decimal(text: &str) -> Decimal {
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (text, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    written(Decimal {
        coefficient: format!("{int}{frac}").parse().unwrap_or(0),
        exponent: exponent - frac.len() as i32,
    })
}

/// A decimal as kfxlib writes it: any zero, `0.0` too, as an empty decimal, which is 0
/// (`IonBinary.serialize_decimal_value`); deckle-kfx writes that only for a zero exponent.
fn written(d: Decimal) -> Decimal {
    if d.coefficient == 0 {
        Decimal {
            coefficient: 0,
            exponent: 0,
        }
    } else {
        d
    }
}

/// `Decimal(text)` for text from the model, or None where kfxgen can't write it (`abs()` of
/// "inf" and "NaN" raises) or deckle-kfx's decimal can't hold it: a coefficient of more than
/// an i128, an exponent past an i32 (DIVERGENCES.md). Digits, a point and an exponent only.
pub(crate) fn checked_decimal(text: &str) -> Option<Decimal> {
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().ok()?),
        None => (text, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = int.trim_start_matches(['+', '-']);
    if int.len() - digits.len() > 1
        || digits.is_empty() && frac.is_empty()
        || !digits
            .chars()
            .chain(frac.chars())
            .all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some(written(Decimal {
        coefficient: format!("{int}{frac}").parse().ok()?,
        exponent: exponent.checked_sub(i32::try_from(frac.len()).ok()?)?,
    }))
}

/// `f"{x:.{precision}g}"`: `precision` significant digits, no trailing zeros, an exponent
/// below 1e-4 or from 10**precision.
pub(crate) fn format_g(x: f64, precision: usize) -> String {
    let p = precision.max(1);
    let strip = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s
        }
    };
    let sci = format!("{:.*e}", p - 1, x);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if exp < -4 || exp >= p as i32 {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa.to_owned()), exp.abs())
    } else {
        strip(format!("{:.*}", (p as i32 - 1 - exp) as usize, x))
    }
}

/// `Decimal(repr(x)).quantize(Decimal("0.001"), rounding=ROUND_HALF_UP).normalize()`, or
/// None where that raises `InvalidOperation`: a result of more than the context's 28 digits,
/// or no number at all. A negative that rounds to nothing is Python's -0, which kfxlib
/// writes as it writes 0 (`serialize_decimal_value`: an empty decimal).
pub(crate) fn quantize_milli(x: f64) -> Option<Decimal> {
    if !x.is_finite() {
        return None;
    }
    // Rust prints the shortest round-trip digits, as `repr` does, with no exponent.
    let text = format!("{}", x.abs());
    let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
    let digits = format!("{int}{:0<3}", &frac[..frac.len().min(3)]);
    let digits = digits.trim_start_matches('0');
    // Past what an i128 holds is past 28 digits too.
    let mut coefficient: i128 = if digits.is_empty() {
        0
    } else {
        digits.parse().ok()?
    };
    // Half up: a tie goes away from zero.
    if frac.as_bytes().get(3).is_some_and(|&d| d >= b'5') {
        coefficient += 1;
    }
    if coefficient >= 10i128.pow(28) {
        return None;
    }
    if coefficient == 0 {
        return Some(Decimal {
            coefficient: 0,
            exponent: 0,
        });
    }
    let mut exponent = -3;
    while coefficient % 10 == 0 {
        coefficient /= 10;
        exponent += 1;
    }
    if x < 0.0 {
        coefficient = -coefficient;
    }
    Some(Decimal {
        coefficient,
        exponent,
    })
}

/// `Decimal(str(x))`. Rust prints a float's shortest round-trip digits, as `repr` does, but
/// without an exponent: the same decimal below 1e16, where `repr` starts writing one.
pub(crate) fn decimal_of_float(x: f64) -> Decimal {
    if x.is_finite() && x.abs() >= 1e16 {
        return decimal(&format!("{x:e}"));
    }
    let mut text = format!("{x}");
    if !text.contains('.') && x.is_finite() {
        // repr(2.0) is "2.0", and Decimal keeps that zero.
        text.push_str(".0");
    }
    decimal(&text)
}

#[cfg(test)]
mod tests {
    use super::decimal_of_float;

    /// A slice after another is the one Python takes, forwards, backwards and past the end.
    #[test]
    fn slices_one_after_another_are_pythons() {
        let s = "aé\u{fffc}b漢c";
        let mut slicer = super::Slicer::new(s);
        for (a, b) in [
            (0usize, 2usize),
            (2, 3),
            (3, 3),
            (4, 9),
            (9, 12),
            (1, 5),
            (5, 2),
            (0, 6),
            (7, 8),
        ] {
            let want: String = s.chars().skip(a).take(b.saturating_sub(a)).collect();
            assert_eq!(slicer.slice(a, b), want, "{a}:{b}");
            let rest: String = s.chars().skip(a).collect();
            assert_eq!(slicer.from(a), rest, "{a}:");
        }
    }

    /// Spot values of Python 3.14's `unicodedata.bidirectional` and `decimal`, ends of
    /// both tables included.
    #[test]
    fn bidi_classes_and_digit_values_are_pythons() {
        use super::{Strong, bidi_strong, digit_value};
        for (c, want) in [
            ('A', Some(Strong::L)),
            ('\u{5d0}', Some(Strong::R)),
            ('\u{627}', Some(Strong::Al)),
            ('1', None),
            (' ', None),
            ('\u{300}', None),
            ('\u{1e900}', Some(Strong::R)),
            ('\u{5ff}', None),
            ('\u{e0001}', None),
            ('\u{10fffd}', Some(Strong::L)),
            ('\0', None),
        ] {
            assert_eq!(bidi_strong(c), want, "{c:?}");
        }
        for (c, want) in [
            ('0', Some(0)),
            ('9', Some(9)),
            ('/', None),
            (':', None),
            ('\u{663}', Some(3)),
            ('\u{b2}', None),
            ('\u{1d7ce}', Some(0)),
            ('\u{1d7ff}', Some(9)),
            ('\u{1fbf9}', Some(9)),
            ('\u{1fbfa}', None),
            ('\u{10ffff}', None),
        ] {
            assert_eq!(digit_value(c), want, "{c:?}");
        }
    }

    /// `int()`: its white space (not U+001C to U+001F), signs, underscores, digits of any
    /// script, the 4300-digit limit, and saturation past an i64.
    #[test]
    fn int_reads_what_python_reads() {
        use super::py_int;
        for (s, want) in [
            (" +7_00 ", Some(700)),
            ("\u{3000}-\u{663}\u{a0}", Some(-3)),
            ("007", Some(7)),
            ("-0", Some(0)),
            ("\u{1c}7", None),
            ("7_", None),
            ("_7", None),
            ("7__0", None),
            ("+-7", None),
            ("+", None),
            ("", None),
            ("7 0", None),
            ("\u{b2}", None),
            ("1e3", None),
        ] {
            assert_eq!(py_int(s), want, "{s:?}");
        }
        assert_eq!(py_int(&"9".repeat(4300)), Some(i64::MAX));
        assert_eq!(py_int(&format!("-{}", "9".repeat(4300))), Some(i64::MIN));
        assert_eq!(py_int(&"0".repeat(4301)), None);
        assert_eq!(py_int(&format!("{}_1", "0".repeat(4299))), Some(1));
    }

    /// `repr(float)`: expected values from Python 3.14.
    #[test]
    fn float_repr_is_pythons() {
        use super::repr_float;
        let cases = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (9.0, "9.0"),
            (0.72, "0.72"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (123456789012345.6, "123456789012345.6"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (-2.5e300, "-2.5e+300"),
            (1.0 / 3.0, "0.3333333333333333"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (0.1 + 0.2, "0.30000000000000004"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
        ];
        for (x, want) in cases {
            assert_eq!(repr_float(x), want, "{x:e}");
        }
    }

    /// `float()`: expected values from Python 3.14.
    #[test]
    fn float_reads_what_python_reads() {
        use super::py_float;
        for (s, want) in [
            ("1", Some(1.0)),
            ("-1.5", Some(-1.5)),
            ("+.5", Some(0.5)),
            ("1.", Some(1.0)),
            ("  2.5 ", Some(2.5)),
            ("\u{3000}1\u{a0}", Some(1.0)),
            ("1\u{3000}", Some(1.0)),
            ("1_0", Some(10.0)),
            ("1e1_0", Some(1e10)),
            ("\u{661}.\u{665}", Some(1.5)),
            ("\u{663}e\u{662}", Some(300.0)),
            ("\u{661}_\u{662}", Some(12.0)),
            ("\u{661}\u{662}", Some(12.0)),
            ("1e+05", Some(100000.0)),
            ("5e-400", Some(0.0)),
            ("1_000.000_1", Some(1000.0001)),
            ("0_0", Some(0.0)),
            ("inf", Some(f64::INFINITY)),
            ("iNf", Some(f64::INFINITY)),
            ("INFINITY", Some(f64::INFINITY)),
            ("-Infinity", Some(f64::NEG_INFINITY)),
            ("1e400", Some(f64::INFINITY)),
            (".", None),
            ("-", None),
            ("+", None),
            ("", None),
            ("\u{1c}1", None),
            ("1\u{1f}", None),
            ("1__0", None),
            ("_1", None),
            ("1_", None),
            ("1_.5", None),
            ("1._5", None),
            ("1_e10", None),
            ("infinit", None),
            ("1e", None),
            ("e5", None),
            ("0x10", None),
            ("1.2.3", None),
            ("--5", None),
            ("1 2", None),
            ("2\u{2028}1", None),
            ("1,5", None),
            ("\u{85}1.5\u{85}", Some(1.5)),
            ("\u{85} 2 \u{85}", Some(2.0)),
            ("+ 1", None),
        ] {
            assert_eq!(py_float(s), want, "{s:?}");
        }
        // float() has no limit on digits, unlike int(): 4301 digits read as Python reads them.
        let ones = "1".repeat(4301);
        let one = format!("{}1", "0".repeat(4300));
        let tiny = format!("0.{}1", "0".repeat(4299));
        for (s, want) in [(&ones, f64::INFINITY), (&one, 1.0), (&tiny, 0.0)] {
            assert_eq!(py_float(s), Some(want), "{} digits", s.len());
        }
        for nan in ["nan", "-nan", "+nan", "NaN"] {
            assert!(py_float(nan).is_some_and(f64::is_nan), "{nan}");
        }
    }

    /// `Decimal(str(1e16))` is 1E+16 and `Decimal(str(2.0))` is 2.0 (issue #13).
    #[test]
    fn a_float_of_1e16_keeps_its_exponent() {
        for (x, coefficient, exponent) in [
            (1e16, 1, 16),
            (-1.2345678901234568e17, -12345678901234568, 1),
            (9999999999999998.0, 99999999999999980, -1),
            (2.0, 20, -1),
        ] {
            let d = decimal_of_float(x);
            assert_eq!((d.coefficient, d.exponent), (coefficient, exponent), "{x}");
        }
    }
}
