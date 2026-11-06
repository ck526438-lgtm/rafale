use std::collections::BTreeSet;
use std::path::PathBuf;

use rafale::detect::{self, Category, Config, Confidence};
use rafale::graph;

fn run() -> (graph::Graph, Vec<detect::Finding>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/sample");
    let list = graph::collect(&[root], &[]);
    assert!(!list.is_empty(), "fixture files were not collected");
    let mut graph = graph::parse(list.clone());
    graph::link(&mut graph, &list);
    let cfg = Config {
        complexity: 6,
        max_hits: 200,
        min_confidence: Confidence::Low,
        ..Default::default()
    };
    let findings = detect::detect(&graph, &cfg);
    (graph, findings)
}

fn names(graph: &graph::Graph, findings: &[detect::Finding], category: Category) -> BTreeSet<String> {
    findings
        .iter()
        .filter(|f| f.category == category)
        .map(|f| graph.funcs[f.func].f.name.clone())
        .collect()
}

#[test]
fn finds_dead_functions_in_every_language() {
    let (graph, findings) = run();
    let dead = names(&graph, &findings, Category::Dead);
    for expected in ["ghost_one", "ghost_two", "orphan_alpha", "orphan_beta", "legacyFormatter"] {
        assert!(dead.contains(expected), "{expected} missing from {dead:?}");
    }
}

#[test]
fn does_not_flag_live_code() {
    let (graph, findings) = run();
    let dead = names(&graph, &findings, Category::Dead);
    for live in [
        "used_helper",
        "public_entry",
        "normalize",
        "greet",
        "render",
        "main",
        "totalCents",
        "totalEuros",
        "tangle",
    ] {
        assert!(!dead.contains(live), "{live} wrongly reported as dead");
    }
}

#[test]
fn names_in_comments_and_strings_do_not_count_as_uses() {
    let (graph, findings) = run();
    let dead = names(&graph, &findings, Category::Dead);
    assert!(dead.contains("ghost_one"));
    assert!(dead.contains("ghost_two"));
}

#[test]
fn finds_clones() {
    let (graph, findings) = run();
    let dup = names(&graph, &findings, Category::Duplicate);
    assert!(dup.contains("totalCents"), "clones: {dup:?}");
    assert!(dup.contains("totalEuros"), "clones: {dup:?}");
}

#[test]
fn finds_the_hairball() {
    let (graph, findings) = run();
    let complex = names(&graph, &findings, Category::Complex);
    assert!(complex.contains("tangle"), "complex: {complex:?}");
}

#[test]
fn harness_entry_points_are_not_dead() {
    let (graph, findings) = run();
    let dead = names(&graph, &findings, Category::Dead);
    for harness in ["bench_the_helper", "bench_only"] {
        assert!(!dead.contains(harness), "{harness} wrongly reported as dead: {dead:?}");
    }
}
