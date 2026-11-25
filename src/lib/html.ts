import type { Addr, Detail } from "./api";
import { formatAddr, longDate } from "./format";

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

function linkify(escaped: string): string {
  return escaped.replace(/\bhttps?:\/\/[^\s<]+[^\s<.,;:!?)\]'"]/g, (u) => `<a href="${u}">${u}</a>`);
}

export function textToHtml(text: string): string {
  return `<div style="white-space:pre-wrap">${linkify(escapeHtml(text))}</div>`;
}

const REMOTE =
  /(?:src|srcset|background|poster)\s*=\s*["']?\s*(?:https?:|\/\/)|url\(\s*["']?\s*(?:https?:|\/\/)|@import\s+(?:url\(\s*)?["']?\s*(?:https?:|\/\/)|<link\b[^>]*\bhref\s*=\s*["']?\s*(?:https?:|\/\/)/i;

export function hasRemoteContent(html: string): boolean {
  return REMOTE.test(html);
}

const MAX_PRELOAD_IMAGES = 24;

function normalizeHttpUrl(raw: string): string | null {
  const s = raw.trim();
  if (!s) return null;
  if (s.startsWith("//")) return `https:${s}`;
  if (/^https?:\/\//i.test(s)) return s;
  return null;
}

function isTinyPixel(el: Element): boolean {
  const w = el.getAttribute("width")?.trim();
  const h = el.getAttribute("height")?.trim();
  return (w === "0" || w === "1") && (h === "0" || h === "1");
}

/**
 * Extracts deduplicated remote http(s) image URLs from email HTML for background preloading
 * (only called for explicitly trusted senders). Skips explicit 0x0 / 1x1 tracking pixels.
 */
export function extractRemoteImageUrls(html: string): string[] {
  if (!hasRemoteContent(html)) return [];
  const doc = new DOMParser().parseFromString(html, "text/html");
  const urls: string[] = [];
  const seen = new Set<string>();
  const add = (raw: string | null | undefined) => {
    if (!raw || urls.length >= MAX_PRELOAD_IMAGES) return;
    const u = normalizeHttpUrl(raw);
    if (u && !seen.has(u)) {
      seen.add(u);
      urls.push(u);
    }
  };

  for (const img of doc.querySelectorAll("img")) {
    if (isTinyPixel(img)) continue;
    add(img.getAttribute("src"));
    const srcset = img.getAttribute("srcset");
    if (srcset) {
      for (const candidate of srcset.split(",")) {
        const first = candidate.trim().split(/\s+/)[0];
        add(first);
      }
    }
    if (urls.length >= MAX_PRELOAD_IMAGES) break;
  }
  return urls;
}

/**
 * Prepares untrusted email HTML for the sandboxed reading frame:
 * - Removes `<meta>` (preventing `<meta http-equiv="refresh">` zero-click navigation),
 *   `<base>` (preventing link hijacking), `<script>`, and active embeds.
 * - Drops `target` attributes via DOM APIs so every link navigates the frame itself
 *   and is intercepted cleanly by the app's external-link handler.
 */
function prepareFrameBody(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("script, meta, base, iframe, object, embed, applet, frame, frameset, portal").forEach((n) => n.remove());
  doc.querySelectorAll("a[target], area[target]").forEach((el) => el.removeAttribute("target"));
  // Preserve <style> blocks that originally lived in <head> (DOMParser puts them in doc.head).
  const headStyles = Array.from(doc.head.querySelectorAll("style"))
    .map((s) => s.outerHTML)
    .join("");
  return headStyles + doc.body.innerHTML;
}

/** A self-contained document for the sandboxed reading frame. Scripts never run. */
export function frameDoc(body: string, allowRemote: boolean, plain = false): string {
  const remote = allowRemote ? " https: http:" : "";
  const csp = `default-src 'none'; img-src data: cid:${remote}; style-src 'unsafe-inline'${remote}; font-src data:${remote}; media-src data:${remote}`;
  const colorScheme = plain ? "light dark" : "light";
  const darkOverride = plain
    ? `<style>
  @media (prefers-color-scheme: dark) {
    html,body{background:#1b1b1a;color:#edece9}
    a{color:#5d87ff}
    blockquote{border-left-color:#3a3936;color:#a6a39c}
  }
</style>`
    : "";
  return `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="color-scheme" content="${colorScheme}">
<style>
  html,body{margin:0;padding:0;background:#fff;overflow:hidden}
  body{padding:4px 2px 12px;display:flow-root;font:14px/1.55 -apple-system,BlinkMacSystemFont,"Helvetica Neue",sans-serif;color:#1d1c1a;word-wrap:break-word;overflow-wrap:anywhere}
  img{max-width:100%;height:auto}
  table{max-width:100%}
  pre{white-space:pre-wrap}
  a{color:#2459e0}
  blockquote{margin:0 0 0 2px;padding-left:12px;border-left:2px solid #e3e1dc;color:#55524c}
</style>${darkOverride}</head><body>${prepareFrameBody(body)}</body></html>`;
}

const XHTML_NS = "http://www.w3.org/1999/xhtml";

/** Elements dropped along with their entire subtrees (prevents scripts, embeds, and mXSS namespaces). */
const DROP_TAGS = new Set([
  "SCRIPT", "STYLE", "LINK", "META", "IFRAME", "OBJECT", "EMBED", "FORM", "BASE", "TITLE", "HEAD",
  "NOSCRIPT", "TEMPLATE", "SVG", "MATH", "AUDIO", "VIDEO", "SOURCE", "TRACK", "CANVAS", "PORTAL",
  "FRAME", "FRAMESET", "APPLET", "TEXTAREA", "SELECT", "OPTION", "OPTGROUP", "BUTTON", "INPUT",
  "FIELDSET", "LEGEND", "DATALIST", "OUTPUT", "DIALOG", "XMP", "PLAINTEXT", "LISTING", "NOEMBED", "NOFRAMES",
]);

/** Strict allowlist of safe HTML elements permitted in the main webview composer. */
const SAFE_TAGS = new Set([
  "A", "ABBR", "B", "BLOCKQUOTE", "BR", "CAPTION", "CODE", "COL", "COLGROUP", "DD", "DEL", "DIV",
  "DL", "DT", "EM", "FIGCAPTION", "FIGURE", "FONT", "H1", "H2", "H3", "H4", "H5", "H6", "HR", "I",
  "IMG", "INS", "LI", "MARK", "OL", "P", "PRE", "Q", "S", "SMALL", "SPAN", "STRIKE", "STRONG",
  "SUB", "SUP", "TABLE", "TBODY", "TD", "TFOOT", "TH", "THEAD", "TR", "U", "UL",
]);

/** Safe inline CSS properties allowed in the composer (no positioning, z-index, or external URLs). */
const SAFE_CSS_PROPS = new Set([
  "color", "background-color", "font-family", "font-size", "font-weight", "font-style",
  "text-decoration", "text-align", "white-space", "line-height",
  "margin", "margin-top", "margin-right", "margin-bottom", "margin-left",
  "padding", "padding-top", "padding-right", "padding-bottom", "padding-left",
  "border", "border-top", "border-right", "border-bottom", "border-left",
  "border-color", "border-style", "border-width", "border-collapse",
  "max-width", "width", "height", "vertical-align", "list-style-type",
]);

const SAFE_DATA_IMAGE = /^data:image\/(?:png|jpeg|jpg|gif|webp);base64,[a-z0-9+/=\s]+$/i;

function isSafeHref(raw: string): boolean {
  // Strip ASCII control characters and whitespace that browsers ignore when parsing URL schemes.
  const cleaned = raw.replace(/[\u0000-\u0020\u007f]+/g, "");
  return /^(?:https?:|mailto:|tel:)/i.test(cleaned);
}

function sanitizeStyle(raw: string): string | null {
  // Strip CSS comments before inspecting declarations.
  const cleaned = raw.replace(/\/\*[\s\S]*?\*\//g, "");
  const kept: string[] = [];
  for (const part of cleaned.split(";")) {
    const colon = part.indexOf(":");
    if (colon < 1) continue;
    const prop = part.slice(0, colon).trim().toLowerCase();
    const val = part.slice(colon + 1).trim();
    if (!SAFE_CSS_PROPS.has(prop) || !val) continue;
    // Block CSS escapes, external fetches, expressions, and injection chars.
    if (/[\\{}<>]|url\s*\(|expression\s*\(|@import|javascript:|behavior\s*:/i.test(val)) continue;
    kept.push(`${prop}:${val}`);
  }
  return kept.length ? kept.join(";") : null;
}

/**
 * Strict allowlist DOM sanitizer for untrusted HTML before placing it in the main app DOM
 * (quoted replies/forwards, reopened drafts, pasted HTML, and signatures).
 * Rebuilds nodes into a fresh inert document so scripting-disabled parser quirks (`<noscript>`,
 * `<template>`, `<svg>`, `<math>`) cannot survive into the live webview.
 */
export function sanitize(html: string): string {
  const srcDoc = new DOMParser().parseFromString(html, "text/html");
  const outDoc = document.implementation.createHTMLDocument("");

  const cleanNode = (node: Node, parent: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      parent.appendChild(outDoc.createTextNode(node.textContent ?? ""));
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const el = node as Element;
    // Reject non-HTML namespaces (SVG, MathML) completely.
    if (el.namespaceURI !== XHTML_NS) return;
    const tag = el.tagName.toUpperCase();
    if (DROP_TAGS.has(tag)) return;

    if (!SAFE_TAGS.has(tag)) {
      for (const child of Array.from(el.childNodes)) cleanNode(child, parent);
      return;
    }

    const copy = outDoc.createElement(tag.toLowerCase());
    for (const a of Array.from(el.attributes)) {
      const name = a.name.toLowerCase();
      const val = a.value;
      if (name === "style") {
        const safeStyle = sanitizeStyle(val);
        if (safeStyle) copy.setAttribute("style", safeStyle);
      } else if (tag === "A" && name === "href") {
        if (isSafeHref(val)) copy.setAttribute("href", val.trim());
      } else if (tag === "IMG") {
        if (name === "src" && SAFE_DATA_IMAGE.test(val.trim())) copy.setAttribute("src", val.trim());
        else if (name === "alt") copy.setAttribute("alt", val);
        else if ((name === "width" || name === "height") && /^\d{1,4}$/.test(val.trim())) copy.setAttribute(name, val.trim());
      } else if (tag === "BLOCKQUOTE" && name === "type" && val.toLowerCase() === "cite") {
        copy.setAttribute("type", "cite");
      } else if ((tag === "TD" || tag === "TH") && (name === "colspan" || name === "rowspan") && /^\d{1,3}$/.test(val.trim())) {
        copy.setAttribute(name, val.trim());
      } else if (
        (tag === "TABLE" || tag === "TR" || tag === "TD" || tag === "TH" || tag === "DIV" || tag === "P") &&
        name === "align" &&
        /^(?:left|center|right|justify)$/i.test(val.trim())
      ) {
        copy.setAttribute("align", val.trim().toLowerCase());
      } else if ((tag === "TR" || tag === "TD" || tag === "TH") && name === "valign" && /^(?:top|middle|bottom|baseline)$/i.test(val.trim())) {
        copy.setAttribute("valign", val.trim().toLowerCase());
      } else if (tag === "FONT" && name === "color" && /^(?:#[0-9a-f]{3,8}|[a-z]{3,20}|rgb\(\s*\d{1,3}\s*,\s*\d{1,3}\s*,\s*\d{1,3}\s*\))$/i.test(val.trim())) {
        copy.setAttribute("color", val.trim());
      }
    }
    // Drop broken images whose remote/unsafe src was stripped.
    if (tag === "IMG" && !copy.hasAttribute("src")) return;

    parent.appendChild(copy);
    for (const child of Array.from(el.childNodes)) cleanNode(child, copy);
  };

  for (const child of Array.from(srcDoc.body.childNodes)) cleanNode(child, outDoc.body);
  return outDoc.body.innerHTML;
}

export function htmlToText(html: string): string {
  const prepared = html
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/(p|div|li|h[1-6]|tr|blockquote)>/gi, "$&\n");
  const doc = new DOMParser().parseFromString(prepared, "text/html");
  doc.querySelectorAll("script, style, head, title, noscript, template").forEach((n) => n.remove());
  doc.querySelectorAll("blockquote").forEach((b) => {
    b.textContent = (b.textContent || "").split("\n").map((l) => "> " + l).join("\n");
  });
  return (doc.body.textContent || "").replace(/\n{3,}/g, "\n\n").trim();
}

function originalBody(d: Detail): string {
  if (d.html) return sanitize(d.html);
  return textToHtml(d.text || "");
}

const addrList = (a: Addr[]) => a.map((x) => escapeHtml(formatAddr(x))).join(", ");

const QUOTE_STYLE = "margin:0 0 0 2px;padding-left:12px;border-left:2px solid #d9d6cf;color:#55524c";

export function quoteReply(d: Detail): string {
  const who = escapeHtml(d.fromName ? `${d.fromName} <${d.fromAddr}>` : d.fromAddr);
  return `<div class="mv-quote-head">On ${escapeHtml(longDate(d.date))}, ${who} wrote:</div><blockquote type="cite" style="${QUOTE_STYLE}">${originalBody(d)}</blockquote>`;
}

/**
 * Prepares composer HTML for sending. Mail clients ignore the app's stylesheet,
 * so the few styles that matter are inlined.
 */
export function emailWrap(inner: string): string {
  const doc = new DOMParser().parseFromString(`<div>${inner}</div>`, "text/html");
  const root = doc.body.firstElementChild as HTMLElement;
  root.querySelectorAll<HTMLElement>("blockquote:not([style])").forEach((b) => b.setAttribute("style", QUOTE_STYLE));
  root.querySelectorAll<HTMLElement>(".mv-quote-head").forEach((h) => {
    h.removeAttribute("class");
    h.setAttribute("style", "color:#8f8b83;font-size:13px;margin-bottom:6px");
  });
  root.querySelectorAll<HTMLElement>("[data-signature]").forEach((s) => s.removeAttribute("data-signature"));
  root.querySelectorAll<HTMLElement>("h2:not([style])").forEach((h) => h.setAttribute("style", "font-size:19px;line-height:1.3;margin:10px 0 4px"));
  root.querySelectorAll<HTMLElement>("h3:not([style])").forEach((h) => h.setAttribute("style", "font-size:16px;line-height:1.35;margin:8px 0 2px"));
  return `<div style="font-family:-apple-system,BlinkMacSystemFont,'Helvetica Neue',Helvetica,Arial,sans-serif;font-size:14px;line-height:1.55;color:#1d1c1a">${root.innerHTML}</div>`;
}

/** Undoes `emailWrap`, for reopening a draft or a queued message in the composer. */
export function unwrapEmail(html: string): string {
  const doc = new DOMParser().parseFromString(sanitize(html), "text/html");
  const only = doc.body.children.length === 1 ? (doc.body.firstElementChild as HTMLElement) : null;
  if (only?.tagName === "DIV" && /^font-family:\s*-apple-system/.test(only.getAttribute("style") || "")) return only.innerHTML;
  return doc.body.innerHTML;
}

export function quoteForward(d: Detail): string {
  const lines = [
    `From: ${escapeHtml(d.fromName ? `${d.fromName} <${d.fromAddr}>` : d.fromAddr)}`,
    `Date: ${escapeHtml(longDate(d.date))}`,
    `Subject: ${escapeHtml(d.subject)}`,
    `To: ${addrList(d.to)}`,
    d.cc.length ? `Cc: ${addrList(d.cc)}` : "",
  ].filter(Boolean);
  return `<div class="mv-quote-head">---------- Forwarded message ----------<br>${lines.join("<br>")}</div><br>${originalBody(d)}`;
}

export function prefixed(prefix: string, subject: string): string {
  const re = new RegExp(`^\\s*${prefix}\\s*:`, "i");
  return re.test(subject) ? subject : `${prefix}: ${subject}`;
}
