import { describe, expect, test } from "bun:test";
import { render } from "@testing-library/react";
import { StatusMark, statusLabel } from "./status";

describe("StatusMark", () => {
  test("encodes status as text, icon and colour", () => {
    for (const s of ["served", "cut", "untouched"] as const) {
      const { container, unmount } = render(<StatusMark status={s} />);
      const el = container.firstElementChild!;
      expect(el.textContent).toContain(statusLabel(s));
      expect(el.querySelector("svg")!.getAttribute("data-shape")).toBe({ served: "filled", cut: "outlined", untouched: "dash" }[s]);
      expect(el.className).toContain(`status-${s}`);
      unmount();
    }
  });

  test("carries a light and a dark colour for the coloured states", () => {
    // The `dark:` variant follows `prefers-color-scheme` (index.css), so both halves of
    // each pair must be on the element. The contrast figures behind these choices
    // (≥ 4.5:1 on the respective backgrounds) cannot be computed under happy-dom, so
    // this asserts the pairing and the ratios were checked by hand.
    const { container: served } = render(<StatusMark status="served" />);
    expect(served.firstElementChild!.className).toContain("text-emerald-700");
    expect(served.firstElementChild!.className).toContain("dark:text-emerald-400");
    const { container: cut } = render(<StatusMark status="cut" />);
    expect(cut.firstElementChild!.className).toContain("text-amber-700");
    expect(cut.firstElementChild!.className).toContain("dark:text-amber-400");
  });
});
