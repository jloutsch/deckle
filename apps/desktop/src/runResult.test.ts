// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, test } from "vitest";
import { notSent, runFailed } from "./runResult";

test("entries the push left unsent come from the Kindle's push and the reader's", () => {
  const report = {
    outcome: { flush: { delivered: ["/K/documents/deckle-daily-2026-11-12.kfx"], failed: [["Emma.kfx", "the Kindle refused it"]] } },
    reader: { flush: { delivered: [], failed: [["broken.kepub.epub", "reading broken.epub: Is a directory"]] } },
  };
  expect(notSent(report)).toEqual([
    { name: "Emma.kfx", why: "the Kindle refused it" },
    { name: "broken.kepub.epub", why: "reading broken.epub: Is a directory" },
  ]);
  expect(notSent({ outcome: { flush: { delivered: [] } } })).toEqual([]);
  expect(notSent(null)).toEqual([]);
});

test("a run whose own edition went isn't shown as failed", () => {
  expect(runFailed({ ok: true, report: {}, log: "" })).toBe(false);
  expect(runFailed({ ok: false, report: { edition_sent: true }, log: "" })).toBe(false);
  expect(runFailed({ ok: false, report: { edition_sent: false }, log: "" })).toBe(true);
  expect(runFailed({ ok: false, report: null, log: "boom" })).toBe(true);
});
