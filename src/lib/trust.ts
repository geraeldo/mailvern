const DOMAIN_RE = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+$/;

/** Returns the lowercased domain part of an email address, or "" if invalid. */
export function emailDomain(email: string): string {
  const e = email.trim().toLowerCase();
  const at = e.lastIndexOf("@");
  if (at <= 0 || at === e.length - 1) return "";
  return e.slice(at + 1);
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
