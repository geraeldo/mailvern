// Mail as plain text for a language model. HTML is parsed in an inert document, so remote
// images (tracking pixels) never load while reading.

const BLOCK = new Set(["P", "DIV", "TR", "H1", "H2", "H3", "H4", "H5", "H6", "BLOCKQUOTE", "TABLE", "SECTION", "ARTICLE", "HEADER", "FOOTER", "PRE", "HR", "UL", "OL"]);

/** Removes quoted earlier messages: Gmail/Apple quote blocks, "On … wrote:" headers and ">" lines. */
function dropQuotes(doc: Document) {
  doc.querySelectorAll('blockquote[type="cite"], .gmail_quote, .gmail_extra, #appendonsend, .mv-quote-head, div[id^="divRplyFwdMsg"]').forEach((n) => n.remove());
}

export function htmlToPlain(html: string, opts: { stripQuotes?: boolean } = {}): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("script, style, head, title, noscript, template").forEach((n) => n.remove());
  if (opts.stripQuotes) dropQuotes(doc);
  let out = "";
  const walk = (n: Node) => {
    if (n.nodeType === Node.TEXT_NODE) {
      out += (n.textContent ?? "").replace(/\s+/g, " ");
      return;
    }
    if (!(n instanceof Element)) return;
    const tag = n.tagName;
    if (tag === "BR") return void (out += "\n");
    if (tag === "IMG") {
      const alt = n.getAttribute("alt")?.trim();
      if (alt) out += `[image: ${alt}]`;
      return;
    }
    if (tag === "LI") out += "\n• ";
    else if (BLOCK.has(tag)) out += "\n";
    n.childNodes.forEach(walk);
    if (tag === "A") {
      const href = n.getAttribute("href") ?? "";
      const text = (n.textContent ?? "").trim();
      if (/^https?:/i.test(href) && text && !href.includes(text) && !text.includes(href)) out += ` (${href})`;
    } else if (tag === "TD" || tag === "TH") out += " | ";
    else if (BLOCK.has(tag)) out += "\n";
  };
  walk(doc.body);
  return tidy(out);
}

/** Drops quoted lines ("> …") and the "On … wrote:" line before them from plain text. */
export function stripPlainQuotes(text: string): string {
  const lines = text.split("\n");
  const keep: string[] = [];
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (/^\s*>/.test(l)) continue;
    if (/\bwrote:\s*$|\bmenulis:\s*$/i.test(l) && /^\s*>/.test(lines[i + 1] ?? lines[i + 2] ?? "")) continue;
    keep.push(l);
  }
  return tidy(keep.join("\n"));
}

export function tidy(s: string): string {
  return s
    .split("\n")
    .map((l) => l.replace(/[ \t ]+/g, " ").trim())
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/** The readable body of a message. */
export function bodyText(d: { html: string | null; text: string | null }, stripQuotes = false): string {
  if (d.html) return htmlToPlain(d.html, { stripQuotes });
  const t = tidy(d.text ?? "");
  return stripQuotes ? stripPlainQuotes(t) : t;
}

/** At most `max` characters, cut at a word, with a note saying how much is left. */
export function clip(s: string, max: number, offset = 0): string {
  const part = s.slice(offset, offset + max);
  const left = s.length - offset - part.length;
  return left > 0 ? `${part}\n\n[… ${left} more characters. Read on with offset ${offset + part.length}.]` : part;
}
