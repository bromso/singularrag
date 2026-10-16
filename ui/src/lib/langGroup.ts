export type LangGroup = "code" | "docs" | "config" | "styles" | "schema";
const GROUPS: Record<string, LangGroup> = { markdown: "docs", html: "docs", text: "docs", json: "config", yaml: "config", toml: "config", css: "styles", prisma: "schema", sql: "schema" };
export const langGroup = (lang: string | null): LangGroup => (lang ? GROUPS[lang] : undefined) ?? "code";
