import type { Status } from "@/api/types";

/**
 * Four states, in priority order. `foreign_indexing` (another process holds the index
 * lock) outranks our own `indexing`, which outranks the leftover stale count; `drain` is
 * still on the API for the MCP side but is always 0 from serve, so the badge ignores it.
 */
export function freshnessText(s: Status | null): string {
  if (!s) return "loading";
  if (s.foreign_indexing) return "another process indexing";
  if (s.indexing) return "indexing";
  if (s.stale_count > 0) return `${s.stale_count} stale`;
  return "fresh";
}
function age(ms: number | null): string {
  if (!ms) return "";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.round(s / 60)}m ago` : `${Math.round(s / 3600)}h ago`;
}
export function FreshnessBadge({ status }: { status: Status | null }) {
  return (
    // Not `role="status"`: the polite live region already announces freshness changes,
    // and a second live region would say everything twice.
    <span aria-label="Index freshness" className="rounded border px-2 py-1 text-sm">
      <span className="font-medium">{freshnessText(status)}</span>
      {status?.git_head && <span className="ml-2 font-mono text-xs">{status.git_head.slice(0, 7)}</span>}
      <span className="ml-2 text-xs text-muted-foreground">{age(status?.indexed_at_ms ?? null)}</span>
    </span>
  );
}
