import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { App } from "./App";

const status = { index_version: "abc123", git_head: "9b1e0d4f", indexed_at_ms: Date.now(), stale_count: 0, lock_timeout: false, foreign_indexing: false, indexing: false, files: { indexed: 4, skipped: 1 }, drain: { chunks: 0, last: { scanned: 5, indexed: 4, unchanged: 0, skipped: 1, removed: 0, remaining: 0, lock_timeout: false } } };
const r = { score: 0.1, file_rank: 0.1, seeds: ["query:session"], referenced_by: [{ path: "src/http/middleware.ts", count: 2 }], pinned: false, fts_hit: true, query_ident_match: true };
const retrieval = { id: 7, session_key: "mcp:claude-code:1:2", session_label: "Claude Code", tool: "repo_map", query: "session", focus_files: [], budget: 1024, limit_n: null, index_version: "abc123", git_head: "9b1e0d4f", stale_count: 0, created_at_ms: Date.now(), served: 1, cut: 0 };
const tree = [{ path: "src/auth/session.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 1, name: "createSession", kind: "function", line_start: 3, line_end: 6, signature: "export function createSession(user: User, ttl: number): Session" }] }];
let saved: any = null;
let mapState: any = null;
// What `GET /api/retrievals` answers; a test can push to it before firing a change event.
let retrievalList: any[] = [];
// `map.toml`'s compare-and-swap token, as the server derives it from the file's mtime.
let mapVersion = 0;
// When set, `PUT /api/map` waits on this before responding, so a test can fire two
// clicks while the first write is still in flight.
let putGate: Promise<void> | null = null;
// When set, `PUT /api/map` answers with it instead of writing.
let putRejection: { status: number; body: unknown } | null = null;
// When set, `GET /api/blast` waits on this before responding, so a test can move
// focus (or click again) while a blast request is still in flight.
let blastGate: Promise<void> | null = null;
// The `subscribe()` module registers its "change" listener via
// `EventSource#addEventListener`; capturing it here lets a test simulate a
// live SSE "change" event (another agent's retrieval) without a real
// EventSource, by invoking the captured listener directly.
let changeHandler: ((e: { data: string }) => void) | null = null;
// When set, `/api/graph` answers with these nodes instead of mirroring `tree` — lets a
// test simulate a file excluded from the map (present in the tree/retrieval, absent
// from the graph) without a second permanent fixture file.
let graphNodesOverride: { path: string; symbols: number; lang: string | null }[] | null = null;
// Extra `items` appended to the `/api/retrievals/7` response.
let extraRetrievalItems: unknown[] = [];
// The `/api/retrievals/7` response's own item(s), reset in `beforeEach` to the
// single default item; a test can override it (e.g. to force a tier-3 "moved"
// join) without disturbing the other tests.
let detailItems: unknown[] = [];
// What `GET /api/status` answers as `index_version`; a test can change this before
// firing a change event to simulate the index having moved on.
let statusVersion = "abc123";
// When non-null, each `GET /api/status` call captures its response body immediately
// (reflecting `statusVersion` at call time, like a real server would) but blocks
// on a fresh gate before returning it, and pushes that gate's resolver here — so a
// test can fire two overlapping loads and then choose which one's status resolves
// first, independent of call order.
let statusGates: (() => void)[] | null = null;
// Counts fetch calls whose URL contains `frag`.
const calls = (frag: string) => (globalThis.fetch as any).mock.calls.filter((c: any[]) => String(c[0]).includes(frag)).length;

beforeEach(() => {
  // The Tree/Map choice persists in localStorage (Task 6); clear it so one test's
  // switch to "map" does not become the next test's starting view.
  localStorage.clear();
  saved = null;
  mapState = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
  mapVersion = 1;
  putGate = null;
  putRejection = null;
  blastGate = null;
  retrievalList = [retrieval];
  changeHandler = null;
  graphNodesOverride = null;
  extraRetrievalItems = [];
  detailItems = [{ rank: 1, symbol_id: 1, path: "src/auth/session.ts", name: "createSession", line_start: 3, score: 0.1, served: true, reasons: r }];
  statusVersion = "abc123";
  statusGates = null;
  (globalThis as any).EventSource = class {
    addEventListener(type: string, cb: (e: { data: string }) => void) {
      if (type === "change") changeHandler = cb;
    }
    close() {}
  };
  window.location.hash = "#token=deadbeef";
  globalThis.fetch = mock(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const json = (b: unknown) => new Response(JSON.stringify(b), { headers: { "Content-Type": "application/json" } });
    if (init?.headers && (init.headers as Record<string, string>).Authorization !== "Bearer deadbeef") return new Response("{\"error\":\"unauthorized\"}", { status: 401 });
    if (url.endsWith("/api/status")) {
      const body = { ...status, index_version: statusVersion };
      if (statusGates) await new Promise<void>((resolve) => { statusGates!.push(resolve); });
      return json(body);
    }
    if (url.includes("/api/blast?")) {
      if (blastGate) await blastGate;
      return json({ root: { path: "src/auth/session.ts", symbol: "createSession" }, files: [{ path: "src/http/middleware.ts", depth: 1, via: "createSession" }], truncated: null });
    }
    if (url.endsWith("/api/graph")) return json({ index_version: "abc123", nodes: graphNodesOverride ?? tree.map((f) => ({ path: f.path, symbols: f.symbols.length, lang: f.lang })), edges: [] });
    if (url.includes("/api/retrievals?")) return json(retrievalList);
    if (url.endsWith("/api/retrievals/7")) return json({ ...retrieval, items: [...detailItems, ...extraRetrievalItems] });
    if (url.endsWith("/api/tree")) return json(tree);
    if (url.endsWith("/api/skipped")) return json([{ path: ".env", reason: "denylisted" }]);
    // GET /api/map returns a fresh object (new identity) each call, reflecting
    // whatever was last PUT — this both matches how the real server behaves
    // (a re-fetch is never the same object) and exercises the case a stale
    // effect dependency on `map`'s identity would have broken.
    if (url.endsWith("/api/map") && init?.method === "PUT") {
      if (putGate) await putGate;
      if (putRejection) return new Response(JSON.stringify(putRejection.body), { status: putRejection.status });
      const { expected_version, ...cfg } = JSON.parse(String(init.body));
      if (expected_version !== mapVersion) {
        return new Response(JSON.stringify({ error: "map.toml changed on disk", field: "expected_version", current: { ...mapState, version: mapVersion } }), { status: 409 });
      }
      mapState = cfg; saved = cfg; mapVersion += 1;
      return json({ ...mapState, version: mapVersion });
    }
    if (url.endsWith("/api/map")) return json({ ...mapState, version: mapVersion });
    return new Response("not found", { status: 404 });
  }) as unknown as typeof fetch;
});
afterEach(() => { mock.restore(); });

describe("App", () => {
  test("keyboard-only loop: select retrieval, expand file, focus symbol, exclude, toast, badge, skipped sheet", async () => {
    const user = userEvent.setup();
    render(<App />);
    const rail = await screen.findByRole("region", { name: "Retrievals" });
    expect(within(rail).getByText("Claude Code")).toBeTruthy();
    await user.click(within(rail).getByRole("button", { name: /repo_map.*session/ }));
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await waitFor(() => expect(within(grid).getByText("createSession")).toBeTruthy());
    expect(within(grid).getByText("Served")).toBeTruthy();
    await user.click(within(grid).getByText("createSession"));
    const panel = screen.getByRole("region", { name: "Details" });
    await waitFor(() => expect(within(panel).getByText("Ranked 1st, score 0.10.")).toBeTruthy());
    expect(within(panel).getByText("Referenced from middleware.ts (2).")).toBeTruthy();
    await user.click(within(panel).getByRole("button", { name: "Exclude file" }));
    await waitFor(() => expect(saved).not.toBeNull());
    expect((saved as any).exclude).toEqual([{ path: "src/auth/session.ts" }]);
    expect(await screen.findByText("Saved. Applies to the next retrieval.")).toBeTruthy();
    const badge = screen.getByLabelText("Index freshness");
    expect(badge.textContent).toContain("fresh");
    expect(badge.textContent).toContain("9b1e0d4");
    await user.click(screen.getByRole("button", { name: /Skipped files/ }));
    expect(await screen.findByText(".env")).toBeTruthy();
    expect(screen.getByText("denylisted")).toBeTruthy();
  });
  test("a symbol served at a line the tree no longer has it at shows as moved in the panel", async () => {
    const user = userEvent.setup();
    detailItems = [{ rank: 1, symbol_id: 999, path: "src/auth/session.ts", name: "createSession", line_start: 9, score: 0.1, served: true, reasons: r }];
    render(<App />);
    const rail = await screen.findByRole("region", { name: "Retrievals" });
    await user.click(within(rail).getByRole("button", { name: /repo_map.*session/ }));
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await waitFor(() => expect(within(grid).getByText("createSession")).toBeTruthy());
    await user.click(within(grid).getByText("createSession"));
    const panel = screen.getByRole("region", { name: "Details" });
    expect(await within(panel).findByText("Moved since this retrieval (was line 9)")).toBeTruthy();
  });
  test("announces a summary at load and only new retrievals after it", async () => {
    render(<App />);
    const live = await screen.findByRole("log", { name: "Announcements" });
    // History is not news: the first load summarises instead of reading out every
    // retrieval already on the page.
    await waitFor(() => expect(live.textContent).toContain("Loaded 1 retrievals"));
    expect(live.textContent).not.toContain("New retrieval from");

    // A later retrieval, arriving live, is announced in full.
    retrievalList = [{ ...retrieval, id: 8, session_label: "Codex", tool: "find_symbol", served: 2, cut: 1 }, retrieval];
    expect(changeHandler).not.toBeNull();
    changeHandler?.({ data: JSON.stringify({ max_retrieval_id: 8 }) });
    await waitFor(() => expect(live.textContent).toContain("New retrieval from Codex: find_symbol, 2 served, 1 cut, fresh"));
  });
  test("a_row_action_button_moves_focus_to_the_detail_panel", async () => {
    const user = userEvent.setup();
    render(<App />);
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    const row = within(grid).getAllByRole("row")[0];
    await user.click(within(row).getByText("src/auth/session.ts"));
    const action = within(row).getByRole("button", { name: "Actions for src/auth/session.ts" });
    expect(action.getAttribute("aria-controls")).toBe("detail-panel");
    action.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(document.activeElement?.id).toBe("detail-heading"));
    expect(document.activeElement?.textContent).toBe("src/auth/session.ts");
  });
  test("has no axe violations once loaded", async () => {
    const { container } = render(<App />);
    await screen.findByRole("treegrid", { name: "Repository" });
    const results = await axe.run(container);
    expect(results.violations).toEqual([]);
  });
  test("a_draft_note_survives_a_live_change_refetch", async () => {
    const user = userEvent.setup();
    render(<App />);
    const rail = await screen.findByRole("region", { name: "Retrievals" });
    await user.click(within(rail).getByRole("button", { name: /repo_map.*session/ }));
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await waitFor(() => expect(within(grid).getByText("createSession")).toBeTruthy());
    await user.click(within(grid).getByText("createSession"));
    const panel = screen.getByRole("region", { name: "Details" });
    const note = () => within(panel).getByLabelText("Note") as HTMLTextAreaElement;
    await waitFor(() => expect(note().value).toBe(""));

    await user.type(note(), "draft");
    expect(note().value).toBe("draft");

    // A live "change" event (another agent's retrieval landed) makes App
    // refetch status/retrievals/tree/skipped/map in the background. The
    // unblurred draft must not be clobbered by that refetch.
    expect(changeHandler).not.toBeNull();
    const mapCallsBefore = (globalThis.fetch as any).mock.calls.filter(
      (c: any[]) => String(c[0]).endsWith("/api/map") && c[1]?.method === "GET",
    ).length;
    changeHandler?.({ data: JSON.stringify({ max_retrieval_id: 7 }) });
    await waitFor(() => {
      const mapCallsAfter = (globalThis.fetch as any).mock.calls.filter(
        (c: any[]) => String(c[0]).endsWith("/api/map") && c[1]?.method === "GET",
      ).length;
      expect(mapCallsAfter).toBeGreaterThan(mapCallsBefore);
    });
    expect(note().value).toBe("draft");

    // Blurring saves the draft.
    await user.tab();
    await waitFor(() => expect(saved).not.toBeNull());
    expect((saved as any).note).toEqual([{ path: "src/auth/session.ts", symbol: "createSession", text: "draft" }]);

    // Switching to a different target and back loads the saved text.
    await user.click(within(grid).getByText("src/auth/session.ts"));
    await waitFor(() => expect(note().value).toBe(""));
    await user.click(within(grid).getByText("createSession"));
    await waitFor(() => expect(note().value).toBe("draft"));
  });
  test("rapid_pin_then_exclude_both_persist", async () => {
    const user = userEvent.setup();
    render(<App />);
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await user.click(within(grid).getByText("src/auth/session.ts"));
    const panel = screen.getByRole("region", { name: "Details" });
    // Hold the first PUT open so the second click is made against the pre-save map.
    let release!: () => void;
    putGate = new Promise<void>((r) => { release = r; });
    await user.click(within(panel).getByRole("button", { name: "Pin file" }));
    await user.click(within(panel).getByRole("button", { name: "Exclude file" }));
    release();
    await waitFor(() => {
      expect(saved?.pin).toEqual([{ path: "src/auth/session.ts" }]);
      expect(saved?.exclude).toEqual([{ path: "src/auth/session.ts" }]);
    });
  });
  test("a_422_names_the_offending_field_in_the_toast", async () => {
    const user = userEvent.setup();
    render(<App />);
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await user.click(within(grid).getByText("src/auth/session.ts"));
    const panel = screen.getByRole("region", { name: "Details" });
    putRejection = { status: 422, body: { error: "path must not contain ..", field: "exclude[0].path" } };
    await user.click(within(panel).getByRole("button", { name: "Exclude file" }));
    expect(await screen.findByText("exclude[0].path: path must not contain ..")).toBeTruthy();
    expect(saved).toBeNull();
  });
  test("a_409_reloads_and_tells_the_user", async () => {
    const user = userEvent.setup();
    render(<App />);
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await user.click(within(grid).getByText("src/auth/session.ts"));
    const panel = screen.getByRole("region", { name: "Details" });
    // A hand edit lands on map.toml after the page read its copy.
    mapState = { pin: [{ path: "src/util/log.ts" }], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
    mapVersion = 42;
    await user.click(within(panel).getByRole("button", { name: "Exclude file" }));
    expect(await screen.findByText("map.toml changed on disk; reloaded, please redo that change")).toBeTruthy();
    expect(saved).toBeNull();
    // The client reloaded from the 409's `current`, so redoing the edit now lands and
    // keeps the hand-written pin instead of destroying it.
    await user.click(within(panel).getByRole("button", { name: "Exclude file" }));
    await waitFor(() => expect(saved).not.toBeNull());
    expect(saved.pin).toEqual([{ path: "src/util/log.ts" }]);
    expect(saved.exclude).toEqual([{ path: "src/auth/session.ts" }]);
  });
  test("the skipped sheet and the note field have no animation classes (spec §6: nothing animates in 3a)", async () => {
    const user = userEvent.setup();
    render(<App />);
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await user.click(within(grid).getByText("src/auth/session.ts"));
    const note = screen.getByLabelText("Note");
    expect(note.className).not.toContain("transition-");
    expect(note.className).not.toContain("duration-");
    await user.click(screen.getByRole("button", { name: /Skipped files/ }));
    await screen.findByText(".env");
    const content = document.querySelector('[data-slot="sheet-content"]');
    expect(content).toBeTruthy();
    const nodes = [content as Element, ...Array.from((content as Element).querySelectorAll("*"))];
    for (const el of nodes) {
      const cls = (el as HTMLElement).className;
      if (typeof cls === "string" && cls.length > 0) {
        expect(cls).not.toContain("transition-");
        expect(cls).not.toContain("duration-");
      }
    }
  });
  test("the boundaries line is a labelled group", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("src/auth/session.ts");
    await user.click(screen.getAllByRole("button", { name: /Actions for src\/auth\/session.ts/ })[0]);
    const panel = screen.getByRole("region", { name: "Details" });
    expect(within(panel).getByRole("group", { name: "Boundaries" })).toBeTruthy();
  });

  test("submitting an empty boundary name is a no-op, not a save", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("src/auth/session.ts");
    await user.click(screen.getAllByRole("button", { name: /Actions for src\/auth\/session.ts/ })[0]);
    const input = await screen.findByLabelText("Add to boundary") as HTMLInputElement;
    expect(input.value).toBe("");
    await user.click(screen.getByRole("button", { name: "Add" }));
    const putCalls = () => (globalThis.fetch as any).mock.calls.filter((c: any[]) => String(c[0]).endsWith("/api/map") && c[1]?.method === "PUT").length;
    expect(putCalls()).toBe(0);
    expect(saved).toBeNull();
  });

  test("a file can be added to a new boundary and removed again", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("src/auth/session.ts");
    await user.click(screen.getAllByRole("button", { name: /Actions for src\/auth\/session.ts/ })[0]);
    const input = await screen.findByLabelText("Add to boundary");
    await user.type(input, "auth");
    await user.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(saved?.boundary).toEqual([{ name: "auth", paths: ["src/auth/session.ts"] }]));
    await user.click(await screen.findByRole("button", { name: "Remove from auth" }));
    await waitFor(() => expect(saved?.boundary).toEqual([]));
  });

  test("excluding a file refetches the graph so the map reflects it", async () => {
    const user = userEvent.setup();
    render(<App />);
    const grid = await screen.findByRole("treegrid", { name: "Repository" });
    await user.click(within(grid).getByText("src/auth/session.ts"));
    const panel = screen.getByRole("region", { name: "Details" });
    const graphCallsBefore = (globalThis.fetch as any).mock.calls.filter((c: any[]) => String(c[0]).endsWith("/api/graph")).length;
    await user.click(within(panel).getByRole("button", { name: "Exclude file" }));
    await waitFor(() => expect(saved).not.toBeNull());
    await waitFor(() => {
      const graphCallsAfter = (globalThis.fetch as any).mock.calls.filter((c: any[]) => String(c[0]).endsWith("/api/graph")).length;
      expect(graphCallsAfter).toBeGreaterThan(graphCallsBefore);
    });
  });

  test("a change event with the same index version refetches retrievals and the map but not the tree or graph", async () => {
    render(<App />);
    await screen.findByText("src/auth/session.ts");
    const tree0 = calls("/api/tree"), graph0 = calls("/api/graph"), rs0 = calls("/api/retrievals?");
    await act(async () => { changeHandler!({ data: JSON.stringify({ max_retrieval_id: 7 }) }); });
    await waitFor(() => expect(calls("/api/retrievals?")).toBe(rs0 + 1));
    expect(calls("/api/tree")).toBe(tree0);
    expect(calls("/api/graph")).toBe(graph0);
  });

  test("a change event with a new index version refetches the tree and the graph", async () => {
    render(<App />);
    await screen.findByText("src/auth/session.ts");
    const tree0 = calls("/api/tree"), graph0 = calls("/api/graph");
    statusVersion = "def456";
    await act(async () => { changeHandler!({ data: JSON.stringify({ max_retrieval_id: 7 }) }); });
    await waitFor(() => expect(calls("/api/tree")).toBe(tree0 + 1));
    expect(calls("/api/graph")).toBe(graph0 + 1);
  });

  test("a superseded load applies nothing, even if it resolves after the load that superseded it", async () => {
    render(<App />);
    await screen.findByText("src/auth/session.ts");
    const tree0 = calls("/api/tree");

    statusGates = [];
    // Load A starts (older generation) and blocks on its /api/status call.
    changeHandler!({ data: JSON.stringify({ max_retrieval_id: 7 }) });
    await waitFor(() => expect(statusGates!.length).toBe(1));

    // Load B starts (newer generation, newer index version) and also blocks.
    statusVersion = "def456";
    changeHandler!({ data: JSON.stringify({ max_retrieval_id: 7 }) });
    await waitFor(() => expect(statusGates!.length).toBe(2));

    // Release B's gate and let it run to completion (including its tree/graph
    // refetch and its `indexRef` write) before releasing A — so A (started first)
    // resolves LAST, the exact ordering that pinned `indexRef`/tree/graph to a
    // stale version before the generation-counter fix.
    const [resolveA, resolveB] = statusGates!;
    await act(async () => { resolveB(); });
    await waitFor(() => expect(calls("/api/tree")).toBe(tree0 + 1));
    await act(async () => { resolveA(); });
    // Give A's now-unblocked chain (Promise.all resolution, the indexRef check,
    // and — in the pre-fix code — a second tree/graph fetch) every remaining tick
    // it needs to finish; `waitFor`'s retrying poll (not a fixed flush count)
    // makes the assertion below deterministic either way.
    await act(async () => { await new Promise((r) => setTimeout(r, 20)); });

    // Only B's tree/graph refetch should have landed: A's late-arriving (superseded)
    // load must not have refetched the tree a second time (the old bug: whichever
    // load resolved last re-fetched and overwrote `indexRef` back to its own stale
    // version, "abc123", causing a second, redundant tree fetch here).
    expect(calls("/api/tree")).toBe(tree0 + 1);

    // `indexRef` must have settled on B's version ("def456"), not A's stale
    // "abc123" — a further change event carrying "def456" again must not refetch
    // the tree, which it would if A's stale write had won.
    statusGates = null;
    const rs0 = calls("/api/retrievals?");
    await act(async () => { changeHandler!({ data: JSON.stringify({ max_retrieval_id: 7 }) }); });
    await waitFor(() => expect(calls("/api/retrievals?")).toBe(rs0 + 1));
    expect(calls("/api/tree")).toBe(tree0 + 1);
  });

  test("a symbol row can show its blast radius in the panel", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /repo_map/ }));
    await user.click(await screen.findByRole("button", { name: "Actions for createSession" }));
    await user.click(await screen.findByRole("button", { name: "Show blast radius" }));
    const list = await screen.findByRole("list", { name: "Blast radius" });
    expect(within(list).getByText(/src\/http\/middleware.ts \(depth 1, via createSession\)/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Hide blast radius" })).toBeTruthy();
  });

  test("a file row offers Show symbols and toggles to Hide symbols", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("src/auth/session.ts");
    await user.click(screen.getAllByRole("button", { name: /Actions for src\/auth\/session.ts/ })[0]);
    await user.click(await screen.findByRole("button", { name: "Show symbols" }));
    expect(await screen.findByRole("button", { name: "Hide symbols" })).toBeTruthy();
  });

  test("a_blast_response_arriving_after_focus_moved_is_dropped", async () => {
    const user = userEvent.setup();
    let release!: () => void;
    blastGate = new Promise<void>((r) => { release = r; });
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /repo_map/ }));
    await user.click(await screen.findByRole("button", { name: "Actions for createSession" }));
    await user.click(await screen.findByRole("button", { name: "Show blast radius" }));
    await user.click(screen.getAllByRole("button", { name: /Actions for src\/auth\/session.ts/ })[0]);
    await act(async () => {
      release();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(screen.queryByRole("list", { name: "Blast radius" })).toBeNull();
    const live = screen.getByRole("log", { name: "Announcements" });
    expect(live.textContent).not.toContain("Blast radius:");
  });

  test("toggling_blast_twice_while_loading_fetches_once", async () => {
    const user = userEvent.setup();
    let release!: () => void;
    blastGate = new Promise<void>((r) => { release = r; });
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /repo_map/ }));
    await user.click(await screen.findByRole("button", { name: "Actions for createSession" }));
    const button = await screen.findByRole("button", { name: "Show blast radius" });
    await user.click(button);
    await waitFor(() => expect(button.getAttribute("aria-busy")).toBe("true"));
    const blastCalls = () => (globalThis.fetch as any).mock.calls.filter((c: any[]) => String(c[0]).includes("/api/blast?")).length;
    expect(blastCalls()).toBe(1);
    // The in-flight guard in App.toggleBlast, not `disabled`, prevents a second fetch —
    // the button stays focusable and keeps keyboard focus across the click (I9).
    await user.click(button);
    expect(blastCalls()).toBe(1);
    expect(document.activeElement).toBe(button);
    release();
    const list = await screen.findByRole("list", { name: "Blast radius" });
    expect(list).toBeTruthy();
  });

  test("the map view shows the summary label, switch-to-table returns focus to the tree, and the choice persists", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /repo_map/ }));
    await user.click(screen.getByRole("radio", { name: "Map" }));
    const img = await screen.findByRole("img", { name: /Map of 1 file\. Retrieval 7: 1 served, 0 cut, 0 untouched\. 0 boundaries\./ });
    expect(img).toBeTruthy();
    expect(localStorage.getItem("singularrag.view")).toBe("map");
    await waitFor(() => expect(screen.getByRole("log", { name: "Announcements" }).textContent).toContain("Map layout ready"));
    await user.click(screen.getByRole("button", { name: "Switch to table" }));
    expect(await screen.findByRole("treegrid")).toBeTruthy();
    const ae = document.activeElement;
    expect(ae?.getAttribute("role") === "treegrid" || (ae?.getAttribute("role") === "row" && ae.closest('[role="treegrid"]') !== null)).toBe(true);
    expect(localStorage.getItem("singularrag.view")).toBe("tree");
  });

  test("the summary counts only files present in the graph, not every row", async () => {
    const secondFile = { path: "src/util/log.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 2, name: "log", kind: "function", line_start: 1, line_end: 2, signature: "export function log(msg: string): void" }] };
    tree.push(secondFile);
    // log.ts is served in the retrieval but excluded from the graph; the summary must
    // not count it (I13: counted over the graph's node paths, not every tree row).
    graphNodesOverride = [{ path: "src/auth/session.ts", symbols: 1, lang: "typescript" }];
    extraRetrievalItems = [{ rank: 2, symbol_id: 2, path: "src/util/log.ts", name: "log", line_start: 1, score: 0.05, served: true, reasons: r }];
    try {
      const user = userEvent.setup();
      render(<App />);
      await user.click(await screen.findByRole("button", { name: /repo_map/ }));
      await user.click(screen.getByRole("radio", { name: "Map" }));
      const img = await screen.findByRole("img", { name: /Map of 1 file\. Retrieval 7: 1 served, 0 cut, 0 untouched\. 0 boundaries\./ });
      expect(img).toBeTruthy();
    } finally {
      tree.pop();
    }
  });

  test("selecting a node on the map focuses the same row in the panel", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /repo_map/ }));
    await user.click(screen.getByRole("radio", { name: "Map" }));
    await screen.findByRole("img");
    const s = (globalThis as any).__sigma.instances.at(-1);
    await act(async () => { s.emit("clickNode", { node: "src/auth/session.ts" }); });
    expect(screen.getByRole("heading", { name: "src/auth/session.ts" })).toBeTruthy();
  });

  test("the details panel scrolls its own overflow instead of the page", async () => {
    render(<App />);
    const panel = await screen.findByRole("region", { name: "Details" });
    expect(panel.className).toContain("overflow-auto");
    expect(panel.className).toContain("min-h-0");
  });

  test("the map view is axe clean", async () => {
    const user = userEvent.setup();
    const { container } = render(<App />);
    await screen.findByText("src/auth/session.ts");
    await user.click(screen.getByRole("radio", { name: "Map" }));
    await screen.findByRole("img");
    expect((await axe.run(container)).violations).toEqual([]);
  });
});
