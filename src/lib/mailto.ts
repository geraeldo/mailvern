/** Parsed fields from a `mailto:` URL (RFC 6068). */
export type MailtoData = {
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  body: string;
};

function splitAddrs(raw: string | null | undefined): string[] {
  if (!raw) return [];
  return raw
    .split(/[,;]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function decodePart(raw: string): string {
  try {
    return decodeURIComponent(raw.replace(/\+/g, " "));
  } catch {
    return raw;
  }
}

export function parseMailto(raw: string): MailtoData | null {
  const trimmed = raw.trim();
  if (!/^mailto:/i.test(trimmed)) return null;
  const rest = trimmed.slice(7);
  const qIdx = rest.indexOf("?");
  const addrPart = qIdx >= 0 ? rest.slice(0, qIdx) : rest;
  const queryPart = qIdx >= 0 ? rest.slice(qIdx + 1) : "";

  const to = splitAddrs(decodePart(addrPart));
  const cc: string[] = [];
  const bcc: string[] = [];
  let subject = "";
  let body = "";

  if (queryPart) {
    for (const pair of queryPart.split("&")) {
      if (!pair) continue;
      const eq = pair.indexOf("=");
      const key = decodePart(eq >= 0 ? pair.slice(0, eq) : pair).toLowerCase();
      const val = eq >= 0 ? decodePart(pair.slice(eq + 1)) : "";
      if (key === "to") to.push(...splitAddrs(val));
      else if (key === "cc") cc.push(...splitAddrs(val));
      else if (key === "bcc") bcc.push(...splitAddrs(val));
      else if (key === "subject") subject = val;
      else if (key === "body") body = val;
    }
  }

  return {
    to: [...new Set(to)],
    cc: [...new Set(cc)],
    bcc: [...new Set(bcc)],
    subject,
    body,
  };
}
