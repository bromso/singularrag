export function summaryLabel(files: number, retrieval: { id: number; served: number; cut: number } | null, boundaries: number): string {
  const b = `${boundaries} ${boundaries === 1 ? "boundary" : "boundaries"}`;
  if (!retrieval) return `Map of ${files} files. No retrieval selected. ${b}.`;
  const untouched = Math.max(0, files - retrieval.served - retrieval.cut);
  return `Map of ${files} files. Retrieval ${retrieval.id}: ${retrieval.served} served, ${retrieval.cut} cut, ${untouched} untouched. ${b}.`;
}
