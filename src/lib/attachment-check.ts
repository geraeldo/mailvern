const MENTION_PATTERNS = [
  /\b(?:i(?:'ve| have)?|we(?:'ve| have)?)\s+attached\b/i,
  /\b(?:please\s+)?(?:find|see)\s+(?:the\s+)?(?:attached|enclosed)\b/i,
  /\battached\s+(?:is|are|here|herewith|below|file|files|pdf|doc|document|invoice|receipt|resume|cv|deck|slides|photo|image|screenshot|spreadsheet|report)\b/i,
  /\battaching\s+(?:the|a|my|our)\b/i,
  /\bin\s+the\s+attachment\b/i,
  /\bsee\s+attachment\b/i,
];

/**
 * Extracts only the user's freshly typed text from a composer HTML body, ignoring
 * signatures (`[data-signature]`) and quoted replies/forwards (`blockquote`).
 */
export function authoredText(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("[data-signature], blockquote").forEach((n) => n.remove());
  return (doc.body.textContent ?? "").trim();
}

/** True when the newly written message text appears to mention an attachment. */
export function mentionsAttachment(html: string): boolean {
  const text = authoredText(html);
  if (!text) return false;
  return MENTION_PATTERNS.some((re) => re.test(text));
}
