//! Development task runner, invoked as `cargo xtask <command>`.
//!
//! Implemented:
//!
//! - `report` -- what each device on this machine reports it can do, in the
//!   terms the layers above the HAL actually branch on. `--json` for a machine.
//! - `drm` -- whether the direct-scanout lane can run here, and what it would
//!   need. Everything it reads is readable without privilege.
//! - `verify` -- run the suite and report the skips, which a plain `cargo test`
//!   discards along with the rest of a passing test's output.
//! - `gallery` -- render every corpus scene onto one sheet, because no
//!   comparison in the suite can see a scene both implementations get wrong the
//!   same way.
//! - `gate` -- everything that has to pass before a commit, as one exit code.
//!
//! Planned:
//!
//! - `device list` / `device run <board> --suite <s> --cell <c>` -- build the
//!   testkit runner for a target, push it to a board, run suites, and pull
//!   back JSON and JUnit reports. The same harness serves an engineer with a
//!   board on the desk and the CI-triggered device rack.
//! - `device baseline <board> --accept` -- re-baseline tolerances as a
//!   reviewed diff.
//! - `device report --last` -- open the report from the most recent run.
//! - `vkms up` -- load VKMS so the full DRM suite runs on a machine with no
//!   board attached. `drm` reports whether that is what this machine needs.
//! - `ci repro <job-id>` -- reproduce a merge-blocking CI failure locally.
//!
//! Session hygiene is a hard requirement for DRM cells: refuse to start if
//! master cannot be acquired, rather than half-running, and restore the
//! previous VT and session state on exit including on panic. An engineer's
//! desktop must survive a failed test run.

mod bench;
mod ci;
mod drivers;
mod drm;
mod gallery;
mod gate;
mod release;
mod report;
mod verify;

const USAGE: &str = "\
cargo xtask <command>

Commands:
  report            What this machine's devices report they can do.
                    --json  emit the same report for a machine to read.
  bench             Time the analytic and tessellated paths against each other
                    on every device here. Prints numbers; passes whatever they
                    say -- regression gating belongs on a quiet runner.
  drm               Whether this machine can run the direct-scanout lane.
  verify            Run the suite and report what did not run. Extra arguments
                    are passed to cargo test.
                    --software  run against Mesa's CPU drivers instead of this
                    machine's, which is what CI uses and covers scenes the
                    hardware here reports as unavailable.
                    Without it the drivers are whatever the environment says.
                    On a machine with many drivers installed, setting
                    VK_DRIVER_FILES is worth doing -- see the loader note in
                    docs/architecture.md.
  gallery [path]    Render every corpus scene onto one sheet to look at.
                    Defaults to corpus.ppm.
  release           The order to publish this workspace's crates in, and what
                    each step needs. Prints; publishes nothing.
  gate              Lint, format, build, the feature matrix and the suite,
                    stopping at the first failure. Exits non-zero if any step
                    did not pass.
                    --software  as for verify, above.
  help              This text.
";

