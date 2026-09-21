import type { MapConfig, Note, Target } from "@/api/types";

const same = (t: Target, path: string, symbol?: string) => t.path === path && (t.symbol ?? undefined) === symbol;

export const isPinned = (c: MapConfig, path: string, symbol?: string) => c.pin.some((t) => same(t, path, symbol));
export const isExcluded = (c: MapConfig, path: string) => c.exclude.some((t) => t.path === path);

export function togglePin(c: MapConfig, path: string, symbol?: string): MapConfig {
  const pin = isPinned(c, path, symbol) ? c.pin.filter((t) => !same(t, path, symbol)) : [...c.pin, symbol ? { path, symbol } : { path }];
  return { ...c, pin };
}
export function toggleExclude(c: MapConfig, path: string): MapConfig {
  const exclude = isExcluded(c, path) ? c.exclude.filter((t) => t.path !== path) : [...c.exclude, { path }];
  return { ...c, exclude };
}
const sameTarget = (n: Note, path: string, symbol?: string) => n.path === path && (n.symbol ?? undefined) === symbol;
const isAgent = (n: Note) => n.by === "agent";
/** One paragraph, like the engine: whitespace runs become one space. */
export const oneParagraph = (text: string) => text.split(/\s+/).filter(Boolean).join(" ");

export function setNote(c: MapConfig, path: string, symbol: string | undefined, text: string): MapConfig {
  const note = c.note.filter((n) => !(sameTarget(n, path, symbol) && !isAgent(n)));
  const t = oneParagraph(text);
  if (t) note.push(symbol ? { path, symbol, text: t } : { path, text: t });
  return { ...c, note };
}
export const noteFor = (c: MapConfig, path: string, symbol?: string) =>
  c.note.find((n) => sameTarget(n, path, symbol) && !isAgent(n))?.text ?? "";
export const agentNoteFor = (c: MapConfig, path: string, symbol?: string) =>
  c.note.find((n) => sameTarget(n, path, symbol) && isAgent(n));
export function removeAgentNote(c: MapConfig, path: string, symbol?: string): MapConfig {
  return { ...c, note: c.note.filter((n) => !(sameTarget(n, path, symbol) && isAgent(n))) };
}

export const boundaryNames = (c: MapConfig) => c.boundary.map((b) => b.name);
export const boundariesOf = (c: MapConfig, path: string) => c.boundary.filter((b) => b.paths.includes(path)).map((b) => b.name);
export function addToBoundary(c: MapConfig, name: string, path: string): MapConfig {
  const n = name.trim();
  if (!n) return c;
  const existing = c.boundary.find((b) => b.name === n);
  if (existing?.paths.includes(path)) return c;
  const boundary = existing
    ? c.boundary.map((b) => (b.name === n ? { ...b, paths: [...b.paths, path] } : b))
    : [...c.boundary, { name: n, paths: [path] }];
  return { ...c, boundary };
}
export function removeFromBoundary(c: MapConfig, name: string, path: string): MapConfig {
  if (!c.boundary.some((b) => b.name === name && b.paths.includes(path))) return c;
  const boundary = c.boundary
    .map((b) => (b.name === name ? { ...b, paths: b.paths.filter((p) => p !== path) } : b))
    .filter((b) => b.paths.length > 0);
  return { ...c, boundary };
}
