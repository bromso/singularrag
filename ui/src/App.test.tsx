import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor, within } from "@testing-library/react";
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
// The `subscribe()` module registers its "change" listener via
// `EventSource#addEventListener`; capturing it here lets a test simulate a
// live SSE "change" event (another agent's retrieval) without a real
// EventSource, by invoking the captured listener directly.
let changeHandler: ((e: { data: string }) => void) | null = null;

beforeEach(() => {
  saved = null;
  mapState = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
  mapVersion = 1;
  putGate = null;
  retrievalList = [retrieval];
  changeHandler = null;
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
    if (url.endsWith("/api/status")) return json(status);
    if (url.includes("/api/retrievals?")) return json(retrievalList);
    if (url.endsWith("/api/retrievals/7")) return json({ ...retrieval, items: [{ rank: 1, symbol_id: 1, path: "src/auth/session.ts", name: "createSession", line_start: 3, score: 0.1, served: true, reasons: r }] });
    if (url.endsWith("/api/tree")) return json(tree);
    if (url.endsWith("/api/skipped")) return json([{ path: ".env", reason: "denylisted" }]);
    // GET /api/map returns a fresh object (new identity) each call, reflecting
    // whatever was last PUT — this both matches how the real server behaves
    // (a re-fetch is never the same object) and exercises the case a stale
    // effect dependency on `map`'s identity would have broken.
    if (url.endsWith("/api/map") && init?.method === "PUT") {
      if (putGate) await putGate;
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
    const badge = screen.getByRole("status", { name: "Index freshness" });
    expect(badge.textContent).toContain("fresh");
    expect(badge.textContent).toContain("9b1e0d4");
    await user.click(screen.getByRole("button", { name: /Skipped files/ }));
    expect(await screen.findByText(".env")).toBeTruthy();
    expect(screen.getByText("denylisted")).toBeTruthy();
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
});
