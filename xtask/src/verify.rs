//! Running the test suite and saying what did not run.
//!
//! `cargo test` captures the output of a passing test, so a test that decides
//! it cannot run, prints why, and returns says nothing at all: the run reports
//! it as passed and the reason is discarded. Every skip in this workspace works
//! that way, because the alternative — failing when a device lacks a capability
//! — would make the suite unrunnable on the machines the suite exists to cover.
//!
//! That is a reasonable design with one hole in it, which is that nobody sees
//! the skips. This runs the suite with output uncaptured and counts them, so a
//! run reports what it covered rather than only that it was green. It found
//! three tests that had been skipping since they were written, and a whole file
//! of them skipping because of a lock two threads were contending for.
//!
//! The word is the contract. A skip is found by looking for "skipping" in the
//! line, so a test that announces itself any other way is counted as having
//! run -- which is the condition this exists to end, reached from the other
//! side. Four sites said "no backend available" and one called itself a note,
//! and all five were invisible here until
//! `a_skip_says_the_word_the_census_counts` in `xtask/tests/documentation.rs`
//! went looking for them.
//!
//! A skip is not a failure and this does not treat it as one. Some are correct:
//! a device without the advanced-blend extension genuinely cannot render those
//! scenes, and the corpus reports that as coverage it did not get. The point is
//! that the number is visible and has to be looked at.

use std::process::Command;

/// A failed test and the panic it produced, if one was found.
pub struct Failure {
    pub name: String,
    /// The `thread ... panicked at` location, and the message under it.
    ///
    /// Empty when the failure was not a panic -- a test binary that aborted,
    /// or a harness-level error -- which is itself worth seeing rather than
    /// papering over with a guess.
    pub detail: Vec<String>,
}

/// One kind of skip, and how many tests reported it.
pub struct Skip {
    pub reason: String,
    pub count: usize,
}

pub struct Outcome {
    pub passed: usize,
    pub failed: usize,
    /// The tests that failed, each with whatever it said on the way out.
    ///
    /// A count on its own says something is wrong and not what, and a name on
    /// its own says which test and not why. On a machine that is not this one
    /// -- a CI runner, somebody else's hardware -- the difference between those
    /// and the panic message is the difference between a fix and another push
    /// to find out. Both were learned that way.
    pub failures: Vec<Failure>,
    /// Test binaries whose summary line never arrived.
    ///
    /// Non-zero means the reading of the run is incomplete, whatever the
    /// counts say -- see the note in `parse`.
    pub lost: usize,
    /// Where the harness's whole output was written, when it could be.
    pub log: Option<String>,
    pub skips: Vec<Skip>,
    /// What the Vulkan validation layer said during the run, deduplicated.
    ///
    /// Separate from `failures` because these are not a test's verdict. A
    /// context that installs a messenger routes what the layer says into a log
    /// its own tests assert on; a context that does not -- the public API's,
    /// which is the path a caller takes -- has the layer's output go to stderr
    /// and reach nobody. Anything here means the run misused Vulkan somewhere,
    /// whatever the tests decided about the pixels.
    pub validation: Vec<Skip>,
    /// Scenes a comparison declined to make, as the suite reported them.
    ///
    /// A skip is a test that did not run; this is a test that ran and covered
    /// less than it holds. The catalog compares every plate across two
    /// backends, and where a plate needs a capability one of them lacks it says
    /// so and moves on -- correctly, since there is no comparison to make. But
    /// the suite then passes, and "834 passed" is the same sentence whether it
    /// compared every plate or four fifths of them.
    ///
    /// So the numbers the suite prints about its own coverage are carried up
    /// here and said out loud, for the reason the skips are.
    pub coverage: Vec<String>,
    /// True where the suite itself came back non-zero.
    pub broke: bool,
}

/// The loader variable that installs a layer for every context in a process.
const LAYER_ENV: &str = "VK_LOADER_LAYERS_ENABLE";

