// SPDX-License-Identifier: GPL-3.0-only
//! The book's pictures and its cover as kfxgen hands them to `generate_full_book`:
//! `extract_cover_image`, `_get_cover_image_data`, `extract_images_from_oeb` and
//! `_check_and_optimize` (`converter.py`), and `cmyk_to_rgb`, `optimize_images`,
//! `optimize_image`, `gif_to_png`, `decodes` and their header readers (`image_optimize.py`), at
//! kfxgen `ad38ec8`. calibre's `scale_image` and `fit_image` (`utils/img.py`,
//! `calibre/__init__.py`) are ported where kfxgen calls them, with the size Qt's
//! `QImage::scaled` then gives (`NOTICE`).
//!
//! A JPEG or PNG within 1,920 px and 1 MiB keeps its bytes. A GIF becomes a PNG, a CMYK JPEG an
//! RGB one, and a picture over either cap is re-encoded, fitted into 1,920 px when its size is
//! the problem. kfxgen's `KFXGEN_IMAGE_*` environment variables are not read: their defaults
//! are the constants here. An SVG, as a file of the manifest or drawn with shapes in a page,
//! is refused: kfxgen draws it with Qt, which this port does not.
//!
//! Hostile input: a picture is only ever read from the archive, by its manifest href, through
//! [`EpubReader`]'s limits; no path from the book reaches the file system. Every decode goes
//! through [`Pictures::decode`], which reads the size the header states and refuses a picture
//! over [`MAX_IMAGE_PIXELS`] before anything is decoded, charges its size to the book's
//! [`MAX_BOOK_IMAGE_PIXELS`] (every decode: a CMYK JPEG is decoded twice), and
//! leaves `image`'s own allocation limit (512 MiB) in place for the rest. A picture that does not decode is left out with a
//! warning, as kfxgen leaves out one Qt can't read. Pictures are read, converted and dropped
//! one at a time; only the results are kept.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use deckle_kfx::epub_zip::{EpubReader, InspectError};
use deckle_limits::{MAX_BOOK_IMAGE_PIXELS, MAX_IMAGE_PIXELS};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageError, ImageReader};

use super::pictures::svg_draws_shapes_only;
use super::{ConvertOptions, Error};
use crate::book::{self, Book};
use crate::epub::package::unquote;
use crate::epub::{Item, Oeb, data_is_bytes};

/// `DEFAULT_MAX_DIM`: the longest side a picture keeps.
const MAX_DIM: u32 = 1920;
/// `DEFAULT_MAX_BYTES`: the most bytes a picture keeps without being re-encoded.
const MAX_BYTES: usize = 1024 * 1024;
/// `DEFAULT_JPEG_QUALITY`.
const JPEG_QUALITY: u8 = 85;

const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
/// `GIF_SIGNATURES`.
const GIF_SIGNATURES: [&[u8]; 2] = [b"GIF87a", b"GIF89a"];
/// `_SOF_MARKERS`: the JPEG frame headers that state a size.
const SOF_MARKERS: [u8; 13] = [
    0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF,
];

/// `generate_full_book`'s `cover_image` and `images`, and kfxgen's warnings about the pictures
/// it left out or kept as they were.
#[derive(Debug, Default)]
pub struct Images {
    pub cover: Option<Vec<u8>>,
    /// Manifest href → bytes, a JPEG or a PNG each.
    pub images: BTreeMap<String, Vec<u8>>,
    pub warnings: Vec<String>,
}

/// The cover and pictures of `book`, read from the EPUB at `path` (`convert_oeb_to_kfx`
/// :10067–10082). The cover's copy among the pictures is the cover's own result, so their
/// bytes are equal, as the generator needs to show it from the cover's resource.
pub fn images(book: &Book, path: &Path, opts: &ConvertOptions) -> Result<Images, Error> {
    refuse_svg(book)?;
    let zip = EpubReader::open(path).map_err(read_error)?;
    let mut p = Pictures {
        zip,
        embed_original: opts.embed_original_images,
        pixels: 0,
        warnings: Vec::new(),
    };
    let cover = p.cover_image(&book.oeb)?;
    let cover_result = match &cover {
        Some((data, href)) => {
            let out = p.finish(data.clone(), href)?;
            if out.is_none() {
                p.warn(format!(
                    "  Skipping the cover {href:?}: it can't be decoded"
                ));
            }
            out
        }
        None => None,
    };

    let mut images = BTreeMap::new();
    for item in &book.oeb.manifest {
        if !item.media_type.to_lowercase().contains("image") {
            continue;
        }
        let Some(data) = p.bytes_of(item)? else {
            continue;
        };
        let href = &item.href;
        let data = if is_jpeg_or_png(&data) {
            data
        } else {
            let converted = if GIF_SIGNATURES.contains(&data.get(..6).unwrap_or(&[])) {
                p.gif_to_png(&data, href)?
            } else {
                None
            };
            let Some(png) = converted else {
                p.warn(format!(
                    "  Skipping image {href:?}: unsupported format (magic bytes {}); only JPEG \
                     and PNG are emitted as KFX resources",
                    hex4(&data)
                ));
                continue;
            };
            png
        };
        let out = match &cover {
            Some((raw, _)) if *raw == data => cover_result.clone(),
            _ => p.finish(data, href)?,
        };
        match out {
            Some(out) => {
                images.insert(href.clone(), out);
            }
            None => p.warn(format!("  Skipping image {href:?}: it can't be decoded")),
        }
    }
    Ok(Images {
        cover: cover_result,
        images,
        warnings: p.warnings,
    })
}

