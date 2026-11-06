use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::path::PathBuf;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use rayon::prelude::*;

use crate::extract::{self, Found, Kind, Src, Vis};
use crate::lang::Language;
use crate::mask;
use crate::walk;

#[derive(Debug)]
pub struct Unit {
    pub path: PathBuf,
    /// Repo-relative path shown in reports.
    pub display: String,
    pub lang: Language,
}

#[derive(Debug)]
pub struct Func {
    pub file: usize,
    pub f: Found,
}

#[derive(Debug, Default)]
pub struct Graph {
    pub units: Vec<Unit>,
    pub funcs: Vec<Func>,
    /// caller -> callees, deduplicated.
    pub calls: Vec<Vec<usize>>,
    /// Referenced at file/module scope: import, registration, top-level call.
    pub module_ref: Vec<bool>,
    /// Occurrences outside the function's own signature and body.
    pub ext_refs: Vec<usize>,
    pub files_scanned: usize,
    pub lines_scanned: usize,
    pub funcs_total: usize,
}

impl Graph {
    pub fn location(&self, i: usize) -> String {
        let f = &self.funcs[i];
        format!("{}:{}", self.units[f.file].display, f.f.start + 1)
    }

    pub fn name(&self, i: usize) -> String {
        self.funcs[i].f.qualified()
    }

    pub fn path(&self, i: usize) -> &Path {
        &self.units[self.funcs[i].file].path
    }
}

pub fn collect(roots: &[PathBuf], excludes: &[String]) -> Vec<(PathBuf, String, Language)> {
    walk::collect(roots, excludes)
}

/// Pass 1 — read every file once and extract function definitions.
pub fn parse(list: Vec<(PathBuf, String, Language)>) -> Graph {
    let parsed: Vec<Option<(Unit, Vec<Found>, usize)>> = list
        .into_par_iter()
        .map(|(path, display, lang)| {
            let Ok(raw) = std::fs::read_to_string(&path) else { return None };
            if raw.trim().is_empty() {
                return None;
            }
            let masked = mask::mask(&raw, lang);
            let lines = masked.lines.len();
            let raw_lines: Vec<String> = raw.lines().map(|l| l.to_string()).collect();
            let src = Src {
                lang,
                masked: &masked,
                raw: &raw_lines,
                is_init: display.ends_with("__init__.py"),
            };
            let mut funcs = extract::extract(&src);
            for f in funcs.iter_mut() {
                f.def_col = masked.lines[f.start].find(&f.name).unwrap_or(0);
            }
            Some((Unit { path, display, lang }, funcs, lines))
        })
        .collect();

    let mut graph = Graph::default();
    for entry in parsed.into_iter().flatten() {
        let (unit, funcs, lines) = entry;
        let file = graph.units.len();
        graph.lines_scanned += lines;
        for f in funcs {
            graph.funcs.push(Func { file, f });
        }
        graph.units.push(unit);
    }
    graph.funcs_total = graph.funcs.len();
    graph.files_scanned = graph.units.len();
    graph.funcs.sort_by_key(|f| (f.file, f.f.start));
    graph
}

/// Pass 2 — attribute every name occurrence to the function that contains it.
pub fn link(graph: &mut Graph, list: &[(PathBuf, String, Language)]) {
    let names: Vec<String> = graph
        .funcs
        .iter()
        .map(|f| f.f.name.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if names.is_empty() {
        return;
    }
    let ac: AhoCorasick = match AhoCorasickBuilder::new()
        .match_kind(MatchKind::LeftmostLongest)
        .build(&names)
    {
        Ok(ac) => ac,
        Err(e) => {
            eprintln!("rafale: cannot build the name automaton: {e}");
            return;
        }
    };

    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, f) in graph.funcs.iter().enumerate() {
        by_name.entry(f.f.name.as_str()).or_default().push(i);
    }

    graph.calls = vec![Vec::new(); graph.funcs.len()];
    graph.module_ref = vec![false; graph.funcs.len()];
    graph.ext_refs = vec![0; graph.funcs.len()];

    // Index by unit so pass 2 can look up ranges for the file being scanned.
    let mut by_unit: Vec<Vec<usize>> = vec![Vec::new(); graph.units.len()];
    for (i, f) in graph.funcs.iter().enumerate() {
        by_unit[f.file].push(i);
    }
    let unit_of: HashMap<String, usize> = graph
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.path.to_string_lossy().to_string(), i))
        .collect();

    for (path, _display, lang) in list {
        let Some(&u) = unit_of.get(&path.to_string_lossy().to_string()) else { continue };
        let Ok(raw) = std::fs::read_to_string(path) else { continue };
        let masked = mask::mask(&raw, *lang);
        let joined = masked.lines.join("\n");
        let mut line_start = Vec::with_capacity(masked.lines.len());
        let mut acc = 0usize;
        for line in &masked.lines {
            line_start.push(acc);
            acc += line.len() + 1;
        }
        let ranges = &by_unit[u];

        for m in ac.find_iter(&joined) {
            let text = &joined[m.start()..m.end()];
            if !word_boundary(&joined, m.start(), m.end()) {
                continue;
            }
            let line = line_at(&line_start, m.start());
            let col = m.start() - line_start[line];
            let owner = owner_of(ranges, graph, line);

            match owner {
                Some(outer) => {
                    let of = &graph.funcs[outer];
                    if of.f.start == line && of.f.def_col == col && text == of.f.name {
                        continue; // the name inside its own signature
                    }
                    for &cand in &by_name[text] {
                        if cand == outer {
                            continue; // recursion is not a use
                        }
                        if !graph.calls[outer].contains(&cand) {
                            graph.calls[outer].push(cand);
                        }
                        graph.ext_refs[cand] += 1;
                    }
                }
                None => {
                    for &cand in &by_name[text] {
                        graph.module_ref[cand] = true;
                        graph.ext_refs[cand] += 1;
                    }
                }
            }
        }
    }
}

fn owner_of(ranges: &[usize], graph: &Graph, line: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut best_span = usize::MAX;
    for &i in ranges {
        let f = &graph.funcs[i].f;
        if f.start <= line && line <= f.end {
            let span = f.end - f.start;
            if span < best_span {
                best_span = span;
                best = Some(i);
            }
        }
    }
    best
}

fn line_at(line_start: &[usize], byte: usize) -> usize {
    match line_start.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

fn word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    // `:` stays a valid neighbour: `Self::build`, `glob::new` and `mod.fn` are
    // exactly the call sites a Rust/Go project uses.
    !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Mark everything reachable from entry points, tests, exported API and module scope.
pub fn reachable(graph: &Graph, public_is_root: bool) -> Vec<bool> {
    let mut seen = vec![false; graph.funcs.len()];
    let mut queue = VecDeque::new();
    for i in 0..graph.funcs.len() {
        let f = &graph.funcs[i].f;
        let root = f.is_root
            || f.is_test
            || f.kind == Kind::Abstract
            || graph.module_ref[i]
            || (public_is_root && f.vis == Vis::Public);
        if root && !seen[i] {
            seen[i] = true;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        for &callee in &graph.calls[i] {
            if !seen[callee] {
                seen[callee] = true;
                queue.push_back(callee);
            }
        }
    }
    seen
}
