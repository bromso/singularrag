import { describe, expect, test } from "bun:test";
import { toolLabel } from "./toolLabel";

describe("toolLabel", () => {
  test("maps the four tool names to their labels", () => {
    expect(toolLabel("repo_map")).toBe("Map");
    expect(toolLabel("find_symbol")).toBe("Find");
    expect(toolLabel("trace_path")).toBe("Trace");
    expect(toolLabel("changed")).toBe("Changed");
  });

  test("passes an unknown name through", () => {
    expect(toolLabel("annotate")).toBe("annotate");
  });
});
