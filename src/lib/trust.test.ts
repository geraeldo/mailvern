import { describe, expect, it } from "vitest";
import { emailDomain, extractEmailAddress, isRemoteImageAllowed, isTrustedEntry, normalizeTrustEntry } from "./trust";

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
