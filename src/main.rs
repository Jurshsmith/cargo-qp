//! cargo-qp — simplest possible version.
//! * Uses `git ls-files -co --exclude-standard` to enumerate every file that is
//!   *not* ignored, whether tracked or un-tracked.
//! * Keeps anything with extension `rs` plus every Cargo.toml.
//! * Adds `crate-name v<version>` headers and copies to clipboard.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use arboard::Clipboard;
use cargo_metadata::MetadataCommand;
use cargo_toml::{Inheritable, Manifest};
use clap::{Parser, ValueHint};

type CrateMap = HashMap<PathBuf, (String, String)>;

/// `cargo clip [OPTIONS] [ext ...]`
#[derive(Parser)]
#[command(name = "cargo-qp", version, about)]
struct Opts {
    /// Directory to operate in (defaults to cwd)
    #[arg(short, long, value_hint = ValueHint::DirPath, default_value = ".")]
    dir: PathBuf,

    /// Extra extensions to include (default: rs toml)
    exts: Vec<String>,

    /// Print to stdout only
    #[arg(long)]
    no_clipboard: bool,
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    let root = opts.dir.canonicalize()?;

    // default extension set
    let mut exts = if opts.exts.is_empty() {
        vec!["rs".into(), "toml".into()]
    } else {
        opts.exts.clone()
    };
    if !exts.contains(&"toml".to_string()) {
        exts.push("toml".into()); // ensure toml present so we keep workspace Cargo.toml
    }

    //--------------------------------------------------------
    // 1. get every non-ignored path via git
    //--------------------------------------------------------
    let output = Command::new("git")
        .args(["ls-files", "-co", "--exclude-standard"])
        .current_dir(&root)
        .output()
        .context("failed to run git ls-files")?;
    if !output.status.success() {
        anyhow::bail!("`git ls-files` failed (exit {:?})", output.status.code());
    }

    let mut wanted = Vec::<PathBuf>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let p = root.join(line.trim());
        if !p.is_file() {
            continue;
        }
        if p.file_name() == Some("Cargo.toml".as_ref()) {
            wanted.push(p);
            continue;
        }
        if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if exts.iter().any(|x| x == ext) {
                wanted.push(p);
            }
        }
    }
    wanted.sort();

    //--------------------------------------------------------
    // 2. build crate map (workspace + loose crates)
    //--------------------------------------------------------
    let crates = build_crate_map(&root)?;

    //--------------------------------------------------------
    // 3. compose output
    //--------------------------------------------------------
    let mut out = String::new();
    for path in &wanted {
        let (name, ver) =
            crate_for_path(path, &crates).unwrap_or_else(|| ("unknown_crate".into(), "?".into()));
        let rel = path.strip_prefix(&root).unwrap_or(path);
        out.push_str(&format!("=== {name} v{ver} :: {} ===\n", rel.display()));
        out.push_str(&std::fs::read_to_string(path)?);
        out.push('\n');
    }

    //--------------------------------------------------------
    // 4. clipboard or stdout
    //--------------------------------------------------------
    if opts.no_clipboard {
        print!("{out}");
    } else if let Err(e) = Clipboard::new().and_then(|mut c| c.set_text(out.clone())) {
        eprintln!("clipboard error ({e}); printing to stdout");
        print!("{out}");
    }

    Ok(())
}

//──────────────────────── helpers ────────────────────────────────────────────

fn build_crate_map(root: &Path) -> Result<CrateMap> {
    let mut map = CrateMap::new();

    // workspace crates
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

    // root crate (if not already covered)
    let root_manifest = root.join("Cargo.toml");
    if !map.contains_key(root) && root_manifest.exists() {
        if let Ok(m) = Manifest::from_path(&root_manifest) {
            if let Some(pkg) = m.package {
                map.insert(root.to_path_buf(), (pkg.name, fmt_ver(&pkg.version)));
            }
        }
    }
    Ok(map)
}

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
