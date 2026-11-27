//! Rules 7 through the bridge (docs/design/watch-officer.md): with the watch officer on watch an
//! alert goes to it first and stays out of the person's Inbox until it says or its deadline
//! passes; a held order it turns down is the dispatcher's own withdrawal; and the log replays.

use super::*;
use crate::decision::delivery_rt::Issue;
use crate::decision::triage::VerdictIn;
use fleetmesh_decide::officer::Verdict;

fn officer() -> fleetmesh_decide::AgentId {
    fleetmesh_decide::AgentId::new("agent-watch")
}

fn issue(r: &Rig, kind: &'static str, id: &str) {
    let until = fleetmesh_decide::Clock::now(&r.clock).plus_ms(600_000);
    r.d.raise(
        id.into(),
        Issue {
            kind,
            uid: Some(fleetmesh_decide::Uid::new("7")),
            problem: "p-9 was not delivered".into(),
            did: "7 kept it and is bringing it back".into(),
            recommend: "nothing is needed for safety".into(),
            at_wall_s: clock::wall(),
            until_ms: Some(until.0),
            intent: None,
            track_ids: Vec::new(),
        },
    );
}

fn person_sees(r: &Rig, id: &str) -> Option<Value> {
    r.d.inbox_json()["alerts"].as_array().expect("alerts").iter().find(|a| a["id"] == id).cloned()
}

fn officer_sees(r: &Rig, id: &str) -> Option<Value> {
    r.d.inbox_json_for_agents()["alerts"].as_array().expect("alerts").iter().find(|a| a["id"] == id).cloned()
}

