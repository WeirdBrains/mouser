//! Windows resources, and every public docs/**/*.md embedded as an in-app help
//! topic: one source for the docs and `mouser help`. Internal maintainer docs
//! (mapped `internal` in docs/sections.toml) and docs/historical/ are skipped.
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn internal_docs(docs: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(docs.join("sections.toml")) else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    let mut in_docs = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_docs = line == "[docs]";
            continue;
        }
        if !in_docs || line.is_empty() || line.starts_with('#') {
            continue;
        }
        // "path.md" = "internal"
        let Some((key, val)) = line.split_once('=') else { continue };
        let key = key.trim().trim_matches('"');
        let val = val.trim().trim_matches('"');
        if val == "internal" {
            out.insert(key.to_string());
        }
    }
    out
}

fn walk(dir: &Path, docs_root: &Path, internal: &HashSet<String>, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() && p.file_name().is_some_and(|n| n == "historical" || n == "plans") {
            continue; // archived and in-progress plans are not help topics
        }
        if p.is_dir() {
            walk(&p, docs_root, internal, out);
        } else if p.extension().is_some_and(|e| e == "md") {
            let rel = p.strip_prefix(docs_root).unwrap().to_string_lossy().replace('\\', "/");
            if internal.contains(&rel) {
                continue;
            }
            out.push(p);
        }
    }
}

fn help_topics() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let docs = root.join("../../docs");
    println!("cargo:rerun-if-changed={}", docs.display());
    println!("cargo:rerun-if-changed={}", docs.join("sections.toml").display());
    let internal = internal_docs(&docs);
    let mut files = vec![];
    walk(&docs, &docs, &internal, &mut files);
    let mut src = String::from("/// (topic id, markdown) generated from docs/ by build.rs\npub const TOPICS: &[(&str, &str)] = &[\n");
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let rel = f.strip_prefix(&docs).unwrap().with_extension("");
        let id = rel.to_string_lossy().replace('\\', "/");
        let id = id.strip_suffix("/README").unwrap_or(&id).to_string();
        let id = if id == "README" { "index".to_string() } else { id };
        src.push_str(&format!("    ({id:?}, include_str!({:?})),\n", f.canonicalize().unwrap()));
    }
    src.push_str("];\n");
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("help_topics.rs"), src).unwrap();
}

fn main() {
    help_topics();
    windows_resources();
}

#[cfg(target_os = "windows")]
fn windows_resources() {
    // Embed icon and version info into the Windows executable
    let mut res = winres::WindowsResource::new();
    res.set("FileDescription", "Mouser terminal emulator");
    res.set("ProductName", "Mouser");
    res.set("CompanyName", "Mouser");
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    // Optional icon file; if missing, build continues
    let icon_path = "resources/windows/mouser.ico";
    if std::path::Path::new(icon_path).exists() {
        res.set_icon(icon_path);
    }
    let _ = res.compile();
}

#[cfg(not(target_os = "windows"))]
fn windows_resources() {}
