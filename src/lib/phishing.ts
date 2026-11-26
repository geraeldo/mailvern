import type { SecurityWarning } from "./api";

/** Extracts the base (eTLD+1-ish) host for quick mismatch checks. */
function baseHost(host: string): string {
  const clean = host.toLowerCase().replace(/^www\./, "").replace(/\.$/, "");
  const parts = clean.split(".").filter(Boolean);
  if (parts.length <= 2) return clean;
  // Common 2-part public suffixes (co.uk, com.au, co.jp, co.id, etc.).
  const lastTwo = parts.slice(-2).join(".");
  if (/^(?:co|com|org|net|gov|edu|ac)\.[a-z]{2}$/.test(lastTwo) && parts.length >= 3) {
    return parts.slice(-3).join(".");
  }
  return lastTwo;
}

const IP_HOST = /^(?:\d{1,3}\.){3}\d{1,3}$|^\[[0-9a-f:]+\]$/i;
const VISIBLE_URL = /^(?:https?:\/\/|www\.)([^\s/]+)/i;

/** Scans an already-parsed HTML Document for high-signal deceptive links (zero extra DOMParser pass). */
export function scanDocPhishing(doc: Document): SecurityWarning[] {
  let deceptiveHref: { shown: string; actual: string } | null = null;
  let credTrickHost: string | null = null;
  let ipLinkHost: string | null = null;
  let punycodeHost: string | null = null;

  for (const a of doc.querySelectorAll("a[href]")) {
    const rawHref = (a.getAttribute("href") ?? "").trim();
    if (!/^https?:\/\//i.test(rawHref)) continue;
    let parsed: URL;
    try {
      parsed = new URL(rawHref);
    } catch {
      continue;
    }
    const actualHost = parsed.hostname.toLowerCase();
    if (!actualHost) continue;

    if ((parsed.username || parsed.password) && !credTrickHost) {
      credTrickHost = actualHost;
    }
    if (IP_HOST.test(actualHost) && !ipLinkHost && !/^127\.|^10\.|^192\.168\./.test(actualHost)) {
      ipLinkHost = actualHost;
    }
    if (actualHost.includes("xn--") && !punycodeHost) {
      punycodeHost = actualHost;
    }

    const visible = (a.textContent ?? "").trim();
    const m = visible.match(VISIBLE_URL);
    if (m && !deceptiveHref) {
      try {
        const shownUrl = new URL(/^https?:\/\//i.test(visible) ? visible : `https://${visible}`);
        const shownHost = shownUrl.hostname.toLowerCase();
        if (shownHost && baseHost(shownHost) !== baseHost(actualHost)) {
          deceptiveHref = { shown: shownHost, actual: actualHost };
        }
      } catch {
        // ignore unparseable visible text
      }
    }
  }

  const out: SecurityWarning[] = [];
  if (deceptiveHref) {
    out.push({
      kind: "deceptive_link",
      severity: "danger",
      title: "Deceptive link detected",
      detail: `A link displays “${deceptiveHref.shown}” but actually goes to ${deceptiveHref.actual}.`,
    });
  }
  if (credTrickHost) {
    out.push({
      kind: "credential_url",
      severity: "danger",
      title: "Suspicious link format",
      detail: `A link embeds an @ sign to disguise its real destination (${credTrickHost}).`,
    });
  }
  if (ipLinkHost) {
    out.push({
      kind: "ip_link",
      severity: "warn",
      title: "Link points to a raw IP address",
      detail: `A link goes directly to ${ipLinkHost} instead of a named website.`,
    });
  }
  if (punycodeHost) {
    out.push({
      kind: "punycode_link",
      severity: "warn",
      title: "Lookalike link domain",
      detail: `A link uses an internationalized domain (${punycodeHost}).`,
    });
  }
  return out;
}

/** Scans email HTML for high-signal deceptive links (mismatched URL text, `@` tricks, raw IPs, punycode). */
export function scanHtmlPhishing(html: string | null | undefined): SecurityWarning[] {
  if (!html) return [];
  const doc = new DOMParser().parseFromString(html, "text/html");
  return scanDocPhishing(doc);
}
