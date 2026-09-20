import { getToken } from "./client";
import type { Status } from "./types";

export function subscribe(onChange: (maxId: number) => void, onFreshness: (s: Status) => void): () => void {
  const es = new EventSource(`/api/events?token=${getToken()}`);
  es.addEventListener("change", (e) => onChange(JSON.parse((e as MessageEvent).data).max_retrieval_id));
  es.addEventListener("freshness", (e) => onFreshness(JSON.parse((e as MessageEvent).data)));
  return () => es.close();
}
