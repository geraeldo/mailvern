const DOMAIN_RE = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+$/;

/**
 * Shared personal/public email providers (mirrors crates/mail-core/src/icons.rs).
 * Trusting these at the domain level would trust every user on the provider.
 */
const SHARED_EMAIL_DOMAINS = new Set([
  "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "msn.com", "passport.com",
  "icloud.com", "me.com", "mac.com", "aol.com", "aim.com", "ymail.com", "rocketmail.com",
  "proton.me", "protonmail.com", "protonmail.ch", "pm.me", "tutanota.com", "tuta.io", "tutamail.com",
  "mail.com", "gmx.com", "gmx.net", "zoho.com", "zohomail.com", "fastmail.com", "fastmail.fm", "hey.com",
  "yandex.com", "yandex.ru", "mail.ru", "inbox.ru", "list.ru", "bk.ru", "qq.com", "163.com", "126.com",
  "yeah.net", "sina.com", "naver.com", "daum.net", "hanmail.net", "web.de", "t-online.de", "orange.fr",
  "free.fr", "laposte.net", "libero.it", "virgilio.it", "btinternet.com", "comcast.net", "att.net",
  "verizon.net", "sbcglobal.net", "cox.net", "mailinator.com", "duck.com", "skiff.com", "hushmail.com",
]);

/** Providers with country-specific TLDs (e.g. yahoo.co.id, hotmail.co.uk, gmx.de). */
const SHARED_EMAIL_LABELS = new Set(["yahoo", "hotmail", "outlook", "live", "gmx", "yandex", "rediffmail"]);

const SECOND_LEVEL_TLDS = new Set(["co", "com", "net", "org", "edu", "gov", "ac"]);

function orgDomain(domain: string): string {
  const parts = domain.split(".");
  if (parts.length <= 2) return domain;
  const last = parts[parts.length - 1];
  const prev = parts[parts.length - 2];
  if (last.length === 2 && SECOND_LEVEL_TLDS.has(prev) && parts.length >= 3) {
    return parts.slice(-3).join(".");
  }
  return parts.slice(-2).join(".");
}

/** Returns true if `domain` is a shared/public email provider like gmail.com or yahoo.co.uk. */
export function isSharedEmailDomain(domain: string): boolean {
  const d = domain.trim().replace(/^@+/, "").replace(/\.+$/, "").toLowerCase();
  if (!d || !d.includes(".")) return false;
  const org = orgDomain(d);
  const first = org.split(".")[0] ?? "";
  return SHARED_EMAIL_DOMAINS.has(org) || SHARED_EMAIL_LABELS.has(first);
}

/** Returns the lowercased domain part of an email address, or "" if invalid. */
export function emailDomain(email: string): string {
  const e = email.trim().toLowerCase();
  const at = e.lastIndexOf("@");
  if (at <= 0 || at === e.length - 1) return "";
  return e.slice(at + 1);
}

/** Returns the sender's domain if it is safe to offer domain-level trust, or "" for shared providers. */
export function trustableDomain(email: string): string {
  const d = emailDomain(email);
  return d && !isSharedEmailDomain(d) ? d : "";
}

/**
 * Normalizes a user-entered trusted sender or domain entry.
 * - `alice@example.com` -> `alice@example.com`
 * - `@example.com`, `*@example.com`, `example.com` -> `@example.com`
 * Returns null if the input is not a valid email address or domain.
 */
export function normalizeTrustEntry(raw: string): string | null {
  let s = raw.trim().toLowerCase();
  if (!s || /\s/.test(s)) return null;
  if (s.startsWith("*@")) s = s.slice(1);
  if (s.startsWith("@")) {
    const domain = s.slice(1);
    return DOMAIN_RE.test(domain) ? `@${domain}` : null;
  }
  const at = s.indexOf("@");
  if (at === -1) {
    return DOMAIN_RE.test(s) ? `@${s}` : null;
  }
  if (s.indexOf("@", at + 1) !== -1) return null;
  const local = s.slice(0, at);
  const domain = s.slice(at + 1);
  if (!local || !DOMAIN_RE.test(domain)) return null;
  return `${local}@${domain}`;
}

export type TrustEntryResult =
  | { ok: true; entry: string }
  | { ok: false; reason: "invalid" }
  | { ok: false; reason: "shared_provider"; domain: string };

/** Normalizes and validates a trust entry, rejecting domain-level entries for shared email providers. */
export function validateTrustEntry(raw: string): TrustEntryResult {
  const entry = normalizeTrustEntry(raw);
  if (!entry) return { ok: false, reason: "invalid" };
  if (entry.startsWith("@")) {
    const domain = entry.slice(1);
    if (isSharedEmailDomain(domain)) {
      return { ok: false, reason: "shared_provider", domain };
    }
  }
  return { ok: true, entry };
}

/**
 * Checks whether an email address matches any entry in the trusted list.
 * Exact emails match case-insensitively; `@domain.com` entries match `domain.com`
 * as well as subdomains such as `mail.domain.com`.
 */
export function isTrustedEntry(email: string, entries: readonly string[]): boolean {
  const e = email.trim().toLowerCase();
  if (!e) return false;
  const domain = emailDomain(e);
  for (const raw of entries) {
    const entry = raw.trim().toLowerCase();
    if (!entry) continue;
    if (entry.startsWith("@")) {
      const d = entry.slice(1);
      if (d && domain && (domain === d || domain.endsWith(`.${d}`))) return true;
    } else if (entry === e) {
      return true;
    }
  }
  return false;
}

export type RemoteImagePolicy = {
  always: boolean;
  entries: readonly string[];
  autoTrustSent: boolean;
  sentRecipients: ReadonlySet<string>;
  ownEmails: ReadonlySet<string>;
};

/** Whether remote images should load automatically for messages from `email`. */
export function isRemoteImageAllowed(email: string, policy: RemoteImagePolicy): boolean {
  if (policy.always) return true;
  const e = email.trim().toLowerCase();
  if (!e) return false;
  if (isTrustedEntry(e, policy.entries)) return true;
  if (policy.autoTrustSent && (policy.sentRecipients.has(e) || policy.ownEmails.has(e))) return true;
  return false;
}

/** Extracts a lowercased email address from either `Name <addr@host>` or `addr@host`. */
export function extractEmailAddress(raw: string): string {
  const m = raw.match(/<([^>]+)>/);
  return (m ? m[1] : raw).trim().toLowerCase();
}
