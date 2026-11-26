import { describe, expect, it } from "vitest";
import { emailWrap, extractRemoteImageUrls, frameDoc, hasRemoteContent, htmlToText, prefixed, sanitize, textToHtml, unwrapEmail } from "./html";

describe("sanitize", () => {
  it("keeps safe markup, safe links, raster data images, and allowlisted styles", () => {
    const input = [
      `<p onclick="alert(1)">Hello <b>world</b></p>`,
      `<a href="https://example.com/ok">good</a>`,
      `<img src="data:image/png;base64,iVBORw0KGgo=" alt="ok" width="120" onerror="alert(6)">`,
      `<div style="position:fixed;background:url(https://evil.test/x);color:red;font-weight:bold">styled</div>`,
    ].join("");

    const out = sanitize(input);
    expect(out).toContain("<p>Hello <b>world</b></p>");
    expect(out).toContain(`<a href="https://example.com/ok">good</a>`);
    expect(out).toContain(`<img src="data:image/png;base64,iVBORw0KGgo=" alt="ok" width="120">`);
    expect(out).toContain(`<div style="color:red;font-weight:bold">styled</div>`);
    expect(out).not.toMatch(/onclick|onerror|position:|url\(/i);
  });

  it("drops scripts, svg/math, noscript, template, remote images, svg data URIs, and javascript: links", () => {
    expect(sanitize(`<script>alert(1)</script><p>ok</p>`)).toBe("<p>ok</p>");
    expect(sanitize(`<p>ok</p><svg onload="alert(1)"><script>alert(2)</script></svg>`)).toBe("<p>ok</p>");
    expect(sanitize(`<svg onload="alert(1)"><circle r="5"/></svg><p>ok</p>`)).toBe("<p>ok</p>");
    expect(sanitize(`<math><mtext><img src=x onerror=alert(3)></mtext></math><p>ok</p>`)).toBe("<p>ok</p>");
    expect(sanitize(`<noscript><img src=x onerror=alert(4)></noscript><p>ok</p>`)).toBe("<p>ok</p>");
    expect(sanitize(`<template><script>alert(5)</script></template><p>ok</p>`)).toBe("<p>ok</p>");
    expect(sanitize(`<a href="javascript:alert(1)">bad</a>`)).toBe("<a>bad</a>");
    expect(sanitize(`<a href="  java\tscript:alert(1)">bad2</a>`)).toBe("<a>bad2</a>");
    expect(sanitize(`<img src="https://tracker.example/pixel.gif"><p>ok</p>`)).toBe("<p>ok</p>");
    expect(sanitize(`<img src="data:image/svg+xml;base64,PHN2Zz4="><p>ok</p>`)).toBe("<p>ok</p>");
    expect(sanitize(`<img src="mailvern-cid://localhost/42/0" alt="inline">`)).toBe(`<img src="mailvern-cid://localhost/42/0" alt="inline">`);
    expect(sanitize(`<img src="mailvern-cid://evil/42/0"><img src="mailvern-cid://localhost/0/0"><p>ok</p>`)).toBe("<p>ok</p>");
  });
});

describe("htmlToText", () => {
  it("converts HTML safely without executing scripts or keeping style/script text", () => {
    const html = `<style>body{color:red}</style><script>window.__xss=1</script><div>Hello<br>World</div><blockquote>Quoted</blockquote>`;
    const txt = htmlToText(html);
    expect(txt).toContain("Hello\nWorld");
    expect(txt).toContain("> Quoted");
    expect(txt).not.toContain("color:red");
    expect(txt).not.toContain("__xss");
  });
});

describe("frameDoc", () => {
  it("strips meta refresh, base, script, and target attributes while keeping head styles", () => {
    const html = `<html><head><style>.a{color:blue}</style><meta http-equiv="refresh" content="0;url=https://evil.test"><base href="https://evil.test/"></head><body><script>alert(1)</script><a href="https://ok.test" target="_blank">go</a></body></html>`;
    const doc = frameDoc(html, false);
    expect(doc).toContain(".a{color:blue}");
    expect(doc).toContain(`<a href="https://ok.test">go</a>`);
    expect(doc).not.toContain("http-equiv=\"refresh\"");
    expect(doc).not.toContain("<base");
    expect(doc).not.toContain("<script");
    expect(doc).not.toContain("target=");
    expect(doc).toContain("img-src data: cid: mailvern-cid:; style-src 'unsafe-inline'");

    const remoteDoc = frameDoc(`<img src="https://ok.test/a.png">`, true);
    expect(remoteDoc).toContain("img-src data: cid: mailvern-cid: https: http:");
  });

  it("strips tracking pixels and known tracker domains when stripTrackers is enabled", () => {
    const html = [
      `<img src="https://cdn.example.com/photo.jpg" width="400" height="300">`,
      `<img src="https://cdn.example.com/tiny.gif" width="1" height="1">`,
      `<img src="https://cdn.example.com/hidden.png" style="display:none">`,
      `<img src="https://mailtrack.io/trace/mail/abc123.png">`,
      `<img src="https://u123.ct.sendgrid.net/wf/open?upn=xyz">`,
      `<img src="data:image/gif;base64,R0lGODlhAQABAAAAACw=" width="1" height="1">`,
    ].join("");
    const stripped = frameDoc(html, true, false, true);
    expect(stripped).toContain("https://cdn.example.com/photo.jpg");
    expect(stripped).toContain("data:image/gif;base64");
    expect(stripped).not.toContain("tiny.gif");
    expect(stripped).not.toContain("hidden.png");
    expect(stripped).not.toContain("mailtrack.io");
    expect(stripped).not.toContain("sendgrid.net/wf/open");

    const unstripped = frameDoc(html, true, false, false);
    expect(unstripped).toContain("mailtrack.io");
  });
});

describe("hasRemoteContent", () => {
  it("detects remote images, srcset, CSS url(), @import, and external stylesheets", () => {
    expect(hasRemoteContent(`<img src="https://example.com/a.png">`)).toBe(true);
    expect(hasRemoteContent(`<img srcset="//cdn.example.com/a.png 2x">`)).toBe(true);
    expect(hasRemoteContent(`<div style="background:url(https://a.test/bg.png)"></div>`)).toBe(true);
    expect(hasRemoteContent(`<style>@import "https://a.test/x.css";</style>`)).toBe(true);
    expect(hasRemoteContent(`<link rel="stylesheet" href="https://a.test/x.css">`)).toBe(true);
    expect(hasRemoteContent(`<img src="data:image/png;base64,AQID">`)).toBe(false);
    expect(hasRemoteContent(`<a href="https://example.com">link</a>`)).toBe(false);
  });
});

describe("extractRemoteImageUrls", () => {
  it("extracts deduplicated http(s) img src and srcset URLs while skipping data URIs and 1x1 pixels", () => {
    const html = [
      `<img src="https://cdn.example.com/hero.png">`,
      `<img src="https://cdn.example.com/hero.png">`,
      `<img src="//cdn.example.com/banner.jpg" srcset="https://cdn.example.com/banner@2x.jpg 2x">`,
      `<img src="data:image/png;base64,AQID">`,
      `<img src="https://track.example.com/open.gif" width="1" height="1">`,
      `<img src="https://track.example.com/zero.gif" width="0" height="0">`,
    ].join("");

    expect(extractRemoteImageUrls(html)).toEqual([
      "https://cdn.example.com/hero.png",
      "https://cdn.example.com/banner.jpg",
      "https://cdn.example.com/banner@2x.jpg",
    ]);
  });
});

describe("emailWrap, unwrapEmail, textToHtml, and prefixed", () => {
  it("round-trips wrapped HTML and strips unsafe markup on unwrap", () => {
    const wrapped = emailWrap(`<div>Hi</div><img src="https://evil.test/p.gif">`);
    expect(unwrapEmail(wrapped)).toBe("<div>Hi</div>");
  });

  it("linkifies plain text safely", () => {
    expect(textToHtml("Visit https://example.com & <smile>")).toContain(
      `<a href="https://example.com">https://example.com</a> &amp; &lt;smile&gt;`,
    );
  });

  it("prefixes subjects idempotently", () => {
    expect(prefixed("Re", "Hello")).toBe("Re: Hello");
    expect(prefixed("Re", "re: Hello")).toBe("re: Hello");
    expect(prefixed("Fwd", "  FWD : Report")).toBe("  FWD : Report");
  });
});
