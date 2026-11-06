# rafale

Find dead and costly functions in a repo in under a second, then optionally get a
refactoring plan from a local model. Rust, one binary, no C toolchain.

```
$ rafale ~/.cargo/registry/src/.../ripgrep-14
102 files · 55477 lines · 2923 functions in 1706 ms

HIGH COMPLEXITY · 37 findings · 2418 lines at stake
  → split into named helpers, drop a level of nesting
  HIGH impl HiArgs::from_low_args        crates/core/flags/hiargs.rs:113  CC 33, depth 5, 210 lines
       hard to test and to follow: branch count above threshold
  HIGH impl Ignore::matched_ignore       crates/ignore/src/dir.rs:578     CC 29, depth 5, 138 lines
       hard to test and to follow: branch count above threshold
```

No `DEAD CODE` section appears because there was nothing to report: on ripgrep's
default settings the detector stays silent.

## Why it is not another linter

The analysis that decides *what to report* is deterministic: a call graph built
from real reference scanning, plus four cheap metrics. No model is consulted to
pick the 40 findings that matter, so the same repo always yields the same list.

The LLM is the last stage, not the first. It sees only the handful of functions
already identified — never your whole repo — and answers a fixed four-line format.
Run `--suggest` against a local Ollama, or run without it and lose nothing.

## Install

```sh
cargo install --path .            # from a clone
```

Prebuilt binaries are attached to each [release].

On a machine with no C toolchain: rafale builds on `x86_64-pc-windows-gnu` with
zero C dependencies — no tree-sitter, no rustls, no libgit2.

## Usage

```sh
rafale                    # scan the current directory
rafale src/ crates/       # several paths at once
rafale . --include-public # audit a library, not just an application
rafale . --format json    # json, csv, md, or the default human
rafale . --suggest --top 3
rafale . --fail-on-findings   # exit 2, for CI
```

## What it reports

| Category | Trigger | Default threshold |
|---|---|---|
| dead code | unreachable from any entry point | — |
| high complexity | cyclomatic branches, or nesting depth ≥ 5 | CC 12 |
| oversized | one screenful of work | 80 lines |
| too many params | positional call sites | 6 |
| duplicated logic | identical control flow after renaming | 40 normalized lines |

Entry points are `main`, `#[test]` / `#[bench]` and other harness attributes,
Python dunders, abstract signatures, anything bound at module scope, and — unless
you pass `--include-public` — every `pub`, `export` or capitalized name, because an
external consumer may call it and that guess is worth more than a false positive.

Each finding carries a confidence tier: **HIGH** for private and never referenced,
**MED** for a dead cluster or an unreferenced export, **LOW** for an export that is
referenced from something else unreachable. `--min-confidence` filters them; the
default `med` is the point where the list stays actionable.

## Measured, not promised

`rafale` scanning the ripgrep tree (102 files, 55 477 lines, 2 923 functions):

- **791 ms** warm, single run.
- **0 dead-code findings at the default settings.** Under `--include-public` it
  reports 9, and all 9 are genuine: `is_tty_stdin`, `Error::io_error`,
  `Glob::file_type_def` and friends have no call site anywhere in that repo.
- 16 tests in the suite, 6 of them end-to-end over a three-language fixture.

Numbers come from one repo. The parsing is heuristic, so treat the list as a
starting point for a reading pass, not as a verdict.

## The local model

```sh
ollama pull qwen2.5-coder:1.5b
rafale . --suggest
```

Measured on a 7 GB CPU-only laptop, no GPU:

| | |
|---|---|
| qwen2.5-coder:1.5b, first suggestion | ~2 min (weights load once) |
| each further suggestion | ~30 s |
| 2 suggestions on real code | 3 min 52 total |
| mistral:7B | failed after 400 s — 500 MB free RAM is not enough |

The verdict format the model is held to:

```
VERDICT: refactor
WHY: The function is complex and difficult to test. It uses conditional
     compilation to handle different operating systems.
PLAN: Extract the common logic into a separate function and select it with cfg.
RISK: If the common logic is not implemented correctly, results could change.
```

Expect direction, not a patch. A 1.5 B model will not rewrite your function; it
tells you which seam to look at. `--top` spreads its budget across categories
instead of asking about the same problem five times.

## Flags

```
--format <human|json|csv|md>       report format
--min-confidence <low|med|high>    lowest confidence to report [med]
--include-public                   audit exported items as normal code
--include-tests                    include test functions
--complexity <N>                   CC threshold [12]
--long <N>                         length threshold, lines [80]
--params <N>                       parameter threshold [6]
--dup-min <N>                      minimum normalized size for clones [40]
--max-hits <N>                     cap kept per category [40]
--exclude <fragment>               skip any path containing this, repeatable
--suggest                          ask Ollama for a refactoring plan
--model <name>                     [qwen2.5-coder:1.5b], or $OLLAMA_MODEL
--url <base>                       [http://127.0.0.1:11434], or $OLLAMA_URL
--top <N>                          findings sent to the model [5]
--timeout <secs>                   per-request generation timeout [120]
--no-color                         disable ANSI colors
--fail-on-findings                 exit 2 when anything is reported
```

## Known limits

- **Heuristic parsing, no AST.** Regex plus brace and indent tracking. Macro-heavy
  code (`criterion_group!`, `lazy_static!`, `macro_rules!` bodies) and
  `cfg`-compiled alternatives are invisible, so a function reachable only through a
  macro can still look dead.
- **Comments and strings are masked, interpolations are not.** A name inside
  `${...}` or an f-string counts as a reference, which is correct but can surprise.
- **Excluded by design:** `target`, `node_modules`, `dist`, `vendor`, `.git`,
  `__pycache__`, `.venv`, `*.min.js`, `*.lock`, `*.pb.go`, `*.d.ts`, files over
  1.5 MB.
- **`.gitignore` is read per directory**, but a global `core.excludesFile` and
  `.git/info/exclude` are not.
- **Ollama over plain HTTP on localhost only.** No TLS, no remote endpoints, no
  streaming.
- One file at a time in the report: rafale never edits your code.

## Development

```sh
cargo test          # 16 tests
cargo build --release
```

## License

MIT.

[release]: https://github.com/ck526438-lgtm/rafale/releases
