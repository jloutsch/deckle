// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { lastWatchError, onWatchError, setWatchError } from "./watchStatus";

describe("the watched folder's last look", () => {
  it("is shared with whoever listens, and a later good look clears it", () => {
    const heard: (string | null)[] = [];
    const stop = onWatchError((e) => heard.push(e));
    setWatchError("watched.json could not be read");
    expect(lastWatchError()).toBe("watched.json could not be read");
    setWatchError(null);
    stop();
    setWatchError("after");
    expect(heard).toEqual(["watched.json could not be read", null]);
    expect(lastWatchError()).toBe("after");
  });
});
