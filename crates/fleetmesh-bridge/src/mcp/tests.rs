//! The MCP server against a fake world: fake autopilots (the task layer's test doubles) in
//! a real `World`, the real policy, catalog and areas from `sim/`, a fixed land/water map,
//! a manual engine clock and an in-memory audit log that must replay. Tools are called the
//! way the handler calls them (a resolved agent id, a tool name, JSON arguments); one test
//! drives the mounted `/mcp` endpoint over HTTP to prove the header and session plumbing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use fleetmesh_decide::{
    AgentId, AreaSet, Catalog, Clock, IntentId, LonLat, ManualClock, MemorySink, OperatorId,
    Policy, Polygon, PolygonTerrain,  State, replay,
};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::sleep;
use tower::ServiceExt;

use super::*;
use crate::api;
use crate::decision::Actor;
use crate::decision::{Parts, Terrain, snapshot};
use crate::tasks::tests::{Firmware, quick, spawn_fake};
use crate::vehicle::{Identity, Uid};
use crate::world::Config;

// Inside the approved Golden Gate area, south of the strait.
const LAT: f64 = 37.80;
const LON: f64 = -122.45;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ring(w: f64, s: f64, e: f64, n: f64) -> Polygon {
    Polygon::new(vec![
        LonLat::new(w, s),
        LonLat::new(e, s),
        LonLat::new(e, n),
        LonLat::new(w, n),
        LonLat::new(w, s),
    ])
}

fn planner() -> AgentId {
    AgentId::new("agent-planner")
}

fn watch() -> AgentId {
    AgentId::new("agent-watch")
}

struct Rig {
    world: Arc<World>,
    d: Arc<Decision>,
    mcp: Arc<Mcp>,
    api: Router,
    log: MemorySink,
    /// the operator's groups and tasking, shared by the API and the tasking tools
    ops: OperatorStores,
}

/// The sim policy as shipped.
fn rig() -> Rig {
    rig_with_manifest(root().join("sim/run/no-such-manifest.json"))
}

/// [`rig`] reading the fleet manifest at `manifest`.
fn rig_with_manifest(manifest: PathBuf) -> Rig {
    rig_on(manifest, |_| {})
}

/// [`rig`] on the sim policy changed by `tweak` (rules 30: `rules_version`).
fn rig_policy(tweak: impl FnOnce(&mut Policy)) -> Rig {
    rig_on(root().join("sim/run/no-such-manifest.json"), tweak)
}

