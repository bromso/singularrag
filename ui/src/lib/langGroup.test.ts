import { describe, expect, test } from "bun:test";
import { langGroup } from "./langGroup";

describe("langGroup", () => {
  test("groups languages", () => {
    expect(langGroup("typescript")).toBe("code");
    expect(langGroup("markdown")).toBe("docs");
    expect(langGroup("text")).toBe("docs");
    expect(langGroup("json")).toBe("config");
    expect(langGroup("css")).toBe("styles");
    expect(langGroup(null)).toBe("code");
  });

  test("schema files form their own group", () => {
    expect(langGroup("prisma")).toBe("schema");
    expect(langGroup("sql")).toBe("schema");
  });
});
