//! `fleetmesh-replay <log.jsonl> [--rules N | --min-rules N] [--anchors <file>] [--prev <log>]...
//! [--trust-resumed]`: recompute every check and policy decision in
//! an audit log from its logged inputs and report any mismatch.
//!
//! - A log is recomputed with the decision rules it records, but only rules accepted: by
//!   default the current ones (or the rules being built, `RULES_NEWEST`). `--rules N` accepts
//!   exactly version N, `--min-rules N` version N or later: an older log is replayed with its
//!   own rules only when asked for, so a log can't pass by claiming older, looser rules.
//! - `--anchors <file>` checks the log against its run-time anchors (`logs/anchors/`): its
//!   first line and every anchored head must hash as anchored, and the log must reach the
//!   last anchor. A rewritten log replays as well as the original: a credentials
//!   run's log is evidence about people only with its anchors checked, and without
//!   `--anchors` the output says they were not.
//! - Rules 10: every passkey login and step-up's assertion is verified against the public key
//!   the run logged (its challenge recomputed from the run, the session and the act it was made
//!   for, its counter counting up, each challenge answered once); the output counts them, and
//!   for a study says whether its participant's every consequential act was by passkey.
//! - Rules 5: a log whose `intent.resumed` events carry intents over from the previous run
//!   is checked against that log: `--prev <file>`, or by default the file the events name in
//!   the same directory.
//! - Rules 6: a log whose carried `platform.restore` events put back lost-link values an
//!   earlier run still owed is checked against the log they name: that log must replay, and
//!   they must be exactly what it still owed. It is found the same way, and it can be another
//!   log than the intents' (`.last-run-<port>` and `.resume-<port>.json` can name different
//!   logs), so `--prev` may be given once for each: each is matched to the events by its file
//!   name (a single `--prev` that matches neither is taken for the intents, as before). Only
//!   a bare file name is looked for beside the log, never a path.
//! - Rules 12: a log whose `run.started` records what the bridge carried over at start
//!   (`carried_from`: the log `.last-run-<port>` named and any whose carry had failed) is
//!   checked against each log it names, found the same way (`--prev` matched by file name,
//!   else beside the log): each must replay, be the one read (its head and last seq), and have
//!   still owed exactly what was carried. A log whose carry failed is reported, and stays
//!   owed until a later start reads it or an operator writes it off.
//! - Rules 15: a failed carry's fleet file (`<log stem>.fleet.json`, `--prev` by name or beside
//!   the log) is compared with what the start recorded (its hash, the platforms it may owe values
//!   to), or, recorded as unreadable, must not read; a log carried from in place of a pointer
//!   that could not be read must name this station's pointer, and one the start found there but
//!   would not carry from is reported as a failed carry.
//! - Rules 29: the orders the fleet handed over at the start (`intent.handed_over`) are checked
//!   against the fleet the run logged, `<log stem>.fleet.json` beside the log: each its
//!   vehicle's starting task there, every starting task there handed over. Without that file the
//!   output says they were not (`UNCHECKED:`).
//! - Either carry-over whose earlier log is not given or found is a mismatch;
//!   `--trust-resumed` accepts both without their logs (trusting their origin), and the
//!   output says so (`UNCHECKED:`). It also says which log each check used, with the head of
//!   the log the carried values name, to compare with that log's anchors.
//!
//! Exit status: 0 all decisions reproduced (and, with `--anchors`, the anchors hold), 1 decision
//! or anchor mismatches found (a carry-over whose earlier log was not given or found, without
//! `--trust-resumed`, included), 2 unreadable log or usage.

use std::path::Path;
use std::process::ExitCode;

use fleetmesh_decide::replay::{
    Mismatch, ReplayOptions, ReplayReport, RulesAccepted, carried_from_logs, is_log_file_name, replay_str_with,
};

