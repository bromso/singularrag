/** `entities` is given only while the entity overlay is on; it joins the file count. */
export function summaryLabel(files: number, retrieval: { id: number; served: number; cut: number } | null, boundaries: number, entities?: number): string {
  const b = `${boundaries} ${boundaries === 1 ? "boundary" : "boundaries"}`;
  const f = `${files} ${files === 1 ? "file" : "files"}${entities === undefined ? "" : ` and ${entities} ${entities === 1 ? "entity" : "entities"}`}`;
  if (!retrieval) return `Map of ${f}. No retrieval selected. ${b}.`;
  const untouched = Math.max(0, files - retrieval.served - retrieval.cut);
  return `Map of ${f}. Retrieval ${retrieval.id}: ${retrieval.served} served, ${retrieval.cut} cut, ${untouched} untouched. ${b}.`;
}
