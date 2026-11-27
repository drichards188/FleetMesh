//! Rules 8 through the bridge (docs/design/incidents.md): a person adopts a play; the dispatcher
//! withdraws its own held orders the play holds back (never an urgent parcel's, never as a try);
//! the plays show in the fleet view and the Inbox; a person lifts it; and the log replays.

use super::*;
use crate::decision::plays::PlayAsk;
use fleetmesh_decide::plays::{PlayKind, PlayLimits};

fn ground_stop(hub: &str) -> PlayAsk {
    PlayAsk { kind: PlayKind::GroundStop, hub: hub.into(), codes: vec![], limits: PlayLimits::default(), except_priority: Some(3), for_s: Some(600), lift: None }
}

/// A held order of the dispatcher's for vehicle 7's delivery of `parcel` (a confirmation, which
/// always waits for a person, stands in for a held delivery).
async fn held(r: &Rig, parcel: &str) -> IntentId {
    let zone = r.d.places().drop_zones.keys().next().expect("a drop zone").clone();
    let req = IntentRequest {
        task: "confirm_drop_zone".into(),
        uids: vec![7],
        params: crate::tasks::TaskParams { parcel: Some(parcel.into()), drop_zone: Some(zone), ..Default::default() },
        constraints: Default::default(),
        rationale: Some("a held order of the dispatcher's, for the test".into()),
        group: None,
    };
    let id = r.d.agent_propose(&fleetmesh_decide::AgentId::new("dispatcher"), req).await.expect("proposed").id;
    r.d.test_job("7", parcel, true, id);
    id
}

fn state(r: &Rig, id: IntentId) -> fleetmesh_decide::State {
    r.d.lock().engine.intent(id).expect("intent").state()
}

#[tokio::test(start_paused = true)]
async fn a_ground_stop_withdraws_the_dispatchers_held_orders_but_an_urgent_parcels() {
    let r = rig();
    r.vehicle(7, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let zone = r.d.places().drop_zones.keys().next().expect("a drop zone").clone();
    r.d.test_parcel("p-1", "hub-presidio", &zone, 1.0, 1);
    let id = held(&r, "p-1").await;
    assert_eq!(state(&r, id), fleetmesh_decide::State::PendingApproval);
    let play = r.d.adopt_play(&op1(), ground_stop("hub-presidio")).expect("adopted");
    assert_eq!(play["kind"], "ground_stop");
    assert_eq!(state(&r, id), fleetmesh_decide::State::Voided, "the dispatcher withdrew it for the play");
    assert_eq!(r.d.test_by_play("7"), Some(true), "withdrawn for a play: not a try");
    // in force: in the fleet view's base, in the Inbox, and listed
    let base = r.d.fleet_json()["bases"].as_array().expect("bases").iter().find(|b| b["id"] == "hub-presidio").cloned().expect("hub");
    assert_eq!(base["plays"].as_array().map(Vec::len), Some(1));
    assert_eq!(r.d.inbox_json()["plays"].as_array().map(Vec::len), Some(1));
    // an urgent parcel's held order stays the person's
    r.d.test_parcel("p-2", "hub-presidio", &zone, 1.0, 3);
    let urgent = held(&r, "p-2").await;
    let other = fleetmesh_decide::plays::Play { id: 9, ..serde_json::from_value(play.clone()).expect("a play") };
    r.d.apply_play(&other);
    assert_eq!(state(&r, urgent), fleetmesh_decide::State::PendingApproval, "urgent: outside the play");
    // lifted by a person
    let pid = play["id"].as_u64().expect("id");
    r.d.lift_play(&op1(), pid).expect("lifted");
    assert!(r.d.plays_json()["plays"].as_array().expect("plays").is_empty());
    assert!(r.d.lift_play(&op1(), pid).is_err(), "lifted already");
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn a_play_is_refused_for_an_unknown_hub_or_outside_the_policy() {
    let r = rig();
    assert!(r.d.adopt_play(&op1(), ground_stop("hub-nowhere")).is_err());
    assert!(r.d.adopt_play(&op1(), PlayAsk { for_s: Some(99_999), ..ground_stop("hub-presidio") }).is_err(), "longer than [plays] max_s");
    assert!(r.d.adopt_play(&op1(), PlayAsk { kind: PlayKind::Limits, ..ground_stop("hub-presidio") }).is_err(), "a limits play names its codes and limits");
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn the_officer_recommends_a_play_for_an_incident_it_sees_and_no_one_else_does() {
    let r = rig();
    // at the Crissy Field hub (hub-presidio in sim/hubs.toml)
    r.vehicle(7, Firmware::Copter, "delivery_drone", (37.8040, -122.4650));
    sleep(Duration::from_millis(500)).await;
    for k in ["a", "b", "c"] {
        let until = fleetmesh_decide::Clock::now(&r.clock).plus_ms(600_000);
        r.d.raise(
            format!("traffic:{k}"),
            crate::decision::delivery_rt::Issue {
                kind: "traffic_noncooperative_near",
                uid: Some(fleetmesh_decide::Uid::new("7")),
                problem: "a drone broadcasting nothing near the hub".into(),
                did: "automation keeps clear of it".into(),
                recommend: "nothing for safety".into(),
                at_wall_s: clock::wall(),
                until_ms: Some(until.0),
                intent: None,
                track_ids: Vec::new(),
            },
        );
    }
    r.d.tick();
    let view = r.d.inbox_json();
    let inc = view["incidents"].as_array().and_then(|l| l.first()).cloned().expect("an incident");
    assert_eq!((inc["id"].as_str(), inc["alerts"].as_u64()), (Some("traffic@hub-presidio"), Some(3)), "{inc}");
    let officer = fleetmesh_decide::AgentId::new("agent-watch");
    let stop = || PlayLimits::default();
    assert!(r.d.recommend_play(&fleetmesh_decide::AgentId::new("agent-planner"), "traffic@hub-presidio", PlayKind::GroundStop, stop(), "why").is_err(), "only the watch officer");
    assert!(r.d.recommend_play(&officer, "weather@hub-presidio", PlayKind::GroundStop, stop(), "why").is_err(), "an incident it does not see");
    r.d.recommend_play(&officer, "traffic@hub-presidio", PlayKind::GroundStop, stop(), "three drones with no Remote ID by the hub: hold departures until they go").expect("recommended");
    let recs = r.d.inbox_json()["recommended_plays"].clone();
    assert_eq!(recs.as_array().map(Vec::len), Some(1), "{recs}");
    assert_eq!(recs[0]["kind"], "ground_stop");
    assert!(r.log.text().contains("play.recommended"), "logged as the officer's action");
    r.replays();
}