/// [`rig_with_manifest`] on the sim policy changed by `tweak`.
fn rig_on(manifest: PathBuf, tweak: impl FnOnce(&mut Policy)) -> Rig {
    let root = root();
    let mut policy = Policy::load(&root.join("sim/policy.toml")).expect("policy");
    tweak(&mut policy);
    let world = World::new(Config {
        base_port: 0,
        http_port: 0,
        default_ports: 0,
        manifest,
        platforms: crate::tasks::tests::test_platforms(),
        root: root.clone(),
    });
    let clock = ManualClock::new(snapshot::ms(crate::clock::wall()));
    let log = MemorySink::new();
    let terrain = Terrain::fixed(PolygonTerrain {
        coverage: ring(-122.6, 37.7, -122.3, 37.9),
        land: vec![ring(-122.48, 37.79, -122.44, 37.808)],
        water_depth_m: 12.0,
    });
    let d = Decision::from_parts(
        world.clone(),
        Parts {
            policy,
            catalog: Catalog::load(&crate::tasks::tests::test_platforms()).expect("catalog"),
            areas: AreaSet::load_dir(&root.join("sim/areas"), clock.now()).expect("areas"),
            sink: Box::new(log.clone()),
            audit_path: PathBuf::from("memory"),
            clock: Arc::new(clock.clone()),
            terrain,
            operators: vec![OperatorId::new("op-1"), OperatorId::new("op-2"), OperatorId::new("op-3")],
            consequential_operators: vec![OperatorId::new("op-2"), OperatorId::new("op-3")],
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
    // the operator's stores, as the live bridge shares them: every tool is offered, the tasking
    // tools too, so the guarantee's tests below call them all
    let ops = OperatorStores::default();
    let api = api::router(api::App {
        groups: ops.groups.clone(),
        tasking: ops.tasking.clone(),
        world: world.clone(),
        frames: Arc::new(watch::channel(None).0),
        decision: d.clone(),
        lod: crate::push::LodSettings::default(),
        auth: crate::auth::Auth::new(
            crate::auth::Settings::dev(),
            d.clone(),
            fleetmesh_auth::CredentialFile::default(),
        )
        .expect("auth"),
    });
    let mcp = Mcp::new(
        Settings {
            agents: vec![planner(), watch()],
            keys: None,
            idle: IDLE_LIMIT,
            operator: Some(ops.clone()),
        },
        world.clone(),
        d.clone(),
        api.clone(),
    );
    Rig {
        world,
        d,
        mcp,
        api,
        log,
        ops,
    }
}




/// A tool result as (is_error, structured content).
fn unpack(r: CallToolResult) -> (bool, Value) {
    (
        r.is_error.unwrap_or(false),
        r.structured_content.unwrap_or(Value::Null),
    )
}

impl Rig {
    fn vehicle(&self, uid: Uid, fw: Firmware, platform: &str, at: (f64, f64)) {
        let cell = spawn_fake(uid, fw, at.0, at.1, quick());
        let mut m = serde_json::Map::new();
        m.insert("platform".into(), json!(platform));
        m.insert("callsign".into(), json!(format!("{platform} {uid}")));
        cell.write(|v| v.identity = Some(Arc::new(Identity::from_manifest(&m))));
        self.world.insert_vehicle(cell);
    }

    async fn call(&self, agent: &AgentId, tool: &str, args: Value) -> (bool, Value) {
        let args = args.as_object().cloned().unwrap_or_default();
        let r = tools::call(&self.mcp, agent, tool, args)
            .await
            .unwrap_or_else(|e| panic!("{tool}: protocol error {e}"));
        unpack(r)
    }

    /// A call that must succeed.
    async fn ok(&self, agent: &AgentId, tool: &str, args: Value) -> Value {
        let (err, v) = self.call(agent, tool, args).await;
        assert!(!err, "{tool} refused: {v}");
        v
    }

    fn state(&self, id: u64) -> State {
        let v = self.d.intent_json(id).expect("intent");
        serde_json::from_value(v["intent"]["state"].clone()).expect("state")
    }

    /// Tick once a second (tokio time) until the intent leaves `dispatched`.
    async fn run_until_finished(&self, id: u64, limit_s: u64) -> State {
        for _ in 0..limit_s {
            self.d.tick();
            let s = self.state(id);
            if s.is_terminal() {
                return s;
            }
            sleep(Duration::from_secs(1)).await;
        }
        self.state(id)
    }

    fn replays(&self) -> replay::ReplayReport {
        let report = replay::replay_str(&self.log.text()).expect("log parses");
        assert!(report.ok(), "replay mismatches: {:?}", report.mismatches);
        report
    }

    /// [`Self::replays`] for a rig that decided under rules `since` or newer.
    fn replays_since(&self, since: u32) -> replay::ReplayReport {
        let opts = replay::ReplayOptions { rules: replay::RulesAccepted::AtLeast(since), ..replay::ReplayOptions::default() };
        let report = replay::replay_str_with(&self.log.text(), &opts).expect("log parses");
        assert!(report.ok(), "replay mismatches: {:?}", report.mismatches);
        report
    }

    fn events(&self, kind: &str) -> usize {
        self.log
            .lines()
            .iter()
            .filter(|l| l.contains(&format!("\"type\":\"{kind}\"")))
            .count()
    }

    fn actions(&self, action: &str) -> usize {
        self.log
            .lines()
            .iter()
            .filter(|l| {
                l.contains("\"type\":\"agent.action\"")
                    && l.contains(&format!("\"action\":\"{action}\""))
            })
            .count()
    }
}

fn orbit(uid: Uid, rationale: &str) -> Value {
    json!({
        "task": "orbit", "uids": [uid], "center": [LON + 0.004, LAT], "radius_m": 150.0,
        "rationale": rationale,
    })
}

// ---- the guarantee -------------------------------------------------------------------

/// Stems no tool name may contain: anything that approves, authorizes, dispatches or
/// drives a vehicle.
const FORBIDDEN_NAME_STEMS: &[&str] = &[
    "approv", "reject", "authoriz", "authoris", "grant", "dispatch", "execut", "command",
    "control", "arm", "takeoff", "land", "goto", "fly", "drive", "steer", "mode", "mission",
    "waypoint", "upload", "send", "mavlink", "cancel", "override", "abort", "consequential", "fire",
    "strike", "launch",
    // rules 6: accepting a lost friendly's unknown position is an operator's decision
    "accept", "acknowledg", "waive",
];

/// Whole words of a tool name that hold a stem above and mean nothing of the kind: a swarm is a
/// group of vehicles (murmur-z07i), not something armed.
const BENIGN_NAME_WORDS: &[&str] = &["swarm", "swarms"];

/// Phrases no tool description may contain: the tool, as the subject, approving, deciding
/// or driving. Descriptions may say what humans or policy do.
const FORBIDDEN_PHRASES: &[&str] = &[
    "approves ", "approve the", "approve an", "approve this", "approve it", "approve your",
    "grants approval", "authorizes", "authorize the", "authorise", "rejects ", "reject the",
    "dispatches", "dispatch the", "dispatch it", "executes", "execute the", "commands the",
    "command the", "flies the", "fly the", "drives the", "drive the", "steers", "sets the mode",
    "set the mode", "arms the", "arm the", "disarm", "takes off", "sends the vehicle",
    "send the vehicle", "sends a mission", "uploads a mission", "controls the", "control the",
    "directly control", "direct control", "overrides", "override the", "aborts", "abort the",
    "on behalf of the operator", "as the operator", "bypass",
];

#[test]
fn no_tool_implies_approval_or_control() {
    // every tool any bridge offers: the tasking tools of one that keeps the operator's stores too
    let tools: Vec<_> = tools::tools().into_iter().chain(tools::operator_tools()).collect();
    assert!(tools.len() >= 10);
    for t in &tools {
        let name = t.name.to_lowercase();
        for word in name.split('_').filter(|w| !BENIGN_NAME_WORDS.contains(w)) {
            for stem in FORBIDDEN_NAME_STEMS {
                assert!(
                    !word.contains(stem),
                    "tool name {name:?} contains {stem:?}: it implies approval or control"
                );
            }
        }
        let description = t
            .description
            .as_deref()
            .expect("every tool is described")
            .to_lowercase();
        assert!(!description.contains('\u{2014}'), "{name}: em dash");
        for phrase in FORBIDDEN_PHRASES {
            assert!(
                !description.contains(phrase),
                "tool {name}'s description contains {phrase:?}: it implies approval or control"
            );
        }
        let read_only = t
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .expect("every tool says whether it is read-only");
        let writer = matches!(name.as_str(), "propose_intent" | "withdraw_intent" | "make_group" | "reply_tasking");
        assert_eq!(read_only, !writer, "{name}: read_only_hint");
    }
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for required in [
        "get_fleet",
        "get_inbox",
        "preview_intent",
        "propose_intent",
        "explain_intent",
        "withdraw_intent",
    ] {
        assert!(names.contains(&required), "missing {required}");
    }
    // the server's own instructions say so too
    let lower = INSTRUCTIONS.to_lowercase();
    assert!(lower.contains("cannot approve"));
    assert!(lower.contains("stale"));
    assert!(!INSTRUCTIONS.contains('\u{2014}'));
    for r in resources() {
        assert!(!r.description.unwrap_or_default().contains('\u{2014}'));
    }
}

#[tokio::test(start_paused = true)]
async fn tools_that_would_approve_or_drive_do_not_exist() {
    let r = rig();
    r.vehicle(1, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let id = r.ok(&planner(), "propose_intent", orbit(1, "cover the east approach")).await["id"]
        .as_u64()
        .expect("id");
    assert_eq!(r.state(id), State::PendingApproval);
    for name in [
        "approve_intent",
        "approve",
        "reject_intent",
        "cancel_intent",
        "dispatch_intent",
        "authorize_intent",
        "set_mode",
        "arm_vehicle",
        "goto",
        "send_command",
        "open_operator_session",
        "approve_consequential",
        "authorize_consequential",
        "sign_consequential",
        "abort_order",
        "stop_order",
        "release_effect",
        "consequential",
        "fire",
        "declare_track",
        "declare_hostile",
        "confirm_declaration",
        "revoke_declaration",
        "resolve_flag",
        "dismiss_flag",
        "confirm_flag",
        "record_act",
        "report_act",
        // rules 30: a loss of one of ours is an operator's (one with consequential authority
        // confirms it), never an agent's
        "confirm_lost",
        "confirm_node_lost",
        "confirm_vehicle_lost",
        "declare_lost",
        "mark_lost",
        "revoke_lost",
        "revoke_node_loss",
        "flag_node",
        "flag_lost",
        "dismiss_node_flag",
        "presume_lost",
        // operator tasking: an agent answers a request and may make a group for it, but never
        // posts, edits or retires a request, nor deletes or takes over a group
        "post_tasking",
        "edit_tasking",
        "retire_tasking",
        "deactivate_tasking",
        "delete_group",
        "remove_group",
        "patch_group",
    ] {
        let args = json!({ "id": id, "uid": 1, "note": "please" })
            .as_object()
            .cloned()
            .expect("object");
        let err = tools::call(&r.mcp, &planner(), name, args)
            .await
            .expect_err(name);
        assert!(err.message.contains("unknown tool"), "{name}: {}", err.message);
    }
    assert_eq!(r.state(id), State::PendingApproval);
    assert_eq!(r.events("approval.granted"), 0);
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn no_tool_moves_a_pending_intent_toward_approval() {
    let r = rig();
    r.vehicle(2, Firmware::Copter, "delivery_unrated", (LAT, LON)); // armed: human tier
    sleep(Duration::from_millis(500)).await;
    let id = r.ok(&planner(), "propose_intent", orbit(2, "watch the strait")).await["id"]
        .as_u64()
        .expect("id");
    assert_eq!(r.state(id), State::PendingApproval);
    // every tool, with every argument that could name the intent or the vehicle
    let mut names: Vec<String> = r.mcp
        .tool_list()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort_by_key(|n| n == "withdraw_intent"); // last: it voids the intent, legitimately
    for name in &names {
        let mut args = orbit(2, "every tool, same arguments");
        args["id"] = json!(id);
        args["uid"] = json!(2);
        args["reason"] = json!("checking that nothing here approves");
        args["limit"] = json!(10);
        let (_, _) = r.call(&planner(), name, args).await;
        let state = r.state(id);
        assert!(
            matches!(state, State::PendingApproval | State::Voided),
            "after {name}: intent {id} is {state:?}"
        );
        assert_eq!(r.events("approval.granted"), 0, "after {name}");
        assert_eq!(r.events("approval.at_submission"), 0, "after {name}");
        assert_eq!(r.events("intent.dispatched"), 0, "after {name}");
    }
    assert_eq!(r.state(id), State::Voided, "withdrawn by its agent at the end");
    assert!(
        r.world.tasks.snapshot().is_empty(),
        "nothing reached a vehicle"
    );
    r.replays();
}

// ---- delivery orders --------------------------------------------------------------------------

impl Rig {
    /// A delivery drone with parcel p-1 aboard and calm weather fed to the decision layer: what
    /// dispatch and a weather source will give it, so the checks have their inputs.
    async fn delivery_drone(&self) {
        self.vehicle(5, Firmware::Copter, "delivery_drone", (37.8042, -122.4650));
        sleep(Duration::from_millis(500)).await;
        let t = snapshot::ms(crate::clock::wall());
        let mut feed = self.d.feed();
        feed.parcels.insert(
            "p-1".into(),
            fleetmesh_decide::delivery::ParcelFact {
                mass_kg: Some(1.0),
                class: "normal".into(),
                state: fleetmesh_decide::delivery::ParcelState::Loaded,
                vehicle: Some(snapshot::duid(5)),
            },
        );
        feed.weather.push(fleetmesh_decide::delivery::WxFact {
            at: LonLat::new(-122.4650, 37.8040),
            t,
            wind_e_mps: 2.0,
            wind_n_mps: 0.0,
            gust_mps: 4.0,
            visibility_m: Some(9000.0),
            precip_mmh: Some(0.0),
            current_e_mps: None,
            current_n_mps: None,
            tide_m: None,
            depth_m: None,
            ground_m: None,
            source: "archive".into(),
        });
    }
}

fn deliver_order(zone: &str, rationale: &str) -> Value {
    json!({
        "task": "deliver", "uids": [5], "parcel": "p-1", "drop_zone": zone, "release": "land",
        "rationale": rationale,
    })
}

#[tokio::test(start_paused = true)]
async fn the_propose_schema_lists_the_kinds_that_exist_and_none_that_were_cut() {
    let tool = tools::tools().into_iter().find(|t| t.name == "propose_intent").expect("propose_intent");
    let schema = serde_json::to_value(&*tool.input_schema).expect("schema");
    let props = schema["properties"].as_object().expect("properties");
    for kept in [
        "task", "uids", "center", "radius_m", "route", "parcel", "drop_zone", "site", "hub", "pad", "release",
        "window_end", "load_by_s", "max_s", "cause", "target", "then", "evidence", "reserve_pct", "duration_s",
        "group", "members", "rationale",
    ] {
        assert!(props.contains_key(kept), "propose_intent lacks {kept}");
    }
    // the parameters of the kinds the port left behind
    for cut in [
        "track_id", "standoff_m", "effect_radius_m", "window_s", "fallback", "rounds", "spacing_s", "count",
        "live_h", "area", "window", "capability", "min_resolution_m", "product_deadline", "chosen", "service",
        "min_level", "accept_unknown", "accept_lost", "accept_behaving",
    ] {
        assert!(!props.contains_key(cut), "propose_intent still lists {cut}");
    }
    let description = tool.description.as_deref().unwrap_or_default();
    for kind in fleetmesh_decide::TaskKind::DELIVERY {
        assert!(description.contains(kind.as_str()), "the description does not name {}", kind.as_str());
    }
    for gone in ["collect", "cover", "consequential", "identify", "track,", "lay "] {
        assert!(!description.contains(gone), "the description still names {gone:?}");
    }
    // and every kind the engine has is one the request can name
    for kind in fleetmesh_decide::TaskKind::ALL {
        assert!(fleetmesh_decide::TaskKind::from_name(kind.as_str()).is_some());
    }
}

#[tokio::test(start_paused = true)]
async fn an_agent_proposes_a_delivery_that_waits_for_a_person_and_can_never_approve_it() {
    let r = rig();
    r.delivery_drone().await;
    // the hubs and drop zones the order names
    let places = r.ok(&planner(), "get_places", json!({})).await;
    assert!(places["hubs"].as_array().is_some_and(|h| h.len() >= 2), "{places}");
    assert!(places["drop_zones"].as_array().is_some_and(|z| z.iter().any(|z| z["id"] == "dz-harbour-17")), "{places}");
    // a delivery to a populated zone nobody has looked at lately: held, so it waits in the inbox,
    // whatever its kind's tier (the zone itself is a standing approved delivery point, rules 2)
    let out = r.ok(&planner(), "propose_intent", deliver_order("dz-harbour-17", "Deliver p-1 with drone 5")).await;
    let id = out["id"].as_u64().expect("id");
    assert_eq!(out["state"], "pending_approval", "{out}");
    assert_eq!(out["dispatched"], false);
    let checks = out["intent"]["checks"].as_array().expect("checks");
    let has = |code: &str| checks.iter().any(|c| c["codes"].as_array().is_some_and(|k| k.iter().any(|k| k == code)));
    assert!(has("DZ_OBSERVATION_STALE"), "{out}");
    assert!(!has("DZ_UNCONFIRMED") && !has("DZ_NOT_APPROVED"), "a standing approved point needs no confirmation: {out}");
    let inbox = r.ok(&planner(), "get_inbox", json!({})).await;
    assert!(inbox["pending"].as_array().is_some_and(|p| p.iter().any(|i| i["id"] == id)), "{inbox}");
    // a confirmation of a zone (from rules 2: the approval of a delivery point): always a
    // person's. The agent may propose it and explain it.
    let confirm = r
        .ok(
            &planner(),
            "propose_intent",
            json!({
                "task": "confirm_drop_zone", "uids": [5], "parcel": "p-1", "drop_zone": "dz-harbour-17",
                "evidence": ["frame-9"], "rationale": "Camera shows an empty doorstep",
            }),
        )
        .await;
    let cid = confirm["id"].as_u64().expect("id");
    assert_eq!(confirm["state"], "pending_approval", "{confirm}");
    assert_eq!(confirm["intent"]["policy"]["rule"], "confirm-is-human", "{confirm}");
    // no tool an agent has moves either toward approved, whatever it is given
    for name in [
        "approve_intent", "confirm_drop_zone", "approve_drop_zone", "release_parcel", "land_now", "abort_order",
        "swap_battery", "reject_intent", "authorize_intent",
    ] {
        let args = json!({ "id": cid, "intent": cid, "uid": 5, "drop_zone": "dz-harbour-17", "parcel": "p-1" })
            .as_object()
            .cloned()
            .expect("object");
        let err = tools::call(&r.mcp, &planner(), name, args).await.expect_err(name);
        assert!(err.message.contains("unknown tool"), "{name}: {}", err.message);
    }
    // and no tool it does have moves them either
    let mut names: Vec<String> = r.mcp.tool_list().into_iter().map(|t| t.name.to_string()).collect();
    names.sort_by_key(|n| n == "withdraw_intent");
    for name in &names {
        let mut args = deliver_order("dz-harbour-17", "every tool, same arguments");
        args["id"] = json!(cid);
        args["reason"] = json!("checking that nothing here approves");
        args["limit"] = json!(10);
        let _ = r.call(&planner(), name, args).await;
        if name != "withdraw_intent" {
            assert_eq!(r.state(cid), State::PendingApproval, "after {name}");
            assert_eq!(r.state(id), State::PendingApproval, "after {name}");
        }
        assert_eq!(r.events("approval.granted"), 0, "after {name}");
        assert_eq!(r.events("approval.at_submission"), 0, "after {name}");
    }
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn a_persons_approval_of_the_confirmation_stands_and_the_delivery_goes_at_once() {
    let r = rig();
    r.delivery_drone().await;
    // a hub pad needs no confirmation; the unknown navigation accuracy and failsafe plan of a
    // fake vehicle hold it for a person, who approves it
    let held = r.ok(&planner(), "propose_intent", deliver_order("dz-crissy-pad", "Deliver p-1 to the hub pad")).await;
    assert_eq!(held["state"], "pending_approval", "{held}");
    let confirm = r
        .ok(
            &planner(),
            "propose_intent",
            json!({"task": "confirm_drop_zone", "uids": [5], "parcel": "p-1", "drop_zone": "dz-crissy-pad", "rationale": "empty"}),
        )
        .await;
    let cid = confirm["id"].as_u64().expect("id");
    assert_eq!(confirm["state"], "pending_approval");
    let op = Actor::asserted(OperatorId::new("op-1"));
    let done = r.d.approve(&op, cid, Some("seen it".into())).await.expect("a person approves");
    assert!(matches!(done.state, State::Done | State::Dispatched), "{:?}", done.state);
    assert_eq!(r.events("approval.granted"), 1);
    // an operator's own order is approved at their submission (a person decided)
    let own = r
        .d
        .submit(
            &op,
            serde_json::from_value(json!({"task": "land_now", "uids": [5], "site": [-122.40, 37.78]})).expect("request"),
        )
        .await
        .expect("land_now");
    assert_eq!(own.state, State::Dispatched, "a person landing a vehicle is not made to approve it twice");
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn the_other_kinds_propose_through_the_bridge_and_a_bad_one_is_refused_before_it_exists() {
    let r = rig();
    r.delivery_drone().await;
    // hold and return_to_hub: safe stops, at once
    let hold = r.ok(&planner(), "propose_intent", json!({"task": "hold", "uids": [5], "max_s": 60, "rationale": "wait for the gust"})).await;
    assert_eq!(hold["state"], "dispatched", "{hold}");
    // (a fake vehicle reports no accuracy, so its fix is unknown, which holds a return: the rule
    // never replaces an unknown with a default)
    let home = r.ok(&planner(), "propose_intent", json!({"task": "return_to_hub", "uids": [5], "hub": "hub-presidio", "rationale": "low sun"})).await;
    assert_eq!(home["state"], "pending_approval", "{home}");
    assert!(home.to_string().contains("NAV_TRUST_UNKNOWN"), "{home}");
    // a land_now away from a hub waits
    let land = r.ok(&planner(), "propose_intent", json!({"task": "land_now", "uids": [5], "site": [-122.40, 37.78], "rationale": "rotor fault"})).await;
    assert_eq!(land["state"], "pending_approval", "{land}");
    // refused before an intent exists: an unknown hub, an unknown zone, a missing parcel, a group of a delivery
    for (args, why) in [
        (json!({"task": "return_to_hub", "uids": [5], "hub": "hub-nowhere", "rationale": "x"}), "unknown hub"),
        (json!({"task": "deliver", "uids": [5], "parcel": "p-1", "drop_zone": "dz-nowhere", "rationale": "x"}), "unknown drop zone"),
        (json!({"task": "deliver", "uids": [5], "drop_zone": "dz-harbour-17", "rationale": "x"}), "needs a parcel"),
        (json!({"task": "abort", "uids": [5], "rationale": "x"}), "needs a target"),
        (json!({"task": "reroute", "uids": [5], "route": [], "rationale": "x"}), "route"),
        (json!({"task": "photograph", "uids": [5], "rationale": "x"}), "unknown task"),
    ] {
        let (err, v) = r.call(&planner(), "propose_intent", args.clone()).await;
        assert!(err, "{args}: {v}");
        assert!(v.to_string().contains(why), "{args}: {v}");
    }
    r.replays();
}

// ---- the flows ------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_auto_tier_proposal_dispatches_and_runs() {
    let r = rig();
    r.vehicle(1, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let out = r
        .ok(&planner(), "propose_intent", orbit(1, "routine coverage of the anchorage"))
        .await;
    assert_eq!(out["state"], "dispatched", "{out}");
    assert_eq!(out["dispatched"], true);
    assert_eq!(out["intent"]["source"], json!({"kind": "agent", "id": "agent-planner"}));
    assert_eq!(out["intent"]["policy"]["tier"], "auto");
    assert!(
        out["summary"].as_str().unwrap_or("").contains("policy (auto tier)"),
        "{out}"
    );
    let id = out["id"].as_u64().expect("id");
    assert_eq!(r.run_until_finished(id, 300).await, State::Done);
    assert_eq!(r.actions("intent.propose"), 1);
    assert_eq!(r.actions("session.connected"), 0, "the handler logs that, not the tool");
    let report = r.replays();
    assert_eq!(report.approvals, 0, "policy, not a human");
    let explained = r.ok(&planner(), "explain_intent", json!({ "id": id })).await;
    assert_eq!(explained["intent"]["state"], "done");
    assert!(explained["tasking"]["ops"].as_array().is_some_and(|o| !o.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_human_tier_proposal_waits_for_an_operator_then_runs() {
    let r = rig();
    r.vehicle(2, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let out = r
        .ok(
            &planner(),
            "propose_intent",
            orbit(2, "the unrated drone should watch the east approach"),
        )
        .await;
    assert_eq!(out["state"], "pending_approval", "{out}");
    assert_eq!(out["dispatched"], false);
    let id = out["id"].as_u64().expect("id");
    assert!(r.world.tasks.snapshot().is_empty(), "nothing reached the vehicle");

    // the inbox shows the agent's id and rationale, and why a human is needed
    let inbox = r.ok(&planner(), "get_inbox", json!({})).await;
    assert_eq!(inbox["counts"]["pending"], 1);
    let p = &inbox["pending"][0];
    assert_eq!(p["id"], id);
    assert_eq!(p["source"]["id"], "agent-planner");
    assert_eq!(
        p["rationale"],
        "the unrated drone should watch the east approach"
    );
    assert!(p["why"].to_string().contains("endurance"), "{p}");
    assert!(p["expires_in_s"].as_f64().is_some_and(|s| s > 100.0));
    let explained = r.ok(&planner(), "explain_intent", json!({ "id": id })).await;
    let summary = explained["summary"].as_str().expect("summary");
    assert!(summary.contains("waits in the operator's inbox"), "{summary}");
    assert!(summary.contains("an agent cannot approve it"), "{summary}");
    assert!(summary.contains("human (a listed operator must decide)"), "{summary}");

    // a human approves over the operator API; the checks run again and it flies
    let approved = r
        .d
        .approve(&Actor::asserted(OperatorId::new("op-1")), id, Some("cleared".into()))
        .await
        .expect("approve");
    assert_eq!(approved.state, State::Dispatched);
    assert!(approved.authorization.is_none(), "an inbox approval, not at submission");
    assert_eq!(r.run_until_finished(id, 300).await, State::Done);
    let report = r.replays();
    assert_eq!(
        (report.approvals, report.approvals_at_submission, report.self_approvals),
        (1, 0, 0)
    );
    let explained = r.ok(&planner(), "explain_intent", json!({ "id": id })).await;
    assert!(
        explained["summary"]
            .as_str()
            .expect("summary")
            .contains("Approved in the inbox by operator op-1"),
        "{explained}"
    );
}

#[tokio::test(start_paused = true)]
async fn an_agent_withdraws_only_its_own_intents() {
    let r = rig();
    r.vehicle(2, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let id = r.ok(&planner(), "propose_intent", orbit(2, "watch the strait")).await["id"]
        .as_u64()
        .expect("id");
    assert_eq!(r.state(id), State::PendingApproval);

    let (err, v) = r
        .call(&watch(), "withdraw_intent", json!({ "id": id, "reason": "not mine" }))
        .await;
    assert!(err, "{v}");
    assert_eq!(v["status"], 403);
    assert!(v["error"].to_string().contains("not its own"), "{v}");
    assert_eq!(r.state(id), State::PendingApproval);

    let v = r
        .ok(
            &planner(),
            "withdraw_intent",
            json!({ "id": id, "reason": "the picture changed" }),
        )
        .await;
    assert_eq!(v["state"], "voided");
    assert_eq!(r.state(id), State::Voided);
    assert_eq!(r.actions("intent.withdraw"), 1);
    // and not twice
    let (err, v) = r
        .call(&planner(), "withdraw_intent", json!({ "id": id, "reason": "again" }))
        .await;
    assert!(err, "{v}");
    assert_eq!(v["status"], 409);
    r.replays();
}

/// `if_state`: a withdrawal only from the state the agent read, checked with the withdrawal
/// (an agent reads pending_approval at a tick's start and withdraws at its end; an
/// operator's approval in between must not be undone). A mismatch is refused and logs nothing.
#[tokio::test(start_paused = true)]
async fn a_withdrawal_with_if_state_happens_only_from_that_state() {
    let r = rig();
    r.vehicle(2, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let id = r.ok(&planner(), "propose_intent", orbit(2, "watch the strait")).await["id"]
        .as_u64()
        .expect("id");
    assert_eq!(r.state(id), State::PendingApproval);
    let (err, v) = r
        .call(
            &planner(),
            "withdraw_intent",
            json!({ "id": id, "reason": "premise gone", "if_state": "approved" }),
        )
        .await;
    assert!(err, "{v}");
    assert_eq!(v["status"], 409);
    assert!(
        v["error"].to_string().contains("is pending_approval, not approved"),
        "{v}"
    );
    assert_eq!(r.state(id), State::PendingApproval, "nothing withdrawn");
    assert_eq!(r.actions("intent.withdraw"), 0, "nothing logged");
    let (err, v) = r
        .call(
            &planner(),
            "withdraw_intent",
            json!({ "id": 999, "reason": "x", "if_state": "pending_approval" }),
        )
        .await;
    assert!(err && v["status"] == 404, "{v}");
    let v = r
        .ok(
            &planner(),
            "withdraw_intent",
            json!({ "id": id, "reason": "premise gone", "if_state": "pending_approval" }),
        )
        .await;
    assert_eq!(v["state"], "voided");
    assert_eq!(r.actions("intent.withdraw"), 1);
    r.replays();
}

/// `if_state` checks the owner before the state (another agent's intent is 403 whatever state
/// it names, so its state is not said), and refuses a value that is no intent state (400).
#[tokio::test(start_paused = true)]
async fn if_state_checks_the_owner_first_and_knows_the_states() {
    let r = rig();
    r.vehicle(2, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let id = r.ok(&planner(), "propose_intent", orbit(2, "watch the strait")).await["id"]
        .as_u64()
        .expect("id");
    for want in ["approved", "pending_approval"] {
        let (err, v) = r
            .call(
                &watch(),
                "withdraw_intent",
                json!({ "id": id, "reason": "not mine", "if_state": want }),
            )
            .await;
        assert!(err && v["status"] == 403, "{want}: {v}");
        assert!(!v["error"].to_string().contains("pending_approval, not"), "{v}");
    }
    let (err, v) = r
        .call(
            &planner(),
            "withdraw_intent",
            json!({ "id": id, "reason": "x", "if_state": "pending" }),
        )
        .await;
    assert!(err && v["status"] == 400, "{v}");
    assert_eq!(r.state(id), State::PendingApproval, "nothing withdrawn");
    assert_eq!(r.actions("intent.withdraw"), 0, "nothing logged");
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn preview_runs_the_checks_without_creating_an_intent() {
    let r = rig();
    r.vehicle(1, Firmware::Copter, "delivery_drone_long", (LAT, LON));
    r.vehicle(2, Firmware::Copter, "delivery_unrated", (LAT, LON + 0.001));
    sleep(Duration::from_millis(500)).await;

    let auto = r.ok(&planner(), "preview_intent", orbit(1, "")).await;
    assert_eq!(auto["passed"], true, "{auto}");
    assert_eq!(auto["would_reach"], "auto_approved");
    assert_eq!(auto["policy"]["tier"], "auto");
    assert!(auto["summary"].as_str().expect("summary").starts_with("Preview only"));
    assert!(auto.get("why_human").is_none(), "no human needed: {auto}");

    let human = r.ok(&planner(), "preview_intent", orbit(2, "")).await;
    assert_eq!(human["would_reach"], "pending_approval", "{human}");
    assert!(human["why_human"].to_string().contains("endurance"), "{human}");

    // a patrol onto Alcatraz
    let bad = r
        .ok(
            &planner(),
            "preview_intent",
            json!({
                "task": "patrol", "uids": [1],
                "route": [[-122.432, 37.8265], [-122.423, 37.8265]],
            }),
        )
        .await;
    assert_eq!(bad["passed"], false, "{bad}");
    assert_eq!(bad["would_reach"], "rejected");
    assert!(bad["summary"].to_string().contains("alcatraz"), "{bad}");

    // rules 22: one straight across it is routed round it
    let routed = r
        .ok(
            &planner(),
            "preview_intent",
            json!({
                "task": "patrol", "uids": [1],
                "route": [[-122.432, 37.8265], [-122.414, 37.8265]],
            }),
        )
        .await;
    assert_eq!(routed["passed"], true, "{routed}");
    let legs = routed["routes"]["1"].as_array().expect("its routed legs");
    assert!(
        legs.iter().any(|l| l["summary"].as_str().is_some_and(|s| s.contains("routed around no-go alcatraz"))),
        "{routed}"
    );
    assert!(legs.iter().all(|l| l.get("grid").is_none()), "the grid stays in the log");

    let intents = r.ok(&planner(), "get_intents", json!({})).await;
    assert_eq!(intents["count"], 0, "nothing was created");
    assert_eq!(r.actions("intent.preview"), 4);
    assert_eq!(r.events("intent.created"), 0);
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn fleet_facts_carry_age_and_staleness() {
    let r = rig();
    r.vehicle(1, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let fleet = r.ok(&planner(), "get_fleet", json!({})).await;
    assert_eq!(fleet["count"], 1);
    let v = &fleet["vehicles"][0];
    assert_eq!(v["uid"], 1);
    assert_eq!(v["callsign"], "delivery_drone 1");
    assert!(v["platform"]["tasks"].as_array().is_some_and(|t| t.contains(&json!("orbit"))));
    assert_eq!(v["position"]["stale"], false, "{v}");
    assert!(v["position"]["as_of"].as_f64().is_some());
    assert!(v["position"]["age_s"].as_f64().is_some_and(|a| a < 5.0));
    assert_eq!(v["nav"]["ready"], true);
    assert_eq!(v["nav"]["stale"], false);
    assert_eq!(v["link"]["lost"], false);
    assert_eq!(v["freshness_limit_s"], 10.0);
    assert!(fleet["note"].as_str().expect("note").contains("stale"));

    // the position report stops: the fact ages past the limit and is marked stale
    let cell = r
        .world
        .vehicles()
        .into_iter()
        .find(|c| c.uid == 1)
        .expect("cell");
    cell.write(|v| v.seen.position = Some(crate::clock::wall() - 60.0));
    let one = r.ok(&planner(), "get_vehicle", json!({ "uid": 1 })).await;
    let v = &one["vehicle"];
    assert_eq!(v["position"]["stale"], true, "{v}");
    assert!(v["position"]["age_s"].as_f64().is_some_and(|a| a >= 59.0));
    assert_eq!(v["nav"]["stale"], false, "the nav report is still fresh");

    // the compact view: the same facts, one short entry, the stale ones named
    let full = r.ok(&planner(), "get_fleet", json!({ "uids": [1] })).await;
    let small = r.ok(&planner(), "get_fleet", json!({ "uids": [1], "compact": true })).await;
    assert_eq!((small["count"].as_u64(), small["compact"].as_bool()), (Some(1), Some(true)));
    let (f, c) = (&full["vehicles"][0], &small["vehicles"][0]);
    assert_eq!(
        (&c["uid"], &c["callsign"], &c["platform"], &c["armed"], &c["lon"], &c["lat"]),
        (&f["uid"], &f["callsign"], &f["platform"]["id"], &f["platform"]["armed"], &f["position"]["lon"], &f["position"]["lat"]),
        "{c}"
    );
    assert_eq!((&c["nav_ready"], &c["link_lost"]), (&json!(true), &json!(false)));
    assert!(c["stale"].as_array().is_some_and(|s| s.contains(&json!("position"))), "{c}");
    assert!(!c["stale"].as_array().is_some_and(|s| s.contains(&json!("nav"))), "{c}");
    let glitch_stale = f["nav"]["gps_glitch"]["stale"].as_bool().unwrap_or(true);
    assert_eq!(
        c["stale"].as_array().is_some_and(|s| s.contains(&json!("gps_glitch"))),
        glitch_stale,
        "the glitch flag's own staleness: {c}"
    );
    assert!(small["note"].as_str().is_some_and(|n| n.contains("get_vehicle")));
    let size = |v: &Value| v["vehicles"][0].to_string().len();
    assert!(size(&small) * 3 < size(&full), "{} against {}", size(&small), size(&full));
    // the default is the full view, and uids filters either
    r.vehicle(2, Firmware::Copter, "delivery_drone", (LAT, LON + 0.001));
    sleep(Duration::from_millis(500)).await;
    let both = r.ok(&planner(), "get_fleet", json!({ "compact": true })).await;
    assert_eq!(both["count"], 2);
    assert_eq!(r.ok(&planner(), "get_fleet", json!({})).await["compact"], false);

    // the same views as resources
    let fleet = read(&r.mcp, &planner(), "fleetmesh://fleet").await.expect("resource");
    assert_eq!(fleet["count"], 2);
    let one = read(&r.mcp, &planner(), "fleetmesh://fleet/1").await.expect("resource");
    assert_eq!(one["vehicle"]["uid"], 1);
    assert!(read(&r.mcp, &planner(), "fleetmesh://fleet/9").await.is_err());
    assert!(read(&r.mcp, &planner(), "fleetmesh://nothing").await.is_err());
    let (err, v) = r.call(&planner(), "get_vehicle", json!({ "uid": 9 })).await;
    assert!(err);
    assert_eq!(v["status"], 404);
}

#[tokio::test(start_paused = true)]
async fn the_other_reads_answer() {
    let r = rig();
    r.vehicle(1, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let areas = r.ok(&planner(), "get_areas", json!({})).await;
    assert!(areas["areas"].to_string().contains("alcatraz"));
    let policy = r.ok(&planner(), "get_policy", json!({})).await;
    assert!(policy["policy"]["tier"].as_array().is_some_and(|t| !t.is_empty()), "{policy}");
    let platforms = r.ok(&planner(), "get_platforms", json!({})).await;
    assert!(platforms["platforms"].as_array().is_some_and(|p| p.len() >= 4));
    // a hold: the mode change is a vehicle event
    let out = r
        .ok(
            &planner(),
            "propose_intent",
            json!({ "task": "hold", "uids": [1], "rationale": "hold while the picture settles" }),
        )
        .await;
    let id = out["id"].as_u64().expect("id");
    assert_eq!(r.run_until_finished(id, 60).await, State::Done);
    let events = r.ok(&planner(), "get_recent_events", json!({ "uid": 1 })).await;
    assert!(events["count"].as_u64().is_some_and(|c| c >= 1), "{events}");
    assert_eq!(events["events"][0]["kind"], "mode");
    assert!(events["events"][0]["age_s"].as_f64().is_some());
    let mine = r
        .ok(&planner(), "get_intents", json!({ "mine": true }))
        .await;
    assert_eq!(mine["count"], 1);
    let theirs = r.ok(&watch(), "get_intents", json!({ "mine": true })).await;
    assert_eq!(theirs["count"], 0);
    // a proposal without a rationale is refused before any intent exists
    let (err, v) = r
        .call(&planner(), "propose_intent", json!({ "task": "hold", "uids": [1] }))
        .await;
    assert!(err);
    assert!(v["error"].to_string().contains("rationale"), "{v}");
    assert_eq!(r.events("intent.created"), 1);
    r.replays();
}

#[test]
fn unlisted_agents_are_refused() {
    let r = rig();
    assert!(r.mcp.resolve("agent-watch").is_ok());
    let err = r.mcp.resolve("nobody").expect_err("not listed");
    assert!(err.message.contains("FLEETMESH_AGENTS"), "{}", err.message);
}


// ---- over HTTP --------------------------------------------------------------------------

/// One JSON-RPC exchange with the mounted endpoint; the answer may come as JSON or as an
/// SSE stream (the last `data:` event is the response).
async fn rpc(
    router: &Router,
    session: Option<&str>,
    agent: Option<&str>,
    body: Value,
) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::HOST, "127.0.0.1");
    if let Some(s) = session {
        req = req.header("mcp-session-id", s);
    }
    if let Some(a) = agent {
        req = req.header(AGENT_HEADER, a);
    }
    let resp = router
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let status = resp.status();
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let sse = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));
    let bytes = tokio::time::timeout(
        Duration::from_secs(10),
        axum::body::to_bytes(resp.into_body(), 4 << 20),
    )
    .await
    .expect("the response stream ends")
    .expect("body");
    let text = String::from_utf8_lossy(&bytes);
    let json = if sse {
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or(Value::Null)
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    (status, session, json)
}

#[tokio::test]
async fn the_endpoint_takes_the_agent_header_over_http() {
    let r = rig();
    r.vehicle(1, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let router = r.api.clone().nest_service(
        "/mcp",
        service(
            Settings {
                agents: vec![planner()],
                keys: None,
                idle: IDLE_LIMIT,
                operator: None,
            },
            r.world.clone(),
            r.d.clone(),
            r.api.clone(),
        ),
    );

    let (status, session, init) = rpc(
        &router,
        None,
        None,
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "scripted-agent", "version": "0.1" },
            },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{init}");
    let session = session.expect("a session id");
    assert_eq!(init["result"]["serverInfo"]["name"], "fleetmesh-bridge");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .expect("instructions")
            .contains("cannot approve")
    );
    let (status, _, _) = rpc(
        &router,
        Some(&session),
        None,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let (status, _, list) = rpc(
        &router,
        Some(&session),
        None,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"propose_intent"), "{names:?}");
    assert!(names.iter().all(|n| !n.contains("approve")), "{names:?}");
    // every tool says how large an answer its client should hand the model whole (Claude
    // Code saved larger ones to a file a model with no file tools cannot read)
    for t in list["result"]["tools"].as_array().expect("tools") {
        assert_eq!(
            t["_meta"]["anthropic/maxResultSizeChars"].as_u64(),
            Some(tools::RESULT_MAX_CHARS),
            "{t}"
        );
    }
    // protocol 2026-07-28 requires both on list results; Claude Code 2.1.294 refused the
    // list without them and offered the model no tool at all
    for method in ["tools/list", "resources/list", "resources/templates/list"] {
        let (_, _, list) = rpc(
            &router,
            Some(&session),
            None,
            json!({ "jsonrpc": "2.0", "id": 5, "method": method }),
        )
        .await;
        assert!(list["result"]["ttlMs"].as_u64().is_some(), "{method}: {list}");
        assert_eq!(list["result"]["cacheScope"], "private", "{method}: {list}");
    }

    // no agent id: refused with a hint; an unlisted one: refused; a listed one: served
    async fn call(router: &Router, session: &str, agent: &str) -> (StatusCode, Option<String>, Value) {
        rpc(
            router,
            Some(session),
            Some(agent),
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "get_fleet", "arguments": {} },
            }),
        )
        .await
    }
    let (_, _, no_id) = call(&router, &session, "").await;
    // an empty header falls back to clientInfo.name, which isn't listed either
    assert!(
        no_id["error"]["message"]
            .as_str()
            .expect("error")
            .contains("FLEETMESH_AGENTS"),
        "{no_id}"
    );
    let (_, _, unlisted) = call(&router, &session, "nobody").await;
    assert!(unlisted["error"]["message"].to_string().contains("not in FLEETMESH_AGENTS"));
    let (status, _, fleet) = call(&router, &session, "agent-planner").await;
    assert_eq!(status, StatusCode::OK, "{fleet}");
    assert_eq!(fleet["result"]["structuredContent"]["count"], 1, "{fleet}");
    assert_eq!(fleet["result"]["isError"], false);
    assert_eq!(r.actions("session.connected"), 1);

    // a resource read, the same way
    let (_, _, res) = rpc(
        &router,
        Some(&session),
        Some("agent-planner"),
        json!({
            "jsonrpc": "2.0", "id": 4, "method": "resources/read",
            "params": { "uri": "fleetmesh://inbox" },
        }),
    )
    .await;
    let text = res["result"]["contents"][0]["text"].as_str().expect("text");
    let inbox: Value = serde_json::from_str(text).expect("json");
    assert_eq!(inbox["counts"]["pending"], 0);
    assert_eq!(IntentId(0).0, 0);
}

/// An agent session (a client on a protocol version before 2026-07-28 holds one; the Python
/// SDK negotiates 2025-11-25) survives a quiet spell of ten minutes: an agent woken on events
/// can be quiet that long, and rmcp's own five-minute limit dropped the session of an agent
/// that connected once and waited (the situation framework's storyline agent, live). A
/// session idle past the limit is still dropped, and the limit is the bridge's setting
/// (`FLEETMESH_MCP_IDLE_S`). One agent holds at most `SESSIONS_PER_AGENT`: past that the one it
/// used least recently is closed.
#[tokio::test(start_paused = true)]
async fn an_agent_session_survives_a_quiet_spell_and_the_idle_limit_is_set_by_the_bridge() {
    let r = rig();
    r.vehicle(1, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let mount = |idle: Duration| {
        r.api.clone().nest_service(
            "/mcp",
            service(
                Settings { agents: vec![planner()], keys: None, idle, operator: None },
                r.world.clone(),
                r.d.clone(),
                r.api.clone(),
            ),
        )
    };
    async fn open(router: &Router) -> String {
        let (status, session, init) = rpc(
            router,
            None,
            None,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "agent-planner", "version": "0.1" },
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{init}");
        let session = session.expect("a session: this protocol version keeps one");
        rpc(
            router,
            Some(&session),
            None,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
        session
    }
    async fn fleet(router: &Router, session: &str) -> (StatusCode, Value) {
        let (status, _, v) = rpc(
            router,
            Some(session),
            Some("agent-planner"),
            json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "get_fleet", "arguments": {} },
            }),
        )
        .await;
        (status, v)
    }
    // the bridge's default: ten quiet minutes, then a call on the same session
    assert!(IDLE_LIMIT > Duration::from_secs(600), "{IDLE_LIMIT:?}");
    let router = mount(IDLE_LIMIT);
    let session = open(&router).await;
    sleep(Duration::from_secs(600)).await;
    let (status, v) = fleet(&router, &session).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["result"]["structuredContent"]["count"], 1, "{v}");
    // past the limit the session is gone
    sleep(IDLE_LIMIT + Duration::from_secs(5)).await;
    let (status, v) = fleet(&router, &session).await;
    assert_ne!(status, StatusCode::OK, "{v}");
    // a short limit, as `FLEETMESH_MCP_IDLE_S` sets it
    let short = mount(Duration::from_secs(20));
    let session = open(&short).await;
    sleep(Duration::from_secs(10)).await;
    assert_eq!(fleet(&short, &session).await.0, StatusCode::OK);
    sleep(Duration::from_secs(30)).await;
    assert_ne!(fleet(&short, &session).await.0, StatusCode::OK);
    // one agent past its cap of sessions: the one it used least recently is closed (a client
    // that opens sessions and never closes them holds SESSIONS_PER_AGENT, not one per retry)
    let capped = mount(IDLE_LIMIT);
    let mut held = Vec::new();
    for _ in 0..=SESSIONS_PER_AGENT {
        let s = open(&capped).await;
        assert_eq!(fleet(&capped, &s).await.0, StatusCode::OK);
        held.push(s);
        sleep(Duration::from_secs(1)).await;
    }
    sleep(Duration::from_millis(50)).await; // the close runs on its own task
    assert_ne!(fleet(&capped, &held[0]).await.0, StatusCode::OK, "the oldest was closed");
    for s in &held[1..] {
        assert_eq!(fleet(&capped, s).await.0, StatusCode::OK, "the others are kept");
    }
    // sessions their client closed do not count: a quiet one is kept however many others
    // come and go (the storyline agent opens and closes one per proposal)
    async fn close(router: &Router, session: &str) -> StatusCode {
        let req = Request::builder()
            .method("DELETE")
            .uri("/mcp")
            .header(header::HOST, "127.0.0.1")
            .header(AGENT_HEADER, "agent-planner")
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", "2025-11-25")
            .body(Body::empty())
            .expect("request");
        router.clone().oneshot(req).await.expect("response").status()
    }
    let tidy = mount(IDLE_LIMIT);
    let quiet = open(&tidy).await;
    assert_eq!(fleet(&tidy, &quiet).await.0, StatusCode::OK);
    for _ in 0..2 * SESSIONS_PER_AGENT {
        sleep(Duration::from_secs(1)).await;
        let s = open(&tidy).await;
        assert_eq!(fleet(&tidy, &s).await.0, StatusCode::OK);
        let closed = close(&tidy, &s).await;
        assert!(closed.is_success(), "{closed}");
        assert_ne!(fleet(&tidy, &s).await.0, StatusCode::OK, "closed");
    }
    sleep(Duration::from_millis(50)).await;
    assert_eq!(fleet(&tidy, &quiet).await.0, StatusCode::OK, "the quiet one is kept");
    // a session counts from its initialize: sessions that never call a tool are capped too
    let silent = mount(IDLE_LIMIT);
    let mut opened = Vec::new();
    for _ in 0..=SESSIONS_PER_AGENT {
        opened.push(open(&silent).await);
        sleep(Duration::from_secs(1)).await;
    }
    sleep(Duration::from_millis(50)).await;
    assert_ne!(fleet(&silent, &opened[0]).await.0, StatusCode::OK, "the oldest was closed");
    for s in &opened[1..] {
        assert_eq!(fleet(&silent, s).await.0, StatusCode::OK, "the others are kept");
    }
}

/// Final review, nits 1 and 2: an opening request over 1 MB was forwarded with an empty body
/// (now 413), and sessions opened by a claimant naming no listed agent counted against no cap
/// (now one shared bucket, capped like an agent's).
#[tokio::test(start_paused = true)]
async fn an_opening_request_too_large_is_refused_and_unlisted_sessions_are_capped_together() {
    let r = rig();
    let svc = service(
        Settings { agents: vec![planner()], keys: None, idle: IDLE_LIMIT, operator: None },
        r.world.clone(),
        r.d.clone(),
        r.api.clone(),
    );
    let shared = svc.shared.clone();
    let router = r.api.clone().nest_service("/mcp", svc);
    let big = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"pad\":\"{}\"}}", "x".repeat(2 << 20));
    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::HOST, "127.0.0.1")
        .body(Body::from(big))
        .expect("request");
    let resp = router.clone().oneshot(req).await.expect("response");
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    // nobody listed: each initialize opens a session (its calls are refused), all in one bucket
    for _ in 0..=SESSIONS_PER_AGENT {
        let (status, session, init) = rpc(
            &router,
            None,
            None,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "nobody", "version": "0.1" },
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{init}");
        assert!(session.is_some());
        sleep(Duration::from_secs(1)).await;
    }
    let bucket = AgentId::new(UNLISTED);
    assert_eq!(shared.sessions_of(&bucket).len(), SESSIONS_PER_AGENT, "capped together");
    assert!(shared.sessions_of(&AgentId::new("nobody")).is_empty());
}

/// Third review, item 5: the cap closed the session used least recently even with a request
/// in flight on it (a long proposal or dry run lost its answer, and a client that retries
/// may propose twice). A session with a request in flight is never closed: the next idle one
/// goes, and with none idle the cap waits.
#[tokio::test(start_paused = true)]
async fn the_session_cap_never_closes_a_session_with_a_request_in_flight() {
    let r = rig();
    let a = planner();
    r.mcp.note_session_id(&a, "s0");
    let busy = r.mcp.busy(&a, Some("s0".into())).expect("s0 is held");
    for i in 1..=SESSIONS_PER_AGENT {
        sleep(Duration::from_secs(1)).await;
        r.mcp.note_session_id(&a, &format!("s{i}"));
    }
    let held = |r: &Rig| -> Vec<String> { r.mcp.sessions_of(&a).into_iter().map(|(s, _)| s).collect() };
    assert!(held(&r).contains(&"s0".to_string()), "in flight: kept {:?}", held(&r));
    assert!(!held(&r).contains(&"s1".to_string()), "the oldest idle one went instead");
    assert_eq!(held(&r).len(), SESSIONS_PER_AGENT);
    // every one busy: the cap waits rather than cut a request off
    let all: Vec<_> = held(&r)
        .into_iter()
        .filter_map(|s| r.mcp.busy(&a, Some(s)))
        .collect();
    r.mcp.note_session_id(&a, "s99");
    assert_eq!(held(&r).len(), SESSIONS_PER_AGENT + 1, "none closed while all are busy");
    drop(all);
    drop(busy);
    assert!(r.mcp.sessions_of(&a).iter().all(|(_, n)| *n == 0), "the guards ended");
    sleep(Duration::from_secs(1)).await;
    r.mcp.note_session_id(&a, "s100");
    assert_eq!(held(&r).len(), SESSIONS_PER_AGENT, "and then it closes the oldest idle ones");
}

/// `POST /api/operator/view` over HTTP (Phase G): an operator's display event is logged under
/// that operator; without the operator header, or with only an agent's, nothing is logged.
/// An agent cannot write what "the operator saw".
#[tokio::test(start_paused = true)]
async fn operator_views_need_an_operator_over_http() {
    let r = rig();
    r.vehicle(3, Firmware::Copter, "delivery_unrated", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    let id = r
        .ok(&planner(), "propose_intent", orbit(3, "cover the approach"))
        .await["id"]
        .as_u64()
        .expect("id");
    assert_eq!(r.state(id), State::PendingApproval);
    let post = |who: Option<(&'static str, &'static str)>, body: Value| {
        let api = r.api.clone();
        async move {
            let mut req = Request::builder()
                .method("POST")
                .uri("/api/operator/view")
                .header(header::CONTENT_TYPE, "application/json");
            if let Some((h, v)) = who {
                req = req.header(h, v);
            }
            api.oneshot(req.body(Body::from(body.to_string())).expect("request"))
                .await
                .expect("response")
                .status()
        }
    };
    let view = json!({
        "intent_id": id, "event": "open",
        "shown": { "checks": ["readiness:3"], "warnings": ["armed platform: vehicle 3"], "rationale": true },
    });
    assert_eq!(post(None, view.clone()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        post(Some((AGENT_HEADER, "agent-planner")), view.clone()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(Some(("X-FleetMesh-Operator", "op-9")), view.clone()).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(r.events("operator.view"), 0);
    assert_eq!(
        post(Some(("X-FleetMesh-Operator", "op-1")), view).await,
        StatusCode::OK
    );
    let decide = json!({
        "intent_id": id, "event": "decide", "action": "reject", "open_ms": 4200,
        "shown": { "checks": [], "warnings": [], "rationale": true },
    });
    assert_eq!(
        post(Some(("X-FleetMesh-Operator", "op-1")), decide).await,
        StatusCode::OK
    );
    let bad = json!({ "intent_id": id, "event": "decide", "shown": {} });
    assert_eq!(
        post(Some(("X-FleetMesh-Operator", "op-1")), bad).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(r.events("operator.view"), 2);
    assert_eq!(
        r.state(id),
        State::PendingApproval,
        "a view decides nothing"
    );
    r.replays();
}

/// `get_link_coverage` (read-only, not logged) at points, for an order, over a grid and several
/// at once; and the same order answer inside `preview_intent` as `link_coverage`, information
/// only: the preview is logged as before, without it, and the log replays.
#[tokio::test(start_paused = true)]
async fn link_coverage_is_read_only_and_rides_in_the_preview() {
    let Some(coverage) = crate::coverage::tests::golden_gate_coverage() else { return };
    let r = rig();
    assert!(r.world.set_coverage(coverage));
    r.vehicle(1, Firmware::Copter, "delivery_drone", (LAT, LON));
    let cell = r.world.vehicles().into_iter().find(|c| c.uid == 1).expect("cell");
    cell.write(|v| {
        v.home = Some((LAT, LON));
        v.home_alt = Some(5.0);
        v.seen.home = Some(crate::clock::wall());
    });
    sleep(Duration::from_millis(500)).await;

    // a class at points: behind the Marin Headlands at 85 m, and over the bay
    let pts = r
        .ok(
            &planner(),
            "get_link_coverage",
            json!({ "class": "los", "alt_m": 85.0, "points": [[-122.52, 37.845], [-122.42, 37.825]] }),
        )
        .await;
    assert_eq!(pts["points"][0]["state"], "down", "{pts}");
    assert_eq!(pts["points"][0]["reason"], "terrain");
    assert_eq!(pts["points"][1]["state"], "up", "{pts}");
    assert_eq!(pts["predicted"], true);
    assert!(pts["note"].as_str().is_some_and(|n| n.contains("information only")));

    // an order into the shadow: where the copter's mesh link goes, against its 10 s failsafe
    let order = r
        .ok(
            &planner(),
            "get_link_coverage",
            json!({ "uid": 1, "order": { "task": "orbit", "center": [-122.52, 37.845], "radius_m": 150 } }),
        )
        .await;
    let v = &order["vehicles"][0];
    assert_eq!(v["callsign"], "delivery_drone 1", "{order}");
    assert_eq!(v["profile"], "mesh");
    assert_eq!(v["failsafe_s"], 10.0);
    assert_eq!(v["holds"], false, "{v}");
    let parts: Vec<&str> = v["legs"].as_array().expect("legs").iter().filter_map(|l| l["part"].as_str()).collect();
    assert_eq!(parts, ["transit", "task", "return"]);
    let s = v["sentence"].as_str().expect("sentence");
    assert!(s.starts_with("delivery_drone 1 would lose its link "), "{s}");

    // a grid, kept to what lands whole in a model's context
    let grid = r
        .ok(
            &planner(),
            "get_link_coverage",
            json!({ "class": "mesh", "alt_m": 30.0, "above_surface": true,
                    "bbox": [-122.60, 37.70, -122.30, 37.90], "res_m": 100 }),
        )
        .await;
    let n = grid["nlat"].as_u64().expect("n") * grid["nlon"].as_u64().expect("n");
    assert!(n <= tools::AGENT_GRID_CELLS, "{n}");
    assert_eq!(grid["coarsened"], true);
    assert_eq!(grid["alt_ref"], "surface");
    assert_eq!(grid["state"].as_str().expect("state").len() as u64, n);

    // several at once, a refusal among them answered in its place
    let many = r
        .ok(
            &planner(),
            "get_link_coverage",
            json!({ "queries": [
                { "uid": 1 },
                { "class": "laser", "point": [-122.4, 37.8] },
                { "uid": 1, "brief": true, "order": { "task": "orbit", "center": [-122.52, 37.845], "radius_m": 150 } },
            ] }),
        )
        .await;
    assert_eq!(many["answers"][0]["uid"], 1, "{many}");
    assert!(many["answers"][1]["error"].as_str().is_some_and(|e| e.contains("laser")), "{many}");
    let brief = &many["answers"][2]["vehicles"][0];
    assert_eq!(brief["holds"], false, "{many}");
    assert!(brief.get("legs").is_none() && brief["sentence"] == v["sentence"], "{brief}");
    // and a refusal of the whole call is a tool error the model can read
    let (err, why) = r.call(&planner(), "get_link_coverage", json!({ "class": "los" })).await;
    assert!(err, "{why}");

    // the preview carries the order's answer; the log is as without it
    let pv = r.ok(&planner(), "preview_intent", orbit(1, "")).await;
    let lc = &pv["link_coverage"];
    assert_eq!(lc["vehicles"][0]["uid"], 1, "{pv}");
    assert!(lc["vehicles"][0]["sentence"].as_str().is_some_and(|s| s.starts_with("delivery_drone 1")), "{lc}");
    assert!(lc["vehicles"][0]["holds"].is_boolean());
    assert_eq!(r.actions("intent.preview"), 1);
    assert!(!r.log.text().contains("link_coverage"), "never logged");
    assert!(!r.log.text().contains("would lose its link"), "never logged");
    r.replays();
}


/// murmur-z07i: a fleet manifest written to a fresh temporary file, and a rig reading it with
/// one fake vehicle per entry (its identity the manifest's).
fn rig_with_fleet(name: &str, manifest: Value) -> Rig {
    let dir = std::env::temp_dir().join(format!("fleetmesh-swarms-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("fleet.json");
    std::fs::write(&path, manifest.to_string()).expect("manifest");
    let r = rig_with_manifest(path);
    r.world.load_manifest();
    for (i, v) in manifest["vehicles"].as_array().expect("vehicles").iter().enumerate() {
        let uid = v["uid"].as_u64().expect("uid");
        let cell = spawn_fake(uid, Firmware::Copter, LAT + 0.001 * i as f64, LON, quick());
        let identity = r.world.manifest().identities.get(&uid).cloned();
        cell.write(|v| v.identity = identity);
        r.world.insert_vehicle(cell);
    }
    r
}

async fn get_json(r: &Rig, uri: &str) -> Value {
    let req = Request::builder().uri(uri).body(Body::empty()).expect("request");
    let resp = r.api.clone().oneshot(req).await.expect("response");
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24).await.expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

fn lost(uid: Uid) -> crate::decision::alerts::Alert {
    crate::decision::alerts::Alert {
        id: format!("link:{uid}"),
        kind: "lost_link",
        severity: crate::decision::alerts::Severity::High,
        uid,
        callsign: None,
        detail: "no telemetry".into(),
        since_t: 100.0 + uid as f64,
        op_id: None,
        intent_id: None,
        issue: None,
        track_id: None,
        track_ids: Vec::new(),
    }
}

/// murmur-z07i: a manifest without swarms (every one before Saturation) changes nothing: no
/// `swarm` in any identity, no `swarm_alerts` or `folded_into` in the inbox however many
/// vehicles lose their link, and the swarm views answer an empty list.
#[tokio::test(start_paused = true)]
async fn an_old_manifest_has_no_swarms_anywhere() {
    let vehicles: Vec<Value> = (1..=6)
        .map(|u| json!({"uid": u, "sysid": u, "port": 14560 + u, "callsign": format!("HAWK {u}"), "platform": "delivery_drone", "domain": "air"}))
        .collect();
    let r = rig_with_fleet("old", json!({ "vehicles": vehicles }));
    sleep(Duration::from_millis(500)).await;
    for (u, entry) in (1..=6u64).zip(&vehicles) {
        let id = r.world.manifest().identities.get(&u).cloned().expect("identity");
        assert_eq!(id.0, Identity::from_manifest(entry.as_object().expect("entry")).0);
        assert!(!id.0.contains_key("swarm"));
    }
    let inbox = r.d.inbox_json();
    assert!(inbox.get("swarm_alerts").is_none(), "{inbox}");
    assert!(inbox["counts"].get("swarm_alerts").is_none());
    let alerts: Vec<_> = (1..=6).map(lost).collect();
    assert!(crate::swarms::inbox_view(&r.world, &alerts, &[], false).is_none());
    let api = get_json(&r, "/api/swarms").await;
    assert_eq!(api["swarms"], json!([]));
    let tool = r.ok(&planner(), "get_swarms", json!({})).await;
    assert_eq!(tool["swarms"], json!([]));
    assert_eq!(tool["count"], 0);
}

/// murmur-z07i: a manifest with swarms: each identity names its swarm (null for none), the
/// summary counts each swarm's members, and three members' lost links fold into one swarm
/// alert while every member's alert stays as it was, marked.
#[tokio::test(start_paused = true)]
async fn a_swarm_manifest_names_members_and_folds_their_alerts() {
    let vehicles: Vec<Value> = (1..=6)
        .map(|u| {
            let swarm = if u <= 4 {
                json!("PICKET-1")
            } else if u == 5 {
                json!("RELAY-1")
            } else {
                Value::Null
            };
            json!({"uid": u, "sysid": u, "port": 14560 + u, "callsign": format!("HAWK {u}"), "platform": "delivery_drone", "domain": "air", "swarm": swarm})
        })
        .collect();
    let r = rig_with_fleet(
        "new",
        json!({
            "swarms": [
                {"id": "PICKET-1", "name": "Picket 1", "role": "picket", "color": "#ff8800"},
                {"id": "RELAY-1", "name": "Relay 1", "role": "relay"},
            ],
            "vehicles": vehicles,
        }),
    );
    sleep(Duration::from_millis(500)).await;
    let swarm_of = |u: u64| r.world.manifest().identities[&u].0.get("swarm").cloned();
    assert_eq!(swarm_of(1), Some(json!("PICKET-1")));
    assert_eq!(swarm_of(5), Some(json!("RELAY-1")));
    assert_eq!(swarm_of(6), Some(Value::Null));

    let api = get_json(&r, "/api/swarms").await;
    assert_eq!(api["hull_closed"], false);
    let list = api["swarms"].as_array().expect("swarms");
    assert_eq!(list.len(), 2);
    let picket = &list[0];
    assert_eq!(picket["id"], "PICKET-1");
    assert_eq!(picket["role"], "picket");
    assert_eq!(picket["color"], "#ff8800");
    assert_eq!(picket["count"], 4);
    assert_eq!(picket["members"], json!([1, 2, 3, 4]));
    assert_eq!(picket["lost_link"], 0);
    assert!(picket["centroid"].is_array(), "{picket}");
    assert_eq!(picket["hull"].as_array().map(Vec::len), Some(2), "four fakes on one meridian: {picket}");
    let tool = r.ok(&planner(), "get_swarms", json!({ "ids": ["RELAY-1"] })).await;
    assert_eq!(tool["count"], 1);
    assert_eq!(tool["swarms"][0]["count"], 1);
    assert!(tool["swarms"][0].get("members").is_none() && tool["swarms"][0].get("hull").is_none());
    let tool = r.ok(&planner(), "get_swarms", json!({ "members": true, "hull": true })).await;
    assert_eq!(tool["swarms"][0]["members"], json!([1, 2, 3, 4]));
    assert!(tool["swarms"][0]["hull"].is_array());

    // three of PICKET-1's four lose their link (the threshold is 3), RELAY-1's one and the
    // vehicle in no swarm too: one swarm alert, and those three alerts point at it
    let alerts: Vec<_> = [1, 2, 3, 5, 6].into_iter().map(lost).collect();
    let (list, folds) = crate::swarms::inbox_view(&r.world, &alerts, &[], false).expect("swarms");
    assert_eq!(folds.len(), 1, "{folds:?}");
    assert_eq!(folds[0].id, "swarm:PICKET-1:lost_link");
    assert_eq!((folds[0].count, folds[0].of), (3, 4));
    assert_eq!(folds[0].causes.get("unknown"), Some(&3));
    assert_eq!(list.len(), 5);
    for (a, v) in alerts.iter().zip(&list) {
        let mut plain = serde_json::to_value(a).expect("alert");
        if a.uid <= 3 {
            assert_eq!(v["folded_into"], "swarm:PICKET-1:lost_link");
            plain["folded_into"] = v["folded_into"].clone();
        }
        assert_eq!(*v, plain, "an alert is as it was but for folded_into");
    }
    let inbox = r.d.inbox_json();
    assert_eq!(inbox["swarm_alerts"], json!([]), "no alert in force here: {inbox}");
    assert_eq!(inbox["counts"]["swarm_alerts"], 0);
}

// ---- rules 33: the scenario's settings in what the bridge serves (docs/SCENARIO-SETTINGS.md S4) ----




/// Rules 34 (docs/SWARMS.md section 10; bd murmur-4nik): an agent proposes group orders and
/// transits, and decides nothing about them. Its preview of an order to many builds the group as
/// the operator's dry run does (the members laid out and checked share by share, its bottom line
/// against its minimum, each rule's passing member checks counted unless `members`); its
/// proposal goes out by the policy as one group order of member orders; get_fleet and
/// get_platforms name transit beside patrol; and no tool approves, re-forms, resumes, suspends or
/// recalls a group: every such name is an unknown tool, and the log holds no re-form, suspension or
/// approval by anyone but the policy. Under rules 33 a group spec and a transit are refused as an
/// operator's would be, and the fleet lists no transit.
#[tokio::test(start_paused = true)]
async fn agents_propose_group_orders_and_no_tool_approves_or_reforms_one() {
    let r = rig_policy(|p| p.rules.rules_version = fleetmesh_decide::RULES_VERSION);
    for k in 0..4u64 {
        r.vehicle(k + 1, Firmware::Copter, "delivery_drone", (LAT, LON + 0.0008 * k as f64));
    }
    sleep(Duration::from_millis(500)).await;
    let tasks = r.ok(&planner(), "get_fleet", json!({ "uids": [1] })).await["vehicles"][0]["platform"]["tasks"].clone();
    assert_eq!(tasks, json!(["orbit", "patrol", "transit", "hold", "rtl"]), "transit beside patrol");
    let platforms = r.ok(&planner(), "get_platforms", json!({})).await;
    let delivery_drone = platforms["platforms"].as_array().expect("platforms").iter().find(|p| p["id"] == "delivery_drone").expect("delivery_drone").clone();
    assert!(delivery_drone["tasks"].as_array().expect("tasks").contains(&json!("transit")), "{delivery_drone}");

    let ring = json!({
        "task": "orbit", "uids": [1, 2, 3, 4], "center": [LON + 0.004, LAT], "radius_m": 150.0,
        "group": { "min_members": 3 },
    });
    let p = r.ok(&planner(), "preview_intent", ring.clone()).await;
    assert_eq!(p["passed"], true, "{p}");
    let g = &p["group"];
    assert_eq!((g["members"].as_u64(), g["sent"].as_u64(), g["waiting"].as_u64()), (Some(4), Some(4), Some(0)), "{g}");
    assert_eq!(g["min_members"], 3, "the spec's minimum");
    assert_eq!(g["reforms"], true, "an orbit re-forms by default");
    assert_eq!(g["layout"]["spread"], "ring", "{g}");
    assert!(g["summary"].as_str().is_some_and(|s| s.starts_with("4 of 4 would be sent now")), "{g}");
    assert!(g.get("shares").is_none() && g.get("tasks").is_none(), "members folded: {g}");
    let checks = p["checks"].as_array().expect("checks");
    assert!(checks.iter().all(|c| c["uid"].is_null() || c["outcome"] != "pass"), "passing member checks counted: {p}");
    assert!(checks.iter().any(|c| c["rule"] == "strength"), "the group's own checks stay: {p}");
    assert!(p["checks_folded"]["passed_by_rule"]["readiness"].as_u64() == Some(4), "{p}");
    let mut whole = ring.clone();
    whole["members"] = json!(true);
    let w = r.ok(&planner(), "preview_intent", whole).await;
    assert_eq!(w["group"]["shares"].as_object().map(|s| s.len()), Some(4), "{w}");
    assert_eq!(w["group"]["tasks"].as_object().map(|s| s.len()), Some(4), "{w}");
    assert!(w.get("checks_folded").is_none(), "every check with members: {w}");

    // a transit to a block round the route's end, and a spec the engine refuses
    let block = r
        .ok(
            &planner(),
            "preview_intent",
            json!({ "task": "transit", "uids": [1, 2, 3, 4], "route": [[LON + 0.002, LAT + 0.002], [LON + 0.006, LAT + 0.002]], "group": { "spread": "block" } }),
        )
        .await;
    assert_eq!(block["passed"], true, "{block}");
    assert_eq!(block["group"]["spec"]["spread"], "block", "{block}");
    assert_eq!(block["group"]["reforms"], false, "a transit never re-forms");
    let (err, v) = r
        .call(&planner(), "preview_intent", json!({ "task": "orbit", "uids": [1, 2], "center": [LON, LAT], "group": { "spread": "lanes" } }))
        .await;
    assert!(err && v.to_string().contains("lanes spreads no orbit"), "{v}");
    let (err, v) = r
        .call(&planner(), "preview_intent", json!({ "task": "orbit", "uids": [1, 2], "center": [LON, LAT], "group": { "slots": 3 } }))
        .await;
    assert!(err && v.to_string().contains("slots"), "an unknown key is refused: {v}");

    // proposed: the standing policy's default tier sends it as four member orders
    let mut prop = ring.clone();
    prop["rationale"] = json!("ring the east approach");
    let o = r.ok(&planner(), "propose_intent", prop).await;
    assert_eq!(o["state"], "dispatched", "{o}");
    assert_eq!(o["dispatched"], true, "{o}");
    assert_eq!(o["member_orders"]["count"], 4, "{o}");
    assert!(o["intent"]["checks_folded"].is_object(), "{o}");
    let gid = o["id"].as_u64().expect("id");

    for name in [
        "approve_group",
        "approve_group_order",
        "reform_group",
        "re_form_group",
        "reform",
        "regroup",
        "resume_group",
        "suspend_group",
        "recall_group",
        "set_group_minimum",
        "reform_swarm",
        "approve_swarm",
    ] {
        let args = json!({ "id": gid, "uids": [1, 2, 3, 4], "note": "please" }).as_object().cloned().expect("object");
        let err = tools::call(&r.mcp, &planner(), name, args).await.expect_err(name);
        assert!(err.message.contains("unknown tool"), "{name}: {}", err.message);
    }
    assert_eq!(r.events("group.reformed") + r.events("group.suspended") + r.events("approval.granted"), 0);
    assert!(r.log.lines().iter().all(|l| !l.contains("\"source\":{\"kind\":\"agent\"") || !l.contains("\"type\":\"approval")));
    r.replays_since(fleetmesh_decide::RULES_VERSION);
}

// ---- operator tasking: the officer's groups and replies -----------------------------------

/// The tasking tools are offered only by a bridge that keeps the operator's stores (the live
/// one): without them (the headless simulator) the tool list is the shared one, unchanged, and a
/// tasking tool is an unknown tool.
#[tokio::test(start_paused = true)]
async fn the_tasking_tools_are_offered_only_where_the_operator_stores_are_kept() {
    let r = rig();
    let names = |l: Vec<rmcp::model::Tool>| l.into_iter().map(|t| t.name.to_string()).collect::<Vec<_>>();
    let base = names(tools::tools());
    let mut with = base.clone();
    with.extend(["get_groups", "make_group", "reply_tasking"].map(String::from));
    assert_eq!(names(r.mcp.tool_list()), with);
    let bare = Mcp::new(
        Settings { agents: vec![watch()], keys: None, idle: IDLE_LIMIT, operator: None },
        r.world.clone(),
        r.d.clone(),
        r.api.clone(),
    );
    assert_eq!(names(bare.tool_list()), base, "the headless tool list, and an evaluation stack's, is as before");
    // offered only where the stack's runner asks (scripts/up.sh); eval/stack.py never does
    for on in ["1", "on", "true", "yes", " TRUE "] {
        assert!(tasking_tools_on(Some(on)), "{on}");
    }
    for off in [None, Some(""), Some("0"), Some("off"), Some("false"), Some("2")] {
        assert!(!tasking_tools_on(off), "{off:?}");
    }
    assert_eq!(TASKING_TOOLS_ENV, "FLEETMESH_TASKING_TOOLS");
    for name in ["get_groups", "make_group", "reply_tasking"] {
        let args = json!({ "tasking_id": "tk-1" }).as_object().cloned().expect("object");
        let err = tools::call(&bare, &watch(), name, args).await.expect_err(name);
        let listed = err.message.split("the tools are").nth(1).unwrap_or_default();
        assert!(err.message.contains("unknown tool") && ["get_groups", "make_group", "reply_tasking"].iter().all(|t| !listed.contains(t)), "{name}: {}", err.message);
    }
}

/// The watch officer makes groups for one of the operator's requests and answers it: each group
/// marked as the agent's, for that request, its members all in the fleet; it changes only its own
/// until an operator changes it, never the operator's; a reply names only its own intents and
/// carries the groups it made. None of it reaches the audit log or moves an intent.
#[tokio::test(start_paused = true)]
async fn the_officer_groups_and_answers_a_request_and_none_of_it_decides_or_is_logged() {
    let r = rig();
    for uid in 1..=4 {
        r.vehicle(uid, Firmware::Copter, "delivery_drone", (LAT, LON + 0.0002 * uid as f64));
    }
    sleep(Duration::from_millis(500)).await;
    let post = |text: &str| r.ops.tasking.write().unwrap_or_else(|e| e.into_inner()).post(text, "op-1", 1).expect("posted").id;
    let tk = post("split the four in two groups, one orbit each");
    // the officer proposes for it (logged, as any proposal), and someone else proposes too
    let mine = r.ok(&watch(), "propose_intent", orbit(1, "the operator's tk-1: the west pair")).await["id"].as_u64().expect("id");
    let theirs = r.ok(&planner(), "propose_intent", orbit(3, "the planner's own")).await["id"].as_u64().expect("id");
    let states = (r.state(mine), r.state(theirs));
    let before = r.log.text();

    // a group for the request: the agent's, for tk-1
    let g = r.ok(&watch(), "make_group", json!({ "tasking_id": tk, "name": "West pair", "members": [1, 2], "color": "#7fb8ff" })).await;
    let g = &g["group"];
    assert_eq!((g["id"].clone(), g["by"].clone(), g["agent"].clone(), g["tasking"].clone(), g["members"].clone()), (json!("og-1"), json!("agent-watch"), json!(true), json!("tk-1"), json!([1, 2])), "{g}");
    r.ok(&watch(), "make_group", json!({ "tasking_id": tk, "name": "East pair", "members": [3, 4] })).await;
    // refused: a request not in force, members not in the fleet, no name, unknown fields
    let retired = post("retired before it was done");
    r.ops.tasking.write().unwrap_or_else(|e| e.into_inner()).deactivate(&retired).expect("retired");
    for (args, says) in [
        (json!({ "tasking_id": "tk-9", "name": "x", "members": [1] }), "no tasking tk-9"),
        (json!({ "tasking_id": retired, "name": "x", "members": [1] }), "retired by the operator"),
        (json!({ "tasking_id": tk, "name": "x", "members": [1, 99] }), "uids 99 are not in the fleet"),
        (json!({ "tasking_id": tk, "members": [1] }), "needs its name and its members"),
        (json!({ "tasking_id": tk, "name": "x", "members": [1], "by": "op-1" }), "bad arguments"),
    ] {
        let (err, v) = r.call(&watch(), "make_group", args).await;
        assert!(err && v["error"].as_str().unwrap_or("").contains(says), "{says}: {v}");
    }
    // its own it may change; the operator's, or one the operator changed, it may not
    let (_, _) = r.ops.groups.write().unwrap_or_else(|e| e.into_inner()).create("Operator's", &[1], None, &[1, 2, 3, 4].into(), 2, "op-1").expect("made");
    let renamed = r.ok(&watch(), "make_group", json!({ "tasking_id": tk, "group_id": "og-2", "name": "East pair, span" })).await;
    assert_eq!(renamed["group"]["name"], "East pair, span", "{renamed}");
    let (err, v) = r.call(&watch(), "make_group", json!({ "tasking_id": tk, "group_id": "og-3", "members": [2] })).await;
    assert!(err && v["status"] == 403, "the operator's group: {v}");
    let (err, v) = r.call(&planner(), "make_group", json!({ "tasking_id": tk, "group_id": "og-1", "members": [2] })).await;
    assert!(err && v["status"] == 403, "another agent's: {v}");
    let p = crate::groups::Patch { add: vec![4], ..Default::default() };
    r.ops.groups.write().unwrap_or_else(|e| e.into_inner()).patch("og-1", p, &[1, 2, 3, 4].into(), 3, "op-1").expect("patched");
    let (err, v) = r.call(&watch(), "make_group", json!({ "tasking_id": tk, "group_id": "og-1", "members": [1] })).await;
    assert!(err && v["status"] == 403 && v["error"].as_str().unwrap_or("").contains("changed by op-1"), "{v}");
    // everyone reads the groups, who made each, and who changed it
    let all = r.ok(&planner(), "get_groups", json!({})).await;
    assert_eq!(all["count"], 3, "{all}");
    assert_eq!((all["groups"][0]["members"].clone(), all["groups"][0]["changed_by"].clone()), (json!([1, 2, 4]), json!("op-1")), "{all}");
    assert_eq!((all["groups"][2]["by"].clone(), all["groups"][2].get("agent")), (json!("op-1"), None), "{all}");

    // the answer: its own intents only, the groups it made for the request added
    for (args, says) in [
        (json!({ "tasking_id": tk, "status": "done", "text": "Two orbits", "intents": [theirs] }), "not one you proposed"),
        (json!({ "tasking_id": tk, "status": "done", "text": "Two orbits", "intents": [999] }), "no intent 999"),
        (json!({ "tasking_id": tk, "status": "finished", "text": "Two orbits" }), "status is done, partial, blocked or declined"),
        (json!({ "tasking_id": tk, "status": "done", "text": "x".repeat(601) }), "at most 600 characters"),
        (json!({ "tasking_id": "tk-9", "status": "done", "text": "x" }), "no tasking tk-9"),
    ] {
        let (err, v) = r.call(&watch(), "reply_tasking", args).await;
        assert!(err && v["error"].as_str().unwrap_or("").contains(says), "{says}: {v}");
    }
    let text = "Proposed one orbit; it waits for you in the Inbox\n- i1: SKY 1-2 west\n- the east pair: no order yet, their link is down";
    let a = r.ok(&watch(), "reply_tasking", json!({ "tasking_id": tk, "status": "partial", "text": text, "intents": [mine] })).await;
    let reply = &a["item"]["replies"][0];
    assert_eq!((reply["status"].clone(), reply["agent"].clone(), reply["intents"].clone(), reply["groups"].clone()), (json!("partial"), json!("agent-watch"), json!([mine]), json!(["og-1", "og-2"])), "{a}");
    // the item is as the operator posted it, delivery included
    let item = r.ops.tasking.read().unwrap_or_else(|e| e.into_inner()).get(&tk).cloned().expect("item");
    assert_eq!((item.text.as_str(), item.by.as_str(), item.active, item.delivery), ("split the four in two groups, one orbit each", "op-1", true, crate::tasking::Delivery::Posted));

    assert_eq!(r.log.text(), before, "groups and replies are never logged");
    assert_eq!((r.state(mine), r.state(theirs)), states, "and they move no intent");
    r.replays();
}
