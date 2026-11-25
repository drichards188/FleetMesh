//! Replay re-derives every decision of a run for every kept intent kind (orbit, patrol, hold,
//! rtl, transit), on the shipped policy (`sim/policy.toml`) and areas (`sim/areas`), under the
//! current rules version: a recorded run replays to identical decisions and the identical hash
//! chain; an edited or removed entry is found; a run claiming another rules version is refused;
//! and the version constants keep the convention of docs/design/rules-versioning.md. The logs of
//! each version replaying under their own rules are in tests/delivery.rs (rules 1 and 2).

use std::path::PathBuf;

use fleetmesh_decide::hash::sha256_hex;
use fleetmesh_decide::*;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn facts(id: &str, endurance_h: Option<f64>, t: Time) -> VehicleFacts {
    VehicleFacts {
        platform: Some(PlatformFacts {
            id: id.into(),
            domain: Domain::Air,
            cruise_mps: Some(12.0),
            min_turn_radius_m: None,
            endurance_h,
            autopilot: Some(AutopilotKind::Copter),
            sensors: Vec::new(),
            climb_mps: None,
            max_mps: None,
            payload_kg: None,
            typical_alt_m: None,
            draft_m: None,
            land_now: None,
            formation: None,
        }),
        autopilot: Some(Fact::new(AutopilotKind::Copter, t)),
        nav_ready: Some(Fact::new(true, t)),
        position: Some(Fact::new(LonLat::new(-122.45, 37.80), t)),
        home: Some(Fact::new(LonLat::new(-122.45, 37.80), t)),
        battery_pct: Some(Fact::new(95.0, t)),
        nav_source: None,
        report_interval_ms: None,
        velocity_mps: None,
        gps_fix: None,
        gps_glitch: None,
        gps_fix_from: None,
        circling: None,
        alt_msl_m: None,
        booted_t: None,
        damage: None,
        lost_link: None,
        reach_now: None,
    }
}

/// A recorded run: five rated drones take one order each of the five kinds (policy sends the
/// safe stops and the routine kinds at once), a sixth with no endurance figure gets an agent's
/// proposal that waits for a person, who approves it; every order is dispatched and ends.
fn record() -> String {
    let t = Time::from_secs(1_000);
    let policy = Policy::load(&root().join("sim/policy.toml")).expect("the shipped policy");
    let roster = Roster::new(vec![OperatorId::new("op-1")], Vec::new()).expect("roster");
    let clock = ManualClock::new(t);
    let log = MemorySink::new();
    let mut engine = Engine::new(policy, roster, Box::new(clock.clone()), Box::new(log.clone())).expect("engine");
    let mut world = WorldSnapshot::new(t);
    world.areas = Some(AreaSet::load_dir(&root().join("sim/areas"), t).expect("areas"));
    for n in 1..=5 {
        world.vehicles.insert(Uid::new(format!("sky-{n}")), facts("delivery_drone", Some(0.4), t));
    }
    world.vehicles.insert(Uid::new("sky-6"), facts("delivery_unrated", None, t));
    let at = LonLat::new(-122.446, 37.80);
    let near = vec![LonLat::new(-122.446, 37.80), LonLat::new(-122.440, 37.802)];
    let tasks = [
        Task::Orbit { center: at, radius_m: 150.0 },
        Task::Patrol { route: near.clone() },
        Task::hold(),
        Task::Rtl {},
        Task::Transit { route: near },
    ];
    let mut ids = Vec::new();
    for (n, task) in tasks.into_iter().enumerate() {
        let uid = Uid::new(format!("sky-{}", n + 1));
        let id = engine
            .agent_port(AgentId::new("planner"))
            .propose(Proposal::new(task, vec![uid]), Rationale::new("routine delivery leg").expect("rationale"), &world, &NoTerrain)
            .expect("proposed")
            .id();
        ids.push(id);
    }
    let waits = engine
        .agent_port(AgentId::new("planner"))
        .propose(
            Proposal::new(Task::Orbit { center: at, radius_m: 150.0 }, vec![Uid::new("sky-6")]),
            Rationale::new("watch the pickup site").expect("rationale"),
            &world,
            &NoTerrain,
        )
        .expect("proposed")
        .id();
    assert_eq!(engine.intent(waits).expect("intent").state(), State::PendingApproval, "no endurance figure: a person decides");
    clock.advance_ms(5_000);
    let session = engine.open_operator_session(OperatorId::new("op-1")).expect("session");
    engine.approve(waits, session.authority(), Some("clear".into()), &world, &NoTerrain).expect("approved");
    ids.push(waits);
    for id in &ids {
        engine.dispatch(*id, &world).expect("dispatched");
    }
    clock.advance_ms(5_000);
    for id in &ids {
        engine.finish(*id, TaskOutcome::Done).expect("finished");
    }
    log.text()
}

