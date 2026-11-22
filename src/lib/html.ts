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

const REMOTE = /(src|background)\s*=\s*["']?\s*https?:|url\(\s*["']?\s*https?:/i;

export function hasRemoteContent(html: string): boolean {
  return REMOTE.test(html);
}

/** A self-contained document for the sandboxed reading frame. Scripts never run. */
export function frameDoc(body: string, allowRemote: boolean): string {
  const remote = allowRemote ? " https: http:" : "";
  const csp = `default-src 'none'; img-src data: cid:${remote}; style-src 'unsafe-inline'${remote}; font-src data:${remote}; media-src data:${remote}`;
  return `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="color-scheme" content="light">
<style>
  html,body{margin:0;padding:0;background:#fff;overflow:hidden}
  body{padding:4px 2px 24px;font:14px/1.55 -apple-system,BlinkMacSystemFont,"Helvetica Neue",sans-serif;color:#1d1c1a;word-wrap:break-word;overflow-wrap:anywhere}
  img{max-width:100%;height:auto}
  table{max-width:100%}
  pre{white-space:pre-wrap}
  a{color:#2459e0}
  blockquote{margin:0 0 0 2px;padding-left:12px;border-left:2px solid #e3e1dc;color:#55524c}
</style></head><body>${body}</body></html>`;
}

/** Strips anything that could restyle or script the app before quoting into the composer. */
export function sanitize(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("script,style,link,meta,iframe,object,embed,form,base,title,head").forEach((n) => n.remove());
  doc.querySelectorAll("*").forEach((el) => {
    for (const a of Array.from(el.attributes)) {
      const n = a.name.toLowerCase();
      if (n.startsWith("on") || n === "class" || n === "id" || ((n === "href" || n === "src") && /^\s*javascript:/i.test(a.value))) {
        el.removeAttribute(a.name);
      }
    }
  });
  return doc.body.innerHTML;
}

export function htmlToText(html: string): string {
  const el = document.createElement("div");
  el.innerHTML = html
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/(p|div|li|h[1-6]|tr|blockquote)>/gi, "$&\n");
  el.querySelectorAll("blockquote").forEach((b) => {
    b.textContent = (b.textContent || "").split("\n").map((l) => "> " + l).join("\n");
  });
  return (el.textContent || "").replace(/\n{3,}/g, "\n\n").trim();
}

function originalBody(d: Detail): string {
  if (d.html) return sanitize(d.html);
  return textToHtml(d.text || "");
}

const addrList = (a: Addr[]) => a.map((x) => escapeHtml(formatAddr(x))).join(", ");

export function quoteReply(d: Detail): string {
  const who = escapeHtml(d.fromName ? `${d.fromName} <${d.fromAddr}>` : d.fromAddr);
  return `<div class="mv-quote-head">On ${escapeHtml(longDate(d.date))}, ${who} wrote:</div><blockquote>${originalBody(d)}</blockquote>`;
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
