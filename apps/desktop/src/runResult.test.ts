// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, test } from "vitest";
import { notSent, runFailed, summary } from "./runResult";

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

test("a push that stopped after the edition went isn't shown as failed", () => {
  const report = {
    edition_sent: true,
    outcome: {
      edition_sent: true,
      flush: { delivered: ["/K/documents/deckle-daily-2026-11-12.kfx"], failed: [["b.kfx", "the Kindle is no longer connected"]], stopped: true },
    },
  };
  expect(runFailed({ ok: false, report, log: "" })).toBe(false);
  expect(notSent(report)).toEqual([{ name: "b.kfx", why: "the Kindle is no longer connected" }]);
});

const sent = (outcome: any, extra: any = {}) => ({ ok: true, log: "", report: { build: { title: "The Daily" }, outcome, ...extra } });

test("the summary says sent to your Kindle only when the edition went to it", () => {
  const went = { edition_sent: true, flush: { delivered: ["/K/documents/deckle-daily-2026-11-12.kfx"], failed: [] } };
  expect(summary(sent(went, { edition_sent: true }), true)).toBe("The Daily — Sent to your Kindle.");
  // Another book went; the edition didn't.
  const other = { flush: { delivered: ["/K/documents/Emma.kfx"], failed: [["deckle-daily-2026-11-12.kfx", "refused"]] } };
  expect(summary(sent(other), true)).not.toContain("Sent to your Kindle");
  // It went to a Kobo; no Kindle took it.
  const kobo = sent({ flush: { delivered: ["/K/documents/Emma.kfx"], failed: [] } }, { edition_sent: true, reader: { edition_sent: true } });
  expect(summary(kobo, true)).not.toContain("Sent to your Kindle");
  expect(summary(sent({ queued: true }), true)).toBe("The Daily — Waiting for your Kindle: it goes over when you plug it in.");
  expect(summary({ ok: true, log: "", report: { title: "The Daily" } }, false)).toBe("The Daily — Preview built. Saved articles stay saved until you send.");
});
