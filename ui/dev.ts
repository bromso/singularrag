import homepage from "./index.html";

const API = process.env.SINGULARRAG_API ?? "http://127.0.0.1:4173";

/** What `proxyToApi` needs of a request; `Request` satisfies it. */
export type ProxyRequest = Pick<Request, "method" | "url" | "headers" | "body">;

/**
 * Forward one `/api/...` call from the dev server to the running `singularrag serve`.
 *
 * The incoming `Host` is the dev server's (`localhost:5173`), and the API rejects any
 * Host that is not its own with 403 before routing (spec §5) — so forwarding the header
 * verbatim made every call fail. Dropping it lets `fetch` set the target's own Host.
 * A request body is a stream, which `fetch` only accepts with `duplex: "half"`.
 */
export function proxyToApi(req: ProxyRequest, apiBase: string): Promise<Response> {
  const url = new URL(req.url);
  const headers = new Headers(req.headers);
  headers.delete("host");
  const init: RequestInit & { duplex?: "half" } = { method: req.method, headers };
  if (req.method !== "GET" && req.method !== "HEAD") {
    init.body = req.body;
    init.duplex = "half";
  }
  return fetch(`${apiBase}${url.pathname}${url.search}`, init);
}

if (import.meta.main) {
  Bun.serve({
    port: 5173,
    routes: { "/": homepage },
    async fetch(req) {
      if (new URL(req.url).pathname.startsWith("/api/")) return proxyToApi(req, API);
      return new Response("Not found", { status: 404 });
    },
  });
  console.log("ui dev server on http://localhost:5173 (api → " + API + ")");
}