const USAGE: &str = "usage: fleetmesh-replay <audit-log.jsonl> [--rules N | --min-rules N] [--anchors <file>] [--prev <previous-log.jsonl>]... [--trust-resumed]";

struct Args {
    path: String,
    rules: RulesAccepted,
    anchors: Option<String>,
    prev: Vec<String>,
    trust_resumed: bool,
}

fn version(v: Option<&String>, flag: &str) -> Result<u32, String> {
    v.and_then(|v| v.parse().ok())
        .filter(|n| *n >= 1)
        .ok_or_else(|| format!("{flag} needs a rules version (1, 2, ...)"))
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut path = None;
    let mut rules = None;
    let mut anchors = None;
    let mut prev = Vec::new();
    let mut trust_resumed = false;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rules" | "--min-rules" => {
                if rules.is_some() {
                    return Err("give one of --rules and --min-rules".into());
                }
                let n = version(it.next(), a)?;
                rules = Some(if a == "--rules" {
                    RulesAccepted::Exactly(n)
                } else {
                    RulesAccepted::AtLeast(n)
                });
            }
            "--anchors" => anchors = Some(it.next().ok_or("--anchors needs a file")?.clone()),
            "--prev" => prev.push(it.next().ok_or("--prev needs a file")?.clone()),
            "--trust-resumed" => trust_resumed = true,
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            p if path.is_none() => path = Some(p.to_string()),
            _ => return Err("one log at a time".into()),
        }
    }
    Ok(Args {
        path: path.ok_or("no log given")?,
        rules: rules.unwrap_or_default(),
        anchors,
        prev,
        trust_resumed,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let args = match parse_args(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("fleetmesh-replay: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let path = &args.path;
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("fleetmesh-replay: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    // the earlier logs this one carries from: rules 5 intents, rules 6 lost-link values
    let earlier = match Earlier::find(path, &text, &args.prev) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fleetmesh-replay: {e}");
            return ExitCode::from(2);
        }
    };
    for p in &earlier.unused {
        println!("--prev {p}: not a log this one's carried-over intents or lost-link values name; not used");
    }
    let carried_logs: Vec<(&str, &str)> = earlier
        .carried
        .iter()
        .map(|(name, _, t)| (name.as_str(), t.as_str()))
        .collect();
    let fleets: Vec<(&str, &[u8])> = earlier
        .fleets
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    // the fleet this run logged, beside it, for the orders it handed over
    let own_fleet = Path::new(path)
        .file_name()
        .map(|n| fleetmesh_decide::replay::fleet_file_name(&n.to_string_lossy()))
        .map(|n| Path::new(path).with_file_name(n))
        .and_then(|f| std::fs::read(&f).ok().map(|b| (f, b)));
    let opts = ReplayOptions {
        own_fleet: own_fleet.as_ref().map(|(_, b)| b.as_slice()),
        fleets: &fleets,
        carried_logs: &carried_logs,
        rules: args.rules,
        previous: earlier.intents.as_ref().map(|(_, t)| t.as_str()),
        trust_resumed: args.trust_resumed,
        restores_from: earlier
            .restores
            .as_ref()
            .map(|(name, _, t)| (name.as_str(), t.as_str())),
    };
    let report = match replay_str_with(&text, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("fleetmesh-replay: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let anchors = match &args.anchors {
        None => None,
        Some(file) => match std::fs::read_to_string(file) {
            Ok(a) => Some((file, fleetmesh_decide::anchor::verify(&text, &a))),
            Err(e) => {
                eprintln!("fleetmesh-replay: anchors {file}: {e}");
                return ExitCode::from(2);
            }
        },
    };
    let accepted = match args.rules {
        RulesAccepted::Exactly(n) => format!("accepting rules {n} only"),
        RulesAccepted::AtLeast(n) => format!("accepting rules {n} or later"),
    };
    println!(
        "{path}: decision rules version {} ({accepted}); {} events, {} intents, {} checks and {} policy decisions recomputed, {} transitions verified",
        report.rules_version,
        report.events,
        report.intents,
        report.checks_recomputed,
        report.policy_recomputed,
        report.transitions_verified
    );
    if let Some(b) = &report.build {
        let dirty = match b.dirty {
            Some(true) => ", working tree dirty",
            Some(false) => "",
            None => ", working tree unknown",
        };
        println!("written by build {}{dirty}", b.commit);
    }
    // whose decisions these are, as far as the log can say
    match report.auth {
        Some(fleetmesh_decide::AuthMode::Credentials) => {
            let list = |s: &std::collections::BTreeSet<fleetmesh_decide::OperatorId>| {
                if s.is_empty() {
                    "none".to_string()
                } else {
                    s.iter().map(|o| o.as_str()).collect::<Vec<_>>().join(", ")
                }
            };
            println!(
                "auth: credentials ({} step-up(s), {} signature(s) quoting one, {} session(s) ended, {} login(s) refused)",
                report.step_ups, report.stepped_signatures, report.sessions_closed, report.logins_refused
            );
            println!(
                "people (own password or passkey): {}; scripted (harness-held credential, not people): {}",
                list(&report.people),
                list(&report.scripted)
            );
            if args.anchors.is_none() {
                println!(
                    "anchors: NOT checked (pass --anchors <file>): until they are, this log is not evidence about people"
                );
            }
        }
        Some(fleetmesh_decide::AuthMode::Dev) => println!(
            "auth: DEV (operator ids asserted by request headers, not authenticated): not evidence about people; exclude from any human-control claim"
        ),
        None => println!("auth: none recorded: not evidence about people"),
    }
    println!(
        "{} area set(s) logged; {} approval(s), {} self-approved; {} authorized at submission",
        report.area_sets, report.approvals, report.self_approvals, report.approvals_at_submission
    );
    if report.queued + report.rechecks + report.resumed + report.reboots + report.requeued > 0 {
        println!(
            "queue: {} queued, {} sent back after a lost link, {} re-check(s) recomputed ({} dispatched, {} back to the inbox, {} voided), {} expired; {} carried over from an earlier run; {} reboot(s)",
            report.queued,
            report.requeued,
            report.rechecks,
            report.recheck_dispatched,
            report.recheck_inbox,
            report.recheck_voided,
            report.queue_expired,
            report.resumed,
            report.reboots
        );
    }
    if report.handed_over > 0 {
        match (report.handover_fleet, &own_fleet) {
            (Some(true), Some((f, _))) => println!(
                "handover: {} order(s) handed over by the fleet at the start, checked against the run's fleet file {}",
                report.handed_over,
                f.display()
            ),
            _ => println!(
                "UNCHECKED: {} order(s) handed over by the fleet at the start: the run's fleet file (<log stem>.fleet.json) was not found beside this log",
                report.handed_over
            ),
        }
    }
    earlier.print(&report, args.trust_resumed);
    if let Some(head) = &report.head_sha256 {
        println!("head {head} (compare with an external anchor to detect a rewrite or truncation)");
    }
    let anchors_ok = match &anchors {
        None => true,
        Some((file, a)) => {
            let last = a.last_anchored.map_or("none".to_string(), |s| s.to_string());
            println!(
                "{} anchor(s) in {file}: up to seq {last}, {} event(s) after it not covered{}",
                a.anchors,
                a.after_last,
                if a.shutdown { "; ended with a shutdown anchor" } else { "" }
            );
            if a.ok() {
                println!("ANCHORS OK: the log matches every anchor");
            } else {
                println!("ANCHOR MISMATCH: {} problem(s)", a.problems.len());
                print_all(&a.problems);
            }
            a.ok()
        }
    };
    if !report.ok() {
        println!("MISMATCH: {} decision(s) differ from the log", report.mismatches.len());
        print_all(&report.mismatches);
        return ExitCode::from(1);
    }
    println!("OK: every decision reproduced exactly");
    if !anchors_ok {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// The earlier logs this one carries from, each read from `--prev` or found beside it.
struct Earlier {
    /// Rules 5: the log the first `intent.resumed` names, and the one used (path, text).
    intents_named: Option<String>,
    intents: Option<(String, String)>,
    /// Rules 6: the log the carried `platform.restore` events name (the first, if they name
    /// several: a mismatch) as used: its name, path, text.
    restores: Option<(String, String, String)>,
    /// Rules 12: the logs `run.started` says the start carried over from (`carried_from`), as
    /// found: name, path, text.
    carried: Vec<(String, String, String)>,
    /// Rules 15: the fleet files beside the logs carried from (`<log stem>.fleet.json`), as
    /// found: name, bytes.
    fleets: Vec<(String, Vec<u8>)>,
    /// `--prev` files none is.
    unused: Vec<String>,
}

impl Earlier {
    fn find(path: &str, text: &str, given: &[String]) -> Result<Earlier, String> {
        let intents_named = text.lines().find_map(|l| {
            if !l.contains("\"type\":\"intent.resumed\"") {
                return None;
            }
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            v.get("from")?.get("log")?.as_str().map(str::to_string)
        });
        let restores_named: Option<String> = None;
        let carried_named = carried_from_logs(text);
        let named = |n: &str| given.iter().find(|g| file_name(g).as_deref() == Some(n)).cloned();
        let intents_file = intents_named.as_deref().and_then(|n| {
            named(n)
                .or_else(|| {
                    // as before: a single --prev is the intents' log, whatever its name (the
                    // head and last seq the events name bind it), unless it is the restores'
                    let only = (given.len() == 1).then(|| given[0].clone())?;
                    let name = file_name(&only);
                    (name != restores_named && !name.as_ref().is_some_and(|n| carried_named.contains(n)))
                        .then_some(only)
                })
                .or_else(|| beside(path, n))
        });
        let restores_file = restores_named
            .as_deref()
            .and_then(|n| named(n).or_else(|| beside(path, n)));
        let read = |f: &String| std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"));
        let intents = match &intents_file {
            Some(f) => Some((f.clone(), read(f)?)),
            None => None,
        };
        let restores = match (&restores_named, &restores_file) {
            (Some(n), Some(f)) => Some((n.clone(), f.clone(), read(f)?)),
            _ => None,
        };
        let mut carried = Vec::new();
        let mut fleets = Vec::new();
        let mut fleet_files = Vec::new();
        for n in &carried_named {
            if let Some(f) = named(n).or_else(|| beside(path, n)) {
                let t = read(&f)?;
                carried.push((n.clone(), f, t));
            }
            // rules 15: a failed carry's fleet file, compared with what the start recorded
            let fleet = fleetmesh_decide::replay::fleet_file_name(n);
            if let Some(f) = named(&fleet).or_else(|| beside(path, &fleet)) {
                let b = std::fs::read(&f).map_err(|e| format!("{f}: {e}"))?;
                fleets.push((fleet, b));
                fleet_files.push(f);
            }
        }
        let unused = given
            .iter()
            .filter(|g| {
                Some(*g) != intents_file.as_ref()
                    && Some(*g) != restores_file.as_ref()
                    && !carried.iter().any(|(_, f, _)| f == *g)
                    && !fleet_files.contains(*g)
            })
            .cloned()
            .collect();
        Ok(Earlier {
            intents_named,
            intents,
            restores,
            carried,
            fleets,
            unused,
        })
    }

    /// Which earlier log each check used, and what was not checked for want of one.
    fn print(&self, report: &ReplayReport, trusted: bool) {
        if report.resumed > 0 {
            let from = self.intents_named.as_deref().unwrap_or("?");
            match &self.intents {
                Some((p, _)) => println!(
                    "carried-over intents: {} from {from}, checked against {p}",
                    report.resumed
                ),
                None if trusted => println!(
                    "UNCHECKED: {} intent(s) carried over from {from}: trusted (--trust-resumed); that log was not given (--prev) or found beside this one",
                    report.resumed
                ),
                None => println!(
                    "carried-over intents: {} from {from}, NOT checked: that log was not given (--prev) or found beside this one",
                    report.resumed
                ),
            }
        }
        for c in &report.carried_restores {
            let from = &c.log;
            // rules 12: a log the start could not read: still owed, what it owed unknown
            if let Some(why) = &c.failed {
                let fleet = match c.fleet_checked {
                    Some(true) => "; its fleet file checked against what the start recorded",
                    Some(false) => "; its fleet file NOT checked (not given with --prev or found beside this log)",
                    None => "",
                };
                // rules 15: a log found in place of the pointer that the start would not carry from
                let found = c
                    .pointer_failed
                    .as_deref()
                    .map(|p| format!(" (found in place of the station's pointer, which could not be read: {p})"))
                    .unwrap_or_default();
                println!(
                    "carried-over log {from}{found}: its carry FAILED at the start ({why}): what it still owed is unknown, owed until a later start reads it or an operator writes it off{fleet}"
                );
                continue;
            }
            if let Some(why) = &c.pointer_failed {
                println!(
                    "carried-over log {from}: found in place of the station's pointer, which could not be read ({why})"
                );
            }
            if let Some((_, p, t)) = self.carried.iter().find(|(n, _, _)| n == from).filter(|_| c.checked) {
                let same = match &self.intents {
                    Some((_, u)) if u == t => ", the carried-over intents' log too",
                    _ => "",
                };
                let head = c.head.as_deref().unwrap_or("none: it can't be read");
                println!(
                    "carried over at start (run.started): {} platform(s)' lost-link values from {from}, checked against what {p} still owed{same} (its head {head}, as read at the start: compare with its anchors)",
                    c.restores
                );
                continue;
            }
            match &self.restores {
                Some((name, p, t)) if c.checked && name == from => {
                    let same = match &self.intents {
                        Some((_, u)) if u == t => ", the carried-over intents' log too",
                        _ => "",
                    };
                    let head = c.head.as_deref().unwrap_or("none: it can't be read");
                    println!(
                        "carried lost-link values: {} platform(s) from {from}, checked against what {p} still owed{same} (its head {head}: compare with its anchors)",
                        c.restores
                    );
                }
                _ if trusted => println!(
                    "UNCHECKED: {} platform(s)' carried lost-link values from {from}: trusted (--trust-resumed); that log was not given (--prev) or found beside this one",
                    c.restores
                ),
                _ => println!(
                    "carried lost-link values: {} platform(s) from {from}, NOT checked: that log was not given (--prev) or found beside this one",
                    c.restores
                ),
            }
        }
    }
}

/// A path's file name, as a log's carried events name a log.
fn file_name(p: &str) -> Option<String> {
    Path::new(p)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
}

/// The file `name` beside the log at `path`, if `name` is a bare file name (never a path out
/// of the log's directory), the file exists and is another file than the log.
fn beside(path: &str, name: &str) -> Option<String> {
    if !is_log_file_name(name) {
        return None;
    }
    let dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let file = dir.join(name);
    let same = match (file.canonicalize(), Path::new(path).canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    (file.exists() && !same).then(|| file.display().to_string())
}

fn print_all(ms: &[Mismatch]) {
    for m in ms {
        let intent = m.intent.map(|i| format!(" {i}")).unwrap_or_default();
        println!("  seq {}{intent}: {}", m.seq, m.what);
        if !m.logged.is_empty() || !m.recomputed.is_empty() {
            println!("    logged:     {}", m.logged);
            println!("    recomputed: {}", m.recomputed);
        }
    }
}