#[tokio::test(start_paused = true)]
async fn an_alert_goes_to_the_officer_first_and_reaches_the_person_only_as_it_says_or_at_its_deadline() {
    let r = rig();
    r.vehicle(7, Firmware::Copter, "delivery_drone", (LAT, LON));
    // with no officer, the person sees an alert at once
    issue(&r, "drop_not_clear", "delivery:p-1");
    r.d.tick();
    assert!(person_sees(&r, "delivery:p-1").is_some());
    // the officer comes on watch: what the person has stays theirs, what comes next goes to it
    r.d.agent_seen(&officer());
    r.d.tick();
    issue(&r, "drop_not_clear", "delivery:p-9");
    assert!(person_sees(&r, "delivery:p-9").is_none(), "arriving: not the person's yet");
    r.d.tick();
    assert!(person_sees(&r, "delivery:p-1").is_some());
    assert!(person_sees(&r, "delivery:p-9").is_none());
    let mine = officer_sees(&r, "delivery:p-9").expect("the officer sees it");
    assert_eq!((mine["triage"]["stage"].as_str(), mine["triage"]["item"].as_str()), (Some("with_officer"), Some("alert:delivery:p-9")));
    // handled: it stays out of the person's Inbox and in the audit trail
    let v = VerdictIn { verdict: Verdict::Handled, note: "automation kept the parcel; it is tried again in 10 min".into(), recommendation: None, intents: vec![] };
    r.d.triage_verdict(&officer(), "alert:delivery:p-9", v).expect("the policy lets it handle a drop_not_clear");
    assert!(person_sees(&r, "delivery:p-9").is_none());
    assert_eq!(r.d.triage_json()["counts"]["handled"], 1);
    // one it may not handle; then one it leaves: the deadline sends it to the person
    issue(&r, "nav_monitor", "nav:7");
    r.d.tick();
    let v = VerdictIn { verdict: Verdict::Handled, note: "fine".into(), recommendation: None, intents: vec![] };
    assert!(r.d.triage_verdict(&officer(), "alert:nav:7", v).is_err(), "nav_monitor is not the officer's to settle");
    r.d.agent_seen(&officer());
    let alert_s = r.d.lock().engine.policy().officer.as_ref().expect("[officer]").deadline_s.alert;
    r.clock.advance_ms(i64::from(alert_s) * 1000 + 1_000);
    r.d.tick();
    let shown = person_sees(&r, "nav:7").expect("the deadline passed: the person's");
    assert_eq!(shown["triage"]["stage"], "untriaged");
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn a_turned_down_order_is_the_dispatchers_own_withdrawal() {
    let r = rig();
    r.vehicle(7, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let zone = r.d.places().drop_zones.keys().next().expect("a drop zone").clone();
    let req = IntentRequest {
        task: "confirm_drop_zone".into(),
        uids: vec![7],
        params: crate::tasks::TaskParams { parcel: Some("p-1".into()), drop_zone: Some(zone), ..Default::default() },
        constraints: Default::default(),
        rationale: Some("a held order of the dispatcher's, for the test".into()),
        group: None,
    };
    let id = r.d.agent_propose(&fleetmesh_decide::AgentId::new("dispatcher"), req).await.expect("proposed").id;
    {
        let core = r.d.lock();
        let i = core.engine.intent(id).expect("intent");
        assert_eq!(i.state(), fleetmesh_decide::State::PendingApproval, "{:?}", i.checks());
    }
    // not one of the dispatcher's jobs: refused, nothing withdrawn
    assert!(r.d.officer_turn_down(id, &officer(), "thin margin").is_err());
    r.d.test_job("7", "p-1", true, id);
    r.d.officer_turn_down(id, &officer(), "the margin home is thin over water").expect("turned down");
    let core = r.d.lock();
    assert_eq!(core.engine.intent(id).expect("intent").state(), fleetmesh_decide::State::Voided);
    drop(core);
    assert_eq!(r.d.test_turned_down("7"), Some(true));
    let log = r.log.text();
    assert!(log.contains("turned down by the watch officer agent-watch: the margin home is thin over water"), "the dispatcher's withdrawal carries the officer's reason");
    r.replays();
}

/// The Inbox read while the decision loop ticks, with the officer on watch and items in triage:
/// never a deadlock (the read took triage's lock, then the decision lock, while the tick took them
/// the other way round, and a live bridge hung on its next stop).
#[tokio::test(start_paused = true)]
async fn the_inbox_is_read_while_the_loop_ticks_without_a_deadlock() {
    let r = rig();
    r.vehicle(7, Firmware::Copter, "delivery_drone", (LAT, LON));
    r.d.agent_seen(&officer());
    r.d.tick();
    for n in 0..20 {
        issue(&r, "drop_not_clear", &format!("delivery:p-{n}"));
    }
    let d = r.d.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for _ in 0..400 {
            let _ = d.inbox_json();
            let _ = d.inbox_json_for_agents();
            let _ = d.triage_json();
            let _ = d.triage_waiting();
        }
        let _ = tx.send(());
    });
    let ticker = {
        let d = r.d.clone();
        std::thread::spawn(move || {
            for _ in 0..400 {
                d.agent_seen(&officer());
                d.tick();
            }
        })
    };
    assert!(rx.recv_timeout(std::time::Duration::from_secs(30)).is_ok(), "the Inbox read and the tick deadlocked");
    reader.join().expect("reader");
    ticker.join().expect("ticker");
}

#[tokio::test(start_paused = true)]
async fn a_vehicle_whose_item_is_with_the_officer_does_not_ask_for_the_person() {
    // the fleet view (a hub's "need you", a fleet row) says what the person's Inbox says: an item
    // the officer still holds is the officer's, one it passed on is the person's
    let r = rig();
    r.vehicle(7, Firmware::Copter, "delivery_drone", (LAT, LON));
    r.d.agent_seen(&officer());
    r.d.tick();
    issue(&r, "drop_not_clear", "delivery:p-9");
    r.d.tick();
    let row = |r: &Rig| {
        let f = r.d.fleet_json();
        let v = f["vehicles"].as_array().expect("vehicles").iter().find(|v| v["uid"] == 7).cloned().expect("vehicle 7");
        (v["state"].as_str().unwrap_or("").to_string(), v["why"].as_str().unwrap_or("").to_string())
    };
    let (state, why) = row(&r);
    assert_eq!(state, "holding", "{why}");
    assert!(why.contains("with the watch officer"), "{why}");
    let v = VerdictIn { verdict: Verdict::Escalated, note: "the zone keeps filling".into(), recommendation: Some("Send it to another zone".into()), intents: vec![] };
    r.d.triage_verdict(&officer(), "alert:delivery:p-9", v).expect("an escalation");
    let (state, why) = row(&r);
    assert_eq!(state, "attention", "passed on: the person's ({why})");
}
