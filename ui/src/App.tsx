import { useCallback, useEffect, useMemo, useState } from "react";
import { Toaster, toast } from "sonner";
import { api, setToken, tokenFromFragment } from "@/api/client";
import { subscribe } from "@/api/events";
import type { MapConfig, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "@/api/types";
import { joinRetrieval } from "@/lib/join";
import { setNote, toggleExclude, togglePin } from "@/lib/mapEdits";
import { DetailPanel } from "@/components/DetailPanel";
import { FreshnessBadge, freshnessText } from "@/components/FreshnessBadge";
import { LiveRegion } from "@/components/LiveRegion";
import { RepoTree, type TreeRow } from "@/components/RepoTree";
import { RetrievalsRail } from "@/components/RetrievalsRail";
import { SkippedSheet } from "@/components/SkippedSheet";

const emptyMap: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };

export function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [retrievals, setRetrievals] = useState<RetrievalSummary[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [detail, setDetail] = useState<RetrievalDetail | null>(null);
  const [tree, setTree] = useState<TreeFile[]>([]);
  const [skipped, setSkipped] = useState<SkippedFile[]>([]);
  const [map, setMap] = useState<MapConfig>(emptyMap);
  const [focused, setFocused] = useState<TreeRow | null>(null);
  const [filter, setFilter] = useState("");
  const [messages, setMessages] = useState<string[]>([]);
  const announce = useCallback((m: string) => setMessages((ms) => [...ms.slice(-9), m]), []);

  useEffect(() => {
    setToken(tokenFromFragment());
    let knownMax = 0;
    const load = async () => {
      const [s, rs, t, sk, m] = await Promise.all([api.status(), api.retrievals(), api.tree(), api.skipped(), api.map()]);
      setStatus(s); setRetrievals(rs); setTree(t); setSkipped(sk); setMap(m);
      for (const r of rs.filter((r) => r.id > knownMax).reverse()) {
        announce(`New retrieval from ${r.session_label}: ${r.tool}, ${r.served} served, ${r.cut} cut, ${r.stale_count ? `${r.stale_count} stale` : "fresh"}`);
      }
      knownMax = Math.max(knownMax, ...rs.map((r) => r.id));
    };
    load().catch((e) => toast.error(String(e.message ?? e)));
    return subscribe(
      () => { load().catch(() => {}); },
      (s) => { setStatus(s); announce(`Index ${freshnessText(s)}`); },
    );
  }, [announce]);

  useEffect(() => {
    if (selected == null) { setDetail(null); return; }
    api.retrieval(selected).then(setDetail).catch((e) => toast.error(String(e.message ?? e)));
  }, [selected]);

  const rows = useMemo(() => joinRetrieval(tree, detail?.items ?? null), [tree, detail]);

  const save = async (next: MapConfig) => {
    try {
      const saved = await api.saveMap(next);
      setMap(saved);
      toast.success("Saved. Applies to the next retrieval.");
    } catch (e: any) {
      toast.error(e.field ? `${e.field}: ${e.message}` : String(e.message ?? e));
    }
  };

  return (
    <div className="grid h-screen grid-cols-[18rem_1fr_22rem] grid-rows-[auto_1fr]">
      <header className="col-span-3 flex items-center gap-3 border-b px-3 py-2">
        <h1 className="text-base font-semibold">singularrag</h1>
        <FreshnessBadge status={status} />
        <SkippedSheet skipped={skipped} />
        <label className="ml-auto text-sm">
          Filter
          <input type="search" value={filter} onChange={(e) => setFilter(e.target.value)} className="ml-2 rounded border bg-background px-2 py-1" placeholder="path or symbol" />
        </label>
      </header>
      <RetrievalsRail retrievals={retrievals} selected={selected} onSelect={setSelected}
        onMore={() => api.retrievals(retrievals[retrievals.length - 1]?.id).then((more) => setRetrievals((rs) => [...rs, ...more]))} />
      <main className="overflow-auto">
        <RepoTree rows={rows} filter={filter} onFocusRow={setFocused} onAction={setFocused} />
      </main>
      <DetailPanel row={focused} map={map}
        onPin={(p, s) => save(togglePin(map, p, s))}
        onExclude={(p) => save(toggleExclude(map, p))}
        onNote={(p, s, t) => { if (t !== (map.note.find((n) => n.path === p && (n.symbol ?? undefined) === s)?.text ?? "")) save(setNote(map, p, s, t)); }} />
      <LiveRegion messages={messages} />
      <Toaster />
    </div>
  );
}
