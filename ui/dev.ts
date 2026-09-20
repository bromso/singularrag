import homepage from "./index.html";

const API = process.env.SINGULARRAG_API ?? "http://127.0.0.1:4173";

Bun.serve({
  port: 5173,
  routes: { "/": homepage },
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname.startsWith("/api/")) {
      return fetch(`${API}${url.pathname}${url.search}`, {
        method: req.method,
        headers: req.headers,
        body: req.body,
      });
    }
    return new Response("Not found", { status: 404 });
  },
});
console.log("ui dev server on http://localhost:5173 (api → " + API + ")");
