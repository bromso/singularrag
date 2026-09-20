import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { proxyToApi, type ProxyRequest } from "./dev";

// `new Request(..., { headers })` drops forbidden header names, `host` among them, so a
// request built here cannot carry the header the real Bun.serve hands the dev server.
// `proxyToApi` therefore takes the structural subset it needs, and this builds one.
const incoming = (url: string, headers: Record<string, string>, init: { method?: string; body?: BodyInit } = {}): ProxyRequest => {
  const h = new Headers();
  for (const [k, v] of Object.entries(headers)) h.set(k, v);
  return {
    method: init.method ?? "GET",
    url,
    headers: h,
    body: init.body ? new Response(init.body).body : null,
  };
};

describe("proxyToApi", () => {
  let originalFetch: typeof globalThis.fetch;
  let seen: { url: string; init: RequestInit & { duplex?: string } } | null = null;

  beforeEach(() => {
    originalFetch = globalThis.fetch;
    seen = null;
    globalThis.fetch = (async (url: string, init?: RequestInit) => {
      seen = { url, init: (init ?? {}) as RequestInit & { duplex?: string } };
      return new Response("{}", { status: 200 });
    }) as any;
  });
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  test("drops Host, keeps the token, and preserves path and query", async () => {
    await proxyToApi(
      incoming("http://localhost:5173/api/retrievals?limit=50&before=7", {
        host: "localhost:5173",
        authorization: "Bearer t0k",
      }),
      "http://127.0.0.1:4173",
    );
    expect(seen!.url).toBe("http://127.0.0.1:4173/api/retrievals?limit=50&before=7");
    const headers = seen!.init.headers as Headers;
    // Forwarding the dev server's Host made `serve` answer 403 before routing (I7).
    expect(headers.get("host")).toBeNull();
    expect(headers.get("authorization")).toBe("Bearer t0k");
    expect(seen!.init.body).toBeUndefined();
  });

  test("streams a body on a PUT with duplex: half", async () => {
    await proxyToApi(
      incoming("http://localhost:5173/api/map", { host: "localhost:5173", "content-type": "application/json" }, { method: "PUT", body: '{"pin":[]}' }),
      "http://127.0.0.1:4173",
    );
    expect(seen!.url).toBe("http://127.0.0.1:4173/api/map");
    expect(seen!.init.method).toBe("PUT");
    expect(seen!.init.duplex).toBe("half");
    expect(await new Response(seen!.init.body as BodyInit).text()).toBe('{"pin":[]}');
  });
});
