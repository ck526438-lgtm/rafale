use std::collections::HashMap;

use crate::extract::{Kind, Vis};
use crate::graph::Graph;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "med",
            Confidence::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Confidence> {
        match s.to_lowercase().as_str() {
            "high" => Some(Confidence::High),
            "med" | "medium" => Some(Confidence::Medium),
            "low" => Some(Confidence::Low),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Dead,
    Duplicate,
    Complex,
    Long,
    Params,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Dead => "dead code",
            Category::Duplicate => "duplicated logic",
            Category::Complex => "high complexity",
            Category::Long => "oversized",
            Category::Params => "too many params",
        }
    }

    pub fn action(self) -> &'static str {
        match self {
            Category::Dead => "delete it, or wire it back in",
            Category::Duplicate => "extract one shared implementation",
            Category::Complex => "split into named helpers, drop a level of nesting",
            Category::Long => "split along its natural seams",
            Category::Params => "group the params into a struct/options object",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub func: usize,
    pub category: Category,
    pub confidence: Confidence,
    pub metric: String,
    pub why: String,
    /// Lines this finding puts back on the table.
    pub lines: usize,
    pub siblings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub complexity: usize,
    pub long: usize,
    pub params: usize,
    pub dup_min: usize,
    pub include_public: bool,
    pub include_tests: bool,
    pub min_confidence: Confidence,
    pub max_hits: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            complexity: 12,
            long: 80,
            params: 6,
            dup_min: 40,
            include_public: false,
            include_tests: false,
            min_confidence: Confidence::Medium,
            max_hits: 40,
        }
    }
}

pub fn detect(graph: &Graph, cfg: &Config) -> Vec<Finding> {
    // Without --include-public, exported items are treated as roots: an external
    // consumer may call them, and that guess is worth more than a false positive.
    let reachable = crate::graph::reachable(graph, !cfg.include_public);
    let mut out = Vec::new();

    for i in 0..graph.funcs.len() {
        let f = &graph.funcs[i].f;
        if !cfg.include_tests && f.is_test {
            continue;
        }
        if f.kind == Kind::Abstract {
            continue;
        }

        if !reachable[i] {
            let refs = graph.ext_refs[i];
            let confidence = match (refs, f.vis) {
                (0, Vis::Private) => Confidence::High,
                (0, Vis::Public) => Confidence::Medium,
                (_, Vis::Private) => Confidence::Medium,
                (_, Vis::Public) => Confidence::Low,
            };
            let why = if refs == 0 {
                "no reference anywhere in the repo".to_string()
            } else {
                format!("only referenced from {} other unreachable function(s)", refs)
            };
            out.push(Finding {
                func: i,
                category: Category::Dead,
                confidence,
                metric: format!("{} lines", f.size),
                why,
                lines: f.size,
                siblings: Vec::new(),
            });
        }

        if f.complexity >= cfg.complexity || f.depth >= 5 {
            out.push(Finding {
                func: i,
                category: Category::Complex,
                confidence: if f.complexity >= cfg.complexity * 2 { Confidence::High } else { Confidence::Medium },
                metric: format!("CC {}, depth {}, {} lines", f.complexity, f.depth, f.size),
                why: "hard to test and to follow: branch count above threshold".to_string(),
                lines: f.size,
                siblings: Vec::new(),
            });
        } else if f.size >= cfg.long {
            out.push(Finding {
                func: i,
                category: Category::Long,
                confidence: Confidence::Medium,
                metric: format!("{} lines", f.size),
                why: "single function doing more than one screenful of work".to_string(),
                lines: f.size,
                siblings: Vec::new(),
            });
        }

        if f.params >= cfg.params {
            out.push(Finding {
                func: i,
                category: Category::Params,
                confidence: Confidence::Medium,
                metric: format!("{} params", f.params),
                why: "call sites become positional and error-prone".to_string(),
                lines: 0,
                siblings: Vec::new(),
            });
        }
    }

    out.extend(duplicates(graph, cfg));
    out.retain(|f| f.confidence >= cfg.min_confidence);
    out.truncate(cfg.max_hits);
    // Capping the whole list would let one category swallow the others: on a real
    // repo the dead functions alone exceed any sane limit and hide every clone.
    let mut per_category = [0usize; 5];
    out.sort_by(|a, b| {
        (b.category == Category::Dead).cmp(&(a.category == Category::Dead))
            .then(b.confidence.cmp(&a.confidence))
            .then(b.lines.cmp(&a.lines))
            .then(a.func.cmp(&b.func))
    });
    out.retain(|f| {
        let slot = &mut per_category[f.category as usize];
        *slot += 1;
        *slot <= cfg.max_hits
    });
    out}

fn duplicates(graph: &Graph, cfg: &Config) -> Vec<Finding> {
    let mut groups: HashMap<(u64, usize), Vec<usize>> = HashMap::new();
    for i in 0..graph.funcs.len() {
        let f = &graph.funcs[i].f;
        if f.dup_size >= cfg.dup_min {
            groups.entry((f.dup_key, f.dup_size)).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for (_, members) in groups {
        if members.len() < 2 {
            continue;
        }
        let names: Vec<String> = members
            .iter()
            .map(|i| format!("{} ({})", graph.name(*i), graph.location(*i)))
            .collect();
        for (rank, &member) in members.iter().enumerate() {
            let others: Vec<String> = names
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != rank)
                .map(|(_, n)| n.clone())
                .collect();
            out.push(Finding {
                func: member,
                category: Category::Duplicate,
                confidence: if others.len() > 1 { Confidence::High } else { Confidence::Medium },
                metric: format!("{} clones", others.len() + 1),
                why: "identical control flow after renaming identifiers".to_string(),
                lines: graph.funcs[member].f.size,
                siblings: others,
            });
        }
    }
    out
}
