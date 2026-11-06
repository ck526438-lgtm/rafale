use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

use regex::Regex;

use crate::lang::Language;
use crate::mask::Masked;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    /// Reachable from outside the crate/package/module: `pub`, `export`, Go `Capitalized`.
    Public,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fn,
    Method,
    /// Signature without a body: trait method, interface, abstract class.
    Abstract,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    /// Dotted scope: `impl MyType`, `class Parser`, ...
    pub container: String,
    pub start: usize,
    pub end: usize,
    pub params: usize,
    pub complexity: usize,
    pub depth: usize,
    pub vis: Vis,
    pub kind: Kind,
    pub is_test: bool,
    /// Treat as an entry point: reachable even when no call site is visible
    /// (`main`, dunders, and trait-impl methods dispatched by the compiler).
    pub is_root: bool,
    pub signature: String,
    /// Byte column of the name on the definition line; set by the linker pass.
    pub def_col: usize,
    pub size: usize,
    pub dup_key: u64,
    pub dup_size: usize,
}

impl Found {
    pub fn qualified(&self) -> String {
        if self.container.is_empty() {
            self.name.clone()
        } else {
            format!("{}::{}", self.container, self.name)
        }
    }
}

pub struct Src<'a> {
    pub lang: Language,
    pub masked: &'a Masked,
    pub raw: &'a [String],
    /// A Python package root (`__init__.py`) *is* the public API; other modules
    /// are ordinary files whose names are private by convention.
    pub is_init: bool,
}

static RE_RUST_FN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[(<]").unwrap());
static RE_GO_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bfunc\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap()
});
static RE_PY_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([ \t]*)(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap()
});
static RE_PY_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([ \t]*)class\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap());
static RE_JS_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bfunction\s*\*?\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\(").unwrap()
});
static RE_JS_ARROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*(?::[^=;{]+)?=\s*(?:async\s+)?(?:\([^)]*\)|[A-Za-z_$][A-Za-z0-9_$]*)\s*=>",
    )
    .unwrap()
});
static RE_JS_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*(?:static\s+|async\s+|\*|(?:public|private|protected|readonly|override|get|set)\s+)*([A-Za-z_$][A-Za-z0-9_$]*)\s*\([^)]*\)\s*(?::[^{;]+)?\{").unwrap()
});
static RE_IMPL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub\s+|unsafe\s+)*impl\s*(?:<[^>]*>\s*)?([A-Za-z0-9_:'<>\s]+?)(?:\s+for\s+([A-Za-z0-9_:'<>\s]+?))?\s*\{").unwrap()
});
static RE_TS_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bclass\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap());
static RE_JS_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bclass\s+([A-Za-z_$][A-Za-z0-9_$]*)").unwrap());
static RE_DECORATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*@([A-Za-z_][A-Za-z0-9_.]*)").unwrap());
static RE_DECISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(if|elif|for|while|case|catch|except|match|switch|finally)\b").unwrap()
});
static RE_PY_BOOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(and|or)\b").unwrap());

const JS_BLOCKED: &[&str] = &[
    "if", "for", "while", "switch", "catch", "return", "do", "else", "try", "with", "typeof",
    "function", "new", "delete", "void", "await", "yield",
];

/// Collapse identifiers and literals so structurally equal bodies hash together.
const KEYWORDS: &[&str] = &[
    "if", "else", "elif", "for", "while", "return", "break", "continue", "try", "except",
    "finally", "with", "fn", "func", "function", "let", "var", "const", "match", "case", "await",
    "yield", "async", "new", "delete", "throw", "raise", "pass", "defer", "go", "switch", "do",
    "in", "of", "true", "false", "None", "nil", "null", "undefined", "static", "struct", "class",
];