/// An SVG kfxgen would draw as a picture, refused: a manifest SVG file
/// (`extract_images_from_oeb`'s `svg_to_png`) or an `<svg>` in a spine page drawn only with
/// shapes, not inside another `<svg>`, in a page that is not itself an SVG
/// (`_inline_svg_pictures`), whether or not the text shows it.
fn refuse_svg(book: &Book) -> Result<(), Error> {
    let refused = |file: &str| Error::Unsupported {
        feature: "SVG pictures",
        file: file.to_owned(),
    };
    if let Some(item) =
        (book.oeb.manifest.iter()).find(|i| i.media_type.to_lowercase() == "image/svg+xml")
    {
        return Err(refused(&item.href));
    }
    for file in &book.files {
        let dom = &file.dom;
        if dom.tag(dom.root()) == "svg" {
            continue;
        }
        let drawn = dom.descendants().any(|n| {
            dom.tag(n) == "svg"
                && !dom.parent(n).is_some_and(|p| dom.tag(p) == "svg")
                && svg_draws_shapes_only(dom, n)
        });
        if drawn {
            return Err(refused(&file.href));
        }
    }
    Ok(())
}

fn read_error(e: InspectError) -> Error {
    Error::Book(book::Error::from(e))
}

fn is_jpeg_or_png(data: &[u8]) -> bool {
    data.starts_with(b"\xff\xd8\xff") || data.starts_with(b"\x89PNG")
}

