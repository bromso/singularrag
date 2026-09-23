import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryPanel } from "./QueryPanel";
// `mock.module` below replaces "@/api/client"'s exports for the whole process, not
// just this file — every other file's `import ... from "@/api/client"` (e.g.
// api/client.test.ts, App.test.tsx) is a live binding onto the same module, so left
// unrestored it would break them for the rest of this `bun test` run. Snapshot the
// real exports into plain objects/functions *before* mocking (a spread copies the
// current values into a new object immune to the later rebind) so `afterAll` can
// re-mock the module back to the real implementation.
import * as realClient from "@/api/client";
const realApi = { ...realClient.api };
const { ApiError: RealApiError, getToken: realGetToken, resetTokenMemo: realResetTokenMemo, setToken: realSetToken, tokenFromFragment: realTokenFromFragment } = realClient;

const calls: unknown[] = [];
mock.module("@/api/client", () => ({
  api: {
    query: async (query: string, budget: number) => {
      calls.push({ query, budget });
      if (query === "boom") throw new Error("query must be 1 to 2000 characters");
      return { retrieval_id: 9, served: 3, cut: 2 };
    },
  },
}));

afterEach(() => { calls.length = 0; });
afterAll(() => {
  mock.module("@/api/client", () => ({
    api: realApi, ApiError: RealApiError, getToken: realGetToken,
    resetTokenMemo: realResetTokenMemo, setToken: realSetToken, tokenFromFragment: realTokenFromFragment,
  }));
});

describe("QueryPanel", () => {
  test("submits the query and budget, reports the result", async () => {
    const done: number[] = [];
    const said: string[] = [];
    render(<QueryPanel onDone={(id) => done.push(id)} announce={(m) => said.push(m)} />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Query"), "where is routing");
    await user.selectOptions(screen.getByLabelText("Budget"), "2048");
    await user.click(screen.getByRole("button", { name: "Run query" }));
    await waitFor(() => expect(done).toEqual([9]));
    expect(calls).toEqual([{ query: "where is routing", budget: 2048 }]);
    expect(said).toContain("3 served, 2 cut");
  });

  test("query_panel_announces_errors", async () => {
    const said: string[] = [];
    render(<QueryPanel onDone={() => {}} announce={(m) => said.push(m)} />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Query"), "boom");
    await user.click(screen.getByRole("button", { name: "Run query" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("1 to 2000"));
    expect(said.some((m) => m.includes("1 to 2000"))).toBe(true);
    const results = await axe.run(document.body);
    expect(results.violations).toEqual([]);
  });
});
