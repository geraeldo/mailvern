import { describe, expect, it } from "vitest";
import { parseMailto } from "./mailto";

describe("parseMailto", () => {
  it("parses recipients, cc, bcc, subject, and body", () => {
    const res = parseMailto(
      "mailto:alice@example.com,bob@example.org?subject=Hello%20World&cc=carol@example.com&bcc=dan@example.com&body=Line%201%0ALine%202",
    );
    expect(res).toEqual({
      to: ["alice@example.com", "bob@example.org"],
      cc: ["carol@example.com"],
      bcc: ["dan@example.com"],
      subject: "Hello World",
      body: "Line 1\nLine 2",
    });
  });

  it("handles query-only to= and plus-encoded spaces", () => {
    const res = parseMailto("mailto:?to=help@acme.co&subject=Order+123");
    expect(res).toEqual({
      to: ["help@acme.co"],
      cc: [],
      bcc: [],
      subject: "Order 123",
      body: "",
    });
  });

  it("returns null for non-mailto URLs", () => {
    expect(parseMailto("https://example.com")).toBeNull();
  });
});