/// `bytes(data[:4]).hex()`.
fn hex4(data: &[u8]) -> String {
    data.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// The archive the pictures are read from, and what kfxgen logs about them.
struct Pictures {
    zip: EpubReader,
    /// `kfxgen_embed_original_images`.
    embed_original: bool,
    /// The pixels decoded so far, against [`MAX_BOOK_IMAGE_PIXELS`].
    pixels: u64,
    warnings: Vec<String>,
}

impl Pictures {
    fn warn(&mut self, w: String) {
        self.warnings.push(w);
    }

    /// `item.data` when Calibre gives it as bytes and it has some: the archive file the
    /// manifest href names, None when there is none.
    fn bytes_of(&mut self, item: &Item) -> Result<Option<Vec<u8>>, Error> {
        if !data_is_bytes(&item.media_type.to_lowercase()) {
            return Ok(None);
        }
        match self.zip.read_bytes(&unquote(&item.href)) {
            Ok(data) => Ok(Some(data).filter(|d| !d.is_empty())),
            Err(InspectError::MissingFile(_)) => Ok(None),
            Err(e) => Err(read_error(e)),
        }
    }

    /// `extract_cover_image`: the cover's bytes and href, from the metadata's cover (as a
    /// manifest id, then as an href), else the guide's cover, else the first manifest picture
    /// with "cover" in its id or href. Calibre's manifest is a set, so which of several such
    /// pictures kfxgen takes last varies from run to run; here it is the first by href.
    fn cover_image(&mut self, oeb: &Oeb) -> Result<Option<(Vec<u8>, String)>, Error> {
        let by_href = |href: &str| oeb.manifest.iter().find(|i| i.href == href);
        if let Some(cover_id) = oeb.metadata.cover.first() {
            for item in oeb.manifest.iter().filter(|i| i.id == *cover_id) {
                if item.media_type.contains("image")
                    && let Some(data) = self.cover_data(item)?
                {
                    return Ok(Some((data, item.href.clone())));
                }
            }
            if let Some(item) = by_href(cover_id)
                && let Some(data) = self.cover_data(item)?
            {
                return Ok(Some((data, item.href.clone())));
            }
        }
        for g in &oeb.guide {
            let kind = g.kind.to_lowercase();
            if (kind == "cover" || kind == "other.ms-coverimage-standard")
                && let Some(item) = by_href(&g.href)
                && let Some(data) = self.cover_data(item)?
            {
                return Ok(Some((data, g.href.clone())));
            }
        }
        for item in &oeb.manifest {
            let id = item.id.to_lowercase();
            let href = item.href.to_lowercase();
            if item.media_type.to_lowercase().contains("image")
                && (id.contains("cover") || href.contains("cover"))
                && let Some(data) = self.cover_data(item)?
            {
                return Ok(Some((data, item.href.clone())));
            }
        }
        Ok(None)
    }

    /// `_get_cover_image_data`: a JPEG or a PNG of more than 100 bytes, by its first bytes.
    fn cover_data(&mut self, item: &Item) -> Result<Option<Vec<u8>>, Error> {
        let Some(data) = self.bytes_of(item)?.filter(|d| d.len() > 100) else {
            return Ok(None);
        };
        if !is_jpeg_or_png(&data) {
            self.warn(format!(
                "  Skipping cover candidate {:?}: unsupported format (magic bytes {}); only \
                 JPEG and PNG are accepted as cover images",
                item.href,
                hex4(&data)
            ));
            return Ok(None);
        }
        Ok(Some(data))
    }

    /// `image_from_data`: `data` decoded, None when `image` can't read it. The size the
    /// header states is checked first: a picture over [`MAX_IMAGE_PIXELS`], one that takes
    /// the book's decodes past [`MAX_BOOK_IMAGE_PIXELS`], or one that needs more than `image`'s
    /// allocation limit, refuses the book.
    fn decode(&mut self, data: &[u8], href: &str) -> Result<Option<DynamicImage>, Error> {
        let reader = || {
            ImageReader::new(Cursor::new(data))
                .with_guessed_format()
                .map_err(ImageError::IoError)
        };
        let Ok((w, h)) = reader().and_then(|r| r.into_dimensions()) else {
            return Ok(None);
        };
        let pixels = u64::from(w) * u64::from(h);
        if pixels > MAX_IMAGE_PIXELS {
            return Err(Error::TooLarge {
                what: format!(
                    "{href}: this picture is larger than {} megapixels",
                    MAX_IMAGE_PIXELS / 1_000_000
                ),
            });
        }
        // Charged before the decode it pays for.
        self.pixels = self.pixels.saturating_add(pixels);
        if self.pixels > MAX_BOOK_IMAGE_PIXELS {
            return Err(Error::TooLarge {
                what: format!(
                    "{href}: this book's pictures are larger than {} megapixels together",
                    MAX_BOOK_IMAGE_PIXELS / 1_000_000
                ),
            });
        }
        match reader().and_then(ImageReader::decode) {
            Ok(img) => Ok(Some(img)),
            Err(ImageError::Limits(_)) => Err(Error::TooLarge {
                what: format!("{href}: this picture needs more memory to decode than allowed"),
            }),
            Err(_) => Ok(None),
        }
    }

    /// `gif_to_png`: the GIF as a PNG at its own size (its first frame on its logical screen,
    /// as Qt reads it), or None, with a warning, when it can't be.
    fn gif_to_png(&mut self, data: &[u8], href: &str) -> Result<Option<Vec<u8>>, Error> {
        let Some((w, h)) = read_image_size(data) else {
            self.warn("  GIF has unreadable dimensions; leaving it unconverted".to_owned());
            return Ok(None);
        };
        let Some(img) = self.decode(data, href)? else {
            self.warn("  GIF to PNG failed (it can't be decoded); skipping the image".to_owned());
            return Ok(None);
        };
        match scale_image(img, w, h, true) {
            Ok((_, png)) => Ok(Some(png)),
            Err(e) => {
                self.warn(format!("  GIF to PNG failed ({e}); skipping the image"));
                Ok(None)
            }
        }
    }

    /// `cmyk_to_rgb`, then `_check_and_optimize`, for one picture: its bytes for the book, or
    /// None when it doesn't decode.
    fn finish(&mut self, data: Vec<u8>, href: &str) -> Result<Option<Vec<u8>>, Error> {
        let data = self.cmyk_to_rgb(data, href)?;
        if self.embed_original {
            // `_drop_undecodable`.
            return Ok(self.decode(&data, href)?.map(|_| data));
        }
        self.optimize_image(data, href)
    }

    /// `cmyk_to_rgb`'s `convert`: a JPEG of four components re-encoded as RGB, as Previewer
    /// writes it; kept as it is when that can't be done.
    fn cmyk_to_rgb(&mut self, data: Vec<u8>, href: &str) -> Result<Vec<u8>, Error> {
        if jpeg_components(&data) != Some(4) {
            return Ok(data);
        }
        let Some(img) = self.decode(&data, href)? else {
            self.warn("  CMYK picture kept as it is: it can't be decoded".to_owned());
            return Ok(data);
        };
        match encode(&img, false) {
            Ok(out) => Ok(out),
            Err(e) => {
                self.warn(format!("  CMYK picture kept as it is: {e}"));
                Ok(data)
            }
        }
    }

    /// `optimize_image`: `data` as it is when within both caps and it decodes; else fitted
    /// into 1,920 px when over it, or re-encoded at its size when only too heavy, and kept as
    /// it was when that re-encode is not smaller. None when it doesn't decode.
    ///
    /// A re-encoded PNG with no alpha is the smaller of it and a JPEG. kfxgen also tries a
    /// palette PNG for a palette source; that is not ported (`DIVERGENCES.md`).
    fn optimize_image(&mut self, data: Vec<u8>, href: &str) -> Result<Option<Vec<u8>>, Error> {
        let size = read_image_size(&data);
        let Some((w, h)) = size.filter(|&(w, h)| w.max(h) > MAX_DIM || data.len() > MAX_BYTES)
        else {
            return Ok(self.decode(&data, href)?.map(|_| data));
        };
        let over_dim = w.max(h) > MAX_DIM;
        let (bw, bh) = if over_dim { (MAX_DIM, MAX_DIM) } else { (w, h) };
        let is_png = data.starts_with(PNG_SIG);
        let Some(img) = self.decode(&data, href)? else {
            return Ok(None);
        };
        let (img, mut out) = match scale_image(img, bw, bh, is_png) {
            Ok(done) => done,
            Err(e) => {
                self.warn(format!("  image optimize failed ({e}); keeping original"));
                return Ok(Some(data));
            }
        };
        // A PNG of colour type grey or RGB, with no `tRNS` anywhere in its bytes, as kfxgen
        // tests it.
        if is_png
            && matches!(out.get(25), Some(0 | 2))
            && !out.windows(4).any(|w| w == b"tRNS")
            && let Ok(jpeg) = encode(&img, false)
            && jpeg.len() < out.len()
        {
            out = jpeg;
        }
        // Over the cap, the fitted one even if it grew; re-encoded for its bytes, never larger.
        if out.len() >= data.len() && !over_dim {
            return Ok(Some(data));
        }
        Ok(Some(out))
    }
}

/// calibre's `scale_image(data, width, height, as_png=…)` from the decoded picture: fitted
/// into the box when it is larger (`fit_image`, then `QImage::scaled`), as a PNG or a JPEG at
/// quality 85. The fitted picture comes back too, for the JPEG `optimize_image` compares.
fn scale_image(
    img: DynamicImage,
    box_w: u32,
    box_h: u32,
    as_png: bool,
) -> Result<(DynamicImage, Vec<u8>), String> {
    let img = match fit_image(img.width(), img.height(), box_w, box_h) {
        None => img,
        Some((fw, fh)) => {
            let (w, h) = qt_keep_aspect(img.width(), img.height(), fw, fh);
            if w == 0 || h == 0 {
                // `QImage::scaled` to an empty size gives a null picture, which can't be
                // written.
                return Err("fitted to no size".to_owned());
            }
            // The filter is free: kfxgen's bytes are Qt's and can't be matched anyway. This one
            // allocates only the result.
            img.thumbnail_exact(w, h)
        }
    };
    let out = encode(&img, as_png).map_err(|e| e.to_string())?;
    Ok((img, out))
}

/// calibre's `fit_image(width, height, pwidth, pheight)`: the size within the box, the scaled
/// side floored, or None when the picture already fits (`scaled` false).
fn fit_image(width: u32, height: u32, pwidth: u32, pheight: u32) -> Option<(u32, u32)> {
    if width < 1 || height < 1 || (height <= pheight && width <= pwidth) {
        return None;
    }
    let (mut w, mut h) = (f64::from(width), f64::from(height));
    let (pw, ph) = (f64::from(pwidth), f64::from(pheight));
    if h > ph {
        (w, h) = ((ph / h * w).floor(), ph);
    }
    if w > pw {
        (w, h) = (pw, (pw / w * h).floor());
    }
    if h > ph {
        (w, h) = ((ph / h * w).floor(), ph);
    }
    // Floored, within the box, and not negative: the casts are exact.
    Some((w as u32, h as u32))
}

/// Qt's `QSize(width, height).scaled(box_w, box_h, Qt::KeepAspectRatio)`, the size
/// `QImage::scaled` gives: the largest size of the picture's shape within the box, in whole
/// pixels. `fit_image` floors one side, so this can come out narrower than its box: a
/// 2000 × 10 picture fitted to 1920 × 9 is drawn 1800 × 9.
fn qt_keep_aspect(width: u32, height: u32, box_w: u32, box_h: u32) -> (u32, u32) {
    let (w, h) = (u64::from(width), u64::from(height));
    let rw = u64::from(box_h) * w / h;
    if rw <= u64::from(box_w) {
        // `rw` is at most `box_w`, a u32.
        (rw as u32, box_h)
    } else {
        // Less than `box_h`: `rw > box_w` means `box_w * h / w < box_h`.
        (box_w, (u64::from(box_w) * h / w) as u32)
    }
}

/// `image_to_data(img, fmt=…)`: a PNG at zlib's level 9, as calibre writes one, or a JPEG at
/// quality 85. kfxgen writes a JPEG only from a picture without alpha (a JPEG, or a PNG with
/// none), so no blending is needed.
fn encode(img: &DynamicImage, as_png: bool) -> Result<Vec<u8>, ImageError> {
    let mut out = Vec::new();
    if as_png {
        let png =
            PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
        img.write_with_encoder(png)?;
    } else {
        let jpeg = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
        match img {
            DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) => {
                img.write_with_encoder(jpeg)?
            }
            DynamicImage::ImageLuma16(_) => {
                DynamicImage::ImageLuma8(img.to_luma8()).write_with_encoder(jpeg)?
            }
            _ => DynamicImage::ImageRgb8(img.to_rgb8()).write_with_encoder(jpeg)?,
        }
    }
    Ok(out)
}

