import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { ApiError, api, setToken, tokenFromFragment } from "@/api/client";
import { subscribe } from "@/api/events";
import type { BlastResult, GraphPayload, MapConfig, MapDoc, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "@/api/types";
import { joinRetrieval } from "@/lib/join";
import { addToBoundary, noteFor, removeFromBoundary, setNote, toggleExclude, togglePin } from "@/lib/mapEdits";
import { summaryLabel } from "@/lib/mapSummary";
import { DetailPanel } from "@/components/DetailPanel";
import { FreshnessBadge, freshnessText } from "@/components/FreshnessBadge";
import { LiveRegion } from "@/components/LiveRegion";
import { MapView } from "@/components/MapView";
import { RepoTree, type TreeRow } from "@/components/RepoTree";
import { RetrievalsRail } from "@/components/RetrievalsRail";
import { SkippedSheet } from "@/components/SkippedSheet";
import { ViewToggle, loadView, saveView, type View } from "@/components/ViewToggle";

const emptyMap: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };

/** A caught value is `unknown`; only an `ApiError` is known to carry `field`/`message`. */
function toastError(e: unknown) {
  if (e instanceof ApiError) toast.error(e.field ? `${e.field}: ${e.message}` : e.message);
  else toast.error(e instanceof Error ? e.message : String(e));
}

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
  const [blast, setBlast] = useState<BlastResult | null>(null);
  const [blastLoading, setBlastLoading] = useState(false);
  const [expandedPath, setExpandedPath] = useState<string | null>(null);
  const [messages, setMessages] = useState<string[]>([]);
  const announce = useCallback((m: string) => setMessages((ms) => [...ms.slice(-9), m]), []);
  const [view, setView] = useState<View>(loadView);
  const changeView = (v: View) => { setView(v); saveView(v); };
  const [graph, setGraph] = useState<GraphPayload | null>(null);
  const graphRef = useRef<string | null>(null);
  const announcedLayouts = useRef(new Set<string>());
  const onLayoutReady = useCallback((v: string) => {
    if (!announcedLayouts.current.has(v)) { announcedLayouts.current.add(v); announce("Map layout ready"); }
  }, [announce]);

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
      if (!graphRef.current || graphRef.current !== s.index_version) {
        const g = await api.graph();
        graphRef.current = g.index_version;
        setGraph(g);
      }
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
    load().catch((e: unknown) => toastError(e));
    return subscribe(
      () => { load().catch(() => {}); },
      (s) => { setStatus(s); announce(`Index ${freshnessText(s)}`); },
    );
  }, [announce, applyMapDoc]);

  useEffect(() => {
    if (selected == null) { setDetail(null); return; }
    api.retrieval(selected).then(setDetail).catch((e: unknown) => toastError(e));
  }, [selected]);

  const rows = useMemo(() => joinRetrieval(tree, detail?.items ?? null), [tree, detail]);

  // A row's action button only selected the row, which row focus had already done, so it
  // did nothing a screen-reader user could notice (I10). Move DOM focus to the panel it
  // controls; the rAF lets React commit the panel's new heading first.
  const openDetail = useCallback((row: TreeRow) => {
    setFocused(row);
    requestAnimationFrame(() => document.getElementById("detail-heading")?.focus());
  }, []);

  // Blast radius is scoped to the focused symbol; switching to a different row (or a
  // file row) invalidates whatever was shown for the previous one. `blastReq` is a
  // request-generation counter: a response only applies its state if the generation
  // it was issued under is still current, so a stale in-flight `api.blast()` (the
  // focus moved, or a newer request for a different target was issued) is dropped
  // instead of clobbering `blast`/`blastLoading`/the announcement for whatever is
  // focused now. `pendingBlastRef` records which (path, symbol) the current
  // generation is for, so a second click while that request is still loading is a
  // no-op instead of double-fetching.
  const focusedSymbol = focused?.kind === "symbol" ? focused.symbol.symbol.name : undefined;
  const blastReq = useRef(0);
  const pendingBlastRef = useRef<{ gen: number; path: string; symbol: string } | null>(null);
  useEffect(() => {
    blastReq.current += 1;
    setBlast(null);
    setBlastLoading(false);
  }, [focused?.path, focusedSymbol]);

  const toggleBlast = (path: string, symbol: string) => {
    const pending = pendingBlastRef.current;
    if (blastLoading && pending && pending.path === path && pending.symbol === symbol) return;
    if (blast && blast.root.path === path && blast.root.symbol === symbol) { setBlast(null); return; }
    const gen = ++blastReq.current;
    pendingBlastRef.current = { gen, path, symbol };
    setBlastLoading(true);
    api.blast(path, symbol)
      .then((b) => {
        if (blastReq.current !== gen) return;
        setBlast(b);
        announce(`Blast radius: ${b.files.length} files to depth ${Math.max(0, ...b.files.map((f) => f.depth))}`);
      })
      .catch((e: unknown) => { if (blastReq.current === gen) toastError(e); })
      .finally(() => { if (blastReq.current === gen) setBlastLoading(false); });
  };
  const toggleExpand = (path: string) => setExpandedPath((p) => (p === path ? null : path));

  const onSelectNode = useCallback((path: string, symbol?: string) => {
    const file = rows.find((r) => r.path === path);
    if (!file) return;
    if (symbol) {
      const s = file.symbols.find((x) => x.symbol.name === symbol);
      if (s) { setFocused({ kind: "symbol", path, file, symbol: s }); return; }
    }
    setFocused({ kind: "file", path, file });
  }, [rows]);

  // `MapView`'s treegrid target may be freshly (re)mounted this same tick — a real
  // rAF never fires in a hidden/background tab (and happy-dom does not schedule it
  // reliably either), so a macrotask tick is what actually lands the focus.
  //
  // react-aria-components' Tree uses a roving tabindex: the first time real DOM
  // focus enters the group, it hands focus off to the active row via an effect
  // (so the container itself is never the *lasting* focus target on a cold
  // entry). Focusing once lets that hand-off settle; focusing again afterwards
  // — now that the group is already "focus within" — lands and keeps focus on
  // the treegrid itself, which is what a screen-reader user landing here needs
  // announced (the region, not an arbitrary row).
  const switchToTable = () => {
    changeView("tree");
    setTimeout(() => {
      const el = document.querySelector('[role="treegrid"]') as HTMLElement | null;
      el?.focus();
      setTimeout(() => el?.focus(), 0);
    }, 0);
  };

  const fileCounts = useMemo(() => ({
    served: rows.filter((r) => r.served > 0).length,
    cut: rows.filter((r) => r.served === 0 && r.cut > 0).length,
  }), [rows]);
  const mapLabel = summaryLabel(graph?.nodes.length ?? 0, detail ? { id: detail.id, ...fileCounts } : null, map.boundary.length);

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
        } else {
          toastError(e);
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
        <ViewToggle value={view} onChange={changeView} />
        <label className="ml-auto text-sm">
          Filter
          <input type="search" value={filter} onChange={(e) => setFilter(e.target.value)} className="ml-2 rounded border bg-background px-2 py-1" placeholder="path or symbol" />
        </label>
      </header>
      <RetrievalsRail retrievals={retrievals} selected={selected} onSelect={setSelected}
        onMore={() => api.retrievals(retrievals[retrievals.length - 1]?.id).then((more) => setRetrievals((rs) => [...rs, ...more]))} />
      <main className={view === "tree" ? "overflow-auto" : "relative overflow-hidden"}>
        {view === "tree" ? (
          <RepoTree rows={rows} filter={filter} seedKey={detail?.id ?? 0} onFocusRow={setFocused} onAction={openDetail} />
        ) : (
          <MapView payload={graph} rows={rows} hasRetrieval={detail !== null}
            focusedPath={focused?.path ?? null} focusedSymbol={focused?.kind === "symbol" ? focused.symbol.symbol.name : null}
            expandedPath={expandedPath} blast={blast} boundaries={map.boundary} ariaLabel={mapLabel}
            onSelectNode={onSelectNode} onToggleExpand={toggleExpand} onSwitchToTable={switchToTable} onLayoutReady={onLayoutReady} />
        )}
      </main>
      <DetailPanel row={focused} map={map}
        onPin={(p, s) => save((c) => togglePin(c, p, s))}
        onExclude={(p) => save((c) => toggleExclude(c, p))}
        onNote={(p, s, t) => { if (t !== noteFor(map, p, s)) save((c) => setNote(c, p, s, t)); }}
        blast={blast} blastLoading={blastLoading} onToggleBlast={toggleBlast}
        expandedPath={expandedPath} onToggleExpand={toggleExpand}
        onAddBoundary={(n, p) => save((c) => addToBoundary(c, n, p))}
        onRemoveBoundary={(n, p) => save((c) => removeFromBoundary(c, n, p))} />
      <LiveRegion messages={messages} />
      <Toaster />
    </div>
  );
}
