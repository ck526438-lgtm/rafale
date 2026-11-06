use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;

use rafale::detect::{self, Confidence, Config};
use rafale::graph;
use rafale::llm::Ollama;
use rafale::report::{self, Format, Totals};

#[derive(Parser)]
#[command(
    name = "rafale",
    version,
    about = "Find dead and costly functions in a repo, then optionally get a refactoring plan from a local Ollama model."
)]
struct Args {
    /// Directories or files to scan.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,

    /// Report format: human, json, csv, md.
    #[arg(long, default_value = "human")]
    format: String,

    /// Lowest confidence to report: low, med, high.
    #[arg(long, default_value = "med")]
    min_confidence: String,

    /// Also analyse `pub`/`export`/Capitalized functions (may have external callers).
    #[arg(long)]
    include_public: bool,

    /// Include test functions in the analysis.
    #[arg(long)]
    include_tests: bool,

    /// Cyclomatic complexity threshold.
    #[arg(long, default_value_t = 12)]
    complexity: usize,

    /// Function length threshold, in lines.
    #[arg(long, default_value_t = 80)]
    long: usize,

    /// Parameter count threshold.
    #[arg(long, default_value_t = 6)]
    params: usize,

    /// Minimum normalized size for duplicate detection.
    #[arg(long, default_value_t = 40)]
    dup_min: usize,

    /// Cap the number of findings kept per category.
    #[arg(long, default_value_t = 40)]
    max_hits: usize,

    /// Skip any path containing this fragment. Repeatable.
    #[arg(long = "exclude")]
    excludes: Vec<String>,

    /// Ask Ollama for a refactoring plan on the top findings.
    #[arg(long)]
    suggest: bool,

    /// Ollama model to use. Falls back to $OLLAMA_MODEL.
    #[arg(long)]
    model: Option<String>,

    /// Ollama endpoint. Falls back to $OLLAMA_URL.
    #[arg(long)]
    url: Option<String>,

    /// How many findings to send to the model, spread across categories.
    #[arg(long, default_value_t = 5)]
    top: usize,

    /// Per-request generation timeout, in seconds.
    #[arg(long, default_value_t = 120)]
    timeout: u64,

    /// Disable ANSI colors.
    #[arg(long)]
    no_color: bool,

    /// Exit with code 2 when at least one finding is reported (CI gate).
    #[arg(long)]
    fail_on_findings: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let started = Instant::now();

    let Some(format) = Format::parse(&args.format) else {
        eprintln!("unknown format {:?}; use human, json, csv or md", args.format);
        return ExitCode::from(64);
    };
    let Some(min_confidence) = Confidence::parse(&args.min_confidence) else {
        eprintln!("unknown confidence {:?}; use low, med or high", args.min_confidence);
        return ExitCode::from(64);
    };

    for path in &args.paths {
        if !path.exists() {
            // A typo must not look like a clean run to a CI gate.
            eprintln!("rafale: no such path: {}", path.display());
            return ExitCode::from(64);
        }
    }

    let list = graph::collect(&args.paths, &args.excludes);
    if list.is_empty() {
        eprintln!("rafale: no supported source files (.rs .go .py .js .ts) under the given paths");
        return ExitCode::SUCCESS;
    }

    let mut graph = graph::parse(list.clone());
    graph::link(&mut graph, &list);

    let cfg = Config {
        complexity: args.complexity,
        long: args.long,
        params: args.params,
        dup_min: args.dup_min,
        include_public: args.include_public,
        include_tests: args.include_tests,
        min_confidence,
        max_hits: args.max_hits,
    };
    let findings = detect::detect(&graph, &cfg);
    // Stamped before --suggest: the headline is the cost of the analyzer, not of
    // a local model that may need a minute just to load its weights.
    let scan_ms = started.elapsed().as_millis();

