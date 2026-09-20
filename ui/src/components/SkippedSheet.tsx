import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import type { SkippedFile } from "@/api/types";

export function SkippedSheet({ skipped }: { skipped: SkippedFile[] }) {
  const groups = new Map<string, string[]>();
  for (const s of skipped) groups.set(s.reason, [...(groups.get(s.reason) ?? []), s.path]);
  return (
    <Sheet>
      <SheetTrigger render={<Button type="button" variant="outline" />}>Skipped files ({skipped.length})</SheetTrigger>
      <SheetContent>
        <SheetHeader><SheetTitle>Skipped files</SheetTitle></SheetHeader>
        {[...groups].map(([reason, paths]) => (
          <section key={reason} className="mt-3">
            <h3 className="text-sm font-medium">{reason}</h3>
            <ul className="mt-1 font-mono text-xs">{paths.map((p) => <li key={p}>{p}</li>)}</ul>
          </section>
        ))}
      </SheetContent>
    </Sheet>
  );
}
