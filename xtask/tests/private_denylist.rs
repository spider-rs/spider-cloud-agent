use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "spider-private-list-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn check(path: Option<&Path>, required: bool) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xtask"));
    cmd.args(["leakcheck", "--tree"])
        .env_remove("SPIDER_LEAKCHECK_WORDS");
    if required {
        cmd.arg("--require-private");
    }
    if let Some(path) = path {
        cmd.env("SPIDER_LEAKCHECK_WORDS", path);
    }
    cmd.output().unwrap()
}

#[test]
fn required_private_list_missing() {
    let output = check(None, true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("is required"));
    assert!(check(None, false).status.success());
}

#[test]
fn required_private_list_unreadable() {
    let fixture = Fixture::new();
    // A directory cannot be read as a text file, even under a privileged user.
    for path in [fixture.0.clone(), fixture.0.join("missing")] {
        let output = check(Some(&path), true);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read"));
    }
}

#[test]
fn required_private_list_empty() {
    let fixture = Fixture::new();
    let path = fixture.0.join("words");
    for contents in ["", "  \n# comments only\n"] {
        std::fs::write(&path, contents).unwrap();
        let output = check(Some(&path), true);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("at least one term"));
        assert!(check(Some(&path), false).status.success());
    }
}

#[test]
fn required_private_list_one_term() {
    let fixture = Fixture::new();
    let path = fixture.0.join("words");
    std::fs::write(&path, "test:synthetic-private-list-sentinel-58209\n").unwrap();
    let output = check(Some(&path), true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Break the gate's input and prove that the supplied list is actually used.
    std::fs::write(&path, "Spider\n").unwrap();
    assert!(!check(Some(&path), true).status.success());
}
