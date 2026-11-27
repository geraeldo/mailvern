// Fetch implementation backed by Rust (`ai_fetch` Tauri command) so AI provider
// requests work reliably across all 34 providers without WebView CORS failures.
// Falls back to `globalThis.fetch` outside Tauri (dev mock / Vitest).
import { Channel, invoke } from "@tauri-apps/api/core";

type AiFetchEvent =
  | { kind: "head"; status: number; headers: [string, string][] }
  | { kind: "chunk"; data: string }
  | { kind: "end" }
  | { kind: "error"; message: string };

const NULL_BODY_STATUSES = new Set([101, 204, 205, 304]);

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

function fromBase64(b64: string): Uint8Array<ArrayBuffer> {
  const binary = atob(b64);
  const out = new Uint8Array(new ArrayBuffer(binary.length));
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

function collectHeaders(h?: HeadersInit): [string, string][] {
  if (!h) return [];
  if (h instanceof Headers) {
    const out: [string, string][] = [];
    h.forEach((v, k) => out.push([k, v]));
    return out;
  }
  if (Array.isArray(h)) return h.map(([k, v]) => [String(k), String(v)]);
  return Object.entries(h).map(([k, v]) => [k, String(v)]);
}

async function encodeBody(body: BodyInit | null | undefined): Promise<{ body: string | null; bodyBase64: boolean }> {
  if (body == null) return { body: null, bodyBase64: false };
  if (typeof body === "string") return { body, bodyBase64: false };
  if (body instanceof URLSearchParams) return { body: body.toString(), bodyBase64: false };
  if (body instanceof Uint8Array) return { body: toBase64(body), bodyBase64: true };
  if (ArrayBuffer.isView(body)) return { body: toBase64(new Uint8Array(body.buffer, body.byteOffset, body.byteLength)), bodyBase64: true };
  if (body instanceof ArrayBuffer) return { body: toBase64(new Uint8Array(body)), bodyBase64: true };
  if (body instanceof Blob) return { body: toBase64(new Uint8Array(await body.arrayBuffer())), bodyBase64: true };
  // Fallback for ReadableStream or FormData
  const buf = await new Response(body).arrayBuffer();
  return { body: toBase64(new Uint8Array(buf)), bodyBase64: true };
}

let nextId = 1;

export const aiFetch: typeof globalThis.fetch = async (input, init) => {
  if (!isTauri()) return globalThis.fetch(input, init);

  const req = input instanceof Request ? input : null;
  const url = req ? req.url : input instanceof URL ? input.toString() : String(input);
  const method = (init?.method ?? req?.method ?? "GET").toUpperCase();
  const headers = init?.headers ? collectHeaders(init.headers) : req ? collectHeaders(req.headers) : [];
  const signal = init?.signal ?? req?.signal ?? undefined;

  if (signal?.aborted) {
    throw new DOMException("The operation was aborted.", "AbortError");
  }

  const rawBody = init?.body !== undefined ? init.body : req && method !== "GET" && method !== "HEAD" ? await req.arrayBuffer() : null;
  const { body, bodyBase64 } = await encodeBody(rawBody);

  const requestId = `ai-${Date.now()}-${nextId++}`;
  const channel = new Channel<AiFetchEvent>();

  return new Promise<Response>((resolve, reject) => {
    let settled = false;
    let closed = false;
    let streamCtrl: ReadableStreamDefaultController<Uint8Array> | null = null;

    const cleanupSignal = () => {
      if (signal) signal.removeEventListener("abort", onAbort);
    };

    const onAbort = () => {
      invoke("ai_fetch_abort", { requestId }).catch(() => {});
      const err = new DOMException("The operation was aborted.", "AbortError");
      if (!settled) {
        settled = true;
        cleanupSignal();
        reject(err);
      } else if (!closed && streamCtrl) {
        closed = true;
        cleanupSignal();
        try {
          streamCtrl.error(err);
        } catch {}
      }
    };

    if (signal) signal.addEventListener("abort", onAbort, { once: true });

    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        streamCtrl = c;
      },
      cancel() {
        closed = true;
        cleanupSignal();
        invoke("ai_fetch_abort", { requestId }).catch(() => {});
      },
    });

    channel.onmessage = (ev) => {
      if (ev.kind === "head") {
        if (settled) return;
        settled = true;
        const respHeaders = new Headers();
        for (const [k, v] of ev.headers) {
          try {
            respHeaders.append(k, v);
          } catch {}
        }
        const respBody = NULL_BODY_STATUSES.has(ev.status) ? null : stream;
        resolve(new Response(respBody, { status: ev.status, headers: respHeaders }));
      } else if (ev.kind === "chunk") {
        if (!closed && streamCtrl) {
          try {
            streamCtrl.enqueue(fromBase64(ev.data));
          } catch {}
        }
      } else if (ev.kind === "end") {
        cleanupSignal();
        if (!closed && streamCtrl) {
          closed = true;
          try {
            streamCtrl.close();
          } catch {}
        }
      } else if (ev.kind === "error") {
        cleanupSignal();
        const err = new Error(ev.message);
        if (!settled) {
          settled = true;
          reject(err);
        } else if (!closed && streamCtrl) {
          closed = true;
          try {
            streamCtrl.error(err);
          } catch {}
        }
      }
    };

    invoke("ai_fetch", {
      requestId,
      url,
      method,
      headers,
      body,
      bodyBase64,
      onEvent: channel,
    }).catch((e) => {
      cleanupSignal();
      const err = e instanceof Error ? e : new Error(String(e));
      if (!settled) {
        settled = true;
        reject(err);
      } else if (!closed && streamCtrl) {
        closed = true;
        try {
          streamCtrl.error(err);
        } catch {}
      }
    });
  });
};
