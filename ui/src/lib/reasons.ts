import type { Reasons } from "@/api/types";

function ordinal(n: number): string {
  const s = ["th", "st", "nd", "rd"], v = n % 100;
  return n + (s[(v - 20) % 10] ?? s[v] ?? s[0]);
}
const base = (p: string) => p.split("/").pop() ?? p;
function list(parts: string[]): string {
  return parts.length <= 1 ? parts.join("") : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

/** Every field of `Reasons` maps to a fixed phrase or nothing; the panel can never show a reason the data does not carry. */
export function reasonsToSentences(r: Reasons, rank: number, score: number): string[] {
  const out = [`Ranked ${ordinal(rank)}, score ${score.toFixed(2)}.`];
  if (r.referenced_by.length) {
    out.push(`Referenced from ${list(r.referenced_by.slice(0, 5).map((x) => `${base(x.path)} (${x.count})`))}.`);
  }
  const q = r.seeds.find((s) => s.startsWith("query:"));
  if (r.query_ident_match && q) out.push(`Matched the query on ${q.slice("query:".length)}.`);
  else if (r.query_ident_match || r.fts_hit) out.push("Matched the query in the index.");
  if (r.note_hit) out.push("Matches a note.");
  if (r.body_hit) out.push("Matches the text of the section.");
  if (r.seeds.includes("focus")) out.push("You focused this file.");
  if (r.pinned || r.seeds.includes("pinned")) out.push("Pinned.");
  return out;
}
