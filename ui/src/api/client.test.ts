import { describe, expect, test, beforeEach, afterEach } from "bun:test";
import { ApiError, api, getToken, setToken, tokenFromFragment } from "./client";
import { subscribe } from "./events";
import type { MapConfig, Status } from "./types";

const mockStatus: Status = {
  index_version: "v1",
  git_head: "abc123",
  indexed_at_ms: 1000,
  stale_count: 0,
  lock_timeout: false,
  foreign_indexing: false,
  indexing: false,
  files: { indexed: 10, skipped: 2 },
  drain: { chunks: 5, last: { scanned: 100, indexed: 95, unchanged: 5, skipped: 0, removed: 0, remaining: 0, lock_timeout: false } },
};

describe("tokenFromFragment", () => {
  afterEach(() => {
    window.location.hash = "";
  });

  test("extracts token from hash only", () => {
    window.location.hash = "#token=abc123";
    expect(tokenFromFragment()).toBe("abc123");
  });

  test("extracts token after other params", () => {
    window.location.hash = "#x=1&token=abc123";
    expect(tokenFromFragment()).toBe("abc123");
  });

  test("extracts token before other params", () => {
    window.location.hash = "#token=abc123&y=2";
    expect(tokenFromFragment()).toBe("abc123");
  });

  test("preserves case in token", () => {
    window.location.hash = "#token=ABC123";
    expect(tokenFromFragment()).toBe("ABC123");
  });

  test("returns empty string for empty hash", () => {
    window.location.hash = "";
    expect(tokenFromFragment()).toBe("");
  });

  test("returns empty string when token param not present", () => {
    window.location.hash = "#x=1&y=2";
    expect(tokenFromFragment()).toBe("");
  });
});

describe("api.status()", () => {
  let originalFetch: typeof globalThis.fetch;

  beforeEach(() => {
    originalFetch = globalThis.fetch;
    setToken("");
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    setToken("");
  });

  test("sends correct headers and URL", async () => {
    const calls: { url: string; init: RequestInit }[] = [];
    globalThis.fetch = (async (url: string, init?: RequestInit) => {
      calls.push({ url, init: init ?? {} });
      return new Response(JSON.stringify(mockStatus), { status: 200 });
    }) as any;

    setToken("t0k");
    const result = await api.status();

    expect(calls).toHaveLength(1);
    expect(calls[0].url).toBe("/api/status");
    expect(calls[0].init.headers).toBeDefined();
    const headers = calls[0].init.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer t0k");
    expect(result).toEqual(mockStatus);
  });

  test("resolves 200 JSON response", async () => {
    globalThis.fetch = (async () => new Response(JSON.stringify(mockStatus), { status: 200 })) as any;

    const result = await api.status();
    expect(result).toEqual(mockStatus);
  });
});

describe("ApiError", () => {
  let originalFetch: typeof globalThis.fetch;

  beforeEach(() => {
    originalFetch = globalThis.fetch;
    setToken("t0k");
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    setToken("");
  });

  test("parses error JSON with field", async () => {
    globalThis.fetch = (async () =>
      new Response(JSON.stringify({ error: "bad", field: "pin[0].path" }), { status: 422 })) as any;

    try {
      await api.status();
      expect.unreachable("should throw");
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      const err = e as ApiError;
      expect(err.status).toBe(422);
      expect(err.message).toBe("bad");
      expect(err.field).toBe("pin[0].path");
    }
  });

  test("retries a 503 with backoff, then succeeds", async () => {
    let calls = 0;
    globalThis.fetch = (async () => {
      calls += 1;
      return calls <= 2
        ? new Response(JSON.stringify({ error: "database is locked" }), { status: 503 })
        : new Response(JSON.stringify(mockStatus), { status: 200 });
    }) as any;

    const result = await api.status();
    expect(calls).toBe(3);
    expect(result).toEqual(mockStatus);
  });

  test("gives up after three 503 retries", async () => {
    let calls = 0;
    globalThis.fetch = (async () => {
      calls += 1;
      return new Response(JSON.stringify({ error: "database is locked" }), { status: 503 });
    }) as any;

    try {
      await api.status();
      expect.unreachable("should throw");
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).status).toBe(503);
    }
    expect(calls).toBe(4); // the first try plus three retries
  });

  test("falls back to statusText for non-JSON error", async () => {
    globalThis.fetch = (async () => new Response("boom", { status: 500, statusText: "Internal Server Error" })) as any;

    try {
      await api.status();
      expect.unreachable("should throw");
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      const err = e as ApiError;
      expect(err.status).toBe(500);
      expect(err.message).toBe("Internal Server Error");
      expect(err.field).toBeUndefined();
    }
  });
});