    let advice = if args.suggest {
        let model = args
            .model
            .or_else(|| std::env::var("OLLAMA_MODEL").ok())
            .unwrap_or_else(|| "qwen2.5-coder:1.5b".to_string());
        let url = args
            .url
            .or_else(|| std::env::var("OLLAMA_URL").ok())
            .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
        suggest(
            &graph,
            &findings,
            &Ollama::new(&url, &model, Duration::from_secs(args.timeout)),
            args.top,
        )
    } else {
        Vec::new()
    };

    let totals = Totals {
        files: graph.files_scanned,
        lines: graph.lines_scanned,
        funcs: graph.funcs_total,
        elapsed_ms: scan_ms,
    };
    let color = !args.no_color
        && format == Format::Human
        && std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none();

    println!(
        "{}",
        report::render(&graph, &findings, &advice, &cfg, &totals, format, color)
    );

    if args.fail_on_findings && !findings.is_empty() {
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

/// One call per finding, sequential: a local model has no reason to be hammered.
fn suggest(
    graph: &graph::Graph,
    findings: &[detect::Finding],
    client: &Ollama,
    top: usize,
) -> Vec<String> {
    match client.models() {
        Ok(installed) => {
            if installed.is_empty() {
                eprintln!(
                    "rafale: ollama is up but no model is pulled. Run: ollama pull {}",
                    client.model
                );
                return Vec::new();
            }
            let wanted = client.model.clone();
            let exact = installed.iter().any(|m| m == &wanted);
            let prefix = installed
                .iter()
                .any(|m| m.split(':').next() == Some(wanted.as_str()));
            if !(exact || prefix) {
                eprintln!(
                    "rafale: model {:?} is not pulled. Run `ollama pull {}`. Available: {}",
                    wanted,
                    wanted,
                    installed.join(", ")
                );
                return Vec::new();
            }
        }
        Err(e) => {
            eprintln!("rafale: {e}\nrafale: skipping --suggest, showing the deterministic report only");
            return Vec::new();
        }
    }

    let mut advice = vec![String::new(); findings.len()];
    let picks = spread(findings, top);
    for (n, &idx) in picks.iter().enumerate() {
        let finding = &findings[idx];
        let func = &graph.funcs[finding.func];
        let lang = graph.units[func.file].lang.to_string();
        let Some(source) = body_of(graph, finding.func) else {
            continue;
        };
        eprintln!(
            "rafale: [{}/{}] {} → {}",
            n + 1,
            picks.len(),
            client.model,
            graph.name(finding.func)
        );
        let prompt = rafale::llm::prompt_for(
            &graph.name(finding.func),
            &lang,
            finding.category.label(),
            &finding.why,
            &source,
        );
        match client.generate(&prompt) {
            Ok(text) => advice[idx] = text,
            Err(e) => {
                eprintln!("rafale: generation failed: {e}");
            }
        }
    }
    advice
}

/// Take the worst finding of each category in turn: a local model has a small
/// budget and five variations of one problem teach the developer nothing.
fn spread(findings: &[detect::Finding], top: usize) -> Vec<usize> {
    let mut by_cat: [Vec<usize>; 5] = std::array::from_fn(|_| Vec::new());
    for (n, f) in findings.iter().enumerate() {
        by_cat[f.category as usize].push(n);
    }
    let mut picks = Vec::new();
    let mut round = 0;
    while picks.len() < top {
        let mut took = false;
        for list in &by_cat {
            if let Some(&n) = list.get(round) {
                picks.push(n);
                took = true;
                if picks.len() == top {
                    break;
                }
            }
        }
        if !took {
            break;
        }
        round += 1;
    }
    picks
}

/// Read the function body back from disk, capped so a giant function cannot
/// blow the model's context window.
fn body_of(graph: &graph::Graph, func: usize) -> Option<String> {
    let f = &graph.funcs[func];
    let raw = std::fs::read_to_string(graph.path(func)).ok()?;
    let lines: Vec<&str> = raw.lines().collect();
    let start = f.f.start.min(lines.len().saturating_sub(1));
    let end = f.f.end.min(start + 220).min(lines.len().saturating_sub(1));
    Some(lines[start..=end].join("\n"))
}
