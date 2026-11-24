import { describe, expect, it } from "vitest";
import { clean, prepare } from "./rephrase";

describe("rephrase prepare & clean", () => {
  const png = "data:image/png;base64,iVBORw0KGgo=";

  it("extracts safe inline images and restores them in text nodes", () => {
    const prep = prepare(`<div>Hi <img src="${png}" alt="pic" width="200" onclick="alert(1)"><img src="https://tracker.test/p.gif"></div>`);
    expect(prep.images).toHaveLength(1);
    expect(prep.html).toContain("[[image 1]]");
    expect(prep.html).not.toContain("tracker.test");

    const cleaned = clean(`<div>Hello [[image 1]]!</div>`, prep.images);
    expect(cleaned).toContain(`<img src="${png}" width="200" style="max-width:100%;height:auto" alt="pic">`);
    expect(cleaned).not.toContain("onclick");
  });

  it("never interpolates image placeholders inside HTML attributes (blocks attribute breakout XSS)", () => {
    const prep = prepare(`<div><img src="${png}" alt="x"></div>`);
    const malicious = `<div><a href="https://example.com/[[image 1]]">click</a></div>`;
    const cleaned = clean(malicious, prep.images);
    expect(cleaned).toBe(`<div><a href="https://example.com/[[image 1]]">click</a></div>`);
    expect(cleaned).not.toContain("<img");
  });

  it("strips dangerous tags and javascript: links from model output", () => {
    const out = clean(`<script>alert(1)</script><a href="javascript:alert(2)">x</a><b>ok</b>`, []);
    expect(out).toBe("<a>x</a><b>ok</b>");
  });
});
