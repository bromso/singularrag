import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { App } from "./App";

const status = { index_version: "abc123", git_head: "9b1e0d4f", indexed_at_ms: Date.now(), stale_count: 0, lock_timeout: false, foreign_indexing: false, files: { indexed: 4, skipped: 1 }, drain: { chunks: 0, last: { scanned: 5, indexed: 4, unchanged: 0, skipped: 1, removed: 0, remaining: 0, lock_timeout: false } } };
const r = { score: 0.1, file_rank: 0.1, seeds: ["query:session"], referenced_by: [{ path: "src/http/middleware.ts", count: 2 }], pinned: false, fts_hit: true, query_ident_match: true };
const retrieval = { id: 7, session_key: "mcp:claude-code:1:2", session_label: "Claude Code", tool: "repo_map", query: "session", focus_files: [], budget: 1024, limit_n: null, index_version: "abc123", git_head: "9b1e0d4f", stale_count: 0, created_at_ms: Date.now(), served: 1, cut: 0 };
const tree = [{ path: "src/auth/session.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 1, name: "createSession", kind: "function", line_start: 3, line_end: 6, signature: "export function createSession(user: User, ttl: number): Session" }] }];
let saved: unknown = null;
let mapState: any = null;
// The `subscribe()` module registers its "change" listener via
// `EventSource#addEventListener`; capturing it here lets a test simulate a
// live SSE "change" event (another agent's retrieval) without a real
// EventSource, by invoking the captured listener directly.
let changeHandler: ((e: { data: string }) => void) | null = null;

beforeEach(() => {
  saved = null;
  mapState = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
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
    if (url.includes("/api/retrievals?")) return json([retrieval]);
    if (url.endsWith("/api/retrievals/7")) return json({ ...retrieval, items: [{ rank: 1, symbol_id: 1, path: "src/auth/session.ts", name: "createSession", line_start: 3, score: 0.1, served: true, reasons: r }] });
    if (url.endsWith("/api/tree")) return json(tree);
    if (url.endsWith("/api/skipped")) return json([{ path: ".env", reason: "denylisted" }]);
    // GET /api/map returns a fresh object (new identity) each call, reflecting
    // whatever was last PUT — this both matches how the real server behaves
    // (a re-fetch is never the same object) and exercises the case a stale
    // effect dependency on `map`'s identity would have broken.
    if (url.endsWith("/api/map") && init?.method === "PUT") { mapState = JSON.parse(String(init.body)); saved = mapState; return json(mapState); }
    if (url.endsWith("/api/map")) return json({ ...mapState });
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
  test("announces a new retrieval in the live region", async () => {
    render(<App />);
    const live = await screen.findByRole("log", { name: "Announcements" });
    await waitFor(() => expect(live.textContent).toContain("New retrieval from Claude Code: repo_map, 1 served, 0 cut, fresh"));
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
  test("the skipped sheet has no animation classes (spec §6: nothing animates in 3a)", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole("treegrid", { name: "Repository" });
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
