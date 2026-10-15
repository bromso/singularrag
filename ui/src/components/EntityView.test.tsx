import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { setToken } from "../api/client";
import type { EntitiesPayload, Entity, EntityDetail } from "../api/types";
import { EntityList, EntityView } from "./EntityView";

const okta: Entity = { id: 1, name: "Okta", type: "system", description: "Single sign-on.", mentions: 2 };
const desk: Entity = { id: 2, name: "IT desk", type: "organisation", description: "", mentions: 1 };
const onboarding: Entity = { id: 3, name: "Onboarding", type: "process", description: "Your first week.", mentions: 1 };
// The overlay knows Okta and the desk but none of Okta's mentions: the detail must supply them.
const overlay: EntitiesPayload = { entities: [okta, desk], relations: [], mentions: [], truncated: true };
const oktaDetail: EntityDetail = {
  entity: okta,
  relations: [{ id: 5, src: 2, dst: 1, description: "administers", symbol_id: 20, path: "docs/it.md", name: "Desk" }],
  mentions: [{ entity_id: 1, symbol_id: 20, path: "docs/it.md", name: "Desk" }, { entity_id: 1, symbol_id: 21, path: "docs/handbook.md", name: "Onboarding" }],
  steps: null,
};
const onboardingDetail: EntityDetail = {
  entity: onboarding, relations: [], mentions: [],
  steps: [{ ordinal: 1, text: "Activate your Okta account", role: "Employee", systems: [{ id: 1, name: "Okta" }], section: { symbol_id: 21, path: "docs/handbook.md", name: "Onboarding", line: 3 }, code: [], more_code: 0 }],
};
const details: Record<number, EntityDetail> = { 1: oktaDetail, 3: onboardingDetail };
let urls: string[] = [];
let failDetail = false;

beforeEach(() => {
  urls = [];
  failDetail = false;
  setToken("t");
  globalThis.fetch = mock(async (input: RequestInfo | URL) => {
    const url = new URL(String(input), "http://x");
    urls.push(url.pathname + url.search);
    const json = (b: unknown) => new Response(JSON.stringify(b), { headers: { "Content-Type": "application/json" } });
    if (url.pathname === "/api/entities/search") {
      const q = (url.searchParams.get("q") ?? "").toLowerCase();
      return json({ entities: [okta, desk, onboarding].filter((e) => e.name.toLowerCase().startsWith(q)) });
    }
    const m = /^\/api\/entities\/(\d+)$/.exec(url.pathname);
    if (m && !failDetail && details[Number(m[1])]) return json(details[Number(m[1])]);
    return new Response(JSON.stringify({ error: "no such entity" }), { status: 404 });
  }) as unknown as typeof fetch;
});
afterEach(() => { mock.restore(); });

test("the entity view fetches the detail and renders its mentions and relations", async () => {
  const { container } = render(<EntityView entity={okta} entities={overlay} onFocusSection={() => {}} />);
  expect(screen.getAllByText("Loading…").length).toBeGreaterThan(0);
  const mentions = await screen.findByRole("list", { name: "Mentioned in" });
  expect(within(mentions).getAllByRole("listitem")).toHaveLength(2);
  expect(urls).toContain("/api/entities/1");
  const relations = screen.getByRole("list", { name: "Relations" });
  expect(relations.textContent).toBe("IT desk → Okta: administers");
  expect((await axe.run(container)).violations).toEqual([]);
});

test("a process entity lists its steps from the detail", async () => {
  render(<EntityView entity={onboarding} entities={overlay} onFocusSection={() => {}} />);
  const steps = await screen.findByRole("list", { name: "Steps of Onboarding" });
  expect(within(steps).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Activate your Okta account (Employee)"]);
});

test("a failed detail falls back to the overlay's data", async () => {
  failDetail = true;
  const withMention: EntitiesPayload = { ...overlay, mentions: [{ entity_id: 1, symbol_id: 20, path: "docs/it.md", name: "Desk" }] };
  render(<EntityView entity={okta} entities={withMention} onFocusSection={() => {}} />);
  const mentions = await screen.findByRole("list", { name: "Mentioned in" });
  expect(within(mentions).getAllByRole("listitem")).toHaveLength(1);
  expect(screen.queryByText("Loading…")).toBeNull();
});

test("the entity list shows the overlay's top entities, and Find an entity lists search results", async () => {
  const user = userEvent.setup();
  const onFocusEntity = mock((_id: number, _e?: Entity) => {});
  const { container } = render(<EntityList entities={overlay} onFocusEntity={onFocusEntity} />);
  const list = screen.getByRole("list", { name: "Entities" });
  expect(within(list).getAllByRole("button").map((b) => b.textContent)).toEqual(["Okta (system), 2 mentions", "IT desk (organisation), 1 mention"]);
  await user.type(screen.getByRole("searchbox", { name: "Find an entity" }), "onb");
  await waitFor(() => expect(within(screen.getByRole("list", { name: "Entities" })).getAllByRole("button").map((b) => b.textContent)).toEqual(["Onboarding (process), 1 mention"]));
  expect(urls.filter((u) => u.startsWith("/api/entities/search"))).toEqual(["/api/entities/search?q=onb"]);
  await user.click(screen.getByRole("button", { name: /^Onboarding/ }));
  expect(onFocusEntity).toHaveBeenCalledWith(3, onboarding);
  expect((await axe.run(container)).violations).toEqual([]);
  await user.clear(screen.getByRole("searchbox", { name: "Find an entity" }));
  await waitFor(() => expect(within(screen.getByRole("list", { name: "Entities" })).getAllByRole("button")).toHaveLength(2));
});
