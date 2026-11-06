//! Repo walker with a `.gitignore` subset. Written by hand so the binary keeps
//! zero Windows-API and zero C dependencies: `ignore`/`walkdir` would drag
//! `windows-sys` in, and that alone requires a full MinGW toolchain to link.
use std::path::{Path, PathBuf};

use crate::lang::Language;

/// Directories that are never worth reading.
pub const BLACKLIST: &[&str] = &[
    "node_modules", "target", "dist", "build", "out", "vendor", ".git", "__pycache__", ".venv",
    "venv", "coverage", ".next", ".nuxt", ".cache", ".cargo", "pod_modules", "DerivedData",
    ".terraform", "bower_components",
];
const SKIP_SUFFIX: &[&str] = &[
    ".min.js", ".min.css", ".lock", "_pb.go", ".pb.", ".d.ts", ".map.", ".generated.",
];
pub const MAX_FILE_BYTES: u64 = 1_500_000;
const MAX_DEPTH: usize = 48;

#[derive(Debug, Clone)]
struct Rule {
    pattern: String,
    dir_only: bool,
    negated: bool,
    /// Anchored patterns are matched from the ignore file's directory.
    anchored: bool,
}

#[derive(Debug, Default)]
struct Scope {
    dir: PathBuf,
    rules: Vec<Rule>,
}

pub fn collect(roots: &[PathBuf], excludes: &[String]) -> Vec<(PathBuf, String, Language)> {
    let mut found = Vec::new();
    for root in roots {
        if root.is_file() {
            if let Some(lang) = root.to_str().and_then(Language::from_path) {
                found.push((root.clone(), root.display().to_string(), lang));
            }
            continue;
        }
        if !root.is_dir() {
            continue;
        }
        let mut scopes = vec![Scope::default()];
        walk(root, root, excludes, &mut scopes, &mut found, 0);
    }
    found.sort_by(|a, b| a.1.cmp(&b.1));
    found.dedup_by(|a, b| a.0 == b.0);
    found
}

fn walk(
    dir: &Path,
    root: &Path,
    excludes: &[String],
    scopes: &mut Vec<Scope>,
    found: &mut Vec<(PathBuf, String, Language)>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let owned = read_ignore(dir);
    let pushed = !owned.rules.is_empty();
    if pushed {
        scopes.push(owned);
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            if pushed {
                scopes.pop();
            }
            return;
        }
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = path.is_dir();
        let rel = relative(&path, root);
        if is_ignored(scopes, &rel, is_dir) {
            continue;
        }
        if !is_dir && keep_file(&rel, &name, excludes) {
            if let Some(lang) = Language::from_path(&name) {
                if path
                    .metadata()
                    .map(|m| m.len() <= MAX_FILE_BYTES)
                    .unwrap_or(false)
                {
                    found.push((path, rel.clone(), lang));
                }
            }
        } else if is_dir {
            dirs.push(path);
        }
    }
    for sub in dirs {
        walk(&sub, root, excludes, scopes, found, depth + 1);
    }
    if pushed {
        scopes.pop();
    }
}

fn keep_file(rel: &str, name: &str, excludes: &[String]) -> bool {
    let lower = rel.to_lowercase();
    if SKIP_SUFFIX.iter().any(|s| name.to_lowercase().ends_with(s) || lower.contains(s)) {
        return false;
    }
    if lower.contains("/testdata/") || lower.ends_with(".min.js") {
        return false;
    }
    !excludes.iter().any(|e| lower.contains(&e.to_lowercase()))
}

fn read_ignore(dir: &Path) -> Scope {
    let mut scope = Scope {
        dir: dir.to_path_buf(),
        rules: Vec::new(),
    };
    let Ok(text) = std::fs::read_to_string(dir.join(".gitignore")) else {
        return scope;
    };
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (negated, body) = match line.strip_prefix('!') {
            Some(rest) => (true, rest.to_string()),
            None => (false, line.to_string()),
        };
        let (dir_only, body) = match body.strip_suffix('/') {
            Some(rest) => (true, rest.to_string()),
            None => (false, body),
        };
        if body.is_empty() {
            continue;
        }
        let anchored = body.contains('/') || body.starts_with("**/");
        let pattern = body.trim_start_matches('/').to_string();
        scope.rules.push(Rule {
            pattern,
            dir_only,
            negated,
            anchored,
        });
    }
    scope
}

fn is_ignored(scopes: &[Scope], rel: &str, is_dir: bool) -> bool {
    if rel.is_empty() {
        return false;
    }
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    if is_dir && BLACKLIST.iter().any(|d| *d == name) {
        return true;
    }
    let mut ignored = false;
    for scope in scopes {
        for rule in &scope.rules {
            if rule.dir_only && !is_dir {
                continue;
            }
            let rel_from_scope = strip_prefix(rel, &scope.dir);
            if matches(rule, &rel_from_scope, name) {
                ignored = !rule.negated;
            }
        }
    }
    ignored
}

