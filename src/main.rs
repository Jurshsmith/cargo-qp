//! cargo-qp — copies every Rust source file *and* every Cargo.toml that
//! is **not** ignored by `.gitignore` into the system clipboard (or stdout).
//!
//! Key points
//! ----------
//! • Walks the work-tree with `ignore::WalkBuilder`, which obeys .gitignore but
//!   still visits *un-tracked* files.
//! • Treats `rs` + `toml` as the built-in default extension set.
//! • Finds the owning crate for each file via cargo-metadata; when a file lives
//!   outside the workspace graph we parse the nearest Cargo.toml.
//! • Adds `crate-name v<version>` headers so LLMs understand context.
//! • Cross-platform clipboard through `arboard` (Wayland, X11, macOS, Win32).

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use arboard::Clipboard;
use cargo_metadata::MetadataCommand;
use cargo_toml::{Inheritable, Manifest};
use clap::{Parser, ValueHint};
use ignore::{DirEntry, WalkBuilder};

/// map: crate-root → (name, version)
type CrateMap = HashMap<PathBuf, (String, String)>;

/// Commander-line interface
#[derive(Parser)]
#[command(name = "cargo-qp", version, about)]
struct Opts {
    /// Path to repo / workspace root
    #[arg(short, long, value_hint = ValueHint::DirPath, default_value = ".")]
    dir: PathBuf,

    /// Extra extensions (space-separated).  Defaults to: rs toml
    exts: Vec<String>,

    /// Write to stdout instead of the clipboard
    #[arg(long)]
    no_clipboard: bool,
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    let exts: Vec<String> = if opts.exts.is_empty() {
        vec!["rs".into(), "toml".into()]
    } else {
        opts.exts.clone()
    };

    //---------------- 1  Build crate map ------------------------------------
    let crates = crate_map(&opts.dir)?;

    //---------------- 2  Walk work-tree (git-aware) --------------------------
    let mut wanted: Vec<PathBuf> = Vec::new();

    let mut builder = WalkBuilder::new(&opts.dir);
    builder
        .hidden(false) // still heed .gitignore, but allow dotfiles
        .parents(true) // read .gitignore files in parents
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .filter_entry(|e| filter_entry(e));

    for result in builder.build() {
        let dent = match result {
            Ok(d) => d,
            Err(err) => {
                eprintln!("Walk error: {err}");
                continue;
            }
        };

        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let path = dent.into_path();

        // Always include any Cargo.toml
        if path.file_name() == Some("Cargo.toml".as_ref()) {
            wanted.push(path);
            continue;
        }

        // Otherwise gate by extension list
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.iter().any(|x| x == ext) {
                wanted.push(path);
            }
        }
    }

    wanted.sort();

    //---------------- 3  Compose output --------------------------------------
    let mut out = String::new();

    for path in &wanted {
        let (name, version) =
            crate_for_path(path, &crates).unwrap_or_else(|| ("unknown_crate".into(), "?".into()));

        let rel = path.strip_prefix(&opts.dir).unwrap_or(path);

        out.push_str(&format!("=== {name} v{version} :: {} ===\n", rel.display()));
        out.push_str(
            &fs::read_to_string(path).with_context(|| format!("reading {}", rel.display()))?,
        );
        out.push('\n');
    }

    //---------------- 4  Clipboard or stdout ---------------------------------
    if opts.no_clipboard {
        print!("{out}");
    } else {
        match Clipboard::new() {
            Ok(mut clip) => clip.set_text(out.clone())?,
            Err(e) => {
                eprintln!("clipboard unavailable ({e}); printing to stdout");
                print!("{out}");
            }
        }
    }

    Ok(())
}

//-----------------------------------------------------------------------------

fn crate_map(root: &Path) -> Result<CrateMap> {
    let mut map = CrateMap::new();

    // a) any workspace crates
    if let Ok(md) = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .exec()
    {
        for pkg in md.packages {
            let dir = pkg
                .manifest_path
                .parent()
                .unwrap()
                .as_std_path()
                .to_path_buf();
            map.insert(dir, (pkg.name, pkg.version.to_string()));
        }
    }

    // b) still add the root crate when not part of workspace
    let root_manifest = root.join("Cargo.toml");
    if !map.contains_key(root) && root_manifest.exists() {
        if let Ok(mani) = Manifest::from_path(&root_manifest) {
            if let Some(pkg) = mani.package {
                map.insert(root.to_path_buf(), (pkg.name, fmt_ver(&pkg.version)));
            }
        }
    }
    Ok(map)
}

/// stringify `Inheritable<String>` (workspace-inherited or explicit)
fn fmt_ver(v: &Inheritable<String>) -> String {
    match v {
        Inheritable::Set(s) => s.clone(),
        _ => "<workspace>".into(),
    }
}

/// find owning crate for *path*
/// (longest-prefix match against crate roots; else parse nearest Cargo.toml)
fn crate_for_path(p: &Path, crates: &CrateMap) -> Option<(String, String)> {
    crates
        .iter()
        .filter(|(root, _)| p.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, pair)| pair.clone())
        .or_else(|| {
            // fallback: climb upward until a manifest is found
            let mut cur = p.parent();
            while let Some(dir) = cur {
                let mani_path = dir.join("Cargo.toml");
                if mani_path.exists() {
                    if let Ok(m) = Manifest::from_path(&mani_path) {
                        if let Some(pkg) = m.package {
                            return Some((pkg.name, fmt_ver(&pkg.version)));
                        }
                    }
                }
                cur = dir.parent();
            }
            None
        })
}

/// Custom walker filter: skip really big vendor dirs
fn filter_entry(e: &DirEntry) -> bool {
    if let Some(name) = e.file_name().to_str() {
        // Prevent massive dump
        const SKIP: &[&str] = &["target", ".git", "node_modules"];
        if SKIP.iter().any(|d| d == &name) {
            return false;
        }
    }
    true
}