/// Everything after the last `(` in a skip line, dropped.
///
/// Reasons carry a device name or an error in parentheses, which differs per
/// machine and would make the same skip count as several. The prefix is what
/// identifies it.
fn normalize(line: &str) -> String {
    let line = line.trim();
    match line.find('(') {
        Some(i) => line[..i].trim_end().to_string(),
        None => line.to_string(),
    }
}

/// Run the suite, with any variables the caller wants in the child's
/// environment.
///
/// The environment is a parameter rather than something decided here, and every
/// caller passes it even when it is empty. What the suite ran against is the
/// single most important fact about a run of it, so a function that chose the
/// device for you would be the wrong place to look when the answer surprises
/// someone -- and an empty slice at the call site is where a reader finds out
/// that nothing was chosen.
pub fn run_with_env(extra: &[String], env: &[(&str, String)]) -> Outcome {
    let mut command = Command::new(env!("CARGO"));
    for (key, value) in env {
        command.env(key, value);
    }
    command.arg("test").arg("--workspace");
    // Every target runs even after one fails, because this is a census: a run
    // that stops at the first failing binary reports the skips of the targets
    // it reached and says nothing about the rest, which is the same partial
    // picture this command exists to replace.
    command.arg("--no-fail-fast");
    for arg in extra {
        command.arg(arg);
    }
    // The validation layer for every context in the run, not only the ones that
    // ask. The ones that ask install a messenger and route what the layer says
    // into a log their own tests assert on. The ones that do not -- the public
    // API's, which is the path a caller actually takes -- had no validation at
    // all, and a descriptor set layout leaked there on every device for as long
    // as the material set has existed while this suite stayed green. Without a
    // messenger the layer writes to stderr, which is already captured below, so
    // scanning for it is what turns it into a failure.
    //
    // A setting already in the environment is left alone: someone debugging one
    // layer should not have this quietly replace it. And if the layer is not
    // installed the loader ignores this, which is not a silent pass -- the
    // several tests that need it report a skip, and naming skips is what this
    // command is for.
    if std::env::var_os(LAYER_ENV).is_none() {
        command.env(LAYER_ENV, "VK_LAYER_KHRONOS_validation");
    }
    // Uncaptured, which is the whole point: without this the skip lines below
    // never reach the output at all.
    command.arg("--").arg("--nocapture");

    let output = match command.output() {
        Ok(output) => output,
        Err(e) => {
            eprintln!("could not run the suite: {e}");
            return Outcome {
                lost: 0,
                passed: 0,
                failed: 0,
                failures: Vec::new(),
                log: None,
                skips: Vec::new(),
                validation: Vec::new(),
                coverage: Vec::new(),
                broke: true,
            };
        }
    };

    // Both streams: the harness writes its summary lines to stdout and a test's
    // own output to stderr, and a skip is the latter.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Kept, because the census is a summary and a summary is not always
    // enough. Twice now a failure on a machine I cannot reach has cost a push
    // to learn something the harness had already said and nothing had written
    // down. The path is printed with the census rather than only on failure,
    // so it is a thing that exists rather than a thing to remember.
    let log = std::path::Path::new("target").join("verify.log");
    let saved = std::fs::create_dir_all("target")
        .and_then(|()| std::fs::write(&log, text.as_bytes()))
        .is_ok();

    let mut outcome = parse(&text, !output.status.success());
    outcome.log = saved.then(|| log.display().to_string());
    outcome
}

