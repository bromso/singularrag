import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { ApiError, api, setToken, tokenFromFragment } from "@/api/client";
import { subscribe } from "@/api/events";
import type { MapConfig, MapDoc, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "@/api/types";
import { joinRetrieval } from "@/lib/join";
import { noteFor, setNote, toggleExclude, togglePin } from "@/lib/mapEdits";
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

  // The map is edited through a compare-and-swap, so a save must build on the newest
  // config the client has seen, not on whatever `map` the clicked render closed over.
  // `mapRef`/`versionRef` hold that newest document; `saveQueue` serialises writes so
  // two rapid clicks apply in order, the second on the first's response.
  const mapRef = useRef<MapConfig>(emptyMap);
  const versionRef = useRef(0);
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const applyMapDoc = useCallback((doc: MapDoc) => {
    const { version, ...cfg } = doc;
    mapRef.current = cfg;
    versionRef.current = version;
    setMap(cfg);
  }, []);

  useEffect(() => {
    setToken(tokenFromFragment());
    // `null` until the first load: history is not news, so the initial page-load
    // announces one summary instead of reading out up to ten past retrievals (I5).
    let knownMax: number | null = null;
    const load = async () => {
      const [s, rs, t, sk, m] = await Promise.all([api.status(), api.retrievals(), api.tree(), api.skipped(), api.map()]);
      setStatus(s); setRetrievals(rs); setTree(t); setSkipped(sk); applyMapDoc(m);
      const seen = knownMax;
      if (seen === null) {
        announce(`Loaded ${rs.length} retrievals`);
      } else {
        for (const r of rs.filter((r) => r.id > seen).reverse()) {
          announce(`New retrieval from ${r.session_label}: ${r.tool}, ${r.served} served, ${r.cut} cut, ${r.stale_count ? `${r.stale_count} stale` : "fresh"}`);
        }
      }
      knownMax = Math.max(seen ?? 0, ...rs.map((r) => r.id));
    };
    load().catch((e) => toast.error(String(e.message ?? e)));
    return subscribe(
      () => { load().catch(() => {}); },
      (s) => { setStatus(s); announce(`Index ${freshnessText(s)}`); },
    );
  }, [announce, applyMapDoc]);

  useEffect(() => {
    if (selected == null) { setDetail(null); return; }
    api.retrieval(selected).then(setDetail).catch((e) => toast.error(String(e.message ?? e)));
  }, [selected]);

  const rows = useMemo(() => joinRetrieval(tree, detail?.items ?? null), [tree, detail]);

  // A row's action button only selected the row, which row focus had already done, so it
  // did nothing a screen-reader user could notice (I10). Move DOM focus to the panel it
  // controls; the rAF lets React commit the panel's new heading first.
  const openDetail = useCallback((row: TreeRow) => {
    setFocused(row);
    requestAnimationFrame(() => document.getElementById("detail-heading")?.focus());
  }, []);

  // `edit` is applied when the save's turn comes, to the config as it is then — not at
  // click time — so the second of two rapid clicks does not discard the first.
  const save = (edit: (cfg: MapConfig) => MapConfig) => {
    saveQueue.current = saveQueue.current.then(async () => {
      try {
        applyMapDoc(await api.saveMap(edit(mapRef.current), versionRef.current));
        toast.success("Saved. Applies to the next retrieval.");
      } catch (e: unknown) {
        if (e instanceof ApiError && e.status === 409 && e.current) {
          applyMapDoc(e.current as MapDoc);
          toast.error("map.toml changed on disk; reloaded, please redo that change");
        } else if (e instanceof ApiError) {
          toast.error(e.field ? `${e.field}: ${e.message}` : e.message);
        } else {
          toast.error(String(e));
        }
      }
    });
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
        <RepoTree rows={rows} filter={filter} seedKey={detail?.id ?? 0} onFocusRow={setFocused} onAction={openDetail} />
      </main>
      <DetailPanel row={focused} map={map}
        onPin={(p, s) => save((c) => togglePin(c, p, s))}
        onExclude={(p) => save((c) => toggleExclude(c, p))}
        onNote={(p, s, t) => { if (t !== noteFor(map, p, s)) save((c) => setNote(c, p, s, t)); }} />
      <LiveRegion messages={messages} />
      <Toaster />
    </div>
  );
}
