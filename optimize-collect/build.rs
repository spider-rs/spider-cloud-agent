//! Bakes the commit the collector was built from into `manifest.collector_rev`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A commit or a checkout moves HEAD's log, so the revision cannot go stale
    // across either. Both paths resolve inside a linked worktree too.
    for name in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(rev) = git(&["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=OPTIMIZE_COLLECT_REV={rev}");
    }
}
