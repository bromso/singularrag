import type { MapConfig, Target } from "@/api/types";

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
export function setNote(c: MapConfig, path: string, symbol: string | undefined, text: string): MapConfig {
  const note = c.note.filter((n) => !(n.path === path && (n.symbol ?? undefined) === symbol));
  if (text.trim()) note.push(symbol ? { path, symbol, text } : { path, text });
  return { ...c, note };
}
export const noteFor = (c: MapConfig, path: string, symbol?: string) => c.note.find((n) => n.path === path && (n.symbol ?? undefined) === symbol)?.text ?? "";

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
