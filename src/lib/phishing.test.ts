import { describe, expect, it } from "vitest";
import { scanHtmlPhishing } from "./phishing";

describe("scanHtmlPhishing", () => {
  it("flags deceptive visible URLs pointing to a different domain", () => {
    const html = `<p>Verify at <a href="https://evil-phish.example/login">https://www.paypal.com/verify</a></p>`;
    const res = scanHtmlPhishing(html);
    expect(res.some((w) => w.kind === "deceptive_link" && w.severity === "danger")).toBe(true);
  });

  it("flags @ credential-trick URLs and raw public IPs", () => {
    const html = `<a href="https://bank.com@attacker.org/x">Click</a> <a href="http://185.10.20.30/login">Here</a>`;
    const res = scanHtmlPhishing(html);
    expect(res.some((w) => w.kind === "credential_url")).toBe(true);
    expect(res.some((w) => w.kind === "ip_link")).toBe(true);
  });

  it("does not flag normal links or subdomain links", () => {
    const html = `<a href="https://docs.github.com/en">https://github.com/docs</a> <a href="https://example.com">Read more</a>`;
    expect(scanHtmlPhishing(html)).toEqual([]);
  });
});
