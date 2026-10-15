export type Reasons = {
  score: number; file_rank: number; seeds: string[];
  referenced_by: { path: string; count: number }[];
  pinned: boolean; fts_hit: boolean; query_ident_match: boolean; note_hit: boolean;
  body_hit: boolean;
  /** Similarity to the query's vector when the section was among the nearest. */
  semantic: number | null; entities: string[]; themes: string[];
  /** Systems a matched process names that this code implements; absent on older rows. */
  implements?: string[];
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
export type Note = { path: string; symbol?: string; text: string; by?: "agent"; session?: string; at?: string };
export type Boundary = { name: string; paths: string[] };
export type MapConfig = { pin: Target[]; exclude: Target[]; note: Note[]; boundary: Boundary[]; deny: { extra_patterns: string[] } };
/** `MapConfig` plus `map.toml`'s mtime in ms (0 when absent): the compare-and-swap token. */
export type MapDoc = MapConfig & { version: number };
export type Status = {
  index_version: string; git_head: string | null; indexed_at_ms: number | null; stale_count: number;
  lock_timeout: boolean; foreign_indexing: boolean; indexing: boolean; files: { indexed: number; skipped: number };
  roots: { name: string; path: string; git_head: string | null }[];
  entities_pending: number; models_unavailable: boolean; embeddings_rebuilding: boolean;
};
export type GraphNode = { path: string; symbols: number; lang: string | null };
export type GraphEdge = { src: number; dst: number; weight: number; names: number };
export type GraphPayload = { index_version: string; nodes: GraphNode[]; edges: GraphEdge[] };
export type BlastFile = { path: string; depth: number; via: string };
export type BlastResult = { root: { path: string; symbol: string }; files: BlastFile[]; truncated: null | "depth" | "files" };
export type QueryResult = { retrieval_id: number; served: number; cut: number };
/** Entity types the extractor emits: person, organisation, system, concept, event, place, document, process, role. */
export type Entity = { id: number; name: string; type: string; description: string; mentions: number };
/** A relation between two entities, stated by one section (`symbol_id`, at `path` / `name`). */
/** `src_name` / `dst_name` name both ends, so a far end past the overlay's cap still reads by name. */
export type Relation = { id: number; src: number; dst: number; description: string; symbol_id: number; path: string; name: string; src_name: string; dst_name: string };
export type Mention = { entity_id: number; symbol_id: number; path: string; name: string };
/** The overlay: the top 500 entities by mentions, and the relations and mentions among them only.
 *  `truncated` is set when more entities exist; those are reached through search and detail. */
export type EntitiesPayload = { entities: Entity[]; relations: Relation[]; mentions: Mention[]; truncated: boolean };

/** How a code symbol came to implement a step: the step's text names it, or it serves a system the step uses. */
export type CodeVia = "mention" | { system: string };
export type CodeLink = { symbol_id: number; path: string; name: string; line: number; via: CodeVia };
/** One ordered step of a process, stated by `section`; `role` is "" when none; `more_code` counts code links cut from `code`. */
export type Step = {
  ordinal: number; text: string; role: string; systems: { id: number; name: string }[];
  section: { symbol_id: number; path: string; name: string; line: number };
  code: CodeLink[]; more_code: number;
};
export type Process = { id: number; name: string; description: string; roles: string[]; steps: Step[] };
/** One page of the processes query: `total` counts every process the filters match, across pages. */
export type ProcessesPayload = { processes: Process[]; total: number; offset: number; limit: number };
export type ProcessParams = { q?: string; role?: string; offset?: number; limit?: number };
/** Up to 20 entities by name: the exact match first, then prefixes by mentions. */
export type EntitySearchPayload = { entities: Entity[] };
/** One entity with every relation it is either end of, every mention, and a process's steps. */
export type EntityDetail = { entity: Entity; relations: Relation[]; mentions: Mention[]; steps?: Step[] | null };
