import { useRef } from "react";

export type View = "tree" | "map";
export const VIEW_KEY = "singularrag.view";
export function loadView(): View {
  try { const v = localStorage.getItem(VIEW_KEY); return v === "map" ? "map" : "tree"; } catch { return "tree"; }
}
export function saveView(v: View) { try { localStorage.setItem(VIEW_KEY, v); } catch {} }

const OPTIONS: { value: View; label: string }[] = [{ value: "tree", label: "Tree" }, { value: "map", label: "Map" }];

export function ViewToggle({ value, onChange }: { value: View; onChange: (v: View) => void }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const move = (from: number, delta: number) => {
    const to = (from + delta + OPTIONS.length) % OPTIONS.length;
    onChange(OPTIONS[to].value);
    refs.current[to]?.focus();
  };
  return (
    <div role="radiogroup" aria-label="View" className="inline-flex rounded border">
      {OPTIONS.map((o, i) => (
        <button key={o.value} ref={(el) => { refs.current[i] = el; }} type="button" role="radio" aria-checked={value === o.value} tabIndex={value === o.value ? 0 : -1}
          className="px-2 py-1 text-sm aria-checked:bg-accent focus-visible:outline-2 focus-visible:outline-ring"
          onClick={() => onChange(o.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowRight" || e.key === "ArrowDown") { e.preventDefault(); move(i, 1); }
            if (e.key === "ArrowLeft" || e.key === "ArrowUp") { e.preventDefault(); move(i, -1); }
          }}>
          {o.label}
        </button>
      ))}
    </div>
  );
}
