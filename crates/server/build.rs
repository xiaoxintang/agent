fn main() {
    println!("cargo:rerun-if-changed=web-dist");
    println!("cargo:rerun-if-changed=migrations");
    assert!(
        std::path::Path::new("web-dist/index.html").is_file(),
        "Missing web assets. Run `cd web && pnpm install --frozen-lockfile && pnpm run build` before building Rust."
    );
}
