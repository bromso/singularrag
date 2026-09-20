import { rmSync } from "node:fs";
import tailwind from "bun-plugin-tailwind";

// A stale `dist/` can leave behind assets from a previous build (a hashed filename that
// no longer exists in the new build's manifest but is still on disk), which the shell
// then never references but a client with an old cached shell might still request.
rmSync("dist", { recursive: true, force: true });

const result = await Bun.build({
  entrypoints: ["./index.html"],
  outdir: "./dist",
  minify: true,
  // Ship React's production build; without this the embedded bundle is the dev build.
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  sourcemap: "none",
  plugins: [tailwind],
  naming: {
    entry: "[name].[ext]",
    chunk: "assets/[name]-[hash].[ext]",
    asset: "assets/[name]-[hash].[ext]",
  },
});
if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}
console.log(`built ${result.outputs.length} files into ui/dist`);
