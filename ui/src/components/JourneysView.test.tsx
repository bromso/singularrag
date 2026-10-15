import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import type { Process, ProcessesPayload } from "../api/types";
import { setToken } from "../api/client";
import { JourneysView } from "./JourneysView";

const payload: ProcessesPayload = { processes: [
  { id: 1, name: "Expense process", description: "how you get money back", roles: ["Manager", "employee"], steps: [
    { ordinal: 1, text: "Submit each expense in Expensify", role: "employee", systems: [{ id: 3, name: "Expensify" }], section: { symbol_id: 10, path: "docs/handbook.md", name: "Expense process", line: 12 }, code: [], more_code: 0 },
    { ordinal: 2, text: "Your manager approves", role: "Manager", systems: [], section: { symbol_id: 10, path: "docs/handbook.md", name: "Expense process", line: 12 },
      code: [{ symbol_id: 40, path: "src/payroll/expense.ts", name: "approveClaim", line: 3, via: "mention" }, { symbol_id: 41, path: "src/vendors/expensify.ts", name: "postClaim", line: 1, via: { system: "Expensify" } }], more_code: 2 },
  ]},
  { id: 2, name: "Release process", description: "", roles: ["release captain"], steps: [
    { ordinal: 1, text: "Cut the release branch", role: "release captain", systems: [], section: { symbol_id: 11, path: "docs/handbook.md", name: "Release process", line: 30 }, code: [], more_code: 0 },
  ]},
], total: 2, offset: 0, limit: 50 };

// The server, as the view sees it: filters `payload` (or `all`) by `q` and `role`, pages it.
let all: Process[] = payload.processes;
let requests: URLSearchParams[] = [];
beforeEach(() => {
  all = payload.processes;
  requests = [];
  setToken("t");
  globalThis.fetch = mock(async (input: RequestInfo | URL) => {
    const url = new URL(String(input), "http://x");
    if (url.pathname !== "/api/processes") return new Response("not found", { status: 404 });
    const p = url.searchParams;
    requests.push(p);
    const q = (p.get("q") ?? "").toLowerCase();
    const role = p.get("role");
    const offset = Number(p.get("offset") ?? 0), limit = Number(p.get("limit") ?? 50);
    const matching = all.filter((x) => x.name.toLowerCase().includes(q) && (!role || x.roles.includes(role)));
    const body: ProcessesPayload = { processes: matching.slice(offset, offset + limit), total: matching.length, offset, limit };
    return new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
  }) as unknown as typeof fetch;
});
afterEach(() => { mock.restore(); });

const view = (props: Partial<{ changeSeq: number; pending: number; onFocusSection: (p: string, n: string, id?: number) => void }> = {}) =>
  <JourneysView changeSeq={props.changeSeq ?? 0} pending={props.pending ?? 0} onFocusSection={props.onFocusSection ?? (() => {})} />;
const processButtons = () => within(screen.getByRole("list", { name: "Processes" })).getAllByRole("button");

test("lists processes, shows the selected one's steps as an ordered list, and the role filter narrows the list", async () => {
  const user = userEvent.setup();
  const onFocusSection = mock(() => {});
  render(view({ onFocusSection }));
  const list = await screen.findByRole("list", { name: "Processes" });
  expect(within(list).getAllByRole("button")).toHaveLength(2);
  await user.click(within(list).getByRole("button", { name: /Expense process/ }));
  const steps = screen.getByRole("list", { name: "Steps of Expense process" });
  expect(within(steps).getAllByRole("listitem")).toHaveLength(2);
  expect(within(steps).getByText("Manager")).toBeTruthy();
  await user.click(within(steps).getAllByRole("button", { name: /Documented in docs\/handbook\.md::Expense process/ })[0]);
  expect(onFocusSection).toHaveBeenCalledWith("docs/handbook.md", "Expense process", 10);
  await user.click(within(steps).getByRole("button", { name: "Implemented by src/payroll/expense.ts::approveClaim" }));
  expect(onFocusSection).toHaveBeenLastCalledWith("src/payroll/expense.ts", "approveClaim", 40);
  expect(within(steps).getByText("+2 more")).toBeTruthy();
  await user.selectOptions(screen.getByRole("combobox", { name: "Role" }), "release captain");
  await waitFor(() => expect(processButtons()).toHaveLength(1));
  expect(requests.at(-1)!.get("role")).toBe("release captain");
});

// The accessible description, resolved from `aria-describedby` (jest-dom is not wired into the preload).
const description = (el: HTMLElement) =>
  (el.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean).map((id) => document.getElementById(id)?.textContent ?? "").join(" ");

