export type Reasons = {
  score: number; file_rank: number; seeds: string[];
  referenced_by: { path: string; count: number }[];
  pinned: boolean; fts_hit: boolean; query_ident_match: boolean; note_hit: boolean;
};
export type Item = { rank: number; symbol_id: number; path: string; name: string; line_start: number; score: number; served: boolean; reasons: Reasons };
export type RetrievalSummary = {
  id: number; session_key: string; session_label: string; tool: string; query: string | null;
  focus_files: string[]; budget: number | null; limit_n: number | null; index_version: string;
  git_head: string | null; stale_count: number; created_at_ms: number; served: number; cut: number;
};
export type RetrievalDetail = RetrievalSummary & { items: Item[] };
export type TreeSymbol = { id: number; name: string; kind: string; line_start: number; line_end: number; signature: string };
export type TreeFile = { path: string; lang: string | null; skipped_reason: string | null; symbols: TreeSymbol[] };
export type SkippedFile = { path: string; reason: string };
export type Target = { path: string; symbol?: string };
export type Note = { path: string; symbol?: string; text: string };
export type Boundary = { name: string; paths: string[] };
export type MapConfig = { pin: Target[]; exclude: Target[]; note: Note[]; boundary: Boundary[]; deny: { extra_patterns: string[] } };
/** `MapConfig` plus `map.toml`'s mtime in ms (0 when absent): the compare-and-swap token. */
export type MapDoc = MapConfig & { version: number };
export type Status = {
  index_version: string; git_head: string | null; indexed_at_ms: number | null; stale_count: number;
  lock_timeout: boolean; foreign_indexing: boolean; indexing: boolean; files: { indexed: number; skipped: number };
};
export type GraphNode = { path: string; symbols: number; lang: string | null };
export type GraphEdge = { src: number; dst: number; weight: number; names: number };
export type GraphPayload = { index_version: string; nodes: GraphNode[]; edges: GraphEdge[] };
export type BlastFile = { path: string; depth: number; via: string };
export type BlastResult = { root: { path: string; symbol: string }; files: BlastFile[]; truncated: null | "depth" | "files" };
