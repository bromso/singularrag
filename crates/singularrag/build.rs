use std::path::Path;

fn main() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    if !dist.join("index.html").exists() {
        panic!(
            "ui/dist/index.html is missing: run `bun install && bun run build` in ui/ first (the UI is embedded into this binary)"
        );
    }
}
