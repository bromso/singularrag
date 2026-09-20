export function LiveRegion({ messages }: { messages: string[] }) {
  return (
    <div role="log" aria-label="Announcements" aria-live="polite" className="sr-only">
      {messages.map((m, i) => <p key={i}>{m}</p>)}
    </div>
  );
}
