export type ItemStatus = "served" | "cut" | "untouched";

export function statusLabel(s: ItemStatus): string {
  return { served: "Served", cut: "Cut", untouched: "Untouched" }[s];
}

/** Three encodings at once: text, icon shape, colour. Never colour alone (spec §4). */
export function StatusMark({ status }: { status: ItemStatus }) {
  const shape = { served: "filled", cut: "outlined", untouched: "dash" }[status];
  const colour = { served: "text-emerald-700 dark:text-emerald-400", cut: "text-amber-700 dark:text-amber-400", untouched: "text-muted-foreground" }[status];
  return (
    <span className={`status-${status} inline-flex items-center gap-1 ${colour}`}>
      <svg data-shape={shape} aria-hidden="true" width="12" height="12" viewBox="0 0 12 12">
        {shape === "filled" && <circle cx="6" cy="6" r="5" fill="currentColor" />}
        {shape === "outlined" && <circle cx="6" cy="6" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.5" />}
        {shape === "dash" && <rect x="2" y="5" width="8" height="2" fill="currentColor" />}
      </svg>
      <span>{statusLabel(status)}</span>
    </span>
  );
}
