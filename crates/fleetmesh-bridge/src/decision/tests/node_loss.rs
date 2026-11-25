//! One of ours gone silent, through the bridge's routes: any operator flags a silent vehicle by
//! hand; only an operator the roster gives the authority confirms it lost, on the silence the
//! review showed (409 otherwise), before any credential is asked for; an agent never (403,
//! whatever it sends); its orders end and its op stops; a report from it revokes the loss and
//! the inbox says so; any operator may revoke a loss with a note. Every log replays.

use axum::body::Body;
use axum::http::{Request, header};
use tower::ServiceExt;

use super::*;
use crate::tasks::tests::{Watch, quick};

fn api(r: &Rig) -> axum::Router {
    let auth = crate::auth::Auth::new(crate::auth::Settings::dev(), r.d.clone(), fleetmesh_auth::CredentialFile::default()).expect("auth");
    crate::api::router(crate::api::App {
        groups: Default::default(),
        tasking: Default::default(),
        world: r.world.clone(),
        frames: Arc::new(tokio::sync::watch::channel(None).0),
        decision: r.d.clone(),
        lod: crate::push::LodSettings::default(),
        auth,
        })
}

async fn call(api: &axum::Router, method: &str, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> (u16, Value) {
    let mut b = Request::builder().method(method).uri(path).header(header::CONTENT_TYPE, "application/json");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = b.body(body.map_or_else(Body::empty, |v| Body::from(v.to_string()))).expect("request");
    let resp = api.clone().oneshot(req).await.expect("response");
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22).await.expect("body");
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn post(api: &axum::Router, who: &str, path: &str, body: Value) -> (u16, Value) {
    call(api, "POST", path, Some(body), &[("x-fleetmesh-operator", who)]).await
}

fn review(r: &Rig) -> Value {
    r.d.inbox_json()["review"].clone()
}

async fn second(r: &Rig) {
    r.clock.advance_ms(1000);
    r.d.tick();
    sleep(Duration::from_secs(1)).await;
}

async fn silence(r: &Rig, _cell: &VehicleCell, w: &Watch, s: u32) {
    w.set_cut(true);
    for _ in 0..s {
        second(r).await;
    }
}

fn alerts(r: &Rig) -> Vec<(String, String, String)> {
    r.d.inbox_json()["alerts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|a| {
            (
                a["kind"].as_str().unwrap_or_default().to_string(),
                a["severity"].as_str().unwrap_or_default().to_string(),
                a["uid"].to_string(),
            )
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn only_the_rosters_authority_confirms_a_silent_vehicle_lost_and_a_report_revokes_it() {
    let r = credentials_rig(fleetmesh_decide::AuthMode::Dev, |_| {}, Vec::new(), fleetmesh_decide::StartCarry::default(), None, None).r;
    let (cell, w) = r.watched(5, Firmware::Copter, "delivery_drone", (LAT, LON), quick());
    sleep(Duration::from_secs(1)).await;
    let api = api(&r);
    let orbit = r.d.submit(&op1(), orbit(5)).await.expect("orbit");
    assert_eq!(orbit.state, State::Dispatched, "{}", r.reason(orbit.id));
    for _ in 0..3 {
        second(&r).await;
    }
    // reporting: nothing to flag
    let (s, v) = post(&api, "op-1", "/api/review/nodes/5/flag", json!({ "note": "no reason" })).await;
    assert_eq!(s, 409, "{v}");
    silence(&r, &cell, &w, 15).await;
    // silence alone presumes nothing
    assert_eq!(review(&r)["node_losses"], json!([]), "{}", review(&r));
    // a return goes out at once even to a silent vehicle (never queued)
    let held = r.d.submit(&op1(), simple("rtl", 5)).await.expect("rtl");
    // flagged by hand (any operator, with a note): on the review list
    let (s, v) = post(&api, "op-1", "/api/review/nodes/5/flag", json!({ "note": "" })).await;
    assert_eq!(s, 422, "{v}");
    let (s, v) = post(&api, "op-1", "/api/review/nodes/5/flag", json!({ "note": "a crew saw it go down" })).await;
    assert_eq!(s, 200, "{v}");
    let rv = review(&r);
    let item = &rv["node_losses"][0];
    assert_eq!((item["uid"].clone(), item["state"].clone(), item["needs_note"].clone()), (json!(5), json!("silent"), json!(true)), "{item}");
    assert_eq!(item["actions"], json!(["confirm", "dismiss"]), "{item}");
    assert_eq!(item["flagged"]["by"], "op-1", "{item}");
    assert!(item["would_end"].as_array().is_some_and(|a| a.contains(&json!(orbit.id.0))), "{item}");
    let (s, nodes) = call(&api, "GET", "/api/nodes", None, &[]).await;
    assert_eq!(s, 200, "{nodes}");
    assert_eq!(nodes["nodes"][0]["uid"], json!(5), "{nodes}");
    let since = item["since"].as_i64().expect("its last report");
    let lost = "/api/review/nodes/5/lost";
    let body = |since: i64, note: &str| json!({ "since": since, "note": note });
    let before = r.log.lines().len();
    // an agent, by its header or its key, whoever it names: never
    for headers in [
        vec![("x-fleetmesh-agent", "agent-planner")],
        vec![("x-fleetmesh-agent", "agent-planner"), ("x-fleetmesh-operator", "op-2")],
        vec![("authorization", "Bearer some-agent-key"), ("x-fleetmesh-operator", "op-2")],
    ] {
        let (s, v) = call(&api, "POST", lost, Some(body(since, "the agent thinks so")), &headers).await;
        assert_eq!(s, 403, "{headers:?}: {v}");
        let (s, v) = call(&api, "POST", "/api/review/nodes/5/revoke", Some(json!({ "note": "x" })), &headers).await;
        assert_eq!(s, 403, "{headers:?}: {v}");
    }
    // not the authority
    let (s, v) = post(&api, "op-1", lost, body(since, "a crew saw it go down")).await;
    assert_eq!(s, 403, "{v}");
    // no note where nothing presumes it lost
    let (s, v) = post(&api, "op-2", lost, body(since, " ")).await;
    assert_eq!(s, 409, "{v}");
    assert!(v["detail"].as_str().unwrap_or_default().contains("nothing presumes it lost"), "{v}");
    assert!(r.log.lines()[before..].iter().all(|l| !l.contains("\"type\":\"node.lost\"")), "nothing confirmed");
    // op-2, saying why
    let (s, v) = post(&api, "op-2", lost, body(since, "a crew saw it go down")).await;
    assert_eq!(s, 200, "{v}");
    // a return is never queued: it went out at once, so both orders were running and ended
    assert_eq!(v["lost"]["voided"], json!([]), "{v}");
    assert_eq!(v["lost"]["ended"], json!([orbit.id.0, held.id.0]), "{v}");
    assert_eq!(r.state(held.id), State::Failed);
    assert_eq!(r.state(orbit.id), State::Failed);
    let ops = r.world.tasks.current_ops();
    let op = ops.iter().find(|o| o.uid == 5).expect("its op");
    assert_eq!(op.state, crate::tasks::OpState::Superseded, "{op:?}");
    second(&r).await;
    second(&r).await;
    assert!(alerts(&r).contains(&("node_lost".into(), "info".into(), "5".into())), "{:?} {}", alerts(&r), r.d.inbox_json());
    let rv = review(&r);
    assert_eq!(rv["node_losses"], json!([]), "the hand flag went with the confirmation: {rv}");
    assert_eq!(rv["nodes_lost"][0]["by"], json!("op-2"), "{rv}");
    let (_, nodes) = call(&api, "GET", "/api/nodes", None, &[]).await;
    assert_eq!(nodes["nodes"][0]["state"], json!("lost"), "{nodes}");
    // lost: it takes no order
    let refused = r.d.submit(&op1(), simple("hold", 5)).await.expect("submitted");
    assert_eq!(refused.state, State::Rejected, "{}", r.reason(refused.id));
    // confirmed again: refused
    let (s, v) = post(&api, "op-3", lost, body(since, "again")).await;
    assert_eq!(s, 409, "{v}");
    // it reports again: the loss is revoked (cause heard), and the inbox says so
    w.set_cut(false);
    for _ in 0..5 {
        second(&r).await;
    }
    let log = r.log.text();
    let revoked = log.lines().find(|l| l.contains("\"type\":\"node.loss_revoked\"")).expect("revoked");
    assert!(revoked.contains("\"kind\":\"heard\""), "{revoked}");
    assert!(alerts(&r).contains(&("node_reported_again".into(), "high".into(), "5".into())), "{:?}", alerts(&r));
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn any_operator_revokes_a_loss_with_a_note_and_dismisses_a_hand_flag() {
    let r = credentials_rig(fleetmesh_decide::AuthMode::Dev, |_| {}, Vec::new(), fleetmesh_decide::StartCarry::default(), None, None).r;
    let (cell, w) = r.watched(6, Firmware::Copter, "delivery_drone", (LAT, LON), quick());
    sleep(Duration::from_secs(1)).await;
    let api = api(&r);
    second(&r).await;
    silence(&r, &cell, &w, 15).await;
    let (s, v) = post(&api, "op-1", "/api/review/nodes/6/flag", json!({ "note": "no beacon" })).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = post(&api, "op-1", "/api/review/nodes/6/dismiss", json!({ "note": "it is fine" })).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(review(&r)["node_losses"], json!([]));
    let (s, v) = post(&api, "op-1", "/api/review/nodes/6/dismiss", json!({})).await;
    assert_eq!(s, 409, "not flagged: {v}");
    // flagged again, then confirmed by the authority on the silence the review shows
    let (s, v) = post(&api, "op-1", "/api/review/nodes/6/flag", json!({ "note": "still no beacon" })).await;
    assert_eq!(s, 200, "{v}");
    let since = review(&r)["node_losses"][0]["since"].as_i64().expect("since");
    let (s, v) = post(&api, "op-3", "/api/review/nodes/6/lost", json!({ "since": since, "note": "crew report" })).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = post(&api, "op-1", "/api/review/nodes/6/revoke", json!({ "note": " " })).await;
    assert_eq!(s, 409, "{v}");
    let (s, v) = post(&api, "op-1", "/api/review/nodes/7/revoke", json!({ "note": "not lost" })).await;
    assert_eq!(s, 409, "{v}");
    let (s, v) = post(&api, "op-1", "/api/review/nodes/6/revoke", json!({ "note": "its crew says it is afloat" })).await;
    assert_eq!(s, 200, "{v}");
    let log = r.log.text();
    let revoked = log.lines().find(|l| l.contains("\"type\":\"node.loss_revoked\"")).expect("revoked");
    assert!(revoked.contains("\"operator\":\"op-1\""), "{revoked}");
    r.replays();
}