/// Turn the harness's output into a census.
///
/// Split from the run so it can be tested on text rather than on a device: the
/// interesting cases here are shapes of output -- a panic with a multi-line
/// message, the same failure named once per target under `--no-fail-fast` --
/// and reproducing those by breaking a real test is neither quick nor
/// something that stays around afterwards.
fn parse(text: &str, broke: bool) -> Outcome {
    let mut passed = 0;
    let mut failed = 0;
    // Every test binary cargo starts, and every summary line one of them
    // printed. They have to match.
    //
    // Counting them is not pedantry. Test binaries run in parallel and write
    // to one pipe, so a line can be cut in half by another binary's output --
    // and a summary line lost that way takes a whole binary's tally with it,
    // silently. That happened: one run of a tree that has six hundred and
    // forty-one tests reported six hundred and twenty-nine, and the run either
    // side of it reported the right number. An undercount is the harmless
    // version. The same loss on a binary that *failed* would report no
    // failures at all, which is a false pass, and this is what makes that
    // impossible to miss.
    let mut binaries = 0usize;
    let mut summaries = 0usize;
    // Lines a suite prints about how much of itself it covered. Recognized by
    // shape rather than by which test wrote them, so a second suite that starts
    // reporting coverage is carried without this having to learn its name.
    let mut coverage: Vec<String> = Vec::new();
    let mut failures: Vec<Failure> = Vec::new();
    // Collected separately and joined afterwards, because the two are not in
    // the order they read in. Under `--nocapture` a panic is written when it
    // happens and the harness prints `... FAILED` when the test finishes, so
    // the message arrives first -- and with tests running in parallel, other
    // tests' output arrives in between. Matching them as they were scanned
    // found nothing at all, and did so only on the machine that had a failure
    // to report.
    let mut panics: Vec<(String, Vec<String>)> = Vec::new();
    let mut reasons: Vec<(String, usize)> = Vec::new();
    // The harness prints each failure twice: once as it happens and again in a
    // trailing list. The first form is taken and the second ignored, because
    // with `--no-fail-fast` the trailing lists arrive per target and a name
    // would otherwise be counted once per binary that mentions it.
    //
    // The panic is matched separately, by the thread name the harness gives
    // each test. Under `--nocapture` it is written straight to stderr as it
    // happens rather than collected into a per-test block, so the two are
    // interleaved with everything else and are paired up by name here.
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = line.trim().strip_prefix("test ") {
            if let Some(name) = name.strip_suffix(" ... FAILED") {
                let name = name.to_string();
                if !failures.iter().any(|f| f.name == name) {
                    failures.push(Failure {
                        name,
                        detail: Vec::new(),
                    });
                }
            }
        }
        if let Some(rest) = line.trim().strip_prefix("thread '") {
            // `thread 'name' panicked at ...` through Rust 1.88, and
            // `thread 'name' (11615) panicked at ...` by 1.97. Splitting on the
            // closing quote and then on the words handles both; matching the
            // older form alone found nothing on the newer toolchain, and found
            // it silently, since a failure with no message looks the same as a
            // failure that printed none.
            if let Some((thread, rest)) = rest.split_once('\'') {
                let location = rest.split_once(" panicked at ").map(|(_, at)| at);
                let Some(location) = location else { continue };
                if !panics.iter().any(|(name, _)| name == thread) {
                    let mut detail = vec![format!("at {location}")];
                    // The message sits under the location, one line or
                    // several, and ends where the harness's note about
                    // RUST_BACKTRACE begins.
                    for next in lines.iter().skip(i + 1) {
                        let next = next.trim();
                        // The message ends at a blank line, at the harness's
                        // note about RUST_BACKTRACE, or at the next line the
                        // harness itself wrote. Relying on the note alone means
                        // relying on a line the harness prints for its own
                        // reasons and could stop printing.
                        if next.is_empty()
                            || next.starts_with("note: ")
                            || next.starts_with("test ")
                            || next.starts_with("failures:")
                        {
                            break;
                        }
                        detail.push(next.to_string());
                    }
                    panics.push((thread.to_string(), detail));
                }
            }
        }
        // `cargo test` announces each binary before running it. Doc-test runs
        // announce themselves differently and print a summary too, so both
        // forms count.
        let trimmed = line.trim_start();
        if trimmed.starts_with("Running ") || trimmed.starts_with("Doc-tests ") {
            binaries += 1;
        }
        if let Some(rest) = line.trim().strip_prefix("test result: ") {
            // "ok. 12 passed; 0 failed; ..."
            //
            // Read before it counts as a summary, which is the difference
            // between catching a mangled line and joining in. The same
            // interleaving that removes a summary line can cut one in the
            // middle of its numbers instead, and a line that arrives as
            // "test result: ok. 1" then someone else's output has the shape of
            // a summary and none of the content. Counted as a summary and
            // parsed with a default, it contributes nothing and hides itself:
            // the total comes out short by a whole binary's tally and `lost`
            // stays at zero, which is precisely the false clean run the count
            // above exists to prevent.
            //
            // This was found the way it would be: a gate that had been
            // reporting 816 reported 813 twice, said nothing was skipped and
            // nothing was lost, and would not reproduce. Two binaries in this
            // tree have three tests.
            let mut fields = rest.split_whitespace();
            let _verdict = fields.next();
            let read_pair = |fields: &mut std::str::SplitWhitespace<'_>, label: &str| match (
                fields.next(),
                fields.next(),
            ) {
                (Some(n), Some(seen)) if seen == label => n.parse::<usize>().ok(),
                _ => None,
            };
            // A line that does not read leaves `summaries` alone on purpose,
            // so `binaries` exceeds it and the run reports itself partial.
            if let (Some(p), Some(f)) = (
                read_pair(&mut fields, "passed;"),
                read_pair(&mut fields, "failed;"),
            ) {
                summaries += 1;
                passed += p;
                failed += f;
            }
        }
        // "drew N of M ..." and "compared N scene(s)", as the catalog prints
        // them. Matched on the shape of the sentence rather than on the test
        // that writes it: a suite is entitled to report its own coverage, and
        // this is the one place that can pass the report on to a reader who
        // only sees a total.
        if let Some(at) = line.find("drew ").or_else(|| line.find("compared ")) {
            let said = line[at..].trim();
            if said
                .split_whitespace()
                .nth(1)
                .is_some_and(|n| n.parse::<usize>().is_ok())
                && !coverage.iter().any(|held| held == said)
            {
                coverage.push(said.to_string());
            }
        }
        if line.contains("skipping") {
            let reason = normalize(line);
            match reasons.iter_mut().find(|(r, _)| *r == reason) {
                Some((_, count)) => *count += 1,
                None => reasons.push((reason, 1)),
            }
        }
    }
    for failure in &mut failures {
        if let Some((_, detail)) = panics.iter().find(|(name, _)| *name == failure.name) {
            failure.detail = detail.clone();
        }
    }
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Reported as a break rather than as a failure, because that is what it
    // is: the run may have been fine and the reading of it was not, and the
    // two want different responses.
    // The layer repeats itself: the same misuse in a loop, or in one test per
    // binary, is the same fault stated many times, and a census that printed
    // each occurrence would bury what it found. Counted by message, like a
    // skip.
    let mut complaints: Vec<(String, usize)> = Vec::new();
    for line in text.lines() {
        let Some(start) = line.find("Validation Error") else {
            continue;
        };
        // From the marker rather than from the start of the line: under
        // `--nocapture` a test's own output shares the line with it.
        let message = normalize_validation(&line[start..]);
        match complaints.iter_mut().find(|(seen, _)| *seen == message) {
            Some((_, count)) => *count += 1,
            None => complaints.push((message, 1)),
        }
    }
    complaints.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Reported as a break rather than as a failure, because that is what it
    // is: the run may have been fine and the reading of it was not, and the
    // two want different responses.
    let broke = broke || summaries < binaries || !complaints.is_empty();
    Outcome {
        passed,
        failed,
        failures,
        lost: binaries.saturating_sub(summaries),
        log: None,
        skips: reasons
            .into_iter()
            .map(|(reason, count)| Skip { reason, count })
            .collect(),
        coverage,
        validation: complaints
            .into_iter()
            .map(|(reason, count)| Skip { reason, count })
            .collect(),
        broke,
    }
}

