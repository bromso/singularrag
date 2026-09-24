import { expect, mock, test } from "bun:test";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import type { ProcessesPayload } from "../api/types";
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
], truncated: false };

test("lists processes, shows the selected one's steps as an ordered list, and the role filter narrows the list", async () => {
  const user = userEvent.setup();
  const onFocusSection = mock(() => {});
  render(<JourneysView payload={payload} pending={0} onFocusSection={onFocusSection} />);
  const list = screen.getByRole("list", { name: "Processes" });
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
  expect(within(screen.getByRole("list", { name: "Processes" })).getAllByRole("button")).toHaveLength(1);
});

// The accessible description, resolved from `aria-describedby` (jest-dom is not wired into the preload).
const description = (el: HTMLElement) =>
  (el.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean).map((id) => document.getElementById(id)?.textContent ?? "").join(" ");

test("each Implemented by button describes how the link was derived", () => {
  render(<JourneysView payload={payload} pending={0} onFocusSection={() => {}} />);
  const steps = screen.getByRole("list", { name: "Steps of Expense process" });
  expect(description(within(steps).getByRole("button", { name: "Implemented by src/payroll/expense.ts::approveClaim" }))).toMatch(/mentioned/);
  expect(description(within(steps).getByRole("button", { name: "Implemented by src/vendors/expensify.ts::postClaim" }))).toMatch(/via Expensify/);
});

test("the journeys view explains an empty corpus", () => {
  render(<JourneysView payload={{ processes: [], truncated: false }} pending={12} onFocusSection={() => {}} />);
  expect(screen.getByText("No processes extracted yet (12 sections pending).")).toBeTruthy();
  expect((screen.getByRole("combobox", { name: "Role" }) as HTMLSelectElement).disabled).toBe(true);
});

test("the journeys view is axe clean with a process selected and a role filter applied", async () => {
  const user = userEvent.setup();
  const { container } = render(<JourneysView payload={payload} pending={0} onFocusSection={() => {}} />);
  // Scoped to the list: the first process is shown by default, so its "Documented in …::Expense process" buttons match too.
  await user.click(within(screen.getByRole("list", { name: "Processes" })).getByRole("button", { name: /Expense process/ }));
  await user.selectOptions(screen.getByRole("combobox", { name: "Role" }), "Manager");
  expect((await axe.run(container)).violations).toEqual([]);
});
