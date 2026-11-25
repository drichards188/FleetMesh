//! murmur-dup: a wall clock that steps or drifts, through the bridge. This WSL host's wall
//! clock stepped back 0.5 to 0.9 s every 32 s for the first two hours or so after boot while
//! its monotonic clock ran fast against the host's; NTP and a resumed virtual machine step a
//! wall clock either way. Each test steps (or drifts) this thread's wall clock
//! (`clock::test_clock`) where that would have moved what the bridge decides, with the engine
//! on the bridge's own clock (`clock::wall`, steered toward the wall clock within a bound,
//! never stepping): before the bridge kept a clock of its own, the facts' stamps, the engine's
//! clock and every age taken between them read the wall clock itself and the step tests
//! failed; on a clock that only followed the monotonic one the drift test failed.

use crate::decision::queue::CarryCheck;
use crate::tasks::tests::Firmware;
use fleetmesh_decide::replay::{ReplayOptions, replay_str_with};

use super::queue::go_silent;
use super::*;

/// A copter's heartbeat may be this old (`max_fact_age_s`; three of its 1 s report
/// intervals are less).
const LIMIT_S: f64 = 10.0;

/// A rig whose waiting intents wait for a human, its engine on the bridge's clock.
fn waiting_bridge_rig() -> Rig {
    rig_clocked(waiting_policy(), fleetmesh_decide::AuthMode::Dev, EngineClock::Bridge)
}

