//! cargo-clip — copy Rust workspace files into the system clipboard.
//! By default: every tracked *.rs  and every tracked Cargo.toml.
//! Never touches ignored/untracked files.

use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::{Context, Result};
use arboard::Clipboard;
use cargo_metadata::{MetadataCommand, Package};
use clap::{Parser, ValueHint};
use git2::{Repository, StatusOptions};

/// CLI:  `cargo clip [OPTIONS] [ext ...]`
#[derive(Parser)]
#[command(name = "cargo-clip", version, about)]
struct Opts {
    /// Path to a workspace root (defaults to current Dir)
    #[arg(short, long, value_hint = ValueHint::DirPath, default_value = ".")]
    dir: PathBuf,

    /// Extra extensions to include (e.g. md json).  If omitted,
    /// default is `rs toml`.
    exts: Vec<String>,

    /// Skip clipboard & print to stdout only
    #[arg(long)]
    no_clipboard: bool,
}

fn main() -> Result<()> {
    let opts = Opts::parse();
    // ── default extensions ──
    let exts: Vec<String> = if opts.exts.is_empty() {
        vec!["rs".into(), "toml".into()] // <- “Rust-related” default
    } else {
        opts.exts.clone()
    };

    // ── workspace metadata ──
    let meta = MetadataCommand::new()
        .manifest_path(opts.dir.join("Cargo.toml"))
        .exec()
        .context("cargo metadata failed")?;
    let mut crates: HashMap<PathBuf, &Package> = HashMap::new();
    for pkg in &meta.packages {
        crates.insert(pkg.manifest_path.parent().unwrap().into(), pkg);
    }

    // ── Git-tracked files (respects .gitignore) ──
    let repo = Repository::discover(&opts.dir).context("not in a git repo")?;
    let mut status_opts = StatusOptions::new();
    status_opts
        .include_untracked(false)
        .include_ignored(false) // NEVER copy ignored files
        .include_unmodified(true);
    let statuses = repo.statuses(Some(&mut status_opts))?;

    // ── collect wanted paths ──
    let mut wanted: Vec<PathBuf> = Vec::new();
    for entry in statuses.iter() {
        if let Some(p) = entry.path() {
            let pb = opts.dir.join(p);
            if pb.file_name() == Some("Cargo.toml".as_ref()) {
                wanted.push(pb);
                continue;
            }
            if let Some(ext) = pb.extension().and_then(|e| e.to_str()) {
                if exts.iter().any(|e| e == ext) {
                    wanted.push(pb);
                }
            }
        }
    }
    wanted.sort();

    // ── build output ──
    let mut out = String::new();
    for path in &wanted {
        let crate_name = crates
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, p)| p.name.as_str())
            .unwrap_or("unknown_crate");

        let rel = path.strip_prefix(&opts.dir).unwrap();
        out.push_str(&format!("=== {crate_name} :: {} ===\n", rel.display()));
        out.push_str(
            &fs::read_to_string(path).with_context(|| format!("reading {}", rel.display()))?,
        );
        out.push('\n');
    }

    // ── clipboard or stdout ──
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