fn strip_prefix(rel: &str, scope_dir: &Path) -> String {
    let base = scope_dir.to_string_lossy().replace('\\', "/");
    let flat = rel.replace('\\', "/");
    if base.is_empty() || base == "." {
        return flat;
    }
    match flat.strip_prefix(&format!("{base}/")) {
        Some(rest) => rest.to_string(),
        None => flat,
    }
}

fn matches(rule: &Rule, rel: &str, name: &str) -> bool {
    let rel = rel.trim_start_matches("./");
    if rule.anchored {
        return glob(&rule.pattern, rel);
    }
    glob(&rule.pattern, name)
        || glob(&format!("**/{}", rule.pattern), rel)
        || rel
            .split('/')
            .any(|part| glob(&rule.pattern, part))
}

/// Git-style glob over path segments, with `**` spanning directories.
fn glob(pattern: &str, text: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let txt: Vec<&str> = text.split('/').filter(|s| !s.is_empty()).collect();
    seg_match(&pat, &txt)
}

fn seg_match<'a>(pat: &[&'a str], txt: &[&'a str]) -> bool {
    match pat.first() {
        None => txt.is_empty(),
        Some(&"**") => {
            if seg_match(&pat[1..], txt) {
                return true;
            }
            !txt.is_empty() && seg_match(pat, &txt[1..])
        }
        Some(&head) => !txt.is_empty() && segment(head, txt[0]) && seg_match(&pat[1..], &txt[1..]),
    }
}

/// Wildcard matching inside one path segment: `*`, `?`, `[...]`.
fn segment(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    wild(&p, 0, &t, 0)
}

fn wild(p: &[char], mut pi: usize, t: &[char], mut ti: usize) -> bool {
    while pi < p.len() {
        match p[pi] {
            '*' => {
                for skip in ti..=t.len() {
                    if wild(p, pi + 1, t, skip) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                if ti >= t.len() {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
            '[' => {
                let Some(end) = p[pi + 1..].iter().position(|&x| x == ']') else {
                    return false;
                };
                let body = &p[pi + 1..pi + 1 + end];
                let (negate, body) = match body.first() {
                    Some('!') | Some('^') => (true, &body[1..]),
                    _ => (false, body),
                };
                if ti >= t.len() {
                    return false;
                }
                let hit = class_hit(body, t[ti]);
                if hit == negate {
                    return false;
                }
                pi += end + 2;
                ti += 1;
            }
            c => {
                if ti >= t.len() || t[ti] != c {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }
    ti == t.len()
}

fn class_hit(body: &[char], c: char) -> bool {
    let mut i = 0;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == '-' {
            if body[i] <= c && c <= body[i + 2] {
                return true;
            }
            i += 3;
        } else {
            if body[i] == c {
                return true;
            }
            i += 1;
        }
    }
    false
}

fn relative(path: &Path, root: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let r = root.to_string_lossy().replace('\\', "/");
    let r = r.trim_end_matches('/').to_string();
    if !r.is_empty() {
        if let Some(rest) = p.strip_prefix(&r) {
            let rest = rest.trim_start_matches('/');
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    match path.file_name().map(|n| n.to_string_lossy().to_string()) {
        Some(name) => name,
        None => p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_matches_within_a_segment() {
        assert!(segment("*.min.js", "app.min.js"));
        assert!(!segment("*.js", "app.ts"));
        assert!(!segment("?ol.dart", "red.dart"));
        assert!(segment("te?t", "test"));
    }

    #[test]
    fn double_star_spans_directories() {
        assert!(glob("**/generated/*", "a/b/generated/gen.rs"));
        assert!(glob("src/**/mod.rs", "src/deep/deeper/mod.rs"));
        assert!(!glob("**/build", "src/builds"));
    }

    #[test]
    fn classes_and_ranges() {
        assert!(segment("[ab]c", "ac"));
        assert!(segment("[!0-9]x", "ax"));
        assert!(segment("[0-9]x", "5x"));
        assert!(!segment("[0-9]x", "ax"));
    }

    #[test]
    fn rule_matching_follows_git_semantics() {
        let anchored = Rule {
            pattern: "src/gen.rs".into(),
            dir_only: false,
            negated: false,
            anchored: true,
        };
        assert!(matches(&anchored, "src/gen.rs", "gen.rs"));
        assert!(!matches(&anchored, "other/src/gen.rs", "gen.rs"));

        let loose = Rule {
            pattern: "*.log".into(),
            dir_only: false,
            negated: false,
            anchored: false,
        };
        assert!(matches(&loose, "deep/dir/app.log", "app.log"));
    }
}
