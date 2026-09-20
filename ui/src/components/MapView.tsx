import { useEffect, useMemo, useRef, useState } from "react";
import Sigma from "sigma";
import { createNodeBorderProgram } from "@sigma/node-border";
import type Graph from "graphology";
import { Button } from "@/components/ui/button";
import type { BlastResult, Boundary, GraphPayload } from "@/api/types";
import type { FileRow } from "@/lib/join";
import { applyPositions, buildGraph, layoutGraph, loadLayout, saveLayout, type Positions } from "@/lib/graph";
import { convexHull, padHull } from "@/lib/hull";
import { edgeStyle, fileStatusOf, nodeStyle, readPalette, satelliteKey, type Palette } from "@/lib/mapStyle";

export type MapViewProps = {
  payload: GraphPayload | null; rows: FileRow[]; hasRetrieval: boolean;
  focusedPath: string | null; focusedSymbol: string | null; expandedPath: string | null;
  blast: BlastResult | null; boundaries: Boundary[]; ariaLabel: string;
  onSelectNode: (path: string, symbol?: string) => void; onToggleExpand: (path: string) => void;
  onSwitchToTable: () => void; onLayoutReady: (indexVersion: string) => void;
};

const HULL_PADDING = 18;
const hue = (name: string) => { let h = 0; for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) % 360; return h; };

/** Latest props in a ref so the reducers (installed once) read current state. */
function useLatest<T>(v: T) { const r = useRef(v); r.current = v; return r; }

/** Sigma's `drawDiscNodeHover` default fills the label box with a hardcoded white, which is
 *  wrong in dark mode. Mirror its geometry (a box behind the label sized off `measureText`
 *  and the label size) but fill it from the theme palette. Reads `palRef.current` so a
 *  `prefers-color-scheme` change updates it without reinstalling the setting. */
function hoverDrawer(palRef: { current: Palette | null }) {
  return (context: CanvasRenderingContext2D, data: { x: number; y: number; size: number; label?: string | null }, settings: { labelSize: number; labelFont: string; labelWeight: string }) => {
    const pal = palRef.current;
    if (!pal || typeof data.label !== "string") return;
    const size = settings.labelSize, font = settings.labelFont, weight = settings.labelWeight;
    context.font = `${weight} ${size}px ${font}`;
    const PADDING = 3;
    const textWidth = context.measureText(data.label).width;
    const x = data.x + data.size + 3 - PADDING;
    const boxWidth = Math.round(textWidth + PADDING * 2);
    const boxHeight = Math.round(size + PADDING * 2);
    const y = data.y - boxHeight / 2;
    context.fillStyle = pal.background;
    const rc = context as CanvasRenderingContext2D & { roundRect?: (x: number, y: number, w: number, h: number, r: number) => void };
    if (typeof rc.roundRect === "function") {
      context.beginPath();
      rc.roundRect(x, y, boxWidth, boxHeight, 3);
      context.fill();
    } else {
      context.fillRect(x, y, boxWidth, boxHeight);
    }
    context.fillStyle = pal.label;
    context.fillText(data.label, data.x + data.size + 3, data.y + size / 3);
  };
}

