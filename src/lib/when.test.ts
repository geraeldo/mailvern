import { describe, expect, it } from "vitest";
import { sendPresets, snoozePresets, toLocalInput, toUnix } from "./when";

describe("when helpers", () => {
  it("computes snooze and send presets in the future", () => {
    // Wednesday 2026-11-25 10:15 local time
    const now = new Date(2026, 10, 25, 10, 15, 0);
    const snooze = snoozePresets(now);
    expect(snooze.map((p) => p.label)).toEqual(["Later today", "This evening", "Tomorrow", "This weekend", "Next week"]);
    for (const p of snooze) expect(p.at.getTime()).toBeGreaterThan(now.getTime());

    const send = sendPresets(now);
    expect(send.map((p) => p.label)).toEqual(["In an hour", "Tomorrow morning", "Tomorrow afternoon", "Monday morning"]);
    for (const p of send) expect(p.at.getTime()).toBeGreaterThan(now.getTime());
  });

  it("formats datetime-local strings and unix seconds", () => {
    const d = new Date(2026, 0, 5, 9, 7, 30);
    expect(toLocalInput(d)).toBe("2026-01-05T09:07");
    expect(toUnix(d)).toBe(Math.floor(d.getTime() / 1000));
  });
});
