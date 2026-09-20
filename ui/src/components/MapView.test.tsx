import { beforeEach, describe, expect, mock, test } from "bun:test";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { MapView } from "./MapView";
import type { GraphPayload } from "@/api/types";
import type { FileRow } from "@/lib/join";

const Fake = (globalThis as any).__sigma;
const payload: GraphPayload = {
  index_version: "v9",
  nodes: [{ path: "src/a.ts", symbols: 2, lang: "typescript" }, { path: "src/b.ts", symbols: 1, lang: null }, { path: "src/c.ts", symbols: 5, lang: null }],
  edges: [{ src: 1, dst: 0, weight: 1, names: 1 }],
};
const sym = (name: string, line: number, status: "served" | "cut" | "untouched") => ({ key: `k${line}`, symbol: { id: line, name, kind: "function", line_start: line, line_end: line, signature: name }, status, item: null });
const rows: FileRow[] = [
  { path: "src/a.ts", lang: "typescript", served: 1, cut: 0, expandedByDefault: true, symbols: [sym("x", 1, "served"), sym("y", 2, "untouched")] },
  { path: "src/b.ts", lang: null, served: 0, cut: 1, expandedByDefault: true, symbols: [sym("z", 3, "cut")] },
  { path: "src/c.ts", lang: null, served: 0, cut: 0, expandedByDefault: false, symbols: [] },
];
const props = () => ({
  payload, rows, hasRetrieval: true, focusedPath: null as string | null, focusedSymbol: null as string | null, expandedPath: null as string | null, blast: null, boundaries: [],
  ariaLabel: "Map of 3 files.", onSelectNode: mock(), onToggleExpand: mock(), onSwitchToTable: mock(), onLayoutReady: mock(),
});

beforeEach(() => { Fake.instances = []; localStorage.clear(); });

