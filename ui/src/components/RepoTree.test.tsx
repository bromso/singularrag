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
    render(<RepoTree rows={joinRetrieval(files, null)} filter="" seedKey={0} onFocusRow={() => {}} onAction={(r) => actions.push(r.path)} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const rows = within(grid).getAllByRole("row");
    expect(rows.length).toBe(2);
    await user.click(within(rows[0]).getByText("src/a.ts"));
    await user.keyboard("{ArrowRight}");
    expect(within(grid).getAllByRole("row").length).toBe(3);
    expect(within(grid).getByText("f")).toBeTruthy();
    // Spec §6/§12 Q1: react-aria-components renders exactly one gridcell per row, so
    // the keyboard model has no cell-to-cell navigation.
    for (const row of within(grid).getAllByRole("row")) {
      expect(within(row).getAllByRole("gridcell").length).toBe(1);
    }
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{ArrowRight}");
    await user.keyboard("{Enter}");
    expect(actions).toEqual(["src/a.ts"]);
  });
  test("arrowing_between_rows_reports_each_focused_row", async () => {
    const user = userEvent.setup();
    const onFocusRow = mock();
    render(<RepoTree rows={joinRetrieval(files, null)} filter="" seedKey={0} onFocusRow={onFocusRow} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const rows = within(grid).getAllByRole("row");
    await user.click(within(rows[0]).getByText("src/a.ts"));
    await user.keyboard("{ArrowDown}");
    expect(onFocusRow.mock.calls[0]?.[0]).toMatchObject({ kind: "file", path: "src/a.ts" });
    expect(onFocusRow.mock.calls[1]?.[0]).toMatchObject({ kind: "file", path: "src/b.ts" });
  });
  test("touched_files_render_first_with_a_retrieval", () => {
    const reasons = { score: 0.5, file_rank: 1, seeds: [], referenced_by: [{ path: "src/x.ts", count: 2 }], pinned: false, fts_hit: false, query_ident_match: false, note_hit: false, body_hit: false };
    const items: Item[] = [
      { rank: 1, symbol_id: 2, path: "src/b.ts", name: "h", line_start: 1, score: 0.5, served: true, reasons },
    ];
    const joined = joinRetrieval(files, items);
    joined[0].symbols[0].moved = true;
    render(<RepoTree rows={joined} filter="" seedKey={0} onFocusRow={() => {}} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const rows = within(grid).getAllByRole("row");
    expect(within(rows[0]).getByText("src/b.ts")).toBeTruthy();
    expect(within(grid).getByText("Referenced from x.ts (2).")).toBeTruthy();
    expect(within(rows[1]).getByText("moved")).toBeTruthy();
  });
  test("filter narrows by path or symbol name", () => {
    render(<RepoTree rows={joinRetrieval(files, null)} filter="h" seedKey={0} onFocusRow={() => {}} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    expect(within(grid).queryByText("src/a.ts")).toBeNull();
    expect(within(grid).getByText("src/b.ts")).toBeTruthy();
  });
  test("a_live_refetch_keeps_the_user_expansion_but_a_new_retrieval_reseeds_it", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<RepoTree rows={joinRetrieval(files, null)} filter="" seedKey={0} onFocusRow={() => {}} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    await user.click(within(grid).getByText("src/a.ts"));
    await user.keyboard("{ArrowRight}");
    expect(within(grid).getByText("f")).toBeTruthy();

    // An SSE "change" event refetches /api/tree, so App hands down a brand-new `rows`
    // array with the same content and the same selection. The expansion must survive.
    rerender(<RepoTree rows={joinRetrieval(files, null)} filter="" seedKey={0} onFocusRow={() => {}} onAction={() => {}} />);
    expect(within(grid).getByText("f")).toBeTruthy();

    // Selecting a different retrieval does re-seed: only its touched files open.
    const reasons = { score: 0.5, file_rank: 1, seeds: [], referenced_by: [], pinned: false, fts_hit: false, query_ident_match: false, note_hit: false, body_hit: false };
    const items: Item[] = [{ rank: 1, symbol_id: 2, path: "src/b.ts", name: "h", line_start: 1, score: 0.5, served: true, reasons }];
    rerender(<RepoTree rows={joinRetrieval(files, items)} filter="" seedKey={7} onFocusRow={() => {}} onAction={() => {}} />);
    expect(within(grid).getByText("h")).toBeTruthy();
    expect(within(grid).queryByText("f")).toBeNull();
  });
  test("rows_carry_a_keyboard_focus_outline_not_just_a_tint", async () => {
    const user = userEvent.setup();
    render(<RepoTree rows={joinRetrieval(files, null)} filter="" seedKey={0} onFocusRow={() => {}} onAction={() => {}} />);
    const grid = screen.getByRole("treegrid", { name: "Repository" });
    const row = within(grid).getAllByRole("row")[0];
    await user.click(within(row).getByText("src/a.ts"));
    await user.keyboard("{ArrowDown}");
    // The tint (`data-[focused]:bg-accent`) is ~1.07:1 against the background; the
    // outline utilities are what make row focus visible. Contrast itself cannot be
    // computed under happy-dom, so this asserts the classes are on the row.
    const focused = within(grid).getAllByRole("row")[1];
    expect(focused.className).toContain("data-[focus-visible]:outline-2");
    expect(focused.className).toContain("data-[focus-visible]:outline-ring");
    expect(focused.className).not.toContain("outline-none");
  });
  test("has no axe violations", async () => {
    // The row action buttons are `aria-controls="detail-panel"`; App renders that panel
    // next to the tree, so the stand-in here keeps the reference resolvable.
    const { container } = render(
      <>
        <RepoTree rows={joinRetrieval(files, null)} filter="" seedKey={0} onFocusRow={() => {}} onAction={() => {}} />
        <section id="detail-panel" aria-label="Details" />
      </>,
    );
    const results = await axe.run(container);
    expect(results.violations).toEqual([]);
  });
});
