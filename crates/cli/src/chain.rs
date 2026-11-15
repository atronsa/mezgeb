//! Tamper evidence from the command line (step 2.09, ADR-0018).
//!
//! ```text
//! mezgeb-cli verify --chain [--checkpoints DIR]
//! mezgeb-cli checkpoint export --dir DIR
//! mezgeb-cli key generate --out FILE
//! ```
//!
//! `verify --chain` recomputes the hash chains of the journal and the audit
//! events from the first link, checks every receipt and checkpoint
//! signature, and checks every checkpoint: those in the database, and the
//! copies in `DIR` that were exported earlier and kept elsewhere. Connect
//! as `mezgeb_verifier`.
//!
//! `checkpoint export` writes each checkpoint in the database to
//! `DIR/checkpoint-<id>.txt`, one line each, and never overwrites one: a
//! file that disagrees with the database is reported. Copy `DIR` somewhere
//! the database's administrators cannot write (ADR-0018).
//!
//! `key generate` writes a new signing key's seed, in hex, to a file that
//! must not exist yet. Keep it like any secret; the service and
//! `close-period` read it from `MEZGEB_SIGNING_KEY_FILE`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mezgeb_engine::{Checkpoint, Signer, checkpoints, verify_chains};
use sqlx::PgConnection;

/// A parsed `checkpoint` or `key` command.
#[derive(Debug, PartialEq, Eq)]
pub enum ChainCommand {
    Export { dir: PathBuf },
    Generate { out: PathBuf },
}

pub const USAGE: &str = "       mezgeb-cli checkpoint export --dir DIR [--database-url URL]\n       \
     mezgeb-cli key generate --out FILE";

pub fn parse(group: &str, args: &[String]) -> Result<ChainCommand, String> {
    let usage = || format!("usage:\n{USAGE}");
    let mut args = args.iter();
    let verb = args.next().map(String::as_str);
    let mut path = None;
    while let Some(arg) = args.next() {
        match (verb, arg.as_str()) {
            (Some("export"), "--dir") | (Some("generate"), "--out") => {
                path = Some(PathBuf::from(args.next().ok_or_else(usage)?));
            }
            _ => return Err(format!("unknown argument {arg:?}\n{}", usage())),
        }
    }
    match (group, verb, path) {
        ("checkpoint", Some("export"), Some(dir)) => Ok(ChainCommand::Export { dir }),
        ("key", Some("generate"), Some(out)) => Ok(ChainCommand::Generate { out }),
        _ => Err(usage()),
    }
}

/// Reads the checkpoint copies in a directory: every `*.txt` file, one
/// checkpoint per line.
pub fn read_copies(dir: &Path) -> Result<Vec<Checkpoint>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            found.push(
                Checkpoint::from_line(line)
                    .ok_or_else(|| format!("{}: not a checkpoint: {line}", path.display()))?,
            );
        }
    }
    found.sort_by_key(|c| c.id);
    Ok(found)
}

/// Verifies the chains; `true` when clean.
pub async fn verify(conn: &mut PgConnection, copies: Option<&Path>) -> Result<bool, String> {
    let external = copies.map(read_copies).transpose()?.unwrap_or_default();
    let report = verify_chains(conn, &external)
        .await
        .map_err(|e| format!("chain verification failed to run: {e}"))?;
    print!("{report}");
    Ok(report.is_clean())
}

/// Runs a `checkpoint` command; `true` when nothing disagreed.
pub async fn export(conn: &mut PgConnection, dir: &Path) -> Result<(bool, String), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let mut out = String::new();
    let mut clean = true;
    for checkpoint in checkpoints(conn).await.map_err(|e| e.to_string())? {
        let path = dir.join(format!("checkpoint-{:06}.txt", checkpoint.id));
        let line = format!("{}\n", checkpoint.to_line());
        match std::fs::read_to_string(&path) {
            Ok(existing) if existing == line => {}
            Ok(_) => {
                clean = false;
                let _ = writeln!(
                    out,
                    "MISMATCH: {} differs from checkpoint {} in the database; left as it is",
                    path.display(),
                    checkpoint.id
                );
            }
            Err(_) => {
                std::fs::write(&path, &line)
                    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                let _ = writeln!(out, "wrote {}", path.display());
            }
        }
    }
    Ok((clean, out))
}

/// Writes a new signing key to `out`, which must not exist.
pub fn generate(out: &Path) -> Result<String, String> {
    let seed = Signer::generate_hex().map_err(|e| e.to_string())?;
    let signer = Signer::from_hex(&seed)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| format!("cannot create {} (it must not exist): {e}", out.display()))?;
    std::io::Write::write_all(&mut file, format!("{seed}\n").as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(format!(
        "wrote a new signing key to {}: key ID {}. Keep the file secret.\n",
        out.display(),
        signer.key_id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_chain_commands() {
        assert_eq!(
            parse("checkpoint", &args(&["export", "--dir", "out"])),
            Ok(ChainCommand::Export { dir: "out".into() })
        );
        assert_eq!(
            parse("key", &args(&["generate", "--out", "k.hex"])),
            Ok(ChainCommand::Generate {
                out: "k.hex".into()
            })
        );
        for (group, bad) in [
            ("checkpoint", &["export"][..]),
            ("checkpoint", &["export", "--out", "x"]),
            ("key", &["generate", "--dir", "x"]),
            ("key", &["rotate"]),
        ] {
            assert!(parse(group, &args(bad)).is_err(), "{group} {bad:?}");
        }
    }

    #[test]
    fn a_generated_key_file_is_never_overwritten() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("mezgeb-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("signing.hex");
        let _ = std::fs::remove_file(&path);
        generate(&path)?;
        let first = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        assert!(Signer::from_hex(&first).is_ok());
        assert!(
            generate(&path).is_err(),
            "an existing key is never overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(&path).map_err(|e| e.to_string())?,
            first
        );
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(())
    }
}
