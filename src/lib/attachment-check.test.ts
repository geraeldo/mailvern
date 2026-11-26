import { describe, expect, it } from "vitest";
import { mentionsAttachment } from "./attachment-check";

describe("mentionsAttachment", () => {
  it("detects explicit attachment phrases in newly authored text", () => {
    expect(mentionsAttachment("<div>Hi Maya, I've attached the Q4 deck.</div>")).toBe(true);
    expect(mentionsAttachment("<div>Please find attached the invoice.</div>")).toBe(true);
    expect(mentionsAttachment("<div>See attached screenshot for details.</div>")).toBe(true);
  });

  it("ignores quoted replies and signatures", () => {
    expect(
      mentionsAttachment(
        '<div>Sounds great, thanks!</div><div data-signature="1">See attached brochure</div><blockquote>I have attached the contract</blockquote>',
      ),
    ).toBe(false);
  });

  it("does not false-positive on normal prose", () => {
    expect(mentionsAttachment("<div>Let's meet tomorrow at 3pm to discuss the roadmap.</div>")).toBe(false);
  });
});
