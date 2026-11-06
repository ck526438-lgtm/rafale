use std::collections::BTreeMap;

use serde::Serialize;

use crate::detect::{Category, Config, Confidence, Finding};
use crate::graph::Graph;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
    Csv,
    Md,
}

impl Format {
    pub fn parse(s: &str) -> Option<Format> {
        match s.to_lowercase().as_str() {
            "human" | "term" => Some(Format::Human),
            "json" => Some(Format::Json),
            "csv" => Some(Format::Csv),
            "md" | "markdown" => Some(Format::Md),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Row {
    pub category: &'static str,
    pub confidence: &'static str,
    pub name: String,
    pub location: String,
    pub metric: String,
    pub why: String,
    pub action: &'static str,
    pub lines: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clones: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<String>,
}

pub fn rows(graph: &Graph, findings: &[Finding], advice: &[String]) -> Vec<Row> {
    findings
        .iter()
        .enumerate()
        .map(|(n, f)| Row {
            category: f.category.label(),
            confidence: f.confidence.label(),
            name: graph.name(f.func),
            location: graph.location(f.func),
            metric: f.metric.clone(),
            why: f.why.clone(),
            action: f.category.action(),
            lines: f.lines,
            clones: f.siblings.clone(),
            advice: advice.get(n).filter(|s| !s.is_empty()).cloned(),
        })
        .collect()
}

pub struct Totals {
    pub files: usize,
    pub lines: usize,
    pub funcs: usize,
    pub elapsed_ms: u128,
}

pub fn render(
    graph: &Graph,
    findings: &[Finding],
    advice: &[String],
    cfg: &Config,
    totals: &Totals,
    format: Format,
    color: bool,
) -> String {
    let rows = rows(graph, findings, advice);
    match format {
        Format::Json => serde_json::to_string_pretty(&serde_json::json!({
            "scan": {
                "files": totals.files,
                "lines": totals.lines,
                "functions": totals.funcs,
                "elapsed_ms": totals.elapsed_ms,
                "thresholds": {
                    "complexity": cfg.complexity,
                    "long": cfg.long,
                    "params": cfg.params,
                    "include_public": cfg.include_public,
                }
            },
            "findings": rows,
        }))
        .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
        Format::Csv => {
            let mut s = String::from("category,confidence,name,location,metric,lines,why,action\n");
            for r in &rows {
                s.push_str(&format!(
                    "{},{},{},{},{},{},\"{}\",\"{}\"\n",
                    r.category,
                    r.confidence,
                    csv_escape(&r.name),
                    r.location,
                    csv_escape(&r.metric),
                    r.lines,
                    csv_escape(&r.why),
                    csv_escape(r.action)
                ));
            }
            s
        }
        Format::Md => {
            let mut s = String::from("| confidence | category | location | name | metric | why |\n|---|---|---|---|---|---|\n");
            for r in &rows {
                s.push_str(&format!(
                    "| {} | {} | `{}` | `{}` | {} | {} |\n",
                    r.confidence, r.category, r.location, r.name, r.metric, r.why
                ));
            }
            s
        }
        Format::Human => human(graph, findings, advice, totals, color),
    }
}

fn csv_escape(s: &str) -> String {
    s.replace('"', "\"\"")
}

fn human(
    graph: &Graph,
    findings: &[Finding],
    advice: &[String],
    totals: &Totals,
    color: bool,
) -> String {
    let mut grouped: BTreeMap<Category, Vec<(usize, &Finding)>> = BTreeMap::new();
    for (n, f) in findings.iter().enumerate() {
        grouped.entry(f.category).or_default().push((n, f));
    }

    let mut s = String::new();
    let head = format!(
        "{} files · {} lines · {} functions in {} ms",
        totals.files, totals.lines, totals.funcs, totals.elapsed_ms
    );
    s.push_str(&style(&head, "1;2", color));
    s.push('\n');

    if findings.is_empty() {
        s.push_str(&style(
            "  nothing above the configured thresholds",
            "32",
            color,
        ));
        s.push('\n');
        return s;
    }

    for (category, list) in &grouped {
        let wasted: usize = list.iter().map(|(_, f)| f.lines).sum();
        let title = format!(
            "\n{} · {} findings · {} lines at stake",
            category.label().to_uppercase(),
            list.len(),
            wasted
        );
        s.push_str(&style(&title, "1", color));
        s.push('\n');
        s.push_str(&style(&format!("  → {}\n", category.action()), "2", color));
        for (n, f) in list {
            let loc = graph.location(f.func);
            let name = graph.name(f.func);
            let (label, code) = match f.confidence {
                Confidence::High => ("HIGH", "1;31"),
                Confidence::Medium => ("MED ", "33"),
                Confidence::Low => ("LOW ", "2"),
            };
            s.push_str(&format!(
                "  {} {:<44} {:<30} {}\n",
                style(label, code, color),
                truncate(&name, 44),
                loc,
                f.metric
            ));
            s.push_str(&style(&format!("       {}\n", f.why), "2", color));
            for sib in &f.siblings {
                s.push_str(&format!("       clone of {sib}\n"));
            }
            if let Some(a) = advice.get(*n) {
                if !a.is_empty() {
                    s.push_str(&format!("\n{}\n", indent(a, 5)));
                }
            }
        }
    }
    s
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn style(text: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
