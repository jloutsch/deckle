# Nesting depth limits — Design

Date: 2026-11-12. Status: revised after validation (QA session, same day). Covers issue #58 and item 2 of #86.

## Why

Deeply nested input overflows the stack. A stack overflow aborts the process; it is not a panic
and cannot be caught. The scan reproduced it in:

| Input | Where the recursion is | Depth that aborts on a 2 MiB stack |
|---|---|---|
| DOCX, ODT, FB2, ComicInfo.xml, PDF edition XHTML | `roxmltree` 0.21 (recursive parser), then the importer's own tree walk | about 3,000 |
| RTF | `Walker::group`, `footnote`, and the recursive drop of `lex::Node` | about 10,000 |
| MOBI 6 | `Writer::children` / `node` / `element` | about 3,000 |
| Ion (KFX, KPF) | `ion/decode.rs` `value` | about 1,700 (measured in release: 878 on 1 MiB, 1,737 on 2 MiB, about 7,000 on 8 MiB) |
| The Daily's article writer | `deckle-digest/src/article.rs:276`, `:328` `Writer::node` / `children` | not measured; same shape as the MOBI 6 writer |
| Feeds | `feed-rs` 3.0.0, called at `feeds.rs:50` | about 1,600 (nested `media:group`) |

The app and the web server run imports on tokio blocking threads
(`deckle-server/src/invoke.rs:40`, `apps/desktop/src-tauri/src/wrap.rs:54`), whose default stack
is 2 MiB. The command line runs imports on the main thread, 8 MiB, but its article extraction
also runs on 2 MiB worker threads (`feeds.rs:340`, `:410`). So the 2 MiB figures apply.

## Goals

- No input can overflow the stack in a release build. Too-deep input gives an ordinary error.
  Debug builds use about 10 KB of stack per `roxmltree` level and abort near 100 to 150 levels
  on 2 MiB, so the guarantee is for release. The tests still run in the normal debug suite: the
  refusal tests never reach the parser, and the tests that parse at the limit use an 8 MiB
  thread.
- Real documents are unaffected.

## Not goals

- Replacing `roxmltree`, or rewriting the walkers as loops (except where noted for RTF drop).
- Raising thread stack sizes. That moves the number; it does not remove the abort.

## The limit

One constant, `MAX_DEPTH = 256`, in the `deckle-limits` crate from the size-limits design (the
XML helper sits behind that crate's `xml` feature).

Why 256: real DOCX and ODT nesting (a table in a table in a list in a text box) stays under 30,
and the deepest of 293 fixture XML and HTML parts is 16. 256 is far above both and far below the
3,000 that aborts. A `roxmltree` level costs roughly 600 bytes of stack in release (2 MiB /
3,000), so 256 levels use about 150 KB.

## Three mechanisms

**1. XML: check depth before parsing.** `roxmltree` has no depth option, so the text is scanned
first. A helper in `limits`:

```rust
/// The deepest element nesting in `xml`, counted from tags alone; stops at `limit + 1`.
pub fn xml_depth_over(xml: &[u8], limit: usize) -> bool;
```

As built, the function is private to the crate (only its tests call it by name): `parse_xml` runs the same scan with the encoding check off, and feeds do not use it.

It is a single pass over the bytes, and it must never count low, because the parsers recurse
before they reject anything. It takes bytes, not `&str`: `feeds.rs:50` passes raw bytes and
`feed-rs` accepts Latin-1 and UTF-16 today.

- **Encoding.** Bytes are scanned as they are; every ASCII-compatible encoding (UTF-8, Latin-1)
  keeps `<`, `>`, `/` and quotes as single bytes. UTF-16 is detected exactly as `quick-xml` detects it (a byte-order mark, or the four bytes
  `00 3C 00 3F` / `3C 00 3F 00`) and scanned in 16-bit units; a looser test counted low in QA.
  A declared encoding that is not ASCII-compatible (ISO-2022-JP, where a two-byte character can
  be the bytes `<"`, or a UTF-16 label the byte test did not confirm) returns "over": such
  feeds are refused. That is a known loss for ISO-2022-JP feeds. Invalid UTF-8 is not a reason to
  refuse.

- `<name` opens and `</` closes. The end of a tag is found with quotes respected, single and double: `>` and `/>`
  inside a quoted attribute value do not end the tag (`<a x="/>">` opens an element). An unquoted
  attribute value (`<a href=x>`) is ordinary and ends at whitespace or `>`; feeds that load today
  use it.
- Comments, CDATA sections and processing instructions are skipped exactly, to their real
  terminators.
- **Entities.** `roxmltree` expands DTD entities, and an entity's value can hold markup, so
  nesting can hide in the DOCTYPE. The scanner reads the internal DTD subset and returns "over"
  for any entity value, in single or double quotes, that contains a literal `<`. Character and
  entity references (`&#60;`, `&lt;`) inside a value stay text in `roxmltree` and are allowed, as is a
  DOCTYPE with no internal subset (RSS 0.91). Real documents use entities for characters, not
  markup.
- Inside a DOCTYPE declaration (`ATTLIST`, `NOTATION`, `ELEMENT`), any `<` before the
  declaration ends returns "over", quoted or not: `roxmltree` ends these at the first `>`
  whatever the quotes say.
- An unterminated comment, CDATA section, processing instruction or quoted value returns "over",
  and `parse_xml` refuses such input itself; it is not handed to the parser.
  No scanner exists yet, so this rule has not been run. Validation checked by hand that the
  constructs feeds use today (unquoted attributes, CDATA and comments containing `<`, `&nbsp;`,
  the RSS 0.91 DOCTYPE; 11 hand-made feeds accepted by `feed-rs`) would not be refused by it.

Every `roxmltree::Document::parse` call on file content goes through one wrapper,
`limits::parse_xml(text, options)`, which takes the caller's `ParsingOptions` (five sites set
`allow_dtd`: `deckle-fb2/src/lib.rs:275`, `deckle-odt/src/lib.rs:172`, `deckle-mobi/src/kf8.rs:630`,
`deckle-core/src/kepub.rs:361`, `deckle-digest/src/schedule.rs:1341`), runs the check and returns "this file is nested more than 256
levels deep" (or, for an entity refusal, "this file defines an entity that contains markup"). Call sites: `deckle-docx/src/package.rs:160`, `deckle-odt/src/lib.rs:175`,
`deckle-fb2/src/lib.rs:278`, `deckle-comic/src/comicinfo.rs:23`, `deckle-mobi/src/kf8.rs:627`,
`deckle-digest/src/pdf.rs:1045`, and the rest: validation counted 48 parse sites (EPUB import,
readership, morgue, library among them), found with `grep -rn "Document::parse"`. This means an
EPUB nested more than 256 deep is refused where it imports today; the deepest of 293 fixture
XML and HTML parts is 16.