export function MapView(props: MapViewProps) {
  const { payload, expandedPath, focusedPath, onLayoutReady, onSelectNode, onToggleExpand, onSwitchToTable, ariaLabel } = props;
  const containerRef = useRef<HTMLDivElement>(null);
  const hullRef = useRef<HTMLCanvasElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const graphRef = useRef<Graph | null>(null);
  // In-memory fallback seed for `layoutGraph` when there is no (or no full) localStorage
  // cache for the new payload's index version — e.g. right after a re-index, before that
  // version has ever been saved. Without this, survivors would relayout from a circular
  // seed and visibly jump on every version change (Task 3: survivors keep their position).
  const lastPositions = useRef<Positions | null>(null);
  const hovered = useRef<string | null>(null);
  // The real Sigma constructor renders synchronously — it calls `nodeReducer`/`edgeReducer`
  // for every item in the graph before `new Sigma(...)` returns — so a reducer must never
  // reach back into the `sigma` closure variable while construction is still in flight (it
  // is in its temporal dead zone and throws). The camera zoom ratio is the only thing the
  // reducers read off `sigma` itself, so it is mirrored into this ref instead: seeded right
  // after construction, then kept current from the camera's own "updated" event.
  const ratioRef = useRef(1);
  // Reducers (and the hover drawer) read this, not a `pal` closed over at construction
  // time, so a `prefers-color-scheme` change can repaint without remounting Sigma.
  const palRef = useRef<Palette | null>(null);
  // Camera state survives a remount (a new payload / index version): captured just
  // before `sigma.kill()`, restored onto the freshly constructed instance.
  const savedCamera = useRef<Record<string, number> | null>(null);
  const [palette, setPalette] = useState<Palette | null>(null);
  const latest = useLatest(props);

  const statusByPath = useMemo(() => new Map(props.rows.map((r) => [r.path, fileStatusOf(r, props.hasRetrieval)])), [props.rows, props.hasRetrieval]);
  const blastDepth = useMemo(() => {
    if (!props.blast) return null;
    const m = new Map<string, number>([[props.blast.root.path, 0]]);
    for (const f of props.blast.files) m.set(f.path, f.depth);
    return m;
  }, [props.blast]);
  const latestDerived = useLatest({ statusByPath, blastDepth });

  // Mount once per payload: build, lay out (cached per index version), install reducers.
  useEffect(() => {
    const el = containerRef.current;
    if (!payload || !el) return;
    const graph = buildGraph(payload);
    const cached = loadLayout(payload.index_version);
    // A cache that predates a file being added/removed no longer covers every node; a
    // partial cache is still a useful seed (existing files keep their position), but the
    // result must be saved back so the cache covers the full graph going forward. When
    // there is no (full) cache at all — e.g. right after a re-index, before this index
    // version has ever been saved — fall back to the in-memory seed from whatever was
    // last laid out, so survivors don't visibly jump to a fresh circular layout.
    const coversAll = !!cached && graph.nodes().every((n) => Object.prototype.hasOwnProperty.call(cached, n));
    const positions = coversAll ? (cached as Positions) : layoutGraph(graph, cached ?? lastPositions.current ?? undefined);
    if (!coversAll) saveLayout(payload.index_version, positions);
    lastPositions.current = positions;
    applyPositions(graph, positions);
    graphRef.current = graph;
    const pal = readPalette(el);
    palRef.current = pal;
    setPalette(pal);
    const sigma = new Sigma(graph, el, {
      allowInvalidContainer: true,
      renderLabels: true,
      labelRenderedSizeThreshold: 0,
      labelColor: { color: pal.label },
      defaultDrawNodeHover: hoverDrawer(palRef),
      defaultNodeType: "circle",
      zIndex: true,
      nodeProgramClasses: {
        border: createNodeBorderProgram({ borders: [{ size: { value: 0.25, mode: "relative" }, color: { attribute: "borderColor" } }, { size: { fill: true }, color: { attribute: "color" } }] }),
      },
      nodeReducer: (node, data) => {
        const p = latest.current;
        const d = latestDerived.current;
        const ratio = ratioRef.current;
        const pal = palRef.current!;
        const sat = data.symbolOf as string | undefined;
        if (sat) {
          const s = nodeStyle(data.symbolName as string, { status: (data.symbolStatus as never) ?? null, focused: p.focusedPath === sat && p.focusedSymbol === data.symbolName, blastDepth: null, blastActive: false, symbols: 0, hovered: false, zoomRatio: ratio }, pal);
          return { ...data, ...s, size: 3 + (s.type === "border" ? 2 : 0), borderColor: s.borderColor ?? pal.focus, label: s.label ?? (data.symbolName as string) };
        }
        const s = nodeStyle(node, {
          status: d.statusByPath.get(node) ?? null,
          focused: p.focusedPath === node,
          blastDepth: d.blastDepth?.get(node) ?? null,
          blastActive: d.blastDepth !== null,
          symbols: (data.symbols as number) ?? 0,
          hovered: hovered.current === node,
          zoomRatio: ratio,
        }, pal);
        return { ...data, size: s.size, color: s.color, borderColor: s.borderColor ?? pal.focus, type: s.type, label: s.label, zIndex: s.zIndex };
      },
      edgeReducer: (edge, data) => {
        const p = latest.current;
        const d = latestDerived.current;
        const pal = palRef.current!;
        const [a, b] = graph.extremities(edge);
        const touchesFocused = p.focusedPath === a || p.focusedPath === b;
        const touchesBlast = !!d.blastDepth && d.blastDepth.has(a) && d.blastDepth.has(b);
        const s = edgeStyle({ touchesFocused, blastActive: d.blastDepth !== null, touchesBlast }, pal);
        return { ...data, color: s.color, size: data.satellite ? 0.5 : s.size, hidden: s.hidden && !data.satellite };
      },
    });
    if (savedCamera.current) sigma.getCamera().setState(savedCamera.current);
    // Now that construction has finished (`sigma` is out of its temporal dead zone), seed
    // the ratio ref from the real camera and keep it current. A refresh is only scheduled
    // when the ratio crosses the 0.5 labelling threshold used by `nodeStyle`, not on every
    // pan/zoom tick.
    const camera = sigma.getCamera();
    ratioRef.current = camera.getState().ratio;
    camera.on("updated", (state) => {
      const wasBelow = ratioRef.current < 0.5;
      const isBelow = state.ratio < 0.5;
      ratioRef.current = state.ratio;
      if (wasBelow !== isBelow) sigma.refresh();
    });
    sigma.on("clickNode", ({ node }) => {
      const a = graph.getNodeAttributes(node);
      if (a.symbolOf) latest.current.onSelectNode(a.symbolOf as string, a.symbolName as string);
      else latest.current.onSelectNode(node, undefined);
    });
    sigma.on("doubleClickNode", ({ node }) => { if (!graph.getNodeAttribute(node, "symbolOf")) latest.current.onToggleExpand(node); });
    sigma.on("enterNode", ({ node }) => { hovered.current = node; sigma.refresh(); });
    sigma.on("leaveNode", () => { hovered.current = null; sigma.refresh(); });
    sigma.on("afterRender", () => drawHulls(sigma, graph, hullRef.current, latest.current.boundaries, palRef.current!));
    sigmaRef.current = sigma;
    onLayoutReady(payload.index_version);
    return () => {
      savedCamera.current = sigma.getCamera().getState() as unknown as Record<string, number>;
      sigma.kill();
      sigmaRef.current = null;
      graphRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [payload]);

  // A theme switch (system dark/light change) does not remount Sigma: re-read the
  // palette into the ref the reducers/hover-drawer read, push the label colour and
  // hover drawer settings, update the container background, and repaint.
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mql = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      const el = containerRef.current;
      const sigma = sigmaRef.current;
      if (!el || !sigma) return;
      const pal = readPalette(el);
      palRef.current = pal;
      setPalette(pal);
      sigma.setSetting("labelColor", { color: pal.label });
      sigma.setSetting("defaultDrawNodeHover", hoverDrawer(palRef));
      sigma.refresh();
    };
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);

  // Repaint when the inputs the reducers read change.
  useEffect(() => { sigmaRef.current?.refresh(); }, [statusByPath, blastDepth, props.focusedPath, props.focusedSymbol, props.boundaries]);

  // Camera follows the focused file, no easing.
  useEffect(() => {
    const sigma = sigmaRef.current, graph = graphRef.current;
    if (!sigma || !graph || !focusedPath || !graph.hasNode(focusedPath)) return;
    const d = sigma.getNodeDisplayData(focusedPath);
    if (d) sigma.getCamera().setState({ x: d.x, y: d.y });
  }, [focusedPath, payload]);

  // Symbol satellites for the expanded file.
  useEffect(() => {
    const graph = graphRef.current;
    if (!graph) return;
    for (const n of graph.nodes()) if (graph.getNodeAttribute(n, "symbolOf")) graph.dropNode(n);
    const row = expandedPath ? props.rows.find((r) => r.path === expandedPath) : null;
    if (row && graph.hasNode(row.path)) {
      const { x, y } = graph.getNodeAttributes(row.path);
      const n = row.symbols.length, radius = n > 8 ? 9 : 6;
      row.symbols.forEach((s, i) => {
        const t = (i / Math.max(1, n)) * Math.PI * 2;
        // Two symbols can share a name and start line (e.g. overloads); `mergeNode`/
        // `mergeEdge` tolerate the resulting key collision instead of throwing.
        const key = satelliteKey(row.path, s.symbol.name, s.symbol.line_start);
        graph.mergeNode(key, { x: x + Math.cos(t) * radius, y: y + Math.sin(t) * radius, size: 3, symbolOf: row.path, symbolName: s.symbol.name, symbolStatus: props.hasRetrieval ? s.status : null });
        graph.mergeEdge(row.path, key, { weight: 0, names: 0, satellite: true });
      });
    }
    sigmaRef.current?.refresh();
  }, [expandedPath, props.rows, props.hasRetrieval, payload]);

  if (!payload) return <div className="p-3 text-sm text-muted-foreground">Loading the map…</div>;
  return (
    <div className="relative h-full">
      <Button type="button" variant="outline" className="absolute left-2 top-2 z-10" onClick={onSwitchToTable}>Switch to table</Button>
      <div ref={containerRef} role="img" aria-label={ariaLabel} className="h-full w-full" style={{ background: palette?.background }} />
      <canvas ref={hullRef} data-testid="hull-layer" aria-hidden="true" className="pointer-events-none absolute inset-0 h-full w-full" />
    </div>
  );
}