pub fn extract(src: &Src) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let mut container = String::new();
    let mut pending_test = false;
    let mut container_indent = usize::MAX;

    for i in 0..src.masked.lines.len() {
        let line = &src.masked.lines[i];
        if line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim_start();
        let mut found_here = false;

        match src.lang {
            Language::Python => {
                if let Some(caps) = RE_PY_CLASS.captures(line) {
                    container = format!("class {}", &caps[2]);
                    container_indent = caps[1].chars().count();
                    continue;
                }
                if trimmed.starts_with('@') {
                    if let Some(d) = RE_DECORATOR.captures(trimmed) {
                        let name = &d[1];
                        pending_test |= name.contains("test") || name.contains("fixture");
                    }
                    continue;
                }
                if let Some(caps) = RE_PY_DEF.captures(line) {
                    let indent = caps[1].chars().count();
                    if indent <= container_indent {
                        container.clear();
                    }
                    let name = caps[2].to_string();
                    if let Some(end) = python_body_end(src, i, indent) {
                        out.push(build(src, i, end, name, &container, pending_test));
                        found_here = true;
                    }
                }
            }
            Language::Rust => {
                if let Some(caps) = RE_IMPL.captures(line) {
                    container = match caps.get(2) {
                        Some(t) => format!("impl {} for {}", &caps[1], t.as_str()),
                        None => format!("impl {}", &caps[1]),
                    };
                }
                if trimmed.starts_with('#') {
                    pending_test |= is_entry_attr(trimmed);
                    continue;
                }
                if let Some(caps) = RE_RUST_FN.captures(line) {
                    let name = caps[1].to_string();
                    match body_shape(src, i) {
                        Shape::Braced(end) => {
                            out.push(build(src, i, end, name, &container, pending_test));
                            found_here = true;
                        }
                        Shape::Abstract => {
                            let mut f = build(src, i, i, name.clone(), &container, pending_test);
                            f.kind = Kind::Abstract;
                            out.push(f);
                            found_here = true;
                        }
                        Shape::None => {}
                    }
                }
            }
            Language::Go => {
                if let Some(caps) = RE_GO_FN.captures(line) {
                    let name = caps[1].to_string();
                    if let Shape::Braced(end) = body_shape(src, i) {
                        out.push(build(src, i, end, name, &container, pending_test));
                        found_here = true;
                    }
                }
            }
            Language::JavaScript | Language::TypeScript => {
                let cls = if src.lang == Language::TypeScript {
                    RE_TS_CLASS.captures(line)
                } else {
                    RE_JS_CLASS.captures(line)
                };
                if let Some(caps) = cls {
                    container = format!("class {}", &caps[1]);
                }
                let name = RE_JS_FN
                    .captures(line)
                    .map(|c| c[1].to_string())
                    .or_else(|| RE_JS_ARROW.captures(line).map(|c| c[1].to_string()))
                    .or_else(|| {
                        RE_JS_METHOD.captures(line).and_then(|c| {
                            let n = c[1].to_string();
                            if JS_BLOCKED.contains(&n.as_str()) {
                                return None;
                            }
                            // `setTimeout(function() {` is a call, not a method.
                            let matched = c.get(0).map(|m| m.as_str()).unwrap_or("");
                            if matched.contains("function") || matched.contains("=>") {
                                return None;
                            }
                            Some(n)
                        })
                    });
                if let Some(name) = name {
                    if let Some(end) = brace_or_arrow_end(src, i) {
                        out.push(build(src, i, end, name, &container, pending_test));
                        found_here = true;
                    }
                }
            }
        }

        if !found_here && !trimmed.starts_with('#') && !trimmed.starts_with('@') {
            pending_test = false;
        }
    }

    out.sort_by_key(|f| f.start);
    out.dedup_by(|a, b| a.start == b.start);
    out
}

enum Shape {
    Braced(usize),
    Abstract,
    None,
}

/// A signature terminated by `;` before any `{` is a declaration without a body.
fn body_shape(src: &Src, start: usize) -> Shape {
    for i in start..src.masked.lines.len() {
        let line = &src.masked.lines[i];
        if let Some(brace) = line.find('{') {
            if line[..brace].contains(';') {
                return Shape::Abstract;
            }
            return match brace_body_end(src, start) {
                Some(end) => Shape::Braced(end),
                None => Shape::None,
            };
        }
        if line.contains(';') {
            return Shape::Abstract;
        }
        if i - start > 12 {
            break;
        }
    }
    Shape::None
}

fn build(src: &Src, start: usize, end: usize, name: String, container: &str, pending_test: bool) -> Found {
    let end = end.min(src.masked.lines.len() - 1).max(start);
    let signature = src.raw.get(start).map(|s| s.trim().to_string()).unwrap_or_default();
    let body = src.masked.lines[start..=end].join("\n");
    let vis = match src.lang {
        Language::Rust => signature.contains("pub"),
        Language::Go => name.starts_with(char::is_uppercase),
        Language::JavaScript | Language::TypeScript => signature.trim_start().starts_with("export"),
        Language::Python => src.is_init && !name.starts_with('_'),
    };
    let line = src.masked.line(start);
    let nested = line.starts_with(' ') || line.starts_with('\t') || src.lang == Language::Python;
    let container = if nested { container } else { "" };
    let kind = if container.is_empty() { Kind::Fn } else { Kind::Method };
    let (dup_key, dup_size) = dup_fingerprint(&body);
    Found {
        complexity: complexity(&body, src.lang),
        depth: max_depth(src.lang, src.masked, start, end),
        params: param_count(&body),
        is_test: pending_test || is_test_name(&name, src.lang),
        is_root: is_root(src.lang, &name)
            || container.contains(" for ")
            || HARNESS_PARAMS.iter().any(|h| signature.contains(h)),
        name,
        container: container.to_string(),
        start,
        end,
        vis: if vis { Vis::Public } else { Vis::Private },
        kind,
        signature,
        def_col: 0,
        size: end - start + 1,
        dup_key,
        dup_size,
    }
}