#[test]
fn a_recorded_run_replays_to_identical_decisions_and_the_identical_chain() {
    let text = record();
    let first = replay::replay_str(&text).expect("parses");
    assert!(first.ok(), "{:?}", first.mismatches);
    // every decision of every kept kind was re-derived, none taken on trust
    assert_eq!(first.intents, 6);
    assert_eq!(first.rules_version, fleetmesh_decide::rules::RULES_VERSION);
    assert!(first.checks_recomputed >= 6, "{}", first.checks_recomputed);
    assert!(first.policy_recomputed >= 6, "{}", first.policy_recomputed);
    assert_eq!((first.approvals, first.self_approvals), (1, 0));
    assert!(first.transitions_verified >= 6 * 4, "{}", first.transitions_verified);
    // the chain: the head replay computes is the hash of the log's own last line
    let last = text.lines().rfind(|l| !l.trim().is_empty()).expect("a last line");
    assert_eq!(first.head_sha256.as_deref(), Some(sha256_hex(last.as_bytes()).as_str()));
    // replaying again gives the identical report
    let again = replay::replay_str(&text).expect("parses");
    assert_eq!(first, again);
}

#[test]
fn an_edited_or_removed_entry_is_found() {
    let text = record();
    let lines: Vec<&str> = text.lines().collect();
    assert!(replay::replay_str(&text).expect("parses").ok());
    // an edited decision: the policy's tier rewritten
    let i = lines.iter().position(|l| l.contains("\"type\":\"policy.decision\"")).expect("a policy decision");
    let mut edited: Vec<String> = lines.iter().map(ToString::to_string).collect();
    edited[i] = edited[i].replacen("\"tier\":\"auto\"", "\"tier\":\"human\"", 1);
    assert_ne!(edited[i], lines[i], "the edit changed the line");
    let r = replay::replay_str(&edited.join("\n")).expect("parses");
    assert!(!r.ok(), "an edited policy decision goes unnoticed");
    // a removed entry: the chain and the sequence break at the gap
    let mut removed: Vec<&str> = lines.clone();
    removed.remove(lines.len() / 2);
    let r = replay::replay_str(&removed.join("\n")).expect("parses");
    assert!(!r.ok(), "a removed entry goes unnoticed");
    assert!(r.mismatches.iter().any(|m| m.what.contains("hash chain broken") || m.what.contains("seq not contiguous")), "{:?}", r.mismatches);
    // a line that is not its event's canonical serialization (an added field the parser ignores)
    let mut padded: Vec<String> = lines.iter().map(ToString::to_string).collect();
    padded[2] = padded[2].replacen('{', "{\"extra\":1,", 1);
    let r = replay::replay_str(&padded.join("\n"));
    assert!(r.map_or(true, |r| !r.ok()), "an added field goes unnoticed");
}

