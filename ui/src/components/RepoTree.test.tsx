import { describe, expect, mock, test } from "bun:test";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { RepoTree } from "./RepoTree";
import { joinRetrieval } from "@/lib/join";
import type { Item, TreeFile } from "@/api/types";

const files: TreeFile[] = [
  { path: "src/a.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 1, name: "f", kind: "function", line_start: 1, line_end: 3, signature: "export function f()" }] },
  { path: "src/b.ts", lang: "typescript", skipped_reason: null, symbols: [{ id: 2, name: "h", kind: "function", line_start: 1, line_end: 2, signature: "export function h()" }] },
];

describe("RepoTree", () => {
  test("renders a treegrid with file rows, expands with the keyboard, and reaches the action button", async () => {
    const user = userEvent.setup();
    const actions: string[] = [];
    render(<RepoTree rows={joinRetrieval(files, null)} filter="" onFocusRow={() => {}} onAction={(r) => actions.push(r.path)} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const rows = within(grid).getAllByRole("row");
    expect(rows.length).toBe(2);
    await user.click(within(rows[0]).getByText("src/a.ts"));
    await user.keyboard("{ArrowRight}");
    expect(within(grid).getAllByRole("row").length).toBe(3);
    expect(within(grid).getByText("f")).toBeTruthy();
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{ArrowRight}");
    await user.keyboard("{Enter}");
    expect(actions).toEqual(["src/a.ts"]);
  });
  test("arrowing_between_rows_reports_each_focused_row", async () => {
    const user = userEvent.setup();
    const onFocusRow = mock();
    render(<RepoTree rows={joinRetrieval(files, null)} filter="" onFocusRow={onFocusRow} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const rows = within(grid).getAllByRole("row");
    await user.click(within(rows[0]).getByText("src/a.ts"));
    await user.keyboard("{ArrowDown}");
    expect(onFocusRow.mock.calls[0]?.[0]).toMatchObject({ kind: "file", path: "src/a.ts" });
    expect(onFocusRow.mock.calls[1]?.[0]).toMatchObject({ kind: "file", path: "src/b.ts" });
  });
  test("touched_files_render_first_with_a_retrieval", () => {
    const reasons = { score: 0.5, file_rank: 1, seeds: [], referenced_by: [{ path: "src/x.ts", count: 2 }], pinned: false, fts_hit: false, query_ident_match: false };
    const items: Item[] = [
      { rank: 1, symbol_id: 2, path: "src/b.ts", name: "h", line_start: 1, score: 0.5, served: true, reasons },
    ];
    render(<RepoTree rows={joinRetrieval(files, items)} filter="" onFocusRow={() => {}} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const rows = within(grid).getAllByRole("row");
    expect(within(rows[0]).getByText("src/b.ts")).toBeTruthy();
    expect(within(grid).getByText("Referenced from x.ts (2).")).toBeTruthy();
  });
  test("filter narrows by path or symbol name", () => {
    render(<RepoTree rows={joinRetrieval(files, null)} filter="h" onFocusRow={() => {}} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    expect(within(grid).queryByText("src/a.ts")).toBeNull();
    expect(within(grid).getByText("src/b.ts")).toBeTruthy();
  });
  test("has no axe violations", async () => {
    const { container } = render(<RepoTree rows={joinRetrieval(files, null)} filter="" onFocusRow={() => {}} onAction={() => {}} />);
    const results = await axe.run(container);
    expect(results.violations).toEqual([]);
  });
});
