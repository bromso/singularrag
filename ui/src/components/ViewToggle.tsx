import { useRef } from "react";

export type View = "tree" | "map" | "journeys";
export const VIEW_KEY = "singularrag.view";
export function loadView(): View {
  try { const v = localStorage.getItem(VIEW_KEY); return v === "map" || v === "journeys" ? v : "tree"; } catch { return "tree"; }
}
export function saveView(v: View) { try { localStorage.setItem(VIEW_KEY, v); } catch {} }

export type Overlay = "files" | "entities";
export const OVERLAY_KEY = "singularrag.overlay";
export function loadOverlay(): Overlay {
  try { return localStorage.getItem(OVERLAY_KEY) === "entities" ? "entities" : "files"; } catch { return "files"; }
}
export function saveOverlay(o: Overlay) { try { localStorage.setItem(OVERLAY_KEY, o); } catch {} }

/** An APG radio group of buttons: one tab stop, arrow keys move (and wrap) the choice. */
function RadioToggle<T extends string>({ label, options, value, onChange }: {
  label: string; options: { value: T; label: string }[]; value: T; onChange: (v: T) => void;
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const move = (from: number, delta: number) => {
    const to = (from + delta + options.length) % options.length;
    onChange(options[to].value);
    refs.current[to]?.focus();
  };
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex rounded border">
      {options.map((o, i) => (
        <button key={o.value} ref={(el) => { refs.current[i] = el; }} type="button" role="radio" aria-checked={value === o.value} tabIndex={value === o.value ? 0 : -1}
          className="px-2 py-1 text-sm aria-checked:bg-foreground aria-checked:text-background focus-visible:outline-2 focus-visible:outline-ring"
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

const VIEW_OPTIONS: { value: View; label: string }[] = [{ value: "tree", label: "Tree" }, { value: "map", label: "Map" }, { value: "journeys", label: "Journeys" }];
export function ViewToggle({ value, onChange }: { value: View; onChange: (v: View) => void }) {
  return <RadioToggle label="View" options={VIEW_OPTIONS} value={value} onChange={onChange} />;
}

const OVERLAY_OPTIONS: { value: Overlay; label: string }[] = [{ value: "files", label: "Files" }, { value: "entities", label: "Files + entities" }];
/** Shown in map view only: whether the map also draws the knowledge layer's entities. */
export function OverlayToggle({ value, onChange }: { value: Overlay; onChange: (v: Overlay) => void }) {
  return <RadioToggle label="Overlay" options={OVERLAY_OPTIONS} value={value} onChange={onChange} />;
}
