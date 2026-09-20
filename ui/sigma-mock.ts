import { mock } from "bun:test";

type Handler = (payload: unknown) => void;
export class FakeSigma {
  static instances: FakeSigma[] = [];
  handlers = new Map<string, Handler[]>();
  settings: Record<string, unknown>;
  killed = false;
  refreshes = 0;
  camera = {
    state: { x: 0.5, y: 0.5, ratio: 1 },
    handlers: new Map<string, Handler[]>(),
    getState() { return this.state; },
    setState(s: Partial<{ x: number; y: number; ratio: number }>) {
      this.state = { ...this.state, ...s };
      for (const h of this.handlers.get("updated") ?? []) h(this.state);
    },
    on(name: string, h: Handler) { const hs = this.handlers.get(name) ?? []; hs.push(h); this.handlers.set(name, hs); return this; },
  };
  constructor(public graph: any, public container: HTMLElement, settings: Record<string, unknown> = {}) {
    this.settings = settings;
    FakeSigma.instances.push(this);
    // The real Sigma constructor renders synchronously: it calls the node and edge
    // reducers for every item in the graph before `new Sigma(...)` returns. Mimic
    // that here so a reducer that reaches back into `sigma` while it is still being
    // constructed (a temporal-dead-zone bug) fails in tests the same way it fails
    // in the browser, instead of only surfacing on first `refresh()`.
    const nodeReducer = settings.nodeReducer as ((n: string, d: Record<string, unknown>) => unknown) | undefined;
    if (nodeReducer) for (const n of this.graph.nodes()) nodeReducer(n, this.graph.getNodeAttributes(n));
    const edgeReducer = settings.edgeReducer as ((e: string, d: Record<string, unknown>) => unknown) | undefined;
    if (edgeReducer) for (const e of this.graph.edges()) edgeReducer(e, this.graph.getEdgeAttributes(e));
  }
  on(name: string, h: Handler) { const hs = this.handlers.get(name) ?? []; hs.push(h); this.handlers.set(name, hs); return this; }
  emit(name: string, payload: unknown) { for (const h of this.handlers.get(name) ?? []) h(payload); }
  getCamera() { return this.camera; }
  getGraph() { return this.graph; }
  getNodeDisplayData(node: string) { const a = this.graph.getNodeAttributes(node); return { x: a.x ?? 0, y: a.y ?? 0 }; }
  graphToViewport(p: { x: number; y: number }) { return { x: p.x, y: p.y }; }
  getDimensions() { return { width: 400, height: 300 }; }
  setSetting(k: string, v: unknown) { this.settings[k] = v; }
  refresh() { this.refreshes += 1; }
  kill() { this.killed = true; }
}
(globalThis as any).__sigma = FakeSigma;
mock.module("sigma", () => ({ default: FakeSigma, Sigma: FakeSigma }));
mock.module("@sigma/node-border", () => ({ createNodeBorderProgram: () => class {} }));