function drawHulls(sigma: Sigma, graph: Graph, canvas: HTMLCanvasElement | null, boundaries: Boundary[], pal: Palette) {
  if (!canvas) return;
  const { width, height } = sigma.getDimensions();
  const dpr = window.devicePixelRatio || 1;
  const pxWidth = Math.round(width * dpr), pxHeight = Math.round(height * dpr);
  if (canvas.width !== pxWidth || canvas.height !== pxHeight) { canvas.width = pxWidth; canvas.height = pxHeight; }
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  for (const b of boundaries) {
    const pts = b.paths.filter((p) => graph.hasNode(p)).map((p) => { const a = graph.getNodeAttributes(p); return sigma.graphToViewport({ x: a.x, y: a.y }); });
    if (pts.length === 0) continue;
    const hull = padHull(convexHull(pts), HULL_PADDING);
    const h = hue(b.name);
    ctx.beginPath();
    hull.forEach((p, i) => (i === 0 ? ctx.moveTo(p.x, p.y) : ctx.lineTo(p.x, p.y)));
    ctx.closePath();
    ctx.fillStyle = `hsl(${h} 60% 50% / 0.12)`;
    ctx.strokeStyle = `hsl(${h} 60% 45% / 0.7)`;
    ctx.lineWidth = 1.5;
    ctx.fill();
    ctx.stroke();
    const top = hull.reduce((m, p) => (p.y < m.y ? p : m), hull[0]);
    ctx.fillStyle = pal.label;
    ctx.font = "12px system-ui, sans-serif";
    ctx.fillText(b.name, top.x, top.y - 4);
  }
}