#[tokio::test(start_paused = true)]
async fn a_wall_clock_stepping_forward_voids_nothing() {
    let r = waiting_bridge_rig();
    r.vehicle(7, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let waiting = r.d.submit(&op1(), orbit(7)).await.expect("submit").id;
    assert_eq!(r.state(waiting), State::PendingApproval);
    // an hour forward (an NTP correction, a resumed VM): the vehicle was heard a moment ago
    clock::test_clock::step_utc(3_600.0);
    r.d.tick();
    assert_eq!(r.state(waiting), State::PendingApproval, "{}", r.reason(waiting));
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn a_vehicle_silent_past_its_limit_is_voided_across_a_backward_step() {
    let r = waiting_bridge_rig();
    r.vehicle(7, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let waiting = r.d.submit(&op1(), orbit(7)).await.expect("submit").id;
    let cell = r.world.vehicles().into_iter().find(|c| c.uid == 7).expect("cell");
    cell.write(|v| v.seen.heartbeat = Some(clock::wall() - (LIMIT_S + 0.5)));
    // this host's step: its silence must not read half a second short of the limit
    clock::test_clock::step_utc(-0.94);
    r.d.tick();
    assert_eq!(r.state(waiting), State::Voided, "{}", r.reason(waiting));
    assert!(r.reason(waiting).contains("lost"), "{}", r.reason(waiting));
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn a_wall_clock_stepping_forward_leaves_the_facts_fresh() {
    let policy = Policy::load(&root().join("sim/policy.toml")).expect("policy");
    let r = rig_clocked(policy, fleetmesh_decide::AuthMode::Dev, EngineClock::Bridge);
    r.vehicle(9, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    clock::test_clock::step_utc(120.0);
    let out = r.d.submit(&op1(), orbit(9)).await.expect("submit");
    let text = out.intent.to_string();
    assert!(!text.contains(" s old (limit"), "{text}");
    assert_eq!(out.state, State::Dispatched, "{text}");
    r.replays();
}




/// A second bridge on the same vehicles, as after a crash and a restart on the same host:
/// its engine on the bridge's clock, which starts at the wall clock again.
fn restart_on_the_bridge_clock(r: &Rig) -> (Arc<Decision>, MemorySink) {
    clock::test_clock::restart();
    let root = root();
    let log = MemorySink::new();
    let d = Decision::from_parts(
        r.world.clone(),
        Parts {
            policy: Policy::load(&root.join("sim/policy.toml")).expect("policy"),
            catalog: Catalog::load(&crate::tasks::tests::test_platforms()).expect("catalog"),
            areas: AreaSet::load_dir(&root.join("sim/areas"), fleetmesh_decide::Clock::now(&clock::BridgeClock))
                .expect("areas"),
            sink: Box::new(log.clone()),
            audit_path: PathBuf::from("memory-b"),
            clock: Arc::new(clock::BridgeClock),
            terrain: Terrain::fixed(PolygonTerrain {
                coverage: ring(-122.6, 37.7, -122.3, 37.9),
                land: vec![ring(-122.48, 37.79, -122.44, 37.808)],
                water_depth_m: 12.0,
            }),
            operators: vec![OperatorId::new("op-1"), OperatorId::new("op-2")],
            consequential_operators: Vec::new(),
            auth: fleetmesh_decide::AuthMode::Dev,
            passkeys: None,
            study: None,
            scripted: Vec::new(),
            carried_from: Vec::new(),
            pointer: None,
            policy_layers: Vec::new(),
        },
    )
    .expect("engine");
    (d, log)
}

/// The review's probe (round 2): early after boot the monotonic clock ran 2.1 % fast
/// against the wall clock, so a bridge clock that only followed it ran 150 s ahead after two
/// hours; a run restarted then starts at the wall clock, and every deadline the crashed run's
/// log carried (a human's approval is held 120 s in the queue) was 150 s later than meant: an
/// approval 30 s old at the restart was held another 240 s. Steered within its bound, the
/// carried approval goes back to the inbox 90 s after the restart, give or take the bound.
#[tokio::test(start_paused = true)]
async fn a_carried_deadline_holds_after_hours_of_a_drifting_clock() {
    clock::test_clock::pin_drifting(T0_S, 0.021);
    // two hours of the bridge's clock read now and then before this run's work
    for _ in 0..120 {
        sleep(Duration::from_secs(60)).await;
        let _ = clock::wall();
    }
    let ahead = clock::wall() - clock::utc();
    assert!(ahead.abs() <= 2.0, "the bridge's clock {ahead:.1} s from the wall clock");
    let r = rig_clocked(
        Policy::load(&root().join("sim/policy.toml")).expect("policy"),
        fleetmesh_decide::AuthMode::Dev,
        EngineClock::Bridge,
    );
    // armed, so a human tier: the operator's own order is its approval; the vehicle is silent
    let (cell, w) = r.watched(46, Firmware::Copter, "delivery_unrated", (LAT, LON), crate::tasks::tests::quick());
    sleep(Duration::from_secs(1)).await;
    go_silent(&cell, &w, 5.0);
    let out = r.d.submit(&op1(), orbit(46)).await.expect("submit");
    assert_eq!(out.state, State::Queued, "{}", r.reason(out.id));
    for _ in 0..30 {
        sleep(Duration::from_secs(1)).await;
        r.d.tick();
    }
    assert_eq!(r.state(out.id), State::Queued);
    // the bridge dies 30 s after the approval; another starts and carries the order over
    let (b, blog) = restart_on_the_bridge_clock(&r);
    // a new process has heard nothing from the silent vehicle (this rig's world outlives the
    // first bridge, with its stamps on the first bridge's clock)
    cell.write(|v| v.seen = Default::default());
    let carried = b.carry_over("a.jsonl", &r.log.text(), &CarryCheck::default()).expect("carried");
    assert_eq!(carried, vec![out.id]);
    let state = |id: IntentId| b.lock().engine.intent(id).map(Intent::state);
    let mut held = None;
    for s in 1..=300 {
        sleep(Duration::from_secs(1)).await;
        b.tick();
        if state(out.id) != Some(State::Queued) {
            held = Some(s);
            break;
        }
    }
    let held = held.expect("still queued 300 s after the restart");
    // back to the inbox (and, its vehicle silent, voided there in the same second)
    let back = b
        .lock()
        .engine
        .intent(out.id)
        .and_then(|i| i.history().iter().find(|h| h.state == State::PendingApproval).cloned())
        .expect("back to the inbox");
    assert!(
        (87..=94).contains(&held),
        "back to the inbox {held} s after the restart, 90 meant: {}",
        back.reason
    );
    let a_text = r.log.text();
    let rb = replay_str_with(
        &blog.text(),
        &ReplayOptions {
            previous: Some(&a_text),
            ..ReplayOptions::default()
        },
    )
    .expect("parses");
    assert!(rb.ok(), "{:?}", rb.mismatches);
}
