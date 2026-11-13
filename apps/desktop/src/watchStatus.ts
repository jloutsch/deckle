// SPDX-License-Identifier: GPL-3.0-or-later
/**
 * Why the last look at the watched folder failed (a damaged list of files tried, a folder gone),
 * or null. App's two-minute look is the only one that adds books; Settings shows its result
 * beside the folder instead of looking again.
 */
let last: string | null = null;
const listeners = new Set<(e: string | null) => void>();

export function setWatchError(e: string | null) {
  last = e;
  listeners.forEach((f) => f(e));
}

export const lastWatchError = () => last;

/** Calls `f` on each later look; returns the unsubscribe. */
export function onWatchError(f: (e: string | null) => void): () => void {
  listeners.add(f);
  return () => {
    listeners.delete(f);
  };
}