/// A validation message reduced to what identifies it.
///
/// The layer states the handles involved, which differ between runs and would
/// make one fault look like a dozen. The VUID identifies it where the layer
/// supplies one; otherwise the message keeps its own text, truncated, since a
/// message with no VUID is still worth naming.
fn normalize_validation(message: &str) -> String {
    if let Some(start) = message.find("VUID-") {
        let end = message[start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .map(|i| start + i)
            .unwrap_or(message.len());
        return message[start..end].to_string();
    }
    let trimmed = message.trim();
    trimmed.chars().take(120).collect()
}

pub fn text(outcome: &Outcome) -> String {
    let mut out = String::new();
    // The caveat rides on the count line, not only under it. This repository's
    // rule for reading a gate is to read the test count rather than the exit
    // code, and a reader following that rule stops at the first line -- so a
    // first line that reads clean while a paragraph four lines down says the
    // number is short puts the correction where the rule sends nobody. It has
    // already caught someone out that way once.
    out.push_str(&format!(
        "{} passed, {} failed{}\n",
        outcome.passed,
        outcome.failed,
        if outcome.lost > 0 {
            // Both numbers, because the dangerous one is the zero: a lost
            // binary takes its failures with it as surely as its passes.
            format!(
                " -- BOTH SHORT, {} binar{} unread",
                outcome.lost,
                if outcome.lost == 1 { "y" } else { "ies" }
            )
        } else {
            String::new()
        }
    ));
    if !outcome.coverage.is_empty() {
        out.push_str("what the suite says it covered:\n");
        for line in &outcome.coverage {
            out.push_str(&format!("    {line}\n"));
        }
    }
    if outcome.lost > 0 {
        out.push_str(&format!(
            "    {} test binar{} said nothing this could read -- no summary, or \
             one cut in half by another binary writing at the same time. This \
             count is short by however many tests they held, and would be short \
             by their failures too.\n",
            outcome.lost,
            if outcome.lost == 1 { "y" } else { "ies" }
        ));
    }
    for failure in &outcome.failures {
        out.push_str(&format!("    FAILED  {}\n", failure.name));
        if failure.detail.is_empty() {
            // Not every failure is a panic: a test binary can abort, or die on
            // a signal, and then there is no message to pair with a name. Say
            // that rather than print a name and a blank, which reads as a
            // parser that missed something.
            out.push_str("              (no panic message; see the log below)\n");
        }
        for line in &failure.detail {
            out.push_str(&format!("              {line}\n"));
        }
    }
    if let Some(log) = &outcome.log {
        if !outcome.failures.is_empty() {
            out.push_str(&format!("    full output in {log}\n"));
        }
    }
    let skipped: usize = outcome.skips.iter().map(|s| s.count).sum();
    if outcome.skips.is_empty() {
        out.push_str("nothing skipped\n");
    } else {
        out.push_str(&format!("{skipped} skipped:\n"));
        for skip in &outcome.skips {
            out.push_str(&format!("  {:>3}  {}\n", skip.count, skip.reason));
        }
        out.push_str(
            "\nA skip passes, so these are tests that did not run. Some are \n\
             correct -- a device without a capability cannot exercise it -- and \n\
             the ones that are not look exactly the same from here.\n",
        );
    }
    if !outcome.validation.is_empty() {
        let total: usize = outcome.validation.iter().map(|c| c.count).sum();
        out.push_str(&format!("\n{total} validation error(s):\n"));
        for complaint in &outcome.validation {
            out.push_str(&format!("  {:>3}  {}\n", complaint.count, complaint.reason));
        }
        out.push_str(
            "\nThe Vulkan validation layer objected during this run, whatever \n\
             the tests decided about the pixels. A context that installs a \n\
             messenger has its own tests for this; these came from the ones \n\
             that do not, which is the path a caller takes. The log has each \n\
             occurrence with its handles.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_is_identified_by_its_prefix() {
        // The parenthesized part names a device or an error, which differs per
        // machine; counting it would split one skip into several.
        assert_eq!(
            normalize("skipping: no Vulkan device (ERROR_INITIALIZATION_FAILED)"),
            "skipping: no Vulkan device"
        );
        assert_eq!(
            normalize("  skipping: both backends are needed"),
            "skipping: both backends are needed"
        );
    }

    /// The shape `cargo test --no-fail-fast -- --nocapture` actually produces.
    ///
    /// The panic precedes the `... FAILED` line, which is the whole reason
    /// this fixture exists: the first version of it had them the other way
    /// around, matching how they read rather than how they are written, and
    /// the parser it was testing had the same mistake.
    const OUTPUT: &str = "\
running 3 tests
skipping: no card node on this machine
thread 'the_broken_one' (11615) panicked at crates/x/tests/y.rs:12:5:
all 2 acquisitions returned the same image,
so nothing is double buffered
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test a_passing_one ... ok
test the_broken_one ... FAILED

failures:
    the_broken_one

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out;
";

    #[test]
    fn a_failure_carries_the_panic_that_produced_it() {
        let outcome = parse(OUTPUT, true);
        assert_eq!(outcome.passed, 1);
        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.failures.len(), 1, "one failure, named once");
        let failure = &outcome.failures[0];
        assert_eq!(failure.name, "the_broken_one");
        assert_eq!(
            failure.detail,
            vec![
                "at crates/x/tests/y.rs:12:5:".to_string(),
                "all 2 acquisitions returned the same image,".to_string(),
                "so nothing is double buffered".to_string(),
            ],
            "the location and the whole message, stopping before the note"
        );
    }

    #[test]
    fn the_panic_header_is_read_with_or_without_a_thread_id() {
        // Rust 1.88 writes the first form and 1.97 the second. This repository
        // is built on both -- the workstation on one, CI on the other -- and
        // reading only one of them fails by finding nothing, which is
        // indistinguishable from a failure that printed no message.
        for header in [
            "thread 'the_broken_one' panicked at src/x.rs:1:1:",
            "thread 'the_broken_one' (11615) panicked at src/x.rs:1:1:",
        ] {
            let text = format!("{header}\nthe message\ntest the_broken_one ... FAILED\n");
            let outcome = parse(&text, true);
            assert_eq!(outcome.failures.len(), 1, "{header}");
            assert_eq!(
                outcome.failures[0].detail,
                vec!["at src/x.rs:1:1:".to_string(), "the message".to_string()],
                "{header}"
            );
        }
    }

    #[test]
    fn the_same_failure_named_by_several_targets_is_counted_once() {
        // `--no-fail-fast` prints a trailing `failures:` list per test binary,
        // so a name can appear repeatedly for one failure.
        let doubled = format!("{OUTPUT}\nfailures:\n    the_broken_one\n");
        let outcome = parse(&doubled, true);
        assert_eq!(outcome.failures.len(), 1);
    }

    #[test]
    fn what_a_suite_says_about_its_own_coverage_is_carried_up() {
        // A suite that compares less than it holds passes, and the total says
        // nothing about it. These lines are the only place that difference is
        // visible, so they have to survive being read.
        let text = "\
test every_catalog_scene_draws_something ... drew 259 of 259 catalog scenes across 2 device(s)
test the_catalog_matches_across_backends ... compared 239 scene(s)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
";
        let outcome = parse(text, false);
        assert_eq!(
            outcome.coverage,
            vec![
                "drew 259 of 259 catalog scenes across 2 device(s)".to_string(),
                "compared 239 scene(s)".to_string(),
            ]
        );
    }

    #[test]
    fn a_sentence_that_only_looks_like_a_coverage_line_is_left_alone() {
        // "compared" and "drew" are ordinary words. What makes one of these a
        // report is a number straight after it, and a line without one is prose
        // that happens to share a verb.
        let text = "the two backends compared badly on this plate
                    a stroke drew outside its clip
";
        let outcome = parse(text, false);
        assert!(
            outcome.coverage.is_empty(),
            "prose was taken for a coverage report: {:?}",
            outcome.coverage
        );
    }

    #[test]
    fn a_skip_is_counted_by_its_reason_without_the_parenthetical() {
        let text = "skipping: no device (llvmpipe)\nskipping: no device (radeonsi)\n";
        let outcome = parse(text, false);
        assert_eq!(outcome.skips.len(), 1, "one reason, two machines");
        assert_eq!(outcome.skips[0].count, 2);
    }
}

#[cfg(test)]
mod lost_summary_tests {
    use super::*;

    #[test]
    fn a_binary_whose_summary_never_arrived_is_reported_rather_than_subtracted() {
        // Two binaries announced, one summary printed -- which is what a line
        // cut in half by another binary's output leaves behind. The count that
        // survives is honest about being partial rather than quietly smaller.
        let text = "\
     Running tests/one.rs (target/debug/deps/one)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
     Running tests/two.rs (target/debug/deps/two)
";
        let outcome = parse(text, false);
        assert_eq!(outcome.passed, 4);
        assert_eq!(outcome.lost, 1);
        assert!(
            outcome.broke,
            "an unreadable run must not be reported as a clean one"
        );
        assert!(text_of(&outcome).contains("said nothing this could read"));
    }

    #[test]
    fn a_summary_cut_in_half_is_lost_rather_than_counted_as_zero() {
        // The same interleaving that removes a summary line can cut one in the
        // middle instead. This one has the shape of a summary and no numbers,
        // which read with a default contributes nothing while still counting
        // as a summary -- so the total comes out short by that binary's tally
        // and nothing says so.
        //
        // That is not hypothetical. A gate reporting 816 reported 813 twice,
        // with nothing skipped and nothing lost, and would not reproduce; two
        // binaries in this tree have three tests.
        let text = "\
     Running tests/one.rs (target/debug/deps/one)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
     Running tests/two.rs (target/debug/deps/two)
test result: ok. 3 passtest result: ok. 9 passed; 0 failed; 0 ignored;
";
        let outcome = parse(text, false);
        // Four from the binary that spoke clearly. The mangled line gives
        // nothing, and says so.
        assert_eq!(outcome.passed, 4);
        assert_eq!(outcome.lost, 1);
        assert!(outcome.broke);
        assert!(text_of(&outcome).contains("said nothing this could read"));
    }

    #[test]
    fn a_summary_whose_count_is_not_a_number_is_lost_too() {
        let text = "\
     Running tests/one.rs (target/debug/deps/one)
test result: ok. ?? passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
";
        let outcome = parse(text, false);
        assert_eq!(outcome.passed, 0);
        assert_eq!(
            outcome.lost, 1,
            "a count that is not a number must not read as zero passes"
        );
    }

    /// The count line says it is short, rather than only the paragraph below.
    ///
    /// Mutate away the suffix and this fails while the three tests above still
    /// pass, which is the point: they check that the loss is *reported*, and
    /// this checks that it is reported where the reader is told to look.
    #[test]
    fn the_count_line_itself_says_when_it_is_short() {
        let text = "\
     Running tests/one.rs (target/debug/deps/one)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
     Running tests/two.rs (target/debug/deps/two)
";
        let rendered = text_of(&parse(text, false));
        let first = rendered.lines().next().expect("a summary line");
        assert!(
            first.contains("4 passed") && first.contains("SHORT"),
            "the first line must carry it: {first:?}"
        );
        assert!(first.contains('1'), "and say how many binaries: {first:?}");

        // And a clean run's first line stays clean, so the marker means
        // something when it does appear.
        let whole = "\
     Running tests/one.rs (target/debug/deps/one)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
";
        let rendered = text_of(&parse(whole, false));
        let first = rendered.lines().next().expect("a summary line");
        assert_eq!(first, "4 passed, 0 failed", "{first:?}");
    }

    #[test]
    fn a_complete_run_reports_nothing_lost() {
        let text = "\
     Running tests/one.rs (target/debug/deps/one)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
     Doc-tests emblema
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;
";
        let outcome = parse(text, false);
        assert_eq!(outcome.passed, 5);
        assert_eq!(outcome.lost, 0);
        assert!(!outcome.broke);
    }

    fn text_of(outcome: &Outcome) -> String {
        super::text(outcome)
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    /// A run that passed every test while misusing Vulkan throughout.
    const DIRTY: &str = "\
     Running tests/public_api.rs (target/debug/deps/public_api-1)
test a_thing ... Validation Error: [ VUID-vkDestroyDevice-device-05137 ] | MessageID = 0x4872eaa0
vkDestroyDevice(): For VkDevice 0x7f9d8c159450, VkDescriptorSetLayout 0x80000000008 has not been destroyed.
ok
test another ... Validation Error: [ VUID-vkDestroyDevice-device-05137 ] | MessageID = 0x4872eaa0
vkDestroyDevice(): For VkDevice 0x7f9d8c1ccca0, VkDescriptorSetLayout 0x60000000006 has not been destroyed.
ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s
";

    #[test]
    fn a_run_that_passed_but_misused_vulkan_does_not_come_back_clean() {
        // The case this exists for, and the reason it is a break rather than a
        // failure: every test agreed about the pixels. The layer's objection is
        // about what the process did to the device, which no assertion here was
        // watching, and a census that reported "2 passed" and stopped would be
        // reporting the truth and hiding the important part.
        let outcome = parse(DIRTY, false);
        assert_eq!(outcome.passed, 2);
        assert_eq!(outcome.failed, 0);
        assert!(
            outcome.broke,
            "a run the validation layer objected to must not be reported as clean"
        );
        assert_eq!(outcome.validation.len(), 1, "one fault, not one per device");
        assert_eq!(outcome.validation[0].count, 2);
        assert_eq!(
            outcome.validation[0].reason,
            "VUID-vkDestroyDevice-device-05137"
        );
        assert!(text(&outcome).contains("2 validation error(s)"));
    }

    #[test]
    fn a_clean_run_says_nothing_about_validation() {
        let clean = "\
     Running tests/public_api.rs (target/debug/deps/public_api-1)
test a_thing ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s
";
        let outcome = parse(clean, false);
        assert!(!outcome.broke);
        assert!(outcome.validation.is_empty());
        assert!(
            !text(&outcome).contains("validation"),
            "a clean run should not mention it at all"
        );
    }

    #[test]
    fn a_message_with_no_vuid_keeps_its_own_text() {
        // Not every objection carries an identifier -- the layer emits some of
        // its own, and a loader or driver message can arrive on the same
        // stream. Dropping those because they do not match the expected shape
        // would be the same silence this whole command exists to remove.
        let message = normalize_validation("Validation Error: something the layer had no VUID for");
        assert!(
            message.contains("something the layer had no VUID for"),
            "got {message:?}"
        );
    }

    #[test]
    fn the_handles_in_a_message_do_not_make_it_a_different_fault() {
        // The layer states which device and which object, and both differ every
        // run and every context. Counted by the raw text, one leak in a suite
        // with sixty contexts would print sixty lines and read as sixty faults.
        let one = normalize_validation(
            "Validation Error: [ VUID-vkDestroyDevice-device-05137 ] For VkDevice 0xaaa, VkDescriptorSetLayout 0x111",
        );
        let other = normalize_validation(
            "Validation Error: [ VUID-vkDestroyDevice-device-05137 ] For VkDevice 0xbbb, VkDescriptorSetLayout 0x222",
        );
        assert_eq!(one, other);
    }
}