/// The running kernel's release, which names its module directory.
///
/// Read from the kernel rather than by running `uname`, so this needs nothing
/// on the path. An unreadable one leaves the module directory pointing
/// nowhere, which reports vkms as absent — the same answer a kernel without it
/// gives, and the same advice either way.
fn kernel_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();

    match command.as_deref() {
        Some("report") => {
            let devices = report::gather();
            if rest.iter().any(|a| a == "--json") {
                print!("{}", report::json(&devices));
            } else {
                print!("{}", report::text(&devices));
            }
        }
        Some("bench") => {
            // Before anything else: an unoptimized build is refused rather
            // than measured. Six runs on a board went into chasing a five
            // percent "regression" that was a debug build's code layout.
            if cfg!(debug_assertions) {
                eprint!("{}", bench::why_not_debug());
                std::process::exit(2);
            }

            // `--skip llvmpipe` leaves a software rasterizer unmeasured. On a
            // small board that stage runs every core flat out for minutes and
            // has locked one up; the board's own GPU is measurable without it.
            let mut skip = Vec::new();
            let mut record: Option<String> = None;
            let mut check: Option<String> = None;
            // One percent, and the number is measured rather than picked. On
            // a Pi 5's V3D, seven of the eight rows repeat to within three
            // tenths of a percent across thirty-one runs, so one percent is
            // several times the worst drift actually seen on a quiet board.
            //
            // It used to be five, because the eighth row -- the Vulkan
            // distance-field figure -- lands in one of two states three
            // percent apart from one process to the next, and one tolerance
            // has to be as loose as the worst row it covers. That row now
            // carries its own tolerance in the baseline file, which is where
            // the measurement justifying it also lives, so the default no
            // longer has to cover it. See `docs/on-a-board.md`.
            let mut tolerance = 1.0_f64;
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                let mut wants = |what: &str| match rest.next() {
                    Some(v) => v.clone(),
                    None => {
                        eprintln!("bench: {arg} wants {what}");
                        std::process::exit(2);
                    }
                };
                match arg.as_str() {
                    "--skip" => skip.push(wants("a device name to match")),
                    "--record" => record = Some(wants("a path to write")),
                    "--check" => check = Some(wants("a path to read")),
                    "--tolerance" => {
                        let v = wants("a percentage");
                        match v.parse::<f64>() {
                            Ok(p) if p >= 0.0 => tolerance = p,
                            _ => {
                                eprintln!("bench: --tolerance wants a percentage, got {v:?}");
                                std::process::exit(2);
                            }
                        }
                    }
                    other => {
                        eprintln!("bench: unknown argument {other}");
                        std::process::exit(2);
                    }
                }
            }
            if record.is_some() && check.is_some() {
                eprintln!("bench: --record and --check ask for opposite things");
                std::process::exit(2);
            }

            // Straight to the handle rather than through `print!`, because the
            // point of streaming is that each line has left this process by the
            // time the next configuration starts.
            let mut out = std::io::stdout().lock();
            let rows = match bench::stream_collecting(&skip, &mut out) {
                Ok(rows) => rows,
                Err(e) => {
                    eprintln!("bench: {e}");
                    std::process::exit(1);
                }
            };

            if let Some(path) = record {
                // Read before writing: the file being overwritten may carry
                // per-row tolerances, which are a judgment rather than a
                // measurement and are not re-derivable from this run.
                let recorded = bench::Baseline::from_run(&rows);
                let recorded = match std::fs::read_to_string(&path) {
                    Ok(text) => match bench::Baseline::parse(&text) {
                        Ok(previous) => recorded.keeping_slack_from(&previous),
                        Err(e) => {
                            eprintln!("bench: {path} is there but does not parse: {e}");
                            std::process::exit(1);
                        }
                    },
                    Err(_) => recorded,
                };
                let text = recorded.render();
                if let Err(e) = std::fs::write(&path, text) {
                    eprintln!("bench: writing {path}: {e}");
                    std::process::exit(1);
                }
                eprintln!("recorded {} rows to {path}", rows.len());
            }

            // Opt-in, and that is the whole reason this is a flag. A bench
            // that gated by default would fail a build on a busy laptop, which
            // is the objection the module documentation raises against gating
            // and which still stands for every run that did not ask.
            if let Some(path) = check {
                let text = match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(e) => {
                        eprintln!("bench: reading {path}: {e}");
                        std::process::exit(1);
                    }
                };
                let baseline = match bench::Baseline::parse(&text) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("bench: {path}: {e}");
                        std::process::exit(1);
                    }
                };
                let found = bench::compare(&baseline, &rows, tolerance / 100.0);
                eprintln!("\nagainst {path}, tolerating {tolerance:.1}%");
                for line in &found.lines {
                    eprintln!("{line}");
                }
                if found.unmatched > 0 {
                    eprintln!(
                        "{} row(s) on one side only, which is not a pass",
                        found.unmatched
                    );
                }
                if found.regressed > 0 || found.unmatched > 0 {
                    std::process::exit(1);
                }
                eprintln!("no configuration regressed");
            }
        }
        Some("drm") => {
            let survey = drm::survey(
                std::path::Path::new("/dev/dri"),
                std::path::Path::new("/sys/class/drm"),
                std::path::Path::new("/proc/modules"),
                &std::path::Path::new("/lib/modules").join(kernel_release()),
            );
            print!("{}", drm::text(&survey));
        }
        Some("gallery") => {
            let path = rest
                .iter()
                .find(|a| !a.starts_with('-'))
                .cloned()
                .unwrap_or_else(|| "corpus.ppm".to_string());
            match gallery::render(6) {
                Ok(sheet) => {
                    if let Err(e) = gallery::write_ppm(&path, &sheet) {
                        eprintln!("writing {path}: {e}");
                        std::process::exit(1);
                    }
                    println!(
                        "{} scene(s) drawn onto {path} ({}x{})",
                        sheet.drawn, sheet.width, sheet.height
                    );
                    print!("{}", gallery::map(&sheet));
                    if !sheet.borrowed.is_empty() {
                        println!(
                            "{} drawn by a fallback device, not this one: {}",
                            sheet.borrowed.len(),
                            sheet.borrowed.join(", ")
                        );
                    }
                    if !sheet.skipped.is_empty() {
                        println!(
                            "{} not rendered by this device: {}",
                            sheet.skipped.len(),
                            sheet.skipped.join(", ")
                        );
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some("gate") => {
            if !gate::run(rest.iter().any(|a| a == "--software")) {
                std::process::exit(1);
            }
        }
        Some("verify") => {
            let software = rest.iter().any(|a| a == "--software");
            let passed: Vec<String> = rest.into_iter().filter(|a| a != "--software").collect();
            let env = if software {
                match drivers::environment() {
                    Ok(env) => {
                        for (key, value) in &env {
                            eprintln!("{key}={value}");
                        }
                        env
                    }
                    Err(why) => {
                        eprintln!("{why}");
                        std::process::exit(1);
                    }
                }
            } else {
                Vec::new()
            };
            let outcome = verify::run_with_env(&passed, &env);
            print!("{}", verify::text(&outcome));
            if outcome.broke || outcome.failed > 0 {
                std::process::exit(1);
            }
        }
        Some("release") => print!("{}", release::report()),
        Some("help") | Some("--help") | Some("-h") | None => print!("{USAGE}"),
        Some(other) => {
            // Named rather than merely refused, and alongside what does exist,
            // because the usual reason to be here is a command from the list in
            // this file's header that has not been written yet.
            eprintln!("xtask: no such command: {other}\n\n{USAGE}");
            std::process::exit(1);
        }
    }
}
