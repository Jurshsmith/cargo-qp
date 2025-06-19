//! cargo-clip — copy Rust source *and* Cargo.toml files to your clipboard.
//! • Default: all tracked *.rs  + every tracked Cargo.toml
//! • Workspace-aware, but still works for a single-crate repo
//! • Respects .gitignore (never leaks ignored / un-tracked files)

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
use git2::{Repository, StatusOptions};

/// crate-root → (name, version)
type CrateMap = HashMap<PathBuf, (String, String)>;

/// `cargo clip [OPTIONS] [ext …]`
#[derive(Parser)]
#[command(name = "cargo-clip", version, about)]
struct Opts {
    /// Workspace / crate root (defaults to cwd)
    #[arg(short, long, value_hint = ValueHint::DirPath, default_value = ".")]
    dir: PathBuf,

    /// Extra extensions to include. Default = `rs toml`
    exts: Vec<String>,

    /// Don’t touch clipboard, just print to stdout
    #[arg(long)]
    no_clipboard: bool,
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    let exts = if opts.exts.is_empty() {
        vec!["rs".into(), "toml".into()]
    } else {
        opts.exts.clone()
    };

    // 1️⃣  Collect crate metadata (workspace or single-crate)
    let crates = build_crate_map(&opts.dir)?;

    // 2️⃣  Enumerate Git-tracked files (obeys .gitignore)
    let repo = Repository::discover(&opts.dir).context("not a git repository")?;
    let statuses = repo.statuses(Some(
        StatusOptions::new()
            .include_untracked(false)
            .include_ignored(false)
            .include_unmodified(true),
    ))?;

    let mut wanted = Vec::<PathBuf>::new();
    for s in statuses.iter() {
        if let Some(p) = s.path() {
            let abs = opts.dir.join(p);
            if abs.file_name() == Some("Cargo.toml".as_ref()) {
                wanted.push(abs);
                continue;
            }
            if abs
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| exts.iter().any(|x| x == e))
                .unwrap_or(false)
            {
                wanted.push(abs);
            }
        }
    }
    wanted.sort();

    // 3️⃣  Compose clipboard / stdout text
    let mut out = String::new();
    for path in &wanted {
        let (name, ver) =
            crate_for_path(path, &crates).unwrap_or_else(|| ("unknown_crate".into(), "?".into()));
        let rel = path.strip_prefix(&opts.dir).unwrap_or(path);
        out.push_str(&format!("=== {name} v{ver} :: {} ===\n", rel.display()));
        out.push_str(
            &fs::read_to_string(path).with_context(|| format!("reading {}", rel.display()))?,
        );
        out.push('\n');
    }

    // 4️⃣  Send to clipboard or stdout
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

//──────────────────────── helpers ────────────────────────────────────────────

fn build_crate_map(root: &Path) -> Result<CrateMap> {
    let mut map = CrateMap::new();

    // a) Workspace members via `cargo metadata`
    if let Ok(md) = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .exec()
    {
        for pkg in md.packages {
            let dir = pkg
                .manifest_path
                .parent()
                .unwrap()
                .as_std_path() // Utf8PathBuf → &Path
                .to_path_buf();
            map.insert(dir, (pkg.name, pkg.version.to_string()));
        }
    }

    // b) Stand-alone repo fallback
    let root_manifest = root.join("Cargo.toml");
    if !map.contains_key(root) && root_manifest.exists() {
        let mani: Manifest = Manifest::from_path(&root_manifest)?;
        if let Some(pkg) = mani.package {
            map.insert(root.to_path_buf(), (pkg.name, fmt_ver(&pkg.version)));
        }
    }
    Ok(map)
}

/// Turn `Inheritable<String>` into something printable.
fn fmt_ver(v: &Inheritable<String>) -> String {
    // Works for every crate version of cargo_toml ≥ 0.15
    match v {
        Inheritable::Set(s) => s.clone(),
        _ => "<workspace>".into(), // covers *any* inherited variant form
    }
}

fn crate_for_path(p: &Path, crates: &CrateMap) -> Option<(String, String)> {
    crates
        .iter()
        .filter(|(root, _)| p.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, v)| v.clone())
        .or_else(|| {
            // Fallback: climb directories to find nearest Cargo.toml
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
