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
});
