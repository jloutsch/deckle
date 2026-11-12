// SPDX-License-Identifier: GPL-3.0-or-later
//! No state file is loaded with "missing or unparseable means default" (state-file safety
//! design): such a load makes the next save write the default over the owner's file. State files
//! go through `deckle_limits::state_file` instead.

use std::path::{Path, PathBuf};

/// Silent loads that may stay, as `crates/…` paths: none of them is ever saved over.
const ALLOWED: &[&str] = &[];

/// The shapes the pattern cannot match, listed by hand: the file and a regex for the old load.
const BY_HAND: &[(&str, &str)] = &[
    // `match fs::read … unwrap_or_else(|_| { …; DaemonState::default() })`
    (
        "crates/deckle-cli/src/daemon.rs",
        r"(?s)match std::fs::read\(path\)[^;]{0,200}?unwrap_or_else",
    ),
    // `if let Ok(t) = fs::read_to_string(…)`, then a new token over whatever was there.
    (
        "crates/deckle-digest/src/bookmarklet.rs",
        r"if let Ok\(\w+\) = std::fs::read_to_string",
    ),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        if p.is_dir() {
            if name != "target" && name != "node_modules" && name != "tests" {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `path:line` of each silent load in the workspace's crates and the desktop app.
fn silent_loads(root: &Path) -> Vec<String> {
    // rustfmt splits the chain over lines, so a file is searched whole; a load spans at most
    // five lines after the read.
    let chain = regex::Regex::new(
        r"(?s)(fs::read|read_to_string)\b[^;]*?\.ok\(\)[^;]*?(from_slice|from_str)[^;]*?unwrap_or_default",
    )
    .unwrap();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    rust_files(&root.join("apps/desktop/src-tauri/src"), &mut files);
    files.sort();
    let mut found = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let rel = f
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for m in chain.find_iter(&text) {
            if m.as_str().lines().count() > 6 {
                continue;
            }
            let line = text[..m.start()].matches('\n').count() + 1;
            if !ALLOWED.contains(&rel.as_str()) {
                found.push(format!("{rel}:{line}"));
            }
        }
    }
    for (file, old) in BY_HAND {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        if let Some(m) = regex::Regex::new(old).unwrap().find(&text) {
            let line = text[..m.start()].matches('\n').count() + 1;
            found.push(format!("{file}:{line}"));
        }
    }
    found
}

#[test]
fn no_state_file_is_loaded_with_unreadable_means_default() {
    let found = silent_loads(&workspace());
    assert!(
        found.is_empty(),
        "{} state file load(s) treat a file that does not parse as empty, and the next save \
         replaces it. Use deckle_limits::state_file::load (see the state-file safety design):\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn the_pattern_finds_a_load_split_over_lines() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("crates/x/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        "fn load(path: &Path) -> State {\n    std::fs::read(path)\n        .ok()\n        \
         .and_then(|b| serde_json::from_slice(&b).ok())\n        .unwrap_or_default()\n}\n",
    )
    .unwrap();
    for (file, _) in BY_HAND {
        std::fs::create_dir_all(dir.path().join(file).parent().unwrap()).unwrap();
        std::fs::write(dir.path().join(file), "").unwrap();
    }
    assert_eq!(silent_loads(dir.path()), ["crates/x/src/lib.rs:2"]);
}
