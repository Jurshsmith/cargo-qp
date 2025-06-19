//! cargo-qp — walk *downwards* from the chosen directory, obey .gitignore,
//! and copy every .rs and Cargo.toml into the clipboard (or stdout).

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

/// crate-root → (name, version)
type CrateMap = HashMap<PathBuf, (String, String)>;

/// `cargo clip [OPTIONS] [ext …]`
#[derive(Parser)]
#[command(name = "cargo-clip", version, about)]
struct Opts {
    /// Directory to start walking (defaults to cwd)
    #[arg(short, long, value_hint = ValueHint::DirPath, default_value = ".")]
    dir: PathBuf,

    /// Extra extensions (default: rs toml)
    exts: Vec<String>,

    /// Print to stdout, skip clipboard
    #[arg(long)]
    no_clipboard: bool,
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    let root = opts
        .dir
        .canonicalize()
        .context("failed to canonicalise start dir")?;

    let exts: Vec<String> = if opts.exts.is_empty() {
        vec!["rs".into(), "toml".into()]
    } else {
        opts.exts.clone()
    };

    // 1️⃣ collect crate metadata
    let crates = build_crate_map(&root)?;

    // 2️⃣ walk from `root` downwards, obeying .gitignore but *including* un-tracked files
    let mut wanted = Vec::<PathBuf>::new();
    let mut walker = WalkBuilder::new(&root);
    walker
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .standard_filters(false) // <- **do not** drop entries like src/ because they're "hidden"
        .follow_links(true) // follow symlinks
        .filter_entry(skip_big_dirs);

    for dent in walker.build() {
        let entry = match dent {
            Ok(e) => e,
            Err(err) => {
                eprintln!("walk error: {err}");
                continue;
            }
        };
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.into_path();

        if path.file_name() == Some("Cargo.toml".as_ref()) {
            wanted.push(path);
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if exts.iter().any(|x| x == ext) {
                wanted.push(path);
            }
        }
    }
    wanted.sort();

    // 3️⃣ compose output
    let mut out = String::new();
    for path in &wanted {
        let (name, ver) =
            crate_for_path(path, &crates).unwrap_or_else(|| ("unknown_crate".into(), "?".into()));
        let rel = path.strip_prefix(&root).unwrap_or(path);
        out.push_str(&format!("=== {name} v{ver} :: {} ===\n", rel.display()));
        out.push_str(&fs::read_to_string(path)?);
        out.push('\n');
    }

    // 4️⃣ clipboard or stdout
    if opts.no_clipboard {
        print!("{out}");
    } else if let Err(e) = Clipboard::new().and_then(|mut c| c.set_text(out.clone())) {
        eprintln!("clipboard error ({e}); printing to stdout");
        print!("{out}");
    }

    Ok(())
}

//──────────────────────── helpers ────────────────────────────────────────────

/// build workspace + single-crate map
fn build_crate_map(root: &Path) -> Result<CrateMap> {
    let mut map = CrateMap::new();

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

/// workspace-inherited vs explicit version → printable
fn fmt_ver(v: &Inheritable<String>) -> String {
    match v {
        Inheritable::Set(s) => s.clone(),
        _ => "<workspace>".into(),
    }
}

/// resolve crate for arbitrary path
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

/// skip gigantic binary trees you almost never want in an LLM prompt
fn skip_big_dirs(e: &DirEntry) -> bool {
    if let Some(name) = e.file_name().to_str() {
        return !matches!(name, "target" | ".git" | "node_modules");
    }
    true
}
