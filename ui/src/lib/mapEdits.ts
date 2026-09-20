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
