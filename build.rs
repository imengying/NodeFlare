use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let dist = root.join("frontend/admin-dist");
    let out = PathBuf::from(env::var("OUT_DIR").expect("build output directory"));
    let release = env::var("PROFILE").as_deref() == Ok("release");

    println!("cargo:rerun-if-changed=frontend/admin-dist/admin.html");
    println!("cargo:rerun-if-changed=frontend/admin-dist/admin.js");
    println!("cargo:rerun-if-changed=frontend/admin-dist/admin.css");
    println!("cargo:rerun-if-env-changed=NODEFLARE_VERSION");

    let files = [
        (
            "admin.html",
            b"<!doctype html><meta charset=\"utf-8\"><title>Admin unavailable</title><p>Run the frontend admin build first.</p>".as_slice(),
        ),
        (
            "admin.js",
            b"console.error('Run the frontend admin build first.');".as_slice(),
        ),
        (
            "admin.css",
            b"body{font-family:system-ui,sans-serif}".as_slice(),
        ),
    ];

    for (name, fallback) in files {
        let source = dist.join(name);
        let body = match fs::read(&source) {
            Ok(body) => body,
            Err(error) if release => panic!(
                "missing embedded admin asset {} ({error}); run `bun run build:frontend` before the release Worker build",
                source.display()
            ),
            Err(_) => fallback.to_vec(),
        };
        fs::write(out.join(name), body).expect("write embedded admin asset");
    }
}
