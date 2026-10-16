import { afterEach, expect, mock, test } from "bun:test";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import type { MapConfig, TreeSymbol } from "../api/types";
import type { FileRow, SymbolRow } from "../lib/join";
import { DetailPanel } from "./DetailPanel";

afterEach(cleanup);

const map: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
const sym = (id: number, name: string, kind: string, line: number, signature: string, refs?: string[]): SymbolRow => ({
  key: `prisma/schema.prisma::${name}`, status: "untouched", item: null, moved: false,
  symbol: { id, name, kind, line_start: line, line_end: line, signature, ...(refs ? { refs } : {}) } as TreeSymbol,
});
const file: FileRow = {
  path: "prisma/schema.prisma", lang: "prisma", served: 0, cut: 0, expandedByDefault: false,
  symbols: [
    sym(4, "user", "table", 1, "table user (1 columns)"),
    sym(1, "website", "table", 10, "table website (2 columns, model Website)"),
    sym(2, "website.website_id", "column", 11, "String @id @map(\"website_id\") @db.Uuid"),
    sym(3, "website.user_id", "column", 12, "String? @map(\"user_id\") @db.Uuid", ["user"]),
  ],
};
const noop = () => {};

function renderPanel(row: SymbolRow) {
  const onFocusSection = mock((_path: string, _name: string, _id?: number) => {});
  const r = render(
    <DetailPanel row={{ kind: "symbol", path: file.path, file, symbol: row }} map={map} entities={null}
      onFocusEntity={noop} onFocusSection={onFocusSection} onPin={noop} onExclude={noop} onNote={noop} onRemoveAgentNote={noop}
      blast={null} blastLoading={false} onToggleBlast={noop} expandedPath={null} onToggleExpand={noop}
      onAddBoundary={noop} onRemoveBoundary={noop} />,
  );
  return { ...r, onFocusSection };
}

test("a table lists its columns with a button to each foreign key's table", async () => {
  const { container, onFocusSection } = renderPanel(file.symbols[1]);
  const list = screen.getByRole("list", { name: "Columns" });
  const items = within(list).getAllByRole("listitem");
  expect(items.map((li) => li.querySelector("code")?.textContent)).toEqual(["website_id", "user_id"]);
  expect(items[1].textContent).toContain("String? @map(\"user_id\") @db.Uuid");
  await userEvent.click(within(list).getByRole("button", { name: "References user" }));
  expect(onFocusSection).toHaveBeenCalledWith("prisma/schema.prisma", "user");
  expect((await axe.run(container)).violations).toEqual([]);
});

test("a column links back to its table and to the table it references", async () => {
  const { container, onFocusSection } = renderPanel(file.symbols[3]);
  await userEvent.click(screen.getByRole("button", { name: "Table website" }));
  expect(onFocusSection).toHaveBeenCalledWith("prisma/schema.prisma", "website");
  expect(screen.getByRole("button", { name: "References user" })).toBeTruthy();
  expect(screen.queryByRole("list", { name: "Columns" })).toBeNull();
  expect((await axe.run(container)).violations).toEqual([]);
});

test("a table with no columns says so", () => {
  renderPanel(file.symbols[0]);
  expect(screen.getByRole("heading", { name: "Columns" })).toBeTruthy();
  expect(screen.getByText("No columns.")).toBeTruthy();
});

test("a function shows neither section", () => {
  renderPanel(sym(9, "getWebsite", "function", 3, "export function getWebsite(websiteId: string)"));
  expect(screen.queryByRole("heading", { name: "Columns" })).toBeNull();
  expect(screen.queryByRole("button", { name: /^Table / })).toBeNull();
});
