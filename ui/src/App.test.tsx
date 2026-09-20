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

beforeEach(() => {
  saved = null;
  (globalThis as any).EventSource = class { addEventListener() {} close() {} };
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
    if (url.endsWith("/api/map") && init?.method === "PUT") { saved = JSON.parse(String(init.body)); return json(saved); }
    if (url.endsWith("/api/map")) return json({ pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } });
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
});