test("each Implemented by button describes how the link was derived", async () => {
  render(view());
  const steps = await screen.findByRole("list", { name: "Steps of Expense process" });
  expect(description(within(steps).getByRole("button", { name: "Implemented by src/payroll/expense.ts::approveClaim" }))).toMatch(/mentioned/);
  expect(description(within(steps).getByRole("button", { name: "Implemented by src/vendors/expensify.ts::postClaim" }))).toMatch(/via Expensify/);
});

test("the journeys view explains an empty corpus", async () => {
  all = [];
  render(view({ pending: 12 }));
  expect(await screen.findByText("No processes extracted yet (12 sections pending).")).toBeTruthy();
  expect((screen.getByRole("combobox", { name: "Role" }) as HTMLSelectElement).disabled).toBe(true);
});

test("the journeys view is axe clean with a process selected, a role filter applied and results shown", async () => {
  const user = userEvent.setup();
  const { container } = render(view());
  // Scoped to the list: the first process is shown by default, so its "Documented in …::Expense process" buttons match too.
  await user.click(within(await screen.findByRole("list", { name: "Processes" })).getByRole("button", { name: /Expense process/ }));
  await user.selectOptions(screen.getByRole("combobox", { name: "Role" }), "Manager");
  await waitFor(() => expect(processButtons()).toHaveLength(1));
  await user.type(screen.getByRole("searchbox", { name: "Find a process" }), "exp");
  await waitFor(() => expect(requests.at(-1)!.get("q")).toBe("exp"));
  expect((await axe.run(container)).violations).toEqual([]);
});

test("the first fetch asks for 50 processes from the start", async () => {
  render(view());
  await screen.findByRole("list", { name: "Processes" });
  expect(requests[0].get("limit")).toBe("50");
  expect(requests[0].get("offset")).toBe("0");
  expect(requests[0].get("q")).toBeNull();
});

test("Find a process narrows the list through the fetch, debounced", async () => {
  const user = userEvent.setup();
  render(view());
  await screen.findByRole("list", { name: "Processes" });
  await user.type(screen.getByRole("searchbox", { name: "Find a process" }), "release");
  await waitFor(() => expect(processButtons()).toHaveLength(1));
  expect(processButtons()[0].textContent).toContain("Release process");
  // One request for the settled text, not one per keystroke.
  expect(requests.filter((r) => r.get("q") !== null).map((r) => r.get("q"))).toEqual(["release"]);
  expect(screen.getByRole("status").textContent).toBe("Showing 1 of 1 processes");
});

test("Show more appends the next page while fewer than the total are shown", async () => {
  const many: Process[] = Array.from({ length: 120 }, (_, i) => ({ id: 100 + i, name: `Process ${String(i).padStart(3, "0")}`, description: "", roles: [], steps: [] }));
  all = many;
  const user = userEvent.setup();
  render(view());
  await waitFor(() => expect(processButtons()).toHaveLength(50));
  expect(screen.getByRole("status").textContent).toBe("Showing 50 of 120 processes");
  await user.click(screen.getByRole("button", { name: "Show more" }));
  await waitFor(() => expect(processButtons()).toHaveLength(100));
  expect(requests.at(-1)!.get("offset")).toBe("50");
  expect(processButtons()[50].textContent).toContain("Process 050");
  await user.click(screen.getByRole("button", { name: "Show more" }));
  await waitFor(() => expect(processButtons()).toHaveLength(120));
  expect(screen.getByRole("status").textContent).toBe("Showing 120 of 120 processes");
  expect(screen.queryByRole("button", { name: "Show more" })).toBeNull();
});

test("a bumped changeSeq refetches, keeping as many processes as were shown", async () => {
  all = Array.from({ length: 70 }, (_, i) => ({ id: 100 + i, name: `Process ${String(i).padStart(3, "0")}`, description: "", roles: [], steps: [] }));
  const user = userEvent.setup();
  const { rerender } = render(view({ changeSeq: 0 }));
  await waitFor(() => expect(processButtons()).toHaveLength(50));
  await user.click(screen.getByRole("button", { name: "Show more" }));
  await waitFor(() => expect(processButtons()).toHaveLength(70));
  const before = requests.length;
  all = [...all, { id: 999, name: "Process 999", description: "", roles: [], steps: [] }];
  await act(async () => { rerender(view({ changeSeq: 1 })); });
  await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Showing 71 of 71 processes"));
  expect(requests.length).toBeGreaterThan(before);
});
