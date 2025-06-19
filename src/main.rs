//! cargo-clip — copy Rust source & Cargo.toml files to the clipboard.
//! • Default: every *.rs *and* every tracked/untracked Cargo.toml
//! • Obeys .gitignore so ignored artefacts never leak
//! • Works for workspaces and single-crate repos (crate name + version header)

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use arboard::Clipboard; // cross-platform clipboard :contentReference[oaicite:1]{index=1}
use cargo_metadata::MetadataCommand;
use cargo_toml::{Inheritable, Manifest}; // to parse standalone manifests
use clap::{Parser, ValueHint};
use git2::{Repository, StatusOptions}; // libgit2 bindings :contentReference[oaicite:2]{index=2}

type CrateMap = HashMap<PathBuf, (String, String)>;

/// `cargo clip [OPTIONS] [ext …]`
#[derive(Parser)]
#[command(name = "cargo-clip", version, about)]
struct Opts {
    /// Workspace or crate root (defaults to cwd)
    #[arg(short, long, value_hint = ValueHint::DirPath, default_value = ".")]
    dir: PathBuf,

    /// Extra extensions to include (default: rs toml)
    exts: Vec<String>,

    /// Print to stdout instead of the clipboard
    #[arg(long)]
    no_clipboard: bool,
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    let extensions = if opts.exts.is_empty() {
        vec!["rs".into(), "toml".into()]
    } else {
        opts.exts.clone()
    };

    // 1️⃣  collect crate metadata
    let crates = build_crate_map(&opts.dir)?;

    // 2️⃣  enumerate git *and* untracked files (obey .gitignore)
    let repo = Repository::discover(&opts.dir).context("not a git repository")?;
    let statuses = repo.statuses(Some(
        StatusOptions::new()
            .include_untracked(true) // ← always include untracked files
            .include_ignored(false) // but still skip ignored
            .include_unmodified(true),
    ))?;

    let mut wanted = Vec::<PathBuf>::new();
    for st in statuses.iter() {
        if let Some(p) = st.path() {
            let abs = opts.dir.join(p);
            if abs.file_name() == Some("Cargo.toml".as_ref()) {
                wanted.push(abs);
                continue;
            }
            if abs
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| extensions.iter().any(|x| x == e))
                .unwrap_or(false)
            {
                wanted.push(abs);
            }
        }
    }
    wanted.sort();

    // 3️⃣  compose prompt text
    let mut out = String::new();
    for path in &wanted {
        let (name, ver) =
            crate_for_path(path, &crates).unwrap_or_else(|| ("unknown_crate".into(), "?".into()));
        let rel = path.strip_prefix(&opts.dir).unwrap_or(path);
        out.push_str(&format!("=== {name} v{ver} :: {} ===\n", rel.display()));
        out.push_str(&fs::read_to_string(path)?);
        out.push('\n');
    }

    // 4️⃣  clipboard or stdout
    if opts.no_clipboard {
        print!("{out}");
    } else {
        match Clipboard::new() {
            // Wayland / X11 / macOS / Windows :contentReference[oaicite:3]{index=3}
            Ok(mut c) => c.set_text(out.clone())?,
            Err(e) => {
                eprintln!("clipboard error ({e}); printing");
                print!("{out}");
            }
        }
    }
    Ok(())
}

//──────────────────────── helpers ────────────────────────────────────────────

fn build_crate_map(root: &Path) -> Result<CrateMap> {
    let mut map = CrateMap::new();

    // workspace members via `cargo metadata`
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

    // standalone repo fallback
    let root_manifest = root.join("Cargo.toml");
    if !map.contains_key(root) && root_manifest.exists() {
        let mani: Manifest = Manifest::from_path(&root_manifest)?;
        if let Some(pkg) = mani.package {
            map.insert(root.to_path_buf(), (pkg.name, fmt_ver(&pkg.version)));
        }
    }
    Ok(map)
}

/// stringify `Inheritable<String>` (covers all crate versions of cargo_toml)
fn fmt_ver(v: &Inheritable<String>) -> String {
    match v {
        Inheritable::Set(s) => s.clone(),
        _ => "<workspace>".into(),
    }
}

fn crate_for_path(p: &Path, crates: &CrateMap) -> Option<(String, String)> {
    crates
        .iter()
        .filter(|(root, _)| p.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, v)| v.clone())
        .or_else(|| {
            let mut cur = p.parent();
            while let Some(dir) = cur {
                let mani = dir.join("Cargo.toml");
                if mani.exists() {
                    if let Ok(m) = Manifest::from_path(&mani) {
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
