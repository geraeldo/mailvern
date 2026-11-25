import { describe, expect, it } from "vitest";
import {
  emailDomain, extractEmailAddress, isRemoteImageAllowed, isSharedEmailDomain,
  isTrustedEntry, normalizeTrustEntry, trustableDomain, validateTrustEntry,
} from "./trust";

describe("emailDomain", () => {
  it("extracts lowercased domain", () => {
    expect(emailDomain("Alice@Example.COM")).toBe("example.com");
    expect(emailDomain("  bob@mail.github.com  ")).toBe("mail.github.com");
    expect(emailDomain("invalid")).toBe("");
    expect(emailDomain("@example.com")).toBe("");
  });
});

describe("normalizeTrustEntry", () => {
  it("normalizes exact emails and domains", () => {
    expect(normalizeTrustEntry("Alice@Example.COM")).toBe("alice@example.com");
    expect(normalizeTrustEntry("github.com")).toBe("@github.com");
    expect(normalizeTrustEntry("@GitHub.com")).toBe("@github.com");
    expect(normalizeTrustEntry("*@mail.stripe.com")).toBe("@mail.stripe.com");
    expect(normalizeTrustEntry("  stripe.com  ")).toBe("@stripe.com");
  });

  it("rejects invalid entries", () => {
    expect(normalizeTrustEntry("")).toBeNull();
    expect(normalizeTrustEntry("   ")).toBeNull();
    expect(normalizeTrustEntry("localhost")).toBeNull();
    expect(normalizeTrustEntry("@localhost")).toBeNull();
    expect(normalizeTrustEntry("a@b@c.com")).toBeNull();
    expect(normalizeTrustEntry("has space@example.com")).toBeNull();
    expect(normalizeTrustEntry(".example.com")).toBeNull();
    expect(normalizeTrustEntry("example..com")).toBeNull();
  });
});

describe("isTrustedEntry", () => {
  it("matches exact email addresses case-insensitively", () => {
    expect(isTrustedEntry("Alice@Example.com", ["alice@example.com"])).toBe(true);
    expect(isTrustedEntry("bob@example.com", ["alice@example.com"])).toBe(false);
  });

  it("matches domain entries and their subdomains without matching suffix spoofs", () => {
    const entries = ["@github.com", "@mail.stripe.com"];
    expect(isTrustedEntry("noreply@github.com", entries)).toBe(true);
    expect(isTrustedEntry("bot@notifications.github.com", entries)).toBe(true);
    expect(isTrustedEntry("receipts@mail.stripe.com", entries)).toBe(true);
    expect(isTrustedEntry("phish@notgithub.com", entries)).toBe(false);
    expect(isTrustedEntry("billing@stripe.com", entries)).toBe(false);
  });
});

describe("isRemoteImageAllowed", () => {
  const base = {
    always: false,
    entries: ["alice@example.com", "@github.com"],
    autoTrustSent: true,
    sentRecipients: new Set(["friend@work.org"]),
    ownEmails: new Set(["me@mydomain.com"]),
  };

  it("allows everything when always is enabled", () => {
    expect(isRemoteImageAllowed("stranger@unknown.net", { ...base, always: true })).toBe(true);
  });

  it("honors trusted entries", () => {
    expect(isRemoteImageAllowed("alice@example.com", base)).toBe(true);
    expect(isRemoteImageAllowed("ci@actions.github.com", base)).toBe(true);
    expect(isRemoteImageAllowed("stranger@unknown.net", base)).toBe(false);
  });

  it("honors autoTrustSent setting for Sent recipients and own accounts", () => {
    expect(isRemoteImageAllowed("FRIEND@work.org", base)).toBe(true);
    expect(isRemoteImageAllowed("me@mydomain.com", base)).toBe(true);
    expect(isRemoteImageAllowed("friend@work.org", { ...base, autoTrustSent: false })).toBe(false);
    expect(isRemoteImageAllowed("me@mydomain.com", { ...base, autoTrustSent: false })).toBe(false);
  });
});

describe("extractEmailAddress", () => {
  it("handles bare and display-name formatted addresses", () => {
    expect(extractEmailAddress("Alice <Alice@Example.com>")).toBe("alice@example.com");
    expect(extractEmailAddress("bob@example.com")).toBe("bob@example.com");
  });
});

describe("shared email provider guardrails", () => {
  it("detects shared webmail providers including country TLDs", () => {
    expect(isSharedEmailDomain("gmail.com")).toBe(true);
    expect(isSharedEmailDomain("@Gmail.com")).toBe(true);
    expect(isSharedEmailDomain("yahoo.co.id")).toBe(true);
    expect(isSharedEmailDomain("hotmail.co.uk")).toBe(true);
    expect(isSharedEmailDomain("icloud.com")).toBe(true);
    expect(isSharedEmailDomain("proton.me")).toBe(true);
    expect(isSharedEmailDomain("github.com")).toBe(false);
    expect(isSharedEmailDomain("mail.stripe.com")).toBe(false);
  });

  it("returns empty trustableDomain for shared providers and real domain for orgs", () => {
    expect(trustableDomain("alice@gmail.com")).toBe("");
    expect(trustableDomain("bob@yahoo.co.jp")).toBe("");
    expect(trustableDomain("noreply@github.com")).toBe("github.com");
    expect(trustableDomain("receipts@mail.stripe.com")).toBe("mail.stripe.com");
  });

  it("blocks domain-level trust for shared providers while allowing exact addresses", () => {
    expect(validateTrustEntry("gmail.com")).toEqual({ ok: false, reason: "shared_provider", domain: "gmail.com" });
    expect(validateTrustEntry("@outlook.com")).toEqual({ ok: false, reason: "shared_provider", domain: "outlook.com" });
    expect(validateTrustEntry("alice@gmail.com")).toEqual({ ok: true, entry: "alice@gmail.com" });
    expect(validateTrustEntry("github.com")).toEqual({ ok: true, entry: "@github.com" });
    expect(validateTrustEntry("not valid")).toEqual({ ok: false, reason: "invalid" });
  });
});
