import { describe, expect, test } from "bun:test";
import { reasonsToSentences } from "./reasons";

const base = { score: 0.11, file_rank: 0.2, seeds: [], referenced_by: [], pinned: false, fts_hit: false, query_ident_match: false, note_hit: false };

describe("reasonsToSentences", () => {
  test("rank and score always come first", () => {
    expect(reasonsToSentences(base, 3, 0.11)[0]).toBe("Ranked 3rd, score 0.11.");
    expect(reasonsToSentences(base, 1, 0.5)[0]).toBe("Ranked 1st, score 0.50.");
    expect(reasonsToSentences(base, 22, 0.001)[0]).toBe("Ranked 22nd, score 0.00.");
  });
  test("references list up to five files with counts", () => {
    const s = reasonsToSentences({ ...base, referenced_by: [{ path: "src/http/middleware.ts", count: 2 }, { path: "src/cli/login.ts", count: 1 }] }, 1, 0.1);
    expect(s).toContain("Referenced from middleware.ts (2) and login.ts (1).");
  });
  test("query match sentences", () => {
    expect(reasonsToSentences({ ...base, query_ident_match: true, seeds: ["query:session store"] }, 1, 0.1)).toContain("Matched the query on session store.");
    expect(reasonsToSentences({ ...base, fts_hit: true }, 1, 0.1)).toContain("Matched the query in the index.");
  });
  test("focus and pinned", () => {
    const s = reasonsToSentences({ ...base, seeds: ["focus", "pinned"], pinned: true }, 1, 0.1);
    expect(s).toContain("You focused this file.");
    expect(s).toContain("Pinned.");
  });
  test("nothing extra when nothing applies", () => {
    expect(reasonsToSentences(base, 4, 0.02)).toEqual(["Ranked 4th, score 0.02."]);
  });
});
