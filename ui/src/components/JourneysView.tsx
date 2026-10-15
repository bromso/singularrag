import { useEffect, useId, useRef, useState } from "react";
import { api } from "../api/client";
import type { Process, Step } from "../api/types";
import { useDebounced } from "../lib/useDebounced";
import type { FocusSection } from "./EntityView";

const PAGE = 50;

type Loaded = { processes: Process[]; total: number };

/** The processes the knowledge layer extracted, each as an ordered list of steps with their
 *  role, systems, the section that documents them and the code that implements them.
 *  The view owns its fetch: the name field (debounced) and the role filter query the server,
 *  "Show more" appends the next page, and a bumped `changeSeq` (an SSE change event)
 *  refetches as many processes as were shown. */
export function JourneysView({ changeSeq, pending, onFocusSection }: { changeSeq: number; pending: number; onFocusSection: FocusSection }) {
  const [selected, setSelected] = useState<number | null>(null);
  const [role, setRole] = useState("");
  const [text, setText] = useState("");
  const q = useDebounced(text.trim(), 200);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  // Roles seen on unfiltered-by-role pages: the select's options must not shrink to the
  // one role chosen. Only grows; a role no longer used stays selectable and finds nothing.
  const [roles, setRoles] = useState<string[]>([]);
  const [loadingMore, setLoadingMore] = useState(false);
  // Orders fetches by when they started: a superseded fetch applies nothing.
  const gen = useRef(0);
  const shown = useRef(0);
  const lastFilter = useRef<string | null>(null);
  shown.current = loaded?.processes.length ?? 0;

  useEffect(() => {
    const my = ++gen.current;
    const filter = `${q}\u0000${role}`;
    // A new filter starts from the first page; a change event keeps as many as were shown.
    const keep = lastFilter.current === filter ? shown.current : 0;
    lastFilter.current = filter;
    setLoadingMore(false);
    (async () => {
      let got: Process[] = [];
      let total = 0;
      do {
        const p = await api.processes({ q: q || undefined, role: role || undefined, offset: got.length, limit: PAGE });
        if (my !== gen.current) return;
        got = [...got, ...p.processes];
        total = p.total;
        if (p.processes.length === 0) break;
      } while (got.length < Math.min(keep, total));
      setLoaded({ processes: got, total });
      if (!role) setRoles((prev) => mergeRoles(prev, got));
    })().catch(() => {});
  }, [q, role, changeSeq]);

  const showMore = async () => {
    if (!loaded) return;
    const my = gen.current;
    setLoadingMore(true);
    try {
      const p = await api.processes({ q: q || undefined, role: role || undefined, offset: loaded.processes.length, limit: PAGE });
      if (my !== gen.current) return;
      setLoaded((prev) => prev && { processes: [...prev.processes, ...p.processes], total: p.total });
      if (!role) setRoles((prev) => mergeRoles(prev, p.processes));
    } catch {
      // The button stays; the next click retries.
    } finally {
      if (my === gen.current) setLoadingMore(false);
    }
  };

  const processes = loaded?.processes ?? [];
  const current = processes.find((p) => p.id === selected) ?? processes[0] ?? null;
  if (!loaded || (loaded.total === 0 && !q && !role && !text)) {
    return (
      <section aria-label="Journeys" className="p-4">
        <label className="block text-sm">Role <select aria-label="Role" disabled className="ml-2 rounded border bg-background px-2 py-1"><option value="">All roles</option></select></label>
        <p className="mt-4 text-sm">{pending > 0 ? `No processes extracted yet (${pending} sections pending).` : "No processes extracted yet."}</p>
      </section>
    );
  }
  const options = role && !roles.includes(role) ? [...roles, role].sort() : roles;
  return (
    <section aria-label="Journeys" className="grid h-full min-h-0 grid-cols-[minmax(12rem,1fr)_3fr] gap-4 p-4">
      <div className="min-h-0 overflow-auto">
        <label className="block text-sm">Find a process
          <input type="search" className="mt-1 block w-full rounded border bg-background px-2 py-1" value={text} onChange={(e) => setText(e.target.value)} />
        </label>
        <label className="mt-2 block text-sm">Role
          <select aria-label="Role" className="ml-2 rounded border bg-background px-2 py-1" value={role} onChange={(e) => setRole(e.target.value)}>
            <option value="">All roles</option>
            {options.map((r) => <option key={r} value={r}>{r}</option>)}
          </select>
        </label>
        <p role="status" className="mt-2 text-xs">{`Showing ${processes.length} of ${loaded.total} processes`}</p>
        {processes.length === 0 ? <p className="mt-3 text-sm">No processes match.</p> : (
          <ul aria-label="Processes" className="mt-3 space-y-1">
            {processes.map((p) => (
              <li key={p.id}>
                <button type="button" aria-pressed={current?.id === p.id} onClick={() => setSelected(p.id)}
                  className="w-full rounded border border-transparent px-2 py-1 text-left text-sm aria-pressed:border-foreground aria-pressed:bg-foreground aria-pressed:text-background focus-visible:outline-2 focus-visible:outline-ring">
                  <span className="font-medium">{p.name}</span>
                  <span className="block text-xs opacity-80">{p.steps.length} {p.steps.length === 1 ? "step" : "steps"}{p.roles.length ? ` · ${p.roles.join(", ")}` : ""}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
        {processes.length < loaded.total && (
          <button type="button" disabled={loadingMore} onClick={() => { void showMore(); }}
            className="mt-2 rounded border px-2 py-1 text-sm focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-60">Show more</button>
        )}
      </div>
      <div className="min-h-0 overflow-auto">
        {current && <ProcessSteps process={current} onFocusSection={onFocusSection} />}
      </div>
    </section>
  );
}

function mergeRoles(prev: string[], ps: Process[]): string[] {
  const next = new Set(prev);
  for (const p of ps) for (const r of p.roles) next.add(r);
  return next.size === prev.length ? prev : Array.from(next).sort();
}

function ProcessSteps({ process, onFocusSection }: { process: Process; onFocusSection: FocusSection }) {
  return (
    <>
      <h2 className="text-base font-semibold">{process.name}</h2>
      {process.description && <p className="text-sm">{process.description}</p>}
      <ol aria-label={`Steps of ${process.name}`} className="mt-3 list-decimal space-y-3 pl-6">
        {process.steps.map((s) => <StepItem key={s.ordinal} step={s} onFocusSection={onFocusSection} />)}
      </ol>
    </>
  );
}

function StepItem({ step, onFocusSection }: { step: Step; onFocusSection: FocusSection }) {
  const sec = step.section;
  // The accessible name is only the target, so the visible provenance ("mentioned" / "via X")
  // is the button's description; `useId` keeps the id unique per step, per view instance.
  const uid = useId();
  return (
    <li>
      <p>{step.text}</p>
      <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-3 text-xs">
        {step.role && <><dt>Role</dt><dd>{step.role}</dd></>}
        {step.systems.length > 0 && <><dt>Systems</dt><dd>{step.systems.map((x) => <span key={x.id} className="mr-2 rounded border px-1">{x.name} <span className="opacity-70">(system)</span></span>)}</dd></>}
        <dt>Documented in</dt>
        <dd><button type="button" className="underline focus-visible:outline-2 focus-visible:outline-ring" aria-label={`Documented in ${sec.path}::${sec.name}`} onClick={() => onFocusSection(sec.path, sec.name, sec.symbol_id)}>{sec.path}::{sec.name}</button></dd>
        {(step.code.length > 0 || step.more_code > 0) && <>
          <dt>Implemented by</dt>
          <dd>
            {step.code.map((c) => (
              <button key={c.symbol_id} type="button" className="mr-2 underline focus-visible:outline-2 focus-visible:outline-ring" aria-label={`Implemented by ${c.path}::${c.name}`} aria-describedby={`${uid}-via-${c.symbol_id}`} onClick={() => onFocusSection(c.path, c.name, c.symbol_id)}>
                {c.path}::{c.name} <span id={`${uid}-via-${c.symbol_id}`} className="opacity-70">({typeof c.via === "string" ? "mentioned" : `via ${c.via.system}`})</span>
              </button>
            ))}
            {step.more_code > 0 && <span>+{step.more_code} more</span>}
          </dd>
        </>}
      </dl>
    </li>
  );
}