describe("api.saveMap()", () => {
  let originalFetch: typeof globalThis.fetch;

  beforeEach(() => {
    originalFetch = globalThis.fetch;
    setToken("t0k");
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
    setToken("");
  });

  test("sends PUT method with JSON body and the compare-and-swap version", async () => {
    const calls: { url: string; init: RequestInit }[] = [];
    globalThis.fetch = (async (url: string, init?: RequestInit) => {
      calls.push({ url, init: init ?? {} });
      const cfg: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
      return new Response(JSON.stringify({ ...cfg, version: 7 }), { status: 200 });
    }) as any;

    const cfg: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
    const doc = await api.saveMap(cfg, 6);

    expect(calls).toHaveLength(1);
    expect(calls[0].init.method).toBe("PUT");
    const headers = calls[0].init.headers as Record<string, string>;
    expect(headers["Content-Type"]).toBe("application/json");
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ ...cfg, expected_version: 6 });
    expect(doc.version).toBe(7);
  });

  test("a 409 carries the server's current document on the error", async () => {
    const current = { pin: [{ path: "src/a.ts" }], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] }, version: 99 };
    globalThis.fetch = (async () =>
      new Response(JSON.stringify({ error: "map.toml changed on disk", field: "expected_version", current }), { status: 409 })) as any;

    const cfg: MapConfig = { pin: [], exclude: [], note: [], boundary: [], deny: { extra_patterns: [] } };
    try {
      await api.saveMap(cfg, 1);
      expect.unreachable("should throw");
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      const err = e as ApiError;
      expect(err.status).toBe(409);
      expect(err.field).toBe("expected_version");
      expect(err.current).toEqual(current);
    }
  });
});

describe("subscribe()", () => {
  let originalEventSource: typeof globalThis.EventSource;

  beforeEach(() => {
    originalEventSource = globalThis.EventSource;
    setToken("t0k");
  });

  afterEach(() => {
    globalThis.EventSource = originalEventSource;
    setToken("");
  });

  test("connects to correct URL and registers listeners", () => {
    let constructedUrl: string = "";
    const listeners: Record<string, Function> = {};
    let closed = false;

    class MockEventSource {
      constructor(url: string) {
        constructedUrl = url;
      }

      addEventListener(event: string, handler: Function) {
        listeners[event] = handler;
      }

      close() {
        closed = true;
      }
    }

    globalThis.EventSource = MockEventSource as any;

    const onChange = () => {};
    const onFreshness = () => {};
    const unsubscribe = subscribe(onChange, onFreshness);

    expect(constructedUrl).toBe("/api/events?token=t0k");
    expect(listeners["change"]).toBeDefined();
    expect(listeners["freshness"]).toBeDefined();

    // Test change event
    const changeHandler = listeners["change"];
    const changeEvent = new MessageEvent("change", { data: '{"max_retrieval_id":7}' });
    let changedValue = 0;
    const onChangeCapture = (maxId: number) => {
      changedValue = maxId;
    };

    // Re-subscribe with capture function
    globalThis.EventSource = MockEventSource as any;
    const unsubscribe2 = subscribe(onChangeCapture, onFreshness);
    const listeners2: Record<string, Function> = {};
    globalThis.EventSource = class MockEventSource2 {
      constructor(url: string) {
        constructedUrl = url;
      }

      addEventListener(event: string, handler: Function) {
        listeners2[event] = handler;
      }

      close() {
        closed = true;
      }
    } as any;

    const unsubscribe3 = subscribe(onChangeCapture, onFreshness);
    listeners2["change"](changeEvent);
    expect(changedValue).toBe(7);

    // Test unsubscribe
    unsubscribe3();
    expect(closed).toBe(true);
  });
});