Feeds: `feed_rs::parser::parse` recurses on its own, and its XML reader decodes the bytes
(by byte-order mark, first bytes, then the declared encoding) before it reads tags. A scan of
the raw bytes cannot agree with it for every encoding: QA crashed it twice more after two rounds
of byte-level fixes (a Shift_JIS, Big5 or GBK trail byte can be `[`). So `feeds.rs` does not scan feeds itself. Before `feed-rs` runs, it
reads the feed once with the same XML reader `feed-rs` uses (`quick-xml`, configured the same
way), which does not recurse, and counts element depth; the same code cannot disagree with
itself about encodings. A feed nested more than 256 deep is reported as unreadable. (A copied
decoder followed by the byte scan was considered and dropped: it could drift from the reader.) The scanner's contract
is stated in its doc comment: its answer is guaranteed for UTF-8 (and the UTF-16 it detects)
only.

Two more feed rules come from the same review. A feed with more than one XML declaration, or
with `encoding` given twice, is reported as unreadable, because a second declaration naming
another encoding makes the reader panic. And the `feed_rs::parser::parse` call is wrapped in
`catch_unwind`: today a parser panic ends the edition process with status 101 and the whole
edition fails; with the wrap, that one feed is reported as unreadable.

A unit test greps the workspace and fails if a new direct `Document::parse` call appears outside
the wrapper (the pattern `apps/desktop/src/platform_boundary.test.ts` uses for the frontend).

For `pdf.rs`, which already has a text fallback for XHTML it cannot lay out, too-deep input takes
that fallback and the edition still prints.

**2. Own recursion: a depth counter.** The RTF walker, the MOBI 6 writer and the Ion decoder each
carry a `depth` argument or field, incremented on entry, and return the crate's error past
`MAX_DEPTH`. The DOCX, ODT and FB2 walkers get the same counter even though mechanism 1 already
bounds them, so that a walker stays safe if it is ever fed a tree from elsewhere.

The Ion decoder uses its own constant, `MAX_ION_DEPTH = 128`. The decoder aborts at about 878
levels on a 1 MiB stack, and the deepest of the 81 fixture `.kfx` files nests 16 levels, so 128
is 8× above real books and about 7× below the abort.

The Daily's article writer (`article.rs` `Writer::node` / `children`, #86 item 2) gets a counter
too. Past `MAX_DEPTH` it stops descending and writes the remaining text of that subtree flat, so
the story still prints.

The RTF lexer is already a loop (`lex.rs:33`); it needs only the check.

**3. RTF: drop without recursion.** `lex::Node` is a tree whose `Drop` recurses. With the lexer
refusing input past `MAX_DEPTH`, no tree deeper than 256 is ever built, so the default drop is
safe and no iterative drop is needed. The design relies on the limit being enforced in the lexer,
before the tree exists; the test below proves it.

## Tests

For each reader: a crafted input nested 100,000 deep returns the error. Where the check runs
before any recursion (the XML scan, RTF), run the test on a thread with a 1 MiB stack
(`std::thread::Builder::new().stack_size(1 << 20)`), so it would abort if recursion came first.
Where the check sits inside the recursion (Ion, MOBI 6 writer, article writer), run it on an
8 MiB thread: the limit, not the stack, must be what stops it.

- DOCX nested tables, ODT nested lists, FB2 nested `cite`, ComicInfo nested elements.
- RTF nested groups and nested `\footnote`.
- MOBI 6 nested `<b>`.
- Ion nested lists.
- `xml_depth_over`: comments, CDATA and self-closing tags containing `<` do not change the count;
  `>` and `/>` inside attribute values do not end a tag; an entity whose value holds markup is
  "over"; a document at exactly 256 passes and 257 fails;
  single-quoted attribute and entity values; unquoted attribute values; a deep Latin-1 feed and
  a deep UTF-16 feed are both refused, and shallow ones of each load.
- The Daily: an article with 20,000 nested `<em>` prints its text.
- A feed with 5,000 nested `media:group` is refused without an abort.

One regression test: every fixture still imports with identical output.

## Not covered

`pulldown-cmark` deep nesting was not tested (`html5ever` was: 20,000 levels on a 2 MiB stack
without an abort). The real abort depth of
`article.rs` and the MOBI 6 writer's frame size were not measured.
