import { beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { ViewToggle, loadView, saveView } from "./ViewToggle";

beforeEach(() => localStorage.clear());

describe("ViewToggle", () => {
  test("is a labelled radiogroup with the current value checked", () => {
    render(<ViewToggle value="tree" onChange={mock()} />);
    const group = screen.getByRole("radiogroup", { name: "View" });
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.textContent)).toEqual(["Tree", "Map"]);
    expect(radios[0].getAttribute("aria-checked")).toBe("true");
    expect(radios[1].getAttribute("tabindex")).toBe("-1");
    expect(group).toBeTruthy();
  });
  test("arrow keys move selection and click selects", async () => {
    const onChange = mock();
    const user = userEvent.setup();
    render(<ViewToggle value="tree" onChange={onChange} />);
    screen.getByRole("radio", { name: "Tree" }).focus();
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenCalledWith("map");
    await user.click(screen.getByRole("radio", { name: "Map" }));
    expect(onChange).toHaveBeenLastCalledWith("map");
  });
  test("persists and reloads, defaulting to tree", () => {
    expect(loadView()).toBe("tree");
    saveView("map");
    expect(loadView()).toBe("map");
    localStorage.setItem("singularrag.view", "bogus");
    expect(loadView()).toBe("tree");
  });
  test("axe clean", async () => {
    const { container } = render(<ViewToggle value="map" onChange={mock()} />);
    expect((await axe.run(container)).violations).toEqual([]);
  });
});
