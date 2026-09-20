import type { ItemStatus } from "./status";

export type Palette = { served: string; cut: string; untouched: string; focus: string; edge: string; edgeDim: string; label: string; background: string };
export type NodeCtx = { status: ItemStatus | null; focused: boolean; blastDepth: number | null; blastActive: boolean; symbols: number; hovered: boolean; zoomRatio: number };
export type NodeStyle = { size: number; color: string; borderColor?: string; type: "circle" | "border"; label: string | null; zIndex: number };
export type EdgeCtx = { touchesFocused: boolean; blastActive: boolean; touchesBlast: boolean };

const DEFAULTS: Palette = { served: "#047857", cut: "#b45309", untouched: "#9ca3af", focus: "#2563eb", edge: "#6b7280", edgeDim: "#e5e7eb", label: "#111827", background: "#ffffff" };

export const nodeSize = (symbols: number) => Math.min(4 + Math.sqrt(Math.max(0, symbols)) * 1.5, 14);

/** Labels for untouched nodes appear only when hovered or zoomed in past ratio 0.5. */
export function nodeStyle(path: string, ctx: NodeCtx, pal: Palette): NodeStyle {
  const base = nodeSize(ctx.symbols);
  let s: NodeStyle;
  if (ctx.blastActive && ctx.blastDepth === null) {
    s = { size: Math.max(2, base * 0.5), color: pal.untouched, type: "circle", label: null, zIndex: 0 };
  } else if (ctx.blastActive) {
    const ring = Math.max(1, 4 - ctx.blastDepth!);
    s = { size: base + ring, color: pal.background, borderColor: ctx.status === "served" ? pal.served : ctx.status === "cut" ? pal.cut : pal.focus, type: "border", label: `${path} · depth ${ctx.blastDepth}`, zIndex: 2 };
  } else if (ctx.status === "served") {
    s = { size: base, color: pal.served, type: "circle", label: path, zIndex: 2 };
  } else if (ctx.status === "cut") {
    s = { size: base, color: pal.background, borderColor: pal.cut, type: "border", label: `${path} · cut`, zIndex: 2 };
  } else if (ctx.status === "untouched") {
    const show = ctx.hovered || ctx.zoomRatio < 0.5;
    s = { size: Math.max(2, base * 0.6), color: pal.untouched, type: "circle", label: show ? path : null, zIndex: 0 };
  } else {
    s = { size: base, color: pal.edge, type: "circle", label: path, zIndex: 1 };
  }
  if (ctx.focused) {
    s = { ...s, type: "border", borderColor: pal.focus, size: s.size + 3, zIndex: 3, label: s.label ?? path };
  }
  return s;
}

export function edgeStyle(ctx: EdgeCtx, pal: Palette): { color: string; size: number; hidden: boolean } {
  if (ctx.blastActive) return { color: ctx.touchesBlast ? pal.edge : pal.edgeDim, size: ctx.touchesBlast ? 1.5 : 0.5, hidden: !ctx.touchesBlast };
  return ctx.touchesFocused ? { color: pal.edge, size: 1.5, hidden: false } : { color: pal.edgeDim, size: 0.5, hidden: false };
}

export function readPalette(el: HTMLElement): Palette {
  const cs = getComputedStyle(el);
  const v = (name: string, fallback: string) => { const x = cs.getPropertyValue(name).trim(); return x || fallback; };
  return {
    served: v("--map-served", DEFAULTS.served), cut: v("--map-cut", DEFAULTS.cut), untouched: v("--map-untouched", DEFAULTS.untouched),
    focus: v("--map-focus", DEFAULTS.focus), edge: v("--map-edge", DEFAULTS.edge), edgeDim: v("--map-edge-dim", DEFAULTS.edgeDim),
    label: v("--foreground", DEFAULTS.label), background: v("--background", DEFAULTS.background),
  };
}
