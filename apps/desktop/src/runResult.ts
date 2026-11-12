// SPDX-License-Identifier: GPL-3.0-or-later
// What a Send's result says besides the edition (device delivery safety design): the outbox
// entries that didn't go, and whether the run box shows the run as failed.
import type { RunResult } from "./types";

export interface NotSent {
  name: string;
  why: string;
}

/** Outbox entries the push left unsent, to the Kindle and to a Kobo or PocketBook: (name, why). */
export function notSent(report: any): NotSent[] {
  const lists = [report?.outcome?.flush?.failed, report?.reader?.flush?.failed];
  return lists
    .flatMap((l) => (Array.isArray(l) ? l : []))
    .filter((f) => Array.isArray(f) && f.length >= 2)
    .map(([name, why]) => ({ name: String(name), why: String(why) }));
}

/** Red only when the run failed and its own edition didn't go: another entry is a warning. */
export function runFailed(r: RunResult): boolean {
  return !r.ok && r.report?.edition_sent !== true;
}
