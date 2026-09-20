import { describe, expect, test } from "bun:test";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { RepoTree } from "./RepoTree";
import { joinRetrieval } from "@/lib/join";
import type { TreeFile } from "@/api/types";

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
