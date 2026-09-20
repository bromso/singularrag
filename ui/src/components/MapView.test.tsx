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
