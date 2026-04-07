fn main() {
    // Ensure ui/dist/ exists so rust-embed compiles before the first `bun run build`.
    // In dev, PORTAL_UI_DEV=1 proxies to the Vite dev server instead of serving embedded files.
    std::fs::create_dir_all("ui/dist").expect("failed to create ui/dist");

    // Re-run this script if the UI build output changes.
    println!("cargo:rerun-if-changed=ui/dist");
}
