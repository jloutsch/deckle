# Size limits for files Deckle opens — Design

Date: 2026-11-12. Status: revised after validation (QA session, same day). Covers issues #57, #59,
#60, #61, #62, item 1 of #64 and item 2 of #79.

## Why

A file the owner opens can make Deckle abort or use gigabytes of memory. The scan found the same
three causes in about fifteen places:

1. **A size the file declares is used to reserve memory.** `Vec::with_capacity(f.size())` on a
   zip entry (`deckle-core` normalize, kepub, whoswho; `deckle-comic`; `deckle-kfx` kpf), a MOBI
   record count, a KFX symbol table `max_id`, a PDF page width. A 166-byte file can ask for 2^62
   bytes; the allocation failure aborts the process and cannot be caught.
2. **Compressed data is inflated without a limit.** EPUB, CBZ, DOCX and CBR entries, MOBI `FONT`
   records and HUFF/CDIC text, images by pixel count.
3. **A count the file declares drives a loop.** MOBI index entry counts, TXT headings, KFX
   position lengths.

Some importers already have limits (`MAX_BYTES` of 50 MB in ODT, FB2, RTF, TXT, MOBI and
Markdown, 200 MB in PDF, 20 MB in KOReader plugins, `MAX_IMAGE` of 10 MB in ODT, FB2 and RTF).
They are separate constants with separate code, and the rest have none.

## Goals

- No input file can make Deckle reserve or fill memory beyond a stated limit.
- One rule per cause, used the same way in every reader, so a new importer has one thing to call.
- A refused file gives an error that names the limit, not a crash.

## Not goals

- No change to which formats Deckle reads. A file inside the limits imports with the same output
  as today; a file over a limit that imports today is refused (or, for TXT chapters and PDF
  renders, handled as the table says).
- No streaming rewrite. Entries are still read whole into memory, inside the limits.
- Nesting depth is a separate design (`2026-11-12-nesting-depth-limits-design.md`).

## The three rules

**Rule 1: never reserve from a declared size.** Remove every `with_capacity` whose argument comes
from the file. Where a reservation is worth keeping for speed, clamp it:
`Vec::with_capacity(declared.min(64 * 1024))`.

**Rule 2: read through a budget.** Every read of compressed or declared-length data goes through
`take(limit + 1)` and fails when more than `limit` bytes arrive. Where the table gives both, two
budgets apply together: one per entry, one for the whole file.

**Rule 3: a declared count is checked against what the bytes can hold** before it sizes a list or
bounds a loop. A count of N items of at least `k` bytes each in a record of `len` bytes is
refused or clamped when `N > len / k`.

## Limits

