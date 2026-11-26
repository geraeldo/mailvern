import type { Addr, Detail } from "./api";
import { escapeHtml, sanitize, textToHtml } from "./html";
import { formatAddr, longDate } from "./format";

function fmtList(list: Addr[]): string {
  return list.map(formatAddr).join(", ");
}

/** Opens the system print dialog for a single message or conversation. */
export function printMessages(subject: string, items: Detail[]) {
  if (!items.length) return;
  const blocks = items
    .map((m) => {
      const from = formatAddr({ name: m.fromName || null, email: m.fromAddr });
      const to = fmtList(m.to);
      const cc = fmtList(m.cc);
      const body = m.html ? sanitize(m.html) : textToHtml(m.text ?? "");
      return `<article class="msg">
        <div class="meta">
          <div><strong>From:</strong> ${escapeHtml(from)}</div>
          <div><strong>Date:</strong> ${escapeHtml(longDate(m.date))}</div>
          ${to ? `<div><strong>To:</strong> ${escapeHtml(to)}</div>` : ""}
          ${cc ? `<div><strong>Cc:</strong> ${escapeHtml(cc)}</div>` : ""}
        </div>
        <div class="body">${body}</div>
      </article>`;
    })
    .join("");

  const html = `<!doctype html><html><head><meta charset="utf-8"><title>${escapeHtml(subject || "(no subject)")}</title>
<style>
  body { margin: 24px 32px; font: 13px/1.5 -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif; color: #1d1c1a; }
  h1 { margin: 0 0 16px; font-size: 18px; border-bottom: 1px solid #ddd; padding-bottom: 10px; }
  .msg { padding: 12px 0 20px; border-bottom: 1px solid #e5e5e5; page-break-inside: avoid; }
  .msg:last-child { border-bottom: 0; }
  .meta { font-size: 12px; color: #555; margin-bottom: 12px; line-height: 1.5; }
  .body img { max-width: 100%; height: auto; }
  .body blockquote { margin: 6px 0 6px 4px; padding-left: 10px; border-left: 2px solid #ccc; color: #555; }
</style></head><body><h1>${escapeHtml(subject || "(no subject)")}</h1>${blocks}</body></html>`;

  const iframe = document.createElement("iframe");
  iframe.style.position = "fixed";
  iframe.style.width = "0";
  iframe.style.height = "0";
  iframe.style.border = "0";
  iframe.style.visibility = "hidden";
  document.body.appendChild(iframe);

  const cleanup = () => {
    setTimeout(() => iframe.remove(), 500);
  };
  iframe.onload = () => {
    try {
      const win = iframe.contentWindow;
      if (!win) return cleanup();
      win.addEventListener("afterprint", cleanup, { once: true });
      win.focus();
      win.print();
      setTimeout(cleanup, 15000);
    } catch {
      cleanup();
    }
  };
  iframe.srcdoc = html;
}