#[test]
fn a_run_with_an_unknown_rules_version_is_refused() {
    let text = record();
    let mut events = audit::parse_log(&text).expect("parses");
    let Some(EventBody::RunStarted { rules_version, .. }) = events.first_mut().map(|e| &mut e.body) else {
        panic!("a log starts with run.started");
    };
    *rules_version = Some(fleetmesh_decide::rules::RULES_NEWEST + 1);
    let r = replay::replay(&events);
    assert!(!r.ok());
    assert!(r.mismatches.iter().any(|m| m.what.contains("newer decision rules")), "{:?}", r.mismatches);
    // and an older one, which this build has no rules for, is refused by default too
    let mut events = audit::parse_log(&text).expect("parses");
    let Some(EventBody::RunStarted { rules_version, .. }) = events.first_mut().map(|e| &mut e.body) else {
        panic!("a log starts with run.started");
    };
    *rules_version = Some(0);
    let r = replay::replay(&events);
    assert!(!r.ok(), "rules version 0 is no version");
}

// ---- the version constants (docs/design/rules-versioning.md section 7) --------------------------

fn source(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Every `pub const <NAME>_SINCE: u32 = N;` in rules.rs, by name.
fn since_constants() -> Vec<(String, u32)> {
    let text = source("crates/fleetmesh-decide/src/rules.rs");
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("pub const ")?;
            let (name, value) = rest.split_once(": u32 = ")?;
            if !name.ends_with("_SINCE") {
                return None;
            }
            Some((name.to_string(), value.trim_end_matches(';').trim().parse().ok()?))
        })
        .collect()
}

#[test]
fn every_since_constant_is_a_version_this_build_has_and_is_used_and_replayed() {
    use fleetmesh_decide::rules::{RULES_MARKERS, RULES_NEWEST, RULES_VERSION};
    let consts = since_constants();
    assert!(!consts.is_empty(), "rules.rs defines the *_SINCE constants");
    let decide_src: String = ["delivery_rules.rs", "rules.rs", "engine.rs", "replay.rs", "policy.rs"]
        .iter()
        .map(|f| source(&format!("crates/fleetmesh-decide/src/{f}")))
        .collect();
    let tests = source("crates/fleetmesh-decide/tests/delivery.rs") + &source("crates/fleetmesh-decide/tests/replay.rs");
    for (name, v) in &consts {
        assert!(*v >= 1 && *v <= RULES_NEWEST, "{name} = {v} is not a version this build has (1..={RULES_NEWEST})");
        assert!(decide_src.contains(&format!("at_least({name})")) || decide_src.contains(&format!("at_least(crate::rules::{name})")), "{name} is defined but no code branches on it with at_least");
        // a replay test names the constant it covers (beside this list)
        assert!(tests.contains(name.as_str()), "no replay test names {name} (rules {v})");
    }
    const { assert!(RULES_VERSION <= RULES_NEWEST) };
    for (since, what) in RULES_MARKERS {
        assert!(*since <= RULES_VERSION, "marker {what} is for rules {since}, newer than {RULES_VERSION}");
    }
}

#[test]
fn no_code_compares_the_rules_version_with_a_literal() {
    // a behaviour is gated on a named constant (`cfg.at_least(NAME_SINCE)`), never a number
    let dirs = ["crates/fleetmesh-decide/src", "crates/fleetmesh-bridge/src"];
    let mut found = Vec::new();
    for d in dirs {
        let mut stack = vec![root().join(d)];
        while let Some(p) = stack.pop() {
            for e in std::fs::read_dir(&p).expect("dir").flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&path).expect("source");
                    for (n, l) in text.lines().enumerate() {
                        let code = l.split("//").next().unwrap_or("");
                        for pat in ["version >= ", "version > ", "version < ", "version <= ", "at_least("] {
                            if let Some(i) = code.find(pat) {
                                let after = &code[i + pat.len()..];
                                if after.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                                    found.push(format!("{}:{}: {}", path.display(), n + 1, l.trim()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(found.is_empty(), "the rules version compared with a literal: {found:#?}");
}
