import { useState } from "react";
import { api } from "@/api/client";

export function QueryPanel({ onDone, announce }: { onDone: (id: number) => void; announce: (m: string) => void }) {
  const [query, setQuery] = useState("");
  const [budget, setBudget] = useState("1024");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const r = await api.query(query, Number(budget));
      announce(`${r.served} served, ${r.cut} cut`);
      onDone(r.retrieval_id);
    } catch (err) {
      const m = err instanceof Error ? err.message : String(err);
      setError(m);
      announce(`Query failed: ${m}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={submit} aria-label="Run a query" className="flex flex-col gap-2 border-b px-3 py-2">
      <label className="text-sm">Query
        <input type="text" value={query} onChange={(e) => setQuery(e.target.value)} className="mt-1 w-full rounded border bg-background px-2 py-1" />
      </label>
      <div className="flex items-end gap-2">
        <label className="text-sm">Budget
          <select value={budget} onChange={(e) => setBudget(e.target.value)} className="ml-2 rounded border bg-background px-2 py-1">
            <option value="1024">1024</option><option value="2048">2048</option><option value="4096">4096</option>
          </select>
        </label>
        <button type="submit" disabled={busy} className="rounded border px-3 py-1 text-sm">Run query</button>
      </div>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    </form>
  );
}