/// `_read_image_size`: a GIF's, PNG's or JPEG's size from its header, as kfxgen reads it.
fn read_image_size(data: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| u32::from(u16::from_be_bytes([data[i], data[i + 1]]));
    if data.len() >= 10 && GIF_SIGNATURES.contains(&&data[..6]) {
        let w = u32::from(u16::from_le_bytes([data[6], data[7]]));
        let h = u32::from(u16::from_le_bytes([data[8], data[9]]));
        return (w != 0 && h != 0).then_some((w, h));
    }
    if data.len() >= 24 && data.starts_with(PNG_SIG) {
        let be32 = |i: usize| u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        return Some((be32(16), be32(20)));
    }
    let i = jpeg_frame_at(data)?;
    Some((be16(i + 7), be16(i + 5)))
}

/// `_jpeg_frame_at`: where a JPEG's frame header starts, when it is whole enough to state the
/// size.
fn jpeg_frame_at(data: &[u8]) -> Option<usize> {
    if !data.starts_with(b"\xff\xd8") {
        return None;
    }
    let (mut i, n) = (2, data.len());
    while i + 1 < n {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // SOI, EOI and the restart markers stand alone.
        if matches!(marker, 0xD0..=0xD9) {
            i += 2;
            continue;
        }
        if i + 4 > n {
            break;
        }
        let seg_len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        if SOF_MARKERS.contains(&marker) {
            return (i + 9 <= n).then_some(i);
        }
        i += 2 + seg_len;
    }
    None
}

