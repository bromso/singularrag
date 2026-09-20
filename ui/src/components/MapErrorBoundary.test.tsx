import { afterEach, beforeEach, describe, expect, mock, spyOn, test } from "bun:test";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MapErrorBoundary } from "./MapErrorBoundary";

function Bomb(): never {
  throw new Error("boom");
}

// React logs a caught render error to the console even when an error boundary handles
// it. That's expected here (the error genuinely happened), so silence it for this file
// rather than let it clutter otherwise-pristine test output.
let errorSpy: ReturnType<typeof spyOn>;
beforeEach(() => { errorSpy = spyOn(console, "error").mockImplementation(() => {}); });
afterEach(() => { errorSpy.mockRestore(); });

describe("MapErrorBoundary", () => {
  test("catches a render error, shows the alert, and its button switches to the table", async () => {
    const onSwitchToTable = mock();
    render(
      <MapErrorBoundary onSwitchToTable={onSwitchToTable}>
        <Bomb />
      </MapErrorBoundary>,
    );
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("The map could not be drawn.");
    const btn = screen.getByRole("button", { name: "Switch to table" });
    await userEvent.setup().click(btn);
    expect(onSwitchToTable).toHaveBeenCalled();
  });

  test("renders children normally when there is no error", () => {
    render(
      <MapErrorBoundary onSwitchToTable={mock()}>
        <div>fine</div>
      </MapErrorBoundary>,
    );
    expect(screen.getByText("fine")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
