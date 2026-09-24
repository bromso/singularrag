import { useMemo, useState } from "react";
import type { Process, ProcessesPayload, Step } from "../api/types";
import type { FocusSection } from "./EntityView";

/** The processes the knowledge layer extracted, each as an ordered list of steps with their
 *  role, systems, the section that documents them and the code that implements them. */
export function JourneysView({ payload, pending, onFocusSection }: { payload: ProcessesPayload | null; pending: number; onFocusSection: FocusSection }) {
  const [selected, setSelected] = useState<number | null>(null);
  const [role, setRole] = useState("");
  const processes = payload?.processes ?? [];
  const roles = useMemo(() => Array.from(new Set(processes.flatMap((p) => p.roles))).sort(), [processes]);
  const shown = role ? processes.filter((p) => p.roles.includes(role)) : processes;
  const current = shown.find((p) => p.id === selected) ?? shown[0] ?? null;
  if (processes.length === 0) {
    return (
      <section aria-label="Journeys" className="p-4">
        <label className="block text-sm">Role <select aria-label="Role" disabled className="ml-2 rounded border bg-background px-2 py-1"><option value="">All roles</option></select></label>
        <p className="mt-4 text-sm">{pending > 0 ? `No processes extracted yet (${pending} sections pending).` : "No processes extracted yet."}</p>
      </section>
    );
  }
  return (
    <section aria-label="Journeys" className="grid h-full min-h-0 grid-cols-[minmax(12rem,1fr)_3fr] gap-4 p-4">
      <div className="min-h-0 overflow-auto">
        <label className="block text-sm">Role
          <select aria-label="Role" className="ml-2 rounded border bg-background px-2 py-1" value={role} onChange={(e) => setRole(e.target.value)}>
            <option value="">All roles</option>
            {roles.map((r) => <option key={r} value={r}>{r}</option>)}
          </select>
        </label>
        <ul aria-label="Processes" className="mt-3 space-y-1">
          {shown.map((p) => (
            <li key={p.id}>
              <button type="button" aria-pressed={current?.id === p.id} onClick={() => setSelected(p.id)}
                className="w-full rounded border border-transparent px-2 py-1 text-left text-sm aria-pressed:border-foreground aria-pressed:bg-foreground aria-pressed:text-background focus-visible:outline-2 focus-visible:outline-ring">
                <span className="font-medium">{p.name}</span>
                <span className="block text-xs opacity-80">{p.steps.length} {p.steps.length === 1 ? "step" : "steps"}{p.roles.length ? ` · ${p.roles.join(", ")}` : ""}</span>
              </button>
            </li>
          ))}
        </ul>
      </div>
      <div className="min-h-0 overflow-auto">
        {current && <ProcessSteps process={current} onFocusSection={onFocusSection} />}
      </div>
    </section>
  );
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
              <button key={c.symbol_id} type="button" className="mr-2 underline focus-visible:outline-2 focus-visible:outline-ring" aria-label={`Implemented by ${c.path}::${c.name}`} onClick={() => onFocusSection(c.path, c.name, c.symbol_id)}>
                {c.path}::{c.name} <span className="opacity-70">({typeof c.via === "string" ? "mentioned" : `via ${c.via.system}`})</span>
              </button>
            ))}
            {step.more_code > 0 && <span>+{step.more_code} more</span>}
          </dd>
        </>}
      </dl>
    </li>
  );
}