/// `_jpeg_components`: a JPEG's colour components (1 grey, 3 colour, 4 CMYK).
fn jpeg_components(data: &[u8]) -> Option<u8> {
    let i = jpeg_frame_at(data)?;
    data.get(i + 9).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_image_floors_as_calibre_does() {
        assert_eq!(fit_image(2400, 1200, 1920, 1920), Some((1920, 960)));
        assert_eq!(fit_image(1921, 7, 1920, 1920), Some((1920, 6)));
        assert_eq!(fit_image(1, 2000, 1920, 1920), Some((0, 1920)));
        assert_eq!(fit_image(1920, 1920, 1920, 1920), None);
        assert_eq!(qt_keep_aspect(1921, 7, 1920, 6), (1646, 6));
        assert_eq!(qt_keep_aspect(2400, 1200, 1920, 960), (1920, 960));
    }

    #[test]
    fn header_readers_stop_at_the_end_of_a_cut_file() {
        for n in 0..12 {
            let cut = &b"\xff\xd8\xff\xc0\x00\x11\x08\x00\x10\x00\x18\x04"[..n];
            assert_eq!(jpeg_frame_at(cut), (n >= 11).then_some(2));
            let _ = read_image_size(cut);
            let _ = jpeg_components(cut);
        }
        // A segment length that runs past the end.
        assert_eq!(jpeg_frame_at(b"\xff\xd8\xff\xe0\xff\xff"), None);
    }
}
