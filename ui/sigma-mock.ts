import { mock } from "bun:test";

type Handler = (payload: unknown) => void;
export class FakeSigma {
  static instances: FakeSigma[] = [];
  handlers = new Map<string, Handler[]>();
  settings: Record<string, unknown>;
  killed = false;
  refreshes = 0;
  camera = { state: { x: 0.5, y: 0.5, ratio: 1 }, getState() { return this.state; }, setState(s: Partial<{ x: number; y: number; ratio: number }>) { this.state = { ...this.state, ...s }; } };
  constructor(public graph: any, public container: HTMLElement, settings: Record<string, unknown> = {}) {
    this.settings = settings;
    FakeSigma.instances.push(this);
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
