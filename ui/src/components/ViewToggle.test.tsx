import { beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { OverlayToggle, ViewToggle, loadOverlay, loadView, saveOverlay, saveView } from "./ViewToggle";

beforeEach(() => localStorage.clear());

describe("ViewToggle", () => {
  test("is a labelled radiogroup with the current value checked", () => {
    render(<ViewToggle value="tree" onChange={mock()} />);
    const group = screen.getByRole("radiogroup", { name: "View" });
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.textContent)).toEqual(["Tree", "Map", "Journeys"]);
    expect(radios[0].getAttribute("aria-checked")).toBe("true");
    expect(radios[1].getAttribute("tabindex")).toBe("-1");
    expect(group).toBeTruthy();
  });
  test("the checked option has a contrasting fill, not just an accent tint", () => {
    render(<ViewToggle value="tree" onChange={mock()} />);
    const [checked] = screen.getAllByRole("radio");
    expect(checked.className).toContain("aria-checked:bg-foreground");
    expect(checked.className).toContain("aria-checked:text-background");
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
    saveView("journeys");
    expect(loadView()).toBe("journeys");
    localStorage.setItem("singularrag.view", "bogus");
    expect(loadView()).toBe("tree");
  });
  test("axe clean", async () => {
    const { container } = render(<ViewToggle value="map" onChange={mock()} />);
    expect((await axe.run(container)).violations).toEqual([]);
  });
});

describe("OverlayToggle", () => {
  test("is a radiogroup labelled Overlay with Files and Files + entities", () => {
    render(<OverlayToggle value="files" onChange={mock()} />);
    const group = screen.getByRole("radiogroup", { name: "Overlay" });
    const radios = Array.from(group.querySelectorAll('[role="radio"]'));
    expect(radios.map((r) => r.textContent)).toEqual(["Files", "Files + entities"]);
    expect(radios[0].getAttribute("aria-checked")).toBe("true");
    expect(radios[1].getAttribute("tabindex")).toBe("-1");
  });
  test("arrow keys move selection and wrap", async () => {
    const onChange = mock();
    const user = userEvent.setup();
    render(<OverlayToggle value="files" onChange={onChange} />);
    screen.getByRole("radio", { name: "Files" }).focus();
    await user.keyboard("{ArrowDown}");
    expect(onChange).toHaveBeenLastCalledWith("entities");
    expect(document.activeElement?.textContent).toBe("Files + entities");
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenLastCalledWith("files");
  });
  test("persists and reloads, defaulting to files", () => {
    expect(loadOverlay()).toBe("files");
    saveOverlay("entities");
    expect(loadOverlay()).toBe("entities");
    localStorage.setItem("singularrag.overlay", "bogus");
    expect(loadOverlay()).toBe("files");
  });
  test("axe clean", async () => {
    const { container } = render(<OverlayToggle value="entities" onChange={mock()} />);
    expect((await axe.run(container)).violations).toEqual([]);
  });
});
