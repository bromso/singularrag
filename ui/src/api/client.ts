import type { BlastResult, GraphPayload, MapConfig, MapDoc, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "./types";

export class ApiError extends Error {
  /** `current` carries the server's document on a 409 so the caller can reload from it. */
  constructor(public status: number, message: string, public field?: string, public current?: unknown) { super(message); }
}

// The token read from this page load's fragment. Memoised because the first read
// strips the fragment from the address bar, and React StrictMode runs the mount
// effect twice: without the memo the second run reads an empty hash and wipes the token.
let fragmentToken: string | null = null;

export function tokenFromFragment(): string {
  if (fragmentToken !== null) return fragmentToken;
  const m = /(?:^#|&)token=([0-9a-fA-F]+)/i.exec(window.location.hash);
  if (!m) return "";
  fragmentToken = m[1];
  // Once read, drop the fragment from the address bar: the token should not sit in a
  // shared screenshot, a copied URL or the browser's history entry.
  try { history.replaceState(null, "", window.location.pathname); } catch {}
  return fragmentToken;
}

/** Test seam: forget the memoised fragment token between cases. */
export function resetTokenMemo() { fragmentToken = null; }

let token = "";
export function setToken(t: string) { token = t; }
export function getToken() { return token; }

/** Spec §2: a busy read connection answers 503 and the client retries with backoff. */
const BUSY_RETRY_MS = [200, 400, 800];
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const res = await fetch(`/api${path}`, {
      method,
      headers: { Authorization: `Bearer ${token}`, ...(body ? { "Content-Type": "application/json" } : {}) },
      body: body ? JSON.stringify(body) : undefined,
    });
    if (res.status === 503 && attempt < BUSY_RETRY_MS.length) {
      await sleep(BUSY_RETRY_MS[attempt]);
      continue;
    }
    if (!res.ok) {
      let msg = res.statusText, field: string | undefined, current: unknown;
      try { const j = await res.json(); msg = j.error ?? msg; field = j.field; current = j.current; } catch {}
      throw new ApiError(res.status, msg, field, current);
    }
    return (await res.json()) as T;
  }
}

export const api = {
  status: () => req<Status>("GET", "/status"),
  retrievals: (before?: number) => req<RetrievalSummary[]>("GET", `/retrievals?limit=50${before ? `&before=${before}` : ""}`),
  retrieval: (id: number) => req<RetrievalDetail>("GET", `/retrievals/${id}`),
  tree: () => req<TreeFile[]>("GET", "/tree"),
  skipped: () => req<SkippedFile[]>("GET", "/skipped"),
  map: () => req<MapDoc>("GET", "/map"),
  // Compare-and-swap: the server refuses the write with 409 if `map.toml`'s mtime moved
  // since `expectedVersion` was read (a hand edit, or another tab).
  saveMap: (cfg: MapConfig, expectedVersion: number) => req<MapDoc>("PUT", "/map", { ...cfg, expected_version: expectedVersion }),
  graph: () => req<GraphPayload>("GET", "/graph"),
  blast: (path: string, symbol: string) => req<BlastResult>("GET", `/blast?path=${encodeURIComponent(path)}&symbol=${encodeURIComponent(symbol)}`),
};
