import type { MapConfig, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "./types";

export class ApiError extends Error {
  constructor(public status: number, message: string, public field?: string) { super(message); }
}

export function tokenFromFragment(): string {
  const m = /(?:^#|&)token=([0-9a-f]+)/.exec(window.location.hash);
  return m?.[1] ?? "";
}

let token = "";
export function setToken(t: string) { token = t; }
export function getToken() { return token; }

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    headers: { Authorization: `Bearer ${token}`, ...(body ? { "Content-Type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    let msg = res.statusText, field: string | undefined;
    try { const j = await res.json(); msg = j.error ?? msg; field = j.field; } catch {}
    throw new ApiError(res.status, msg, field);
  }
  return (await res.json()) as T;
}

export const api = {
  status: () => req<Status>("GET", "/status"),
  retrievals: (before?: number) => req<RetrievalSummary[]>("GET", `/retrievals?limit=50${before ? `&before=${before}` : ""}`),
  retrieval: (id: number) => req<RetrievalDetail>("GET", `/retrievals/${id}`),
  tree: () => req<TreeFile[]>("GET", "/tree"),
  skipped: () => req<SkippedFile[]>("GET", "/skipped"),
  map: () => req<MapConfig>("GET", "/map"),
  saveMap: (cfg: MapConfig) => req<MapConfig>("PUT", "/map", cfg),
};