fn is_root(lang: Language, name: &str) -> bool {
    match lang {
        Language::Rust | Language::Go | Language::JavaScript | Language::TypeScript => {
            name == "main"
        }
        Language::Python => name == "main" || (name.starts_with("__") && name.ends_with("__")),
    }
}

/// Attributes the test harness invokes by itself, so no call site exists.
fn is_entry_attr(line: &str) -> bool {
    const ENTRY: &[&str] = &["test", "bench", "fuzz", "quickcheck", "proptest", "test_case"];
    let body = line.trim_start_matches('#').trim_start_matches(['[', '!']);
    body.split(|c: char| !c.is_alphanumeric())
        .any(|tok| ENTRY.contains(&tok))
}

/// Benches registered through a harness macro (`criterion_group!`, `test::Bencher`).
const HARNESS_PARAMS: &[&str] = &["Bencher", "Criterion"];

fn is_test_name(name: &str, lang: Language) -> bool {
    match lang {
        Language::Python => name.starts_with("test_") || name.ends_with("_test"),
        Language::Rust => name.starts_with("test") || name.ends_with("_test"),
        Language::Go => name.starts_with("Test") || name.starts_with("Benchmark"),
        Language::JavaScript | Language::TypeScript => name.starts_with("test"),
    }
}

/// Count commas at paren depth 1 of the signature.
fn param_count(body: &str) -> usize {
    let Some(open) = body.find('(') else { return 0 };
    let mut depth = 0i32;
    let mut commas = 0;
    let mut non_empty = false;
    for c in body[open..].chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' if depth == 1 => commas += 1,
            _ if depth == 1 && !c.is_whitespace() => non_empty = true,
            _ => {}
        }
    }
    if non_empty {
        commas + 1
    } else {
        0
    }
}

fn complexity(body: &str, lang: Language) -> usize {
    let mut n = 1usize;
    n += RE_DECISION.find_iter(body).count();
    n += body.matches("&&").count() + body.matches("||").count();
    if lang == Language::Python {
        n += RE_PY_BOOL.find_iter(body).count();
    }
    n
}

fn max_depth(lang: Language, masked: &Masked, start: usize, end: usize) -> usize {
    if lang.uses_indent_blocks() {
        let base = indent_of(masked.line(start));
        let mut max = 0usize;
        for i in start..=end {
            let line = masked.line(i);
            if line.trim().is_empty() {
                continue;
            }
            max = max.max(indent_of(line).saturating_sub(base) / 4 + 1);
        }
        return max;
    }
    let mut depth = 0i32;
    let mut max = 0usize;
    for i in start..=end {
        for c in masked.line(i).chars() {
            match c {
                '{' => {
                    depth += 1;
                    max = max.max(depth as usize);
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
    }
    max.max(0) as usize
}

fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// Line closing the first `{` opened at or after `start`.
fn brace_body_end(src: &Src, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut opened = false;
    for i in start..src.masked.lines.len() {
        for c in src.masked.lines[i].chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return Some(i);
        }
    }
    None
}

/// Arrow functions may be a single expression ended by `;` or a blank line.
fn brace_or_arrow_end(src: &Src, start: usize) -> Option<usize> {
    if let Some(e) = brace_body_end(src, start) {
        return Some(e);
    }
    if src.masked.line(start).contains("=>") {
        for i in start..src.masked.lines.len().min(start + 40) {
            let line = src.masked.line(i);
            if line.contains(';') || line.trim().is_empty() {
                return Some(i);
            }
        }
        return Some(start);
    }
    None
}

fn python_body_end(src: &Src, start: usize, def_indent: usize) -> Option<usize> {
    let mut end = start;
    for i in start + 1..src.masked.lines.len() {
        let line = &src.masked.lines[i];
        if line.trim().is_empty() {
            continue;
        }
        if indent_of(line) <= def_indent {
            break;
        }
        end = i;
    }
    (end != start).then_some(end)
}

fn dup_fingerprint(body: &str) -> (u64, usize) {
    let mut norm = String::with_capacity(body.len());
    let mut word = String::new();
    for c in body.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut norm);
            if !c.is_whitespace() {
                norm.push(c);
            }
        }
    }
    flush(&mut word, &mut norm);
    let mut h = DefaultHasher::new();
    norm.hash(&mut h);
    (h.finish(), norm.chars().count())
}

fn flush(word: &mut String, norm: &mut String) {
    if word.is_empty() {
        return;
    }
    if KEYWORDS.iter().any(|k| *k == word.as_str()) {
        norm.push_str(word);
        norm.push(' ');
    } else {
        norm.push('_');
    }
    word.clear();
}
