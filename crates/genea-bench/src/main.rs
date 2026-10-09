//! `genea-bench`: runs the benchmark scenarios against the real `genea`
//! binary and checks the Genea budgets (spec #19, ticket #23). See
//! `bench/README.md`; `scripts/bench.sh` builds and runs it.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use genea_bench::{
    budgets::{Check, verdicts},
    report::{self, Output},
    scenarios::{self, Context, Options},
};
use serde_json::json;

const USAGE: &str = "\
usage: genea-bench [options]

Runs the benchmark scenarios against target/release/genea and checks the
Genea budgets. Results go to bench/results/<label>.jsonl and
bench/results/<label>.summary.txt. Exits non-zero if a budget is missed
or a scenario couldn't run.

options:
  --only NAME[,NAME…]   run only these scenarios (start, typing, scroll,
                        dead-keys, idle, open-1mb, open-100mb)
  --quick               fewer runs: a smoke test, not a release gate
  --no-cold             skip the cold start runs (they need `sudo -v`)
  --runs N              warm start pairs (default 30)
  --cold-runs N         cold start pairs (default 10)
  --label LABEL         results file name (default: the UTC start time)
  --genea PATH          the Genea binary (default: next to genea-bench)
  --floor PATH          the start-floor binary (default: next to genea-bench)
  --workspace DIR       the Typical workspace (default:
                        bench/workspaces/out/typical)
  --out DIR             results directory (default: bench/results)
";

struct Args {
    only: Option<Vec<String>>,
    options: Options,
    label: Option<String>,
    genea: Option<PathBuf>,
    floor: Option<PathBuf>,
    workspace: Option<PathBuf>,
    out: Option<PathBuf>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed =
        Args { only: None, options: Options::full(), label: None, genea: None, floor: None, workspace: None, out: None };
    let (mut runs, mut cold_runs, mut no_cold) = (None, None, false);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        let number = |v: String| v.parse::<usize>().map_err(|_| format!("not a number: {v}"));
        match arg.as_str() {
            "--only" => parsed.only = Some(value()?.split(',').map(str::to_string).collect()),
            "--quick" => parsed.options = Options::quick(),
            "--no-cold" => no_cold = true,
            "--runs" => runs = Some(number(value()?)?),
            "--cold-runs" => cold_runs = Some(number(value()?)?),
            "--label" => parsed.label = Some(value()?),
            "--genea" => parsed.genea = Some(value()?.into()),
            "--floor" => parsed.floor = Some(value()?.into()),
            "--workspace" => parsed.workspace = Some(value()?.into()),
            "--out" => parsed.out = Some(value()?.into()),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option: {other}")),
        }
    }
    // After --quick, whatever order they came in.
    if let Some(runs) = runs {
        parsed.options.warm_runs = runs;
    }
    if let Some(cold_runs) = cold_runs {
        parsed.options.cold_runs = cold_runs;
    }
    if no_cold {
        parsed.options.cold = false;
    }
    Ok(parsed)
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            if !error.is_empty() {
                eprintln!("genea-bench: {error}\n");
            }
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("genea-bench: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Args) -> Result<u8, String> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = repo.canonicalize().unwrap_or(repo);
    let bin_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    let genea = args.genea.unwrap_or_else(|| bin_dir.join("genea"));
    let floor = args.floor.unwrap_or_else(|| bin_dir.join("genea-floor"));
    let workspace = args.workspace.unwrap_or_else(|| repo.join("bench/workspaces/out/typical"));
    for (what, path, fix) in [
        ("Genea", &genea, "cargo build --release"),
        ("the start floor", &floor, "cargo build --release"),
        ("the Typical workspace", &workspace, "bench/workspaces/typical/setup.sh"),
    ] {
        if !path.exists() {
            return Err(format!("{what} isn't at {}; run `{fix}` first", path.display()));
        }
    }
    let scenarios: Vec<_> = scenarios::all()
        .into_iter()
        .filter(|s| args.only.as_ref().is_none_or(|only| only.iter().any(|o| o == s.name())))
        .collect();
    if let Some(only) = &args.only
        && let Some(unknown) = only.iter().find(|o| !scenarios.iter().any(|s| s.name() == o.as_str()))
    {
        return Err(format!("no scenario named {unknown}"));
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let label = args.label.unwrap_or_else(|| report::utc_label(now));
    let out_dir = args.out.unwrap_or_else(|| repo.join("bench/results"));
    let out = Output::create(&out_dir.join(format!("{label}.jsonl")))?;
    let mut cx = Context { genea, floor, workspace, options: args.options, out, scenario: "session" };

    let fingerprint = report::git_commit(&cx.workspace);
    let session = json!({
        "label": label,
        "started_unix": now,
        "machine": report::machine(),
        "genea_commit": report::git_commit(&repo),
        "genea_dirty": report::git_dirty(&repo),
        "genea_binary": cx.genea,
        "workspace": cx.workspace,
        "workspace_fingerprint": fingerprint,
        "file": scenarios::FILE,
        "options": format!("{:?}", cx.options),
    });
    cx.record("session", session);
    eprintln!(
        "genea-bench: {label}, Typical {}, {} scenario(s)",
        fingerprint.as_deref().unwrap_or("(not a git repository)"),
        scenarios.len()
    );

    let mut checks: Vec<Check> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for scenario in &scenarios {
        cx.scenario = scenario.name();
        eprintln!("genea-bench: {}", scenario.name());
        match scenario.run(&mut cx) {
            Ok(scenario_checks) => {
                for check in &scenario_checks {
                    let mut record = check.to_json();
                    record["scenario"] = json!(scenario.name());
                    cx.out.write(&record);
                }
                checks.extend(scenario_checks);
            }
            Err(error) => {
                cx.record("error", json!({ "error": error }));
                errors.push(format!("ERROR {}: {error}", scenario.name()));
            }
        }
    }

    let verdict = verdicts(&checks);
    let code = if errors.is_empty() { verdict.exit_code() } else { 1 };
    let mut summary = format!(
        "Genea benchmark {label}\nmachine: {}\nTypical workspace: {}\n\n",
        machine_line(),
        fingerprint.as_deref().unwrap_or("?")
    );
    for error in &errors {
        summary.push_str(error);
        summary.push('\n');
    }
    summary.push_str(&verdict.text());
    if !errors.is_empty() {
        summary.push_str(&format!("FAILED: {} scenario(s) couldn't run\n", errors.len()));
    }
    cx.scenario = "session";
    cx.record("verdict", json!({ "passed": code == 0, "exit_code": code, "errors": errors }));
    let summary_path = out_dir.join(format!("{label}.summary.txt"));
    std::fs::write(&summary_path, &summary).map_err(|e| format!("{}: {e}", summary_path.display()))?;
    print!("\n{summary}");
    println!("\nresults: {}\nsummary: {}", cx.out.path().display(), summary_path.display());
    Ok(code as u8)
}

fn machine_line() -> String {
    let m = report::machine();
    let s = |k: &str| m[k].as_str().unwrap_or("?").to_string();
    format!("{} ({}), macOS {}", s("model"), s("chip"), s("macos"))
}