describe("MapView", () => {
  test("renders the summary label, the switch control before the canvas, and mounts Sigma once", async () => {
    const p = props();
    render(<MapView {...p} />);
    const img = screen.getByRole("img", { name: "Map of 3 files." });
    const btn = screen.getByRole("button", { name: "Switch to table" });
    expect(btn.compareDocumentPosition(img) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(img.getAttribute("tabindex")).toBeNull();
    await act(async () => {});
    expect(Fake.instances).toHaveLength(1);
    expect(p.onLayoutReady).toHaveBeenCalledWith("v9");
    expect(localStorage.getItem("singularrag.layout.v9")).toBeTruthy();
    await userEvent.setup().click(btn);
    expect(p.onSwitchToTable).toHaveBeenCalled();
  });

  test("reducers paint status from the rows: served filled, cut ringed, untouched dim", async () => {
    render(<MapView {...props()} />);
    await act(async () => {});
    const s = Fake.instances[0];
    const reduce = s.settings.nodeReducer as (n: string, d: Record<string, unknown>) => Record<string, unknown>;
    const attrs = (n: string) => s.graph.getNodeAttributes(n);
    expect(reduce("src/a.ts", attrs("src/a.ts"))).toMatchObject({ type: "circle", label: "src/a.ts" });
    expect(reduce("src/b.ts", attrs("src/b.ts"))).toMatchObject({ type: "border", label: "src/b.ts · cut" });
    expect(reduce("src/c.ts", attrs("src/c.ts")).label).toBeNull();
  });

  test("clicking a node selects it; double-click asks to expand; focus pans the camera", async () => {
    const p = props();
    const { rerender } = render(<MapView {...p} />);
    await act(async () => {});
    const s = Fake.instances[0];
    act(() => s.emit("clickNode", { node: "src/b.ts" }));
    expect(p.onSelectNode).toHaveBeenCalledWith("src/b.ts", undefined);
    act(() => s.emit("doubleClickNode", { node: "src/b.ts" }));
    expect(p.onToggleExpand).toHaveBeenCalledWith("src/b.ts");
    s.graph.mergeNodeAttributes("src/c.ts", { x: 0.9, y: 0.1 });
    rerender(<MapView {...p} focusedPath="src/c.ts" />);
    await act(async () => {});
    expect(s.camera.state).toMatchObject({ x: 0.9, y: 0.1 });
    expect((s.settings.nodeReducer as any)("src/c.ts", s.graph.getNodeAttributes("src/c.ts")).type).toBe("border");
  });

  test("expanding a file adds satellite symbol nodes and collapsing removes them", async () => {
    const p = props();
    const { rerender } = render(<MapView {...p} />);
    await act(async () => {});
    const s = Fake.instances[0];
    expect(s.graph.order).toBe(3);
    rerender(<MapView {...p} expandedPath="src/a.ts" />);
    await act(async () => {});
    expect(s.graph.order).toBe(5);
    const reduce = s.settings.nodeReducer as any;
    const key = "sym:src/a.ts::x::1";
    expect(reduce(key, s.graph.getNodeAttributes(key))).toMatchObject({ type: "circle", label: "x" });
    act(() => s.emit("clickNode", { node: key }));
    expect(p.onSelectNode).toHaveBeenCalledWith("src/a.ts", "x");
    rerender(<MapView {...p} expandedPath={null} />);
    await act(async () => {});
    expect(s.graph.order).toBe(3);
  });

  test("a blast result dims everything outside it and labels depth", async () => {
    const p = props();
    const { rerender } = render(<MapView {...p} />);
    await act(async () => {});
    const s = Fake.instances[0];
    rerender(<MapView {...p} blast={{ root: { path: "src/a.ts", symbol: "x" }, files: [{ path: "src/b.ts", depth: 1, via: "x" }], truncated: null }} />);
    await act(async () => {});
    const reduce = s.settings.nodeReducer as any;
    expect(reduce("src/b.ts", s.graph.getNodeAttributes("src/b.ts")).label).toBe("src/b.ts · depth 1");
    expect(reduce("src/c.ts", s.graph.getNodeAttributes("src/c.ts")).label).toBeNull();
    expect(reduce("src/a.ts", s.graph.getNodeAttributes("src/a.ts")).label).toBe("src/a.ts · depth 0");
  });

  test("boundary hulls are drawn for members present in the graph", async () => {
    const p = props();
    render(<MapView {...p} boundaries={[{ name: "auth", paths: ["src/a.ts", "src/b.ts", "src/nope.ts"] }]} />);
    await act(async () => {});
    const s = Fake.instances[0];
    const canvas = screen.getByTestId("hull-layer") as HTMLCanvasElement;
    const calls: string[] = [];
    (canvas as any).getContext = () => new Proxy({}, { get: (_t, k) => (k === "canvas" ? canvas : (...a: unknown[]) => { calls.push(String(k)); return undefined; }) });
    act(() => s.emit("afterRender", {}));
    expect(calls).toContain("fill");
    expect(calls).toContain("fillText");
  });

  test("a prefers-color-scheme change re-reads the palette and updates sigma in place", async () => {
    const listeners: Record<string, (e: unknown) => void> = {};
    const mql = {
      matches: false,
      addEventListener: (name: string, cb: (e: unknown) => void) => { listeners[name] = cb; },
      removeEventListener: (name: string, cb: (e: unknown) => void) => { if (listeners[name] === cb) delete listeners[name]; },
    };
    const originalMatchMedia = window.matchMedia;
    (window as any).matchMedia = () => mql;
    try {
      const p = props();
      render(<MapView {...p} />);
      await act(async () => {});
      const s = Fake.instances[0];
      const setSettingSpy = mock(s.setSetting.bind(s));
      s.setSetting = setSettingSpy;
      const refreshesBefore = s.refreshes;
      expect(listeners.change).toBeDefined();
      act(() => listeners.change?.({ matches: true }));
      expect(setSettingSpy.mock.calls.some((c) => c[0] === "labelColor")).toBe(true);
      expect(setSettingSpy.mock.calls.some((c) => c[0] === "defaultDrawNodeHover")).toBe(true);
      expect(s.refreshes).toBeGreaterThan(refreshesBefore);
      expect(Fake.instances).toHaveLength(1);
    } finally {
      (window as any).matchMedia = originalMatchMedia;
    }
  });

  test("the hover drawer fills a box with the palette background then draws the label in the label colour", async () => {
    const p = props();
    render(<MapView {...p} />);
    await act(async () => {});
    const s = Fake.instances[0];
    const drawer = s.settings.defaultDrawNodeHover as (ctx: unknown, data: Record<string, unknown>, settings: Record<string, unknown>) => void;
    expect(typeof drawer).toBe("function");
    const order: string[] = [];
    const target: Record<string, unknown> = {};
    const ctx = new Proxy(target, {
      get: (t, k) => {
        if (k === "fillStyle") return t.fillStyle;
        if (k === "measureText") return () => ({ width: 40 });
        return (..._a: unknown[]) => { order.push(String(k)); return undefined; };
      },
      set: (t, k, v) => { t[k as string] = v; if (k === "fillStyle") order.push(`fillStyle:${v}`); return true; },
    });
    drawer(ctx, { x: 10, y: 10, size: 4, label: "src/a.ts" }, { labelSize: 12, labelFont: "system-ui", labelWeight: "normal" });
    expect(order.some((e) => e === "fillRect" || e === "fill")).toBe(true);
    expect(order).toContain("fillText");
    const bgIdx = order.indexOf("fillStyle:#ffffff");
    const labelIdx = order.indexOf("fillStyle:#111827");
    const textIdx = order.indexOf("fillText");
    expect(bgIdx).toBeGreaterThanOrEqual(0);
    expect(labelIdx).toBeGreaterThan(bgIdx);
    expect(textIdx).toBeGreaterThan(labelIdx);
  });

  test("camera state survives a payload remount (index version change)", async () => {
    const p = props();
    const { rerender } = render(<MapView {...p} />);
    await act(async () => {});
    const s1 = Fake.instances[0];
    s1.camera.setState({ x: 0.3, y: 0.7, ratio: 2 });
    const v2 = { ...payload, index_version: "v10" };
    rerender(<MapView {...p} payload={v2} />);
    await act(async () => {});
    expect(Fake.instances).toHaveLength(2);
    const s2 = Fake.instances[1];
    expect(s2.camera.state).toMatchObject({ x: 0.3, y: 0.7, ratio: 2 });
  });

  test("the hull canvas is sized for devicePixelRatio and scaled back down with a transform", async () => {
    const originalDpr = window.devicePixelRatio;
    (window as any).devicePixelRatio = 2;
    try {
      const p = props();
      render(<MapView {...p} />);
      await act(async () => {});
      const s = Fake.instances[0];
      const canvas = screen.getByTestId("hull-layer") as HTMLCanvasElement;
      const setTransformCalls: unknown[][] = [];
      (canvas as any).getContext = () => new Proxy({}, { get: (_t, k) => (k === "canvas" ? canvas : (...a: unknown[]) => { if (k === "setTransform") setTransformCalls.push(a); return undefined; }) });
      act(() => s.emit("afterRender", {}));
      expect(canvas.width).toBe(800);
      expect(canvas.height).toBe(600);
      expect(setTransformCalls.length).toBeGreaterThan(0);
      expect(setTransformCalls[0][0]).toBe(2);
    } finally {
      (window as any).devicePixelRatio = originalDpr;
    }
  });

  test("expanding a file whose symbols share a name and start line does not throw (merged, not duplicated)", async () => {
    const p = props();
    const clash1 = sym("dup", 1, "served");
    const clash2 = { ...sym("dup", 1, "cut"), key: "k1b" };
    const clashRows: FileRow[] = [
      { path: "src/a.ts", lang: "typescript", served: 1, cut: 0, expandedByDefault: true, symbols: [clash1, clash2] },
      { path: "src/b.ts", lang: null, served: 0, cut: 1, expandedByDefault: true, symbols: [] },
      { path: "src/c.ts", lang: null, served: 0, cut: 0, expandedByDefault: false, symbols: [] },
    ];
    render(<MapView {...p} rows={clashRows} expandedPath="src/a.ts" />);
    await act(async () => {});
    const s = Fake.instances[0];
    expect(s.graph.hasNode("sym:src/a.ts::dup::1")).toBe(true);
  });

  test("a layout cache covering only some nodes is filled in and rewritten to cover all", async () => {
    localStorage.setItem("singularrag.layout.v9", JSON.stringify({ "src/a.ts": { x: 10, y: 20 }, "src/b.ts": { x: 30, y: 40 } }));
    const p = props();
    render(<MapView {...p} />);
    await act(async () => {});
    expect(p.onLayoutReady).toHaveBeenCalledWith("v9");
    const s = Fake.instances[0];
    for (const n of ["src/a.ts", "src/b.ts", "src/c.ts"]) {
      const { x, y } = s.graph.getNodeAttributes(n);
      expect(Number.isFinite(x)).toBe(true);
      expect(Number.isFinite(y)).toBe(true);
    }
    const cached = JSON.parse(localStorage.getItem("singularrag.layout.v9")!);
    expect(Object.keys(cached).sort()).toEqual(["src/a.ts", "src/b.ts", "src/c.ts"]);
  });

  test("unmount kills sigma; no payload renders an empty state", async () => {
    const p = props();
    const { unmount } = render(<MapView {...p} />);
    await act(async () => {});
    unmount();
    expect(Fake.instances[0].killed).toBe(true);
    render(<MapView {...p} payload={null} />);
    expect(screen.getByText("Loading the map…")).toBeTruthy();
  });

  test("axe finds nothing", async () => {
    const { container } = render(<MapView {...props()} />);
    await act(async () => {});
    const r = await axe.run(container);
    expect(r.violations).toEqual([]);
  });
});
