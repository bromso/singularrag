import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Toaster, toast } from "sonner";
import { ApiError, api, setToken, tokenFromFragment } from "@/api/client";
import { subscribe } from "@/api/events";
import type { BlastResult, EntitiesPayload, GraphPayload, MapConfig, MapDoc, RetrievalDetail, RetrievalSummary, SkippedFile, Status, TreeFile } from "@/api/types";
import { joinRetrieval } from "@/lib/join";
import { addToBoundary, boundariesOf, noteFor, removeAgentNote, removeFromBoundary, setNote, toggleExclude, togglePin } from "@/lib/mapEdits";
import { fileStatusOf } from "@/lib/mapStyle";
import { summaryLabel } from "@/lib/mapSummary";
import { DetailPanel } from "@/components/DetailPanel";
import { FreshnessBadge, freshnessText } from "@/components/FreshnessBadge";
import { LiveRegion } from "@/components/LiveRegion";
import { MapErrorBoundary } from "@/components/MapErrorBoundary";
import { MapView } from "@/components/MapView";
import { QueryPanel } from "@/components/QueryPanel";
import { RepoTree, type RevealRequest, type TreeRow } from "@/components/RepoTree";
import { RetrievalsRail } from "@/components/RetrievalsRail";
import { SkippedSheet } from "@/components/SkippedSheet";
import { OverlayToggle, ViewToggle, loadOverlay, loadView, saveOverlay, saveView, type Overlay, type View } from "@/components/ViewToggle";

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
  const [root, setRoot] = useState("");
  const [blast, setBlast] = useState<BlastResult | null>(null);
  const [blastLoading, setBlastLoading] = useState(false);
  const [expandedPath, setExpandedPath] = useState<string | null>(null);
  const [messages, setMessages] = useState<string[]>([]);
  const announce = useCallback((m: string) => setMessages((ms) => [...ms.slice(-9), m]), []);
  const [view, setView] = useState<View>(loadView);
  const changeView = (v: View) => { setView(v); saveView(v); };
  const [overlay, setOverlay] = useState<Overlay>(loadOverlay);
  const changeOverlay = (o: Overlay) => { setOverlay(o); saveOverlay(o); };
  const [graph, setGraph] = useState<GraphPayload | null>(null);
  const [entities, setEntities] = useState<EntitiesPayload | null>(null);
  // Every change event refetches the entities; an unchanged answer keeps the old object,
  // so the map (which rebuilds its graph when `entities` changes identity) stays put.
  const entitiesJson = useRef("");
  const [reveal, setReveal] = useState<RevealRequest | null>(null);
  const revealSeq = useRef(0);
  const indexRef = useRef<string | null>(null);
  // SSE "change" events are not coalesced, so two `load()` calls can overlap.
  // Without a guard, whichever resolves last wins even if it started first,
  // pinning `indexRef`/tree/graph to a stale version. `loadGen` orders loads by
  // when they *started*, not when they resolve — the same pattern `toggleBlast`
  // uses with `blastReq` — so a superseded load applies nothing at all.
  const loadGen = useRef(0);
  // De-dupes freshness announcements: the watcher broadcasts a full status on every SSE
  // event, so an unchanged text (e.g. two "indexing" events in a row) must not repeat.
  const lastFreshnessRef = useRef<string | null>(null);
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
      const gen = ++loadGen.current;
      // Extraction moves on without the index version changing, so the entities are
      // refetched on every load. Handled here so an early return below leaves no
      // unhandled rejection; the `await` at the end reports a failure.
      const entitiesReq = api.entities();
      entitiesReq.catch(() => {});
      const [s, rs, sk, m] = await Promise.all([api.status(), api.retrievals(), api.skipped(), api.map()]);
      if (gen !== loadGen.current) return;
      setStatus(s); setRetrievals(rs); setSkipped(sk); applyMapDoc(m);
      // Seed the de-dup ref with what the badge already shows, so the first live SSE
      // freshness event that repeats it does not announce the same text twice.
      lastFreshnessRef.current = `Index ${freshnessText(s)}`;
      // The tree and the graph are functions of the index: refetch them only when it changed.
      if (indexRef.current !== s.index_version) {
        const [t, g] = await Promise.all([api.tree(), api.graph()]);
        if (gen !== loadGen.current) return;
        indexRef.current = s.index_version;
        setTree(t); setGraph(g);
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
      const e = await entitiesReq;
      if (gen !== loadGen.current) return;
      const json = JSON.stringify(e);
      if (json !== entitiesJson.current) { entitiesJson.current = json; setEntities(e); }
    };
    load().catch((e: unknown) => toastError(e));
    return subscribe(
      () => { load().catch(() => {}); },
      (s) => {
        setStatus(s);
        const text = `Index ${freshnessText(s)}`;
        if (text !== lastFreshnessRef.current) { lastFreshnessRef.current = text; announce(text); }
      },
    );
  }, [announce, applyMapDoc]);

  useEffect(() => {
    if (selected == null) { setDetail(null); return; }
    api.retrieval(selected).then(setDetail).catch((e: unknown) => toastError(e));
  }, [selected]);

  const rows = useMemo(() => joinRetrieval(tree, detail?.items ?? null), [tree, detail]);
  const filteredRows = useMemo(() => (root ? rows.filter((r) => r.path.startsWith(`${root}/`)) : rows), [rows, root]);
  // The map's own node/edge set, not just the tree rows: `GraphEdge.src`/`dst` are
  // indices into `graph.nodes`, so dropping nodes outside the selected root means
  // re-indexing every surviving edge (and dropping edges that touched a dropped node).
  const filteredGraph = useMemo(() => {
    if (!graph || !root) return graph;
    const prefix = `${root}/`;
    const remap = new Map<number, number>();
    const nodes = graph.nodes.filter((n, i) => {
      if (!n.path.startsWith(prefix)) return false;
      remap.set(i, remap.size);
      return true;
    });
    const edges = graph.edges
      .filter((e) => remap.has(e.src) && remap.has(e.dst))
      .map((e) => ({ ...e, src: remap.get(e.src)!, dst: remap.get(e.dst)! }));
    return { ...graph, nodes, edges };
  }, [graph, root]);

  // A query submitted from the panel becomes a new UI retrieval; refetch the list from
  // the server (rather than optimistically inserting) so it lands with whatever fields
  // the server fills in, then select it so the rail and detail panel jump to it.
  const onQueryDone = useCallback((id: number) => {
    api.retrievals().then(setRetrievals).catch((e: unknown) => toastError(e));
    setSelected(id);
  }, []);

  // A change of root can strand the currently focused/expanded row on a path that's no
  // longer shown — clear it rather than leave the detail panel pointing at a hidden file.
  useEffect(() => {
    if (!root) return;
    const prefix = `${root}/`;
    setFocused((f) => (f && f.kind !== "entity" && !f.path.startsWith(prefix) ? null : f));
    setExpandedPath((p) => (p && !p.startsWith(prefix) ? null : p));
  }, [root]);

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
  const focusedPath = focused && focused.kind !== "entity" ? focused.path : null;
  const focusedSymbol = focused?.kind === "symbol" ? focused.symbol.symbol.name : undefined;
  const blastReq = useRef(0);
  const pendingBlastRef = useRef<{ gen: number; path: string; symbol: string } | null>(null);
  useEffect(() => {
    blastReq.current += 1;
    setBlast(null);
    setBlastLoading(false);
  }, [focusedPath, focusedSymbol]);

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
    const file = filteredRows.find((r) => r.path === path);
    if (!file) return;
    if (symbol) {
      const s = file.symbols.find((x) => x.symbol.name === symbol);
      if (s) { setFocused({ kind: "symbol", path, file, symbol: s }); return; }
    }
    setFocused({ kind: "file", path, file });
  }, [filteredRows]);

  // An entity shares the `focused` slot with rows. From the panel's list (the keyboard
  // path) focus moves to the panel's heading, as a row action does; a map click does not.
  const focusEntity = useCallback((id: number, moveFocus: boolean) => {
    const entity = entities?.entities.find((e) => e.id === id);
    if (!entity) return;
    setFocused({ kind: "entity", entity });
    if (moveFocus) requestAnimationFrame(() => document.getElementById("detail-heading")?.focus());
  }, [entities]);
  const onSelectEntity = useCallback((id: number) => focusEntity(id, false), [focusEntity]);

  // A refetch replaces the entity objects: keep a focused entity current, or drop it if gone.
  useEffect(() => {
    setFocused((f) => {
      if (f?.kind !== "entity") return f;
      const e = entities?.entities.find((x) => x.id === f.entity.id);
      return e ? (e === f.entity ? f : { kind: "entity", entity: e }) : null;
    });
  }, [entities]);

  // "Go to section": reveal the section's row in the tree (switching from the map) and
  // focus it; the row's own focus report then moves the panel to that section.
  const focusSection = (path: string, name: string, symbolId?: number) => {
    const file = filteredRows.find((r) => r.path === path);
    const sym = file && (file.symbols.find((s) => s.symbol.id === symbolId) ?? file.symbols.find((s) => s.symbol.name === name));
    if (!file || !sym) { toast.error(`${path} :: ${name} is not in the tree`); return; }
    if (view !== "tree") changeView("tree");
    setReveal({ path, key: sym.key, seq: ++revealSeq.current });
  };

  // `MapView`'s treegrid target may be freshly (re)mounted this same tick — a real
  // rAF never fires in a hidden/background tab (and happy-dom does not schedule it
  // reliably either), so a macrotask tick is what actually lands the focus.
  // Focus lands on the tree's current row, not the treegrid container itself —
  // that's react-aria's roving-tabindex design for the APG treegrid pattern.
  const switchToTable = () => {
    changeView("tree");
    setTimeout(() => (document.querySelector('[role="treegrid"]') as HTMLElement | null)?.focus(), 0);
  };

  // Counted over the (root-filtered) graph's own file set, not every tree row: an
  // excluded file is a row (the tree still lists it) but not a graph node, and a file
  // outside the selected root isn't on the map either — neither must be counted.
  const graphPaths = useMemo(() => new Set((filteredGraph?.nodes ?? []).map((n) => n.path)), [filteredGraph]);
  const fileCounts = useMemo(() => ({
    served: rows.filter((r) => graphPaths.has(r.path) && fileStatusOf(r, true) === "served").length,
    cut: rows.filter((r) => graphPaths.has(r.path) && fileStatusOf(r, true) === "cut").length,
  }), [rows, graphPaths]);
  const mapEntities = overlay === "entities" ? entities : null;
  const mapLabel = summaryLabel(filteredGraph?.nodes.length ?? 0, detail ? { id: detail.id, ...fileCounts } : null, map.boundary.length, mapEntities?.entities.length);

  // Sorted-path JSON, so add-then-remove-in-a-different-order still reads as unchanged.
  const excludeKey = (exclude: MapConfig["exclude"]) => JSON.stringify(exclude.map((t) => t.path).slice().sort());

  // `edit` is applied when the save's turn comes, to the config as it is then — not at
  // click time — so the second of two rapid clicks does not discard the first.
  const save = (edit: (cfg: MapConfig) => MapConfig) => {
    saveQueue.current = saveQueue.current.then(async () => {
      const prevExcludeKey = excludeKey(mapRef.current.exclude);
      try {
        const doc = await api.saveMap(edit(mapRef.current), versionRef.current);
        applyMapDoc(doc);
        toast.success("Saved. Applies to the next retrieval.");
        // The map (nodes/edges) is a separate document from map.toml; a changed exclude
        // set can add or remove files from it, so the map must be refetched to match.
        if (excludeKey(doc.exclude) !== prevExcludeKey) {
          const g = await api.graph();
          setGraph(g);
        }
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
        {view === "map" && <OverlayToggle value={overlay} onChange={changeOverlay} />}
        {status && status.roots.length > 1 && (
          <label className="text-sm">
            Root
            <select value={root} onChange={(e) => setRoot(e.target.value)} className="ml-2 rounded border bg-background px-2 py-1">
              <option value="">All roots</option>
              {status.roots.map((r) => <option key={r.name} value={r.name}>{r.name}</option>)}
            </select>
          </label>
        )}
        <label className="ml-auto text-sm">
          Filter
          <input type="search" value={filter} onChange={(e) => setFilter(e.target.value)} className="ml-2 rounded border bg-background px-2 py-1" placeholder="path or symbol" />
        </label>
      </header>
      <div className="flex min-h-0 flex-col">
        <QueryPanel onDone={onQueryDone} announce={announce} />
        <RetrievalsRail retrievals={retrievals} selected={selected} onSelect={setSelected}
          onMore={() => api.retrievals(retrievals[retrievals.length - 1]?.id).then((more) => setRetrievals((rs) => [...rs, ...more]))} />
      </div>
      <main className={view === "tree" ? "min-h-0 overflow-auto" : "relative min-h-0 overflow-hidden"}>
        {view === "tree" ? (
          <RepoTree rows={filteredRows} filter={filter} seedKey={detail?.id ?? 0} onFocusRow={setFocused} onAction={openDetail}
            reveal={reveal} onRevealed={() => setReveal(null)} />
        ) : (
          <MapErrorBoundary onSwitchToTable={switchToTable}>
            <MapView payload={filteredGraph} rows={filteredRows} hasRetrieval={detail !== null}
              focusedPath={focusedPath} focusedSymbol={focusedSymbol ?? null}
              entities={mapEntities} focusedEntity={focused?.kind === "entity" ? focused.entity.id : null} onSelectEntity={onSelectEntity}
              expandedPath={expandedPath} blast={blast} boundaries={map.boundary} ariaLabel={mapLabel}
              onSelectNode={onSelectNode} onToggleExpand={toggleExpand} onSwitchToTable={switchToTable} onLayoutReady={onLayoutReady} />
          </MapErrorBoundary>
        )}
      </main>
      <DetailPanel row={focused} map={map} entities={entities}
        onFocusEntity={(id) => focusEntity(id, true)} onFocusSection={focusSection}
        onPin={(p, s) => save((c) => togglePin(c, p, s))}
        onExclude={(p) => save((c) => toggleExclude(c, p))}
        onNote={(p, s, t) => { if (t !== noteFor(map, p, s)) save((c) => setNote(c, p, s, t)); }}
        onRemoveAgentNote={(p, s) => save((c) => removeAgentNote(c, p, s))}
        blast={blast} blastLoading={blastLoading} onToggleBlast={toggleBlast}
        expandedPath={expandedPath} onToggleExpand={toggleExpand}
        onAddBoundary={(n, p) => save((c) => addToBoundary(c, n, p))}
        onRemoveBoundary={(n, p) => { if (boundariesOf(map, p).includes(n)) save((c) => removeFromBoundary(c, n, p)); }} />
      <LiveRegion messages={messages} />
      <Toaster />
    </div>
  );
}
