/** The rail's short label for a retrieval tool; unknown names pass through. */
const LABELS: Record<string, string> = { repo_map: "Map", find_symbol: "Find", trace_path: "Trace", changed: "Changed" };
export const toolLabel = (tool: string): string => LABELS[tool] ?? tool;
