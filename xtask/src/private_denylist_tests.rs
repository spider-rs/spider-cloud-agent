// Keep these tests in the unit test executable. A shared Cargo target directory's
// un-hashed binary path can be replaced by another checkout while tests run.
#![allow(clippy::unwrap_used)]

use std::path::Path;
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

fn check(path: Option<&Path>, required: bool) -> Result<Vec<super::Word>, String> {
    super::load_from(path, required)
}

#[test]
fn required_private_list_missing() {
    let output = check(None, true);
    assert!(output.unwrap_err().contains("is required"));
    assert!(check(None, false).is_ok());
}

#[test]
fn required_private_list_unreadable() {
    let fixture = Fixture::new();
    // A directory cannot be read as a text file, even under a privileged user.
    for path in [fixture.0.clone(), fixture.0.join("missing")] {
        let output = check(Some(&path), true);
        assert!(output.unwrap_err().contains("cannot read"));
    }
}

#[test]
fn required_private_list_empty() {
    let fixture = Fixture::new();
    let path = fixture.0.join("words");
    for contents in ["", "  \n# comments only\n"] {
        std::fs::write(&path, contents).unwrap();
        let output = check(Some(&path), true);
        assert!(output.unwrap_err().contains("at least one term"));
        assert!(check(Some(&path), false).is_ok());
    }
}

#[test]
fn required_private_list_one_term() {
    let fixture = Fixture::new();
    let path = fixture.0.join("words");
    std::fs::write(&path, "test:synthetic-private-list-sentinel-58209\n").unwrap();
    let words = check(Some(&path), true).unwrap();
    assert!(words
        .iter()
        .any(|word| word.term == "synthetic-private-list-sentinel-58209"));
    // A loaded private term must produce a finding in the same matcher as public terms.
    std::fs::write(&path, "Spider\n").unwrap();
    let words = check(Some(&path), true).unwrap();
    let word = words.iter().find(|word| word.term == "spider").unwrap();
    assert_eq!(super::find_term("Spider", &word.term), vec![0]);
}