| What | Limit | Why this number |
|---|---|---|
| EPUB, KPF, CBZ: one entry, unpacked | 256 MB | Largest real entry seen is a picture-book image or an embedded font; 256 MB leaves a wide margin. |
| EPUB, KPF: whole archive, unpacked | 1 GB | The KFX generator's own peak is about 36 KiB per chapter plus media; a 1 GB book is past what a Kindle opens. |
| CBZ, CBR: whole archive, unpacked | 4 GB | Comics are large; pages are processed one at a time. |
| CBZ, CBR: one page, compressed file size | 256 MB | The same as one archive entry. A 1200 dpi scan page can pass 64 MB as a file. |
| DOCX: whole package, unpacked, all parts and images together | 50 MB | One total, as ODT does (`deckle-odt/src/lib.rs:157-160`). No per-image cap: a real .docx on the validation machine holds a 41.5 MB image. |
| MOBI/AZW3: inflated `FONT` record | 32 MB | Larger than any embedded font. |
| MOBI/AZW3: all inflated `FONT` records of one book | 256 MB | About 100 `FONT` records of 3 MB input can otherwise inflate to about 3.2 GB. Only fonts inflate, so the total applies to them. |
| MOBI: decompressed text | the header's text length, itself capped at 256 MB | The header states it; stop when it is reached. |
| Image decode: width × height | 100 megapixels, except comic pages | A 10000 × 10000 page takes about 400 MB at 4 bytes per pixel. Comic pages get no new pixel cap: they decode one at a time, and `image`'s own limit already refuses a decode over 512 MiB with a clean error before allocating (`limits.rs:100-119`), which is the bound; fitting a page can briefly take about twice that, because the resize works on a float copy. A pixel cap below that would send large grey scans out unfitted, which they are not today. |
| PDF: rendered page, width × height in pixels | 100 megapixels | The point-size cap alone allows a 28,800 × 28,800 px render (about 3.3 GB). A page over the cap is rendered at a lower scale that fits, not refused, so real PDFs keep importing. |
| PDF: page width or height | 14,400 pt | The PDF specification's own maximum. |
| TXT: chapters | 10,000 | One XHTML file per heading. Past the limit the importer stops splitting and keeps the rest in the last chapter; it does not refuse the file. Table-of-contents entries stop at 10,000 too (`deckle-txt/src/lib.rs:162-165` pushes one per heading). |
| KFX symbol table `max_id` | 1,000,000 | The imported shared table holds 850 symbols (`symtab.rs:28`) and books add a few thousand. |
| KFX positions and page ranges: the total over the whole book | one position per character of the book's text plus one per other value, counted in the storylines and text | A position cannot exceed what the text holds. The unpacked byte size was tried first and refused nothing: QA passed a 835 KB file padded with empty resources. The file's size on disk is too small a bound: a real fixture of 19,241 bytes holds 78,982 positions. The bound is on the book's total, not per section: `positions.rs:260-262` restarts per section and `navigation.rs:484` inserts one key per position of a range, so a per-section bound multiplies by the section count. |
| KFX, KPF: decoded Ion values in one file | 5,000,000 | As built (`MAX_ION_VALUES`). Bounds the decoder only: about 320 bytes per value, so about 1.6 GB for the decoded tree; symbol text is charged one unit per 64 bytes. QA counted values in 1,589 real `.kfx` and `.kpf` files; the largest held 1,971,808, and an earlier limit of 1,500,000 refused real books. Copies made later in conversion are not bounded by it (#79). |

The numbers are my calls. Validation measured 2,014 local EPUB and DOCX files: the largest EPUB
unpacks to 754.5 MB and three more pass 300 MB, so the 1 GB total stays; the largest single entry
is 23 MB. They are constants in one place so they can be changed without touching the readers.

## Where the code goes

A new crate `deckle-limits` holds the constants and two helpers. It has to be a new crate:
`deckle-core` depends on kfx, mobi, docx, pdf, comic, txt, odt, rtf and markdown, so those crates
cannot reach a module inside it. `deckle-limits` depends on nothing in the workspace and, with default features, on no external
crate. Two optional features carry the helpers of the other designs, so the importers that need
neither gain no dependency: `xml` (the nesting-depth wrapper, needs `roxmltree`) and `state`
(the state-file helper, needs `tempfile`, `serde`, `serde_json`).

```rust
/// Reads `r` to the end, failing with `TooLarge` past `limit` bytes.
pub fn read_capped(r: impl Read, limit: u64) -> io::Result<Vec<u8>>; // TooLarge inside the io::Error

/// A running total for one file: `add(n)` fails once the total passes `limit`.
pub struct Budget { /* limit, used */ }
```

Each reader's error type gains (or reuses) a `TooLarge` variant whose message names the limit in
plain words: "this book unpacks to more than 1 GB".

The existing per-crate `MAX_BYTES` and `MAX_IMAGE` constants move to `deckle-limits` unchanged in value.

## Order of work

Each step is its own pull request and closes or narrows one issue.

1. `deckle-limits`, plus every zip reader (#57). The cap goes **inside `EpubReader`**
   (`deckle-core/src/epub.rs:20-33` `open`, `:58-74` `read_string` and `read_bytes`), because
   `normalize.rs:170` and most other callers read through it. Then the readers that open a zip
   themselves: normalize, kepub, whoswho, comic (`pdf.rs:28-33` too), kpf,
   `deckle-library/src/details.rs:449-456` `rewrite_epub`, `deckle-core/src/reader.rs:112-115`
   `fixed_layout_of`, `deckle-readership/src/layout.rs:112-117`, and
   `deckle-kfx/src/source_epub.rs:23-36` (the EPUB inside a KPF, which must draw on the KPF's
   budget, not a new one; `SourceEpub::parse` runs in `fixup/mod.rs:50` after `read_kpf` returns,
   so the remaining budget is carried in the `Kpf` struct). The implementer greps `ZipArchive::new` and `by_name`/`by_index` and
   covers every hit. `zip` 9 stops an entry at its declared size, so summing declared sizes
   first is a valid cheap pre-check before reading.
2. MOBI/AZW3 (#59): record counts, font inflate and the whole-book font total, HUFF/CDIC
   output cap (checked inside the recursion in `text.rs:143` `unpack_at`), index counts, CTOC
   length with checked addition, and `lib.rs:207` (#59 item 6).
3. DOCX, CBR, TXT (#61). CBR: `bsdtar` extracts to disk (`cbr.rs:31-38`), so `take` cannot apply
   and listed sizes are declared sizes. The archive is converted once, `bsdtar -cf - --format pax @<archive>`, and
   the output is read as a tar stream. `pax`, not `ustar`: validation ran both, and `ustar`
   silently leaves out any page whose path is over 100 characters while exiting 0. Each member
   is read through `read_capped` against the page and archive budgets and written to the
   existing temp folder, where `convert_with` reads pages by name as it does today
   (`lib.rs:186`, `:529`); pages are not held in memory together. The writer refuses a member
   whose path is absolute or has a `..` component, and any link member, as
   `koreader_plugins.rs:545` does; `bsdtar -x` refused these until now and the stream does not.
   A non-zero exit from `bsdtar` fails the import even when a valid tar was produced (a
   truncated archive gives exit 1 and an empty tar), and stderr is drained while stdout is read.
   `deckle-comic` gains a tar reader for this. Reading one entry per `bsdtar` call is not used:
   `bsdtar` treats the name as a pattern, and a solid RAR or 7z would be decompressed again for
   each page.
4. PDF page size and render size (#60): the pixel cap applies where the page is rendered
   (`engine.rs:189-203`, `lib.rs:88` `render_scan`), and `read_page` (`engine.rs:255`) clamps the width used by the
   gutter and channel tables (`layout.rs:1116`, `:549`). Image pixel cap (#64 item 1) at every decode site: `deckle-comic`,
   `normalize.rs:256`, `thumbnail.rs:34`, `pipeline.rs:580`, `covers.rs:26`, digest
   `bundle.rs:566`, `nonogram.rs:371`, app `cookbook.rs:312`. `image` 0.25 already caps one
   decode at 512 MiB; the cap here also has to precede conversions such as `to_rgba8`.
5. KFX symbol table and positions (#62, #79 item 2), and overlapping entity ranges (#62 item 2,
   `container.rs:171-177`, `:238`): a container whose entity lengths sum to more than the file
   length is refused.

## Tests

For each reader, one test per rule that applies, using the smallest crafted input:

- a declared size of 2^60 is refused with `TooLarge`, and the test process does not abort;
- an entry that inflates past the limit is refused, and peak memory stays under the limit plus
  the reader's normal overhead (assert on bytes read, not on RSS);
- a count larger than the record can hold is refused or clamped.

The reviewers' crafted files are in the session scratch folder and are not in the repository. The
tests build their inputs in code, as the existing fixtures do.

One regression test per reader: a real fixture book still imports byte-for-byte as before.

## What a user sees

Nothing, for real books. For a refused file: one error line naming the limit. The README's format
list gets one line: "Files over these sizes are refused" with a link to the table, which moves to
`docs/books.md`.

## Open questions

1. Should the limits be settable by the owner (an environment variable), or fixed? This design
   fixes them; a setting can be added when someone meets a real book over a limit.

## Not verified

The validation session read the code for the sites above and did not run Deckle's tests. The
line numbers for the sites added in the revision are the validator's; I re-read `epub.rs:20-33`
and none of the others. RAR and 7z inputs, and decoding very large grey or RGBA pages, were not tried.
