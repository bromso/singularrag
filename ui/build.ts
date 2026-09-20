import tailwind from "bun-plugin-tailwind";

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
