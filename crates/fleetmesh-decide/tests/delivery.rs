//! The delivery orders (docs/design/delivery-orders.md) and rules (delivery-rules.md) in the
//! engine: each of the nine kinds proposes, is checked, routed by the shipped policy
//! (`sim/policy.toml`) and ends; every rule has each of its outcomes and reason codes; the policy
//! loader refuses a file that loosens `confirm_drop_zone`; and every decision replays.

use std::collections::BTreeMap;
use std::path::PathBuf;

use fleetmesh_decide::delivery::{
    DeliveryInputs, DzObservation, LinkAlong, NavFacts, NavTrust, ObservedState, ParcelFact, ParcelState, Places,
    TrafficFact, WxFact,
};
use fleetmesh_decide::intent::{AbortThen, Place, Release, RerouteCause};
use fleetmesh_decide::rules::{self, Outcome, RuleId};
use fleetmesh_decide::policy::{TierRule, When};
use fleetmesh_decide::world::{LostLinkAction, LostLinkPlan, LostLinkSource, NavSource};
use fleetmesh_decide::*;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn t0() -> Time {
    Time::from_secs(10_000)
}

fn places() -> Places {
    Places::load(&root().join("sim/hubs.toml")).expect("the shipped places")
}

fn catalog() -> Catalog {
    Catalog::load(&root().join("sim/platforms.toml")).expect("the shipped catalog")
}

fn hub() -> LonLat {
    places().hubs["hub-presidio"].at()
}

/// A ready vehicle of `platform` at the Presidio hub: fresh facts, a 3D fix, 95% battery and the
/// failsafe policy expects.
fn vehicle(platform: &str, t: Time) -> VehicleFacts {
    let p = catalog().get(platform).cloned().expect("platform");
    let ap = p.autopilot.expect("autopilot");
    let (action, timer) = (LostLinkAction::Rtl, 10.0);
    VehicleFacts {
        platform: Some(p),
        autopilot: Some(Fact::new(ap, t)),
        nav_ready: Some(Fact::new(true, t)),
        position: Some(Fact::new(hub(), t)),
        home: Some(Fact::new(hub(), t)),
        battery_pct: Some(Fact::new(95.0, t)),
        gps_fix: Some(Fact::new(3, t)),
        gps_glitch: Some(Fact::new(false, t)),
        velocity_mps: Some(Fact::new([0.0, 0.0], t)),
        lost_link: Some(Fact::new(
            LostLinkPlan { action, timer_s: Some(timer), source: LostLinkSource::Read, circle_m: None },
            t,
        )),
        ..VehicleFacts::default()
    }
}

fn calm(t: Time) -> WxFact {
    WxFact {
        at: hub(),
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
    }
}

fn good_nav() -> NavFacts {
    NavFacts { sats: Some(12), hdop: Some(0.9), h_acc_m: Some(2.0), ..NavFacts::default() }
}

fn parcel(mass: f64, uid: &str) -> ParcelFact {
    ParcelFact { mass_kg: Some(mass), class: "normal".into(), state: ParcelState::Loaded, vehicle: Some(Uid::new(uid)) }
}

/// A world with `sky-1`, a delivery drone at the hub with parcel `p-1` aboard, the shipped areas
/// and places, calm weather, a good fix and a clear drop zone.
fn world() -> WorldSnapshot {
    let t = t0();
    let mut w = WorldSnapshot::new(t);
    w.areas = Some(AreaSet::load_dir(&root().join("sim/areas"), t).expect("areas"));
    w.vehicles.insert(Uid::new("sky-1"), vehicle("delivery_drone", t));
    let mut di = DeliveryInputs { places: places(), ..DeliveryInputs::default() };
    di.parcels.insert("p-1".into(), parcel(1.0, "sky-1"));
    di.weather.push(calm(t));
    di.nav.insert(Uid::new("sky-1"), good_nav());
    di.observations.insert("dz-harbour-17".into(), DzObservation { t, state: ObservedState::Clear, count: 0 });
    w.delivery = Some(di);
    w
}

fn di(w: &mut WorldSnapshot) -> &mut DeliveryInputs {
    w.delivery.as_mut().expect("delivery inputs")
}

fn sky() -> Uid {
    Uid::new("sky-1")
}

fn deliver_to(zone: &str) -> Task {
    let z = &places().drop_zones[zone];
    Task::Deliver {
        parcel: "p-1".into(),
        drop_zone: zone.into(),
        at: z.at(),
        route: Vec::new(),
        release: Release::Land,
        window_end: None,
    }
}

fn deliver() -> Task {
    deliver_to("dz-harbour-17")
}

fn spec(task: Task) -> IntentSpec {
    IntentSpec {
        id: IntentId(1),
        created_t: t0(),
        source: Source::Human { id: OperatorId::new("op-1") },
        task,
        uids: vec![sky()],
        constraints: Constraints::default(),
        rationale: None,
        grouping: Grouping::default(),
    }
}

fn cfg() -> RulesConfig {
    RulesConfig::default()
}

fn run(task: Task, w: &WorldSnapshot) -> Vec<CheckResult> {
    rules::evaluate(&spec(task), w, &cfg())
}

/// The shipped comparison policy (`sim/policy-strict.toml`): the rules 1 drop-zone procedure, a
/// person's confirmation of each zone within `confirm_valid_s`.
fn strict() -> Policy {
    Policy::load(&root().join("sim/policy-strict.toml")).expect("the strict policy")
}

fn run_with(task: Task, w: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    rules::evaluate(&spec(task), w, cfg)
}

/// Rules 1: the rules config of a run under rules version 1 (a log made before standing approvals).
fn rules_v1() -> RulesConfig {
    RulesConfig { rules_version: 1, ..RulesConfig::default() }
}

/// `zone` as a new delivery point: no standing approval (rules 2).
fn new_point(w: &mut WorldSnapshot, zone: &str) {
    di(w).places.drop_zones.get_mut(zone).expect("zone").approval = None;
}

/// Every fact of the rig's world restamped at its clock (`ms` later), with calm weather and a
/// clear look at `dz-harbour-17`, as a later decision would read them.
fn later(r: &mut Rig, ms: i64) {
    r.clock.advance_ms(ms);
    r.world.t = r.world.t.plus_ms(ms);
    let t = r.world.t;
    for f in r.world.vehicles.values_mut() {
        f.position = f.position.as_ref().map(|p| Fact::new(p.value, t));
        f.battery_pct = f.battery_pct.as_ref().map(|p| Fact::new(p.value, t));
        f.autopilot = f.autopilot.as_ref().map(|p| Fact::new(p.value, t));
        f.nav_ready = f.nav_ready.as_ref().map(|p| Fact::new(p.value, t));
        f.gps_fix = f.gps_fix.as_ref().map(|p| Fact::new(p.value, t));
        f.gps_glitch = f.gps_glitch.as_ref().map(|p| Fact::new(p.value, t));
        f.home = f.home.as_ref().map(|p| Fact::new(p.value, t));
        f.velocity_mps = f.velocity_mps.as_ref().map(|p| Fact::new(p.value, t));
        f.lost_link = f.lost_link.as_ref().map(|p| Fact::new(p.value.clone(), t));
    }
    di(&mut r.world).weather = vec![calm(t)];
    di(&mut r.world).observations.insert("dz-harbour-17".into(), DzObservation { t, state: ObservedState::Clear, count: 0 });
}

fn result(rs: &[CheckResult], rule: RuleId) -> &CheckResult {
    rs.iter().find(|r| r.rule == rule).unwrap_or_else(|| panic!("no {rule:?} result in {rs:#?}"))
}

#[track_caller]
fn expect(rs: &[CheckResult], rule: RuleId, outcome: Outcome, code: Option<&str>) {
    let r = result(rs, rule);
    assert_eq!(r.outcome, outcome, "{rule:?}: {} ({:?})", r.detail, r.codes);
    if let Some(c) = code {
        assert!(r.codes.iter().any(|k| k == c), "{rule:?} lacks {c}: {} ({:?})", r.detail, r.codes);
    }
}

#[track_caller]
fn expect_pass(rs: &[CheckResult], rule: RuleId) {
    expect(rs, rule, Outcome::Pass, None);
}

fn areas_from(features: &str) -> AreaSet {
    let text = format!("{{\"type\":\"FeatureCollection\",\"features\":[{features}]}}");
    AreaSet::from_geojson("test", &text, t0()).expect("areas")
}

fn square(id: &str, kind: &str, props: &str, lon: f64, lat: f64, half: f64) -> String {
    format!(
        "{{\"type\":\"Feature\",\"properties\":{{\"id\":\"{id}\",\"kind\":\"{kind}\"{props}}},\"geometry\":{{\"type\":\"Polygon\",\"coordinates\":[[[{a},{c}],[{b},{c}],[{b},{d}],[{a},{d}],[{a},{c}]]]}}}}",
        a = lon - half,
        b = lon + half,
        c = lat - half,
        d = lat + half
    )
}

/// The ops area of the shipped file, so a test can add areas of its own to it.
const OPS: &str = r#"{"type":"Feature","properties":{"id":"gg-ops","kind":"approved"},"geometry":{"type":"Polygon","coordinates":[[[-122.53,37.77],[-122.39,37.77],[-122.39,37.87],[-122.53,37.87],[-122.53,37.77]]]}}"#;

/// The middle of the hub-to-harbour route.
fn mid() -> LonLat {
    let (a, b) = (hub(), places().drop_zones["dz-harbour-17"].at());
    LonLat::new((a.lon + b.lon) / 2.0, (a.lat + b.lat) / 2.0)
}

// ---- the nine kinds: parameters --------------------------------------------------------------

#[test]
fn every_kind_takes_valid_params_and_refuses_bad_ones() {
    let ok = |t: Task| Proposal::new(t, vec![sky()]).validate();
    let site = Place { id: Some("hub-presidio".into()), at: hub() };
    assert_eq!(ok(Task::Pickup { parcel: "p-1".into(), site: site.clone(), load_by_s: Some(300) }), Ok(()));
    assert_eq!(ok(deliver()), Ok(()));
    assert_eq!(ok(Task::Hold { max_s: Some(60) }), Ok(()));
    assert_eq!(ok(Task::Reroute { route: vec![hub()], cause: RerouteCause::KeepOut }), Ok(()));
    assert_eq!(ok(Task::ReturnToHub { hub: Some(site.clone()) }), Ok(()));
    assert_eq!(ok(Task::ReturnToHub { hub: None }), Ok(()));
    assert_eq!(ok(Task::LandNow { site: None }), Ok(()));
    assert_eq!(ok(Task::Abort { target: IntentId(3), then: AbortThen::Hold }), Ok(()));
    assert_eq!(ok(Task::SwapBattery { hub: "hub-presidio".into(), pad: Some("pad-2".into()) }), Ok(()));
    assert_eq!(
        ok(Task::ConfirmDropZone { drop_zone: "dz-harbour-17".into(), parcel: "p-1".into(), evidence: vec!["frame-9".into()] }),
        Ok(())
    );
    // the nine are the delivery kinds, and the same kinds name themselves
    assert_eq!(TaskKind::DELIVERY.len(), 9);
    assert!(TaskKind::DELIVERY.iter().all(|k| TaskKind::from_name(k.as_str()) == Some(*k) && k.is_delivery()));
    // bad ones
    let bad = |t: Task| matches!(ok(t), Err(ProposalError::BadParams(_)));
    assert!(bad(Task::Pickup { parcel: String::new(), site: site.clone(), load_by_s: None }), "no parcel");
    assert!(bad(Task::Pickup { parcel: "p 1".into(), site: site.clone(), load_by_s: None }), "a space in an id");
    assert!(bad(Task::Pickup { parcel: "p-1".into(), site: Place { id: None, at: LonLat::new(500.0, 0.0) }, load_by_s: None }));
    assert!(bad(Task::Deliver {
        parcel: "p-1".into(),
        drop_zone: String::new(),
        at: hub(),
        route: vec![],
        release: Release::Land,
        window_end: None
    }));
    assert!(bad(Task::Deliver {
        parcel: "p-1".into(),
        drop_zone: "dz".into(),
        at: hub(),
        route: vec![],
        release: Release::Land,
        window_end: Some("tomorrow".into())
    }));
    assert!(bad(Task::Hold { max_s: Some(0) }));
    assert!(bad(Task::Reroute { route: vec![], cause: RerouteCause::Weather }));
    assert!(bad(Task::SwapBattery { hub: String::new(), pad: None }));
    assert!(bad(Task::ConfirmDropZone { drop_zone: "dz".into(), parcel: String::new(), evidence: vec![] }));
    // no vehicle
    assert_eq!(Proposal::new(deliver(), vec![]).validate(), Err(ProposalError::NoVehicles));
    // the wire form: {"kind": ..., "params": {...}}
    let v = serde_json::to_value(deliver()).expect("json");
    assert_eq!(v["kind"], "deliver");
    assert_eq!(v["params"]["parcel"], "p-1");
    assert_eq!(v["params"]["release"], "land");
    let back: Task = serde_json::from_value(v).expect("round trip");
    assert_eq!(back, deliver());
    // a delivery order is one vehicle's: a group order of one is refused
    assert!(Spread::default_for(TaskKind::Deliver).is_none());
}

use fleetmesh_decide::group::Spread;

// ---- routing and lifecycle through the engine -------------------------------------------------

struct Rig {
    engine: Engine,
    clock: ManualClock,
    log: MemorySink,
    world: WorldSnapshot,
}

impl Rig {
    fn new() -> Rig {
        Rig::with_policy(Policy::load(&root().join("sim/policy.toml")).expect("the shipped policy"))
    }

    fn with_policy(policy: Policy) -> Rig {
        let clock = ManualClock::new(t0());
        let log = MemorySink::new();
        let roster = Roster::new(vec![OperatorId::new("op-1"), OperatorId::new("op-2")], Vec::new()).expect("roster");
        let engine = Engine::new(policy, roster, Box::new(clock.clone()), Box::new(log.clone())).expect("engine");
        Rig { engine, clock, log, world: world() }
    }

    fn agent(&mut self, task: Task) -> IntentId {
        let w = self.world.clone();
        self.engine
            .agent_port(AgentId::new("agent-planner"))
            .propose(Proposal::new(task, vec![sky()]), Rationale::new("a plain reason").expect("rationale"), &w, &NoTerrain)
            .expect("proposed")
            .id()
    }

    fn human(&mut self, task: Task) -> IntentId {
        let s = self.engine.open_operator_session(OperatorId::new("op-1")).expect("session");
        let w = self.world.clone();
        self.engine.submit(s.authority(), Proposal::new(task, vec![sky()]), &w, &NoTerrain).expect("submitted").id()
    }

    fn approve(&mut self, id: IntentId) -> State {
        let s = self.engine.open_operator_session(OperatorId::new("op-2")).expect("session");
        let w = self.world.clone();
        self.engine.approve(id, s.authority(), Some("seen".into()), &w, &NoTerrain).expect("approved").state()
    }

    fn state(&self, id: IntentId) -> State {
        self.engine.intent(id).expect("intent").state()
    }

    fn tier(&self, id: IntentId) -> (Tier, String) {
        let p = self.engine.intent(id).expect("intent").policy().expect("a decision");
        (p.tier, p.rule.clone())
    }

    fn codes(&self, id: IntentId) -> Vec<String> {
        self.engine.intent(id).expect("intent").checks().iter().flat_map(|c| c.codes.clone()).collect()
    }

    fn replays(&self) -> fleetmesh_decide::replay::ReplayReport {
        let r = fleetmesh_decide::replay::replay_str(&self.log.text()).expect("parses");
        assert!(r.ok(), "{:?}", r.mismatches);
        r
    }

    /// Dispatch an approved or auto-approved order and end it.
    fn run_out(&mut self, id: IntentId) {
        let w = self.world.clone();
        self.engine.dispatch(id, &w).expect("dispatched");
        self.engine.finish(id, TaskOutcome::Done).expect("finished");
    }
}

fn confirm(zone: &str) -> Task {
    Task::ConfirmDropZone { drop_zone: zone.into(), parcel: "p-1".into(), evidence: vec![] }
}

#[test]
fn pickup_from_a_hub_goes_at_once_and_from_any_other_point_waits_for_a_person() {
    let mut r = Rig::new();
    di(&mut r.world).parcels.insert("p-1".into(), ParcelFact { vehicle: None, state: ParcelState::Ready, ..parcel(1.0, "sky-1") });
    let site = Place { id: Some("hub-presidio".into()), at: hub() };
    let at_hub = r.agent(Task::Pickup { parcel: "p-1".into(), site, load_by_s: None });
    assert_eq!(r.state(at_hub), State::AutoApproved, "{:?}", r.engine.intent(at_hub).expect("i").checks());
    assert_eq!(r.tier(at_hub).1, "routine-delivery");
    // a point nobody declared safe: a person
    let elsewhere = r.agent(Task::Pickup {
        parcel: "p-1".into(),
        site: Place { id: None, at: LonLat::new(-122.455, 37.806) },
        load_by_s: None,
    });
    assert_eq!(r.state(elsewhere), State::PendingApproval);
    assert_eq!(r.tier(elsewhere).0, Tier::Human);
    r.run_out(at_hub);
    r.replays();
}

#[test]
fn pickup_preconditions_refuse_an_unknown_parcel_a_taken_one_and_an_unknown_hub() {
    let mut w = world();
    di(&mut w).parcels.insert("p-1".into(), ParcelFact { vehicle: None, state: ParcelState::Ready, ..parcel(1.0, "sky-1") });
    let site = |id: &str| Place { id: Some(id.into()), at: hub() };
    let pickup = |parcel: &str, id: &str| Task::Pickup { parcel: parcel.into(), site: site(id), load_by_s: None };
    expect_pass(&run(pickup("p-1", "hub-presidio"), &w), RuleId::Order);
    expect(&run(pickup("p-9", "hub-presidio"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_PARCEL_UNKNOWN"));
    expect(&run(pickup("p-1", "hub-nowhere"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_UNKNOWN_SITE"));
    expect(&run(pickup("p-1", "hub-marina"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_SITE_MISMATCH"));
    di(&mut w).parcels.insert("p-1".into(), ParcelFact { vehicle: Some(Uid::new("sky-2")), state: ParcelState::Ready, ..parcel(1.0, "x") });
    expect(&run(pickup("p-1", "hub-presidio"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_PARCEL_ASSIGNED"));
    di(&mut w).parcels.insert("p-1".into(), parcel(1.0, "sky-1"));
    expect(&run(pickup("p-1", "hub-presidio"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_PARCEL_NOT_READY"));
    di(&mut w).places.hubs.get_mut("hub-presidio").expect("hub").open = false;
    expect(&run(pickup("p-1", "hub-presidio"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_HUB_CLOSED"));
}

#[test]
fn deliver_to_a_standing_approved_point_goes_at_once_with_no_person() {
    // rules 2: dz-harbour-17 is a doorstep the customer chose (`approval = "customer"`)
    let mut r = Rig::new();
    let id = r.agent(deliver());
    assert_eq!(r.state(id), State::AutoApproved, "{:?}", r.engine.intent(id).expect("i").checks());
    assert_eq!(r.tier(id).1, "routine-delivery");
    assert!(!r.codes(id).iter().any(|c| c.starts_with("DZ_")), "{:?}", r.codes(id));
    r.run_out(id);
    // and hours later still: a standing approval has no time limit
    later(&mut r, 3 * 3_600_000);
    let again = r.agent(deliver());
    assert_eq!(r.state(again), State::AutoApproved, "{:?}", r.engine.intent(again).expect("i").checks());
    let report = r.replays();
    assert_eq!(report.rules_version, fleetmesh_decide::rules::RULES_VERSION);
}

#[test]
fn deliver_to_a_new_delivery_point_waits_and_its_approval_stands_from_then_on() {
    let mut r = Rig::new();
    new_point(&mut r.world, "dz-harbour-17");
    let id = r.agent(deliver());
    assert_eq!(r.state(id), State::PendingApproval, "{:?}", r.engine.intent(id).expect("i").checks());
    assert!(r.codes(id).contains(&"DZ_NOT_APPROVED".to_string()), "{:?}", r.codes(id));
    assert!(!r.codes(id).contains(&"DZ_UNCONFIRMED".to_string()), "{:?}", r.codes(id));
    assert_eq!(r.tier(id), (Tier::Human, "everything-else".into()), "not the routine tier: the point is not approved");
    assert_eq!(r.approve(id), State::Approved);
    r.run_out(id);
    // the approval approved the point: the next delivery there goes at once, and so does one
    // long after the hour a confirmation used to last
    later(&mut r, 10_000);
    let again = r.agent(deliver());
    assert_eq!(r.state(again), State::AutoApproved, "{:?}", r.engine.intent(again).expect("i").checks());
    assert_eq!(r.tier(again).1, "routine-delivery");
    r.run_out(again);
    later(&mut r, 2 * 3_600_000);
    let much_later = r.agent(deliver());
    assert_eq!(r.state(much_later), State::AutoApproved, "{:?}", r.engine.intent(much_later).expect("i").checks());
    r.replays();
}

#[test]
fn a_withdrawn_approval_needs_a_person_again() {
    let mut w = world();
    di(&mut w).places.drop_zones.get_mut("dz-harbour-17").expect("zone").approval =
        Some(fleetmesh_decide::delivery::StandingApproval::Withdrawn);
    let rs = run(deliver(), &w);
    expect(&rs, RuleId::Dropzone, Outcome::Hold, Some("DZ_NOT_APPROVED"));
    assert!(result(&rs, RuleId::Dropzone).detail.contains("withdrawn"), "{}", result(&rs, RuleId::Dropzone).detail);
    // a person's approval of it stands again
    di(&mut w).confirmed.insert("dz-harbour-17".into(), t0().plus_ms(-5 * 3_600_000));
    expect_pass(&run(deliver(), &w), RuleId::Dropzone);
}

#[test]
fn under_the_strict_procedure_a_deliver_to_an_unconfirmed_zone_waits_and_its_approval_is_the_confirmation() {
    // sim/policy-strict.toml: the standing approval is not read; a person confirms each zone
    let mut r = Rig::with_policy(strict());
    let id = r.agent(deliver());
    assert_eq!(r.state(id), State::PendingApproval, "{:?}", r.engine.intent(id).expect("i").checks());
    assert!(r.codes(id).contains(&"DZ_UNCONFIRMED".to_string()), "{:?}", r.codes(id));
    assert_eq!(r.tier(id), (Tier::Human, "everything-else".into()), "not the routine tier: the zone is not confirmed");
    assert_eq!(r.approve(id), State::Approved);
    r.run_out(id);
    // the approval stood as the drop zone's confirmation: the next delivery there goes at once
    later(&mut r, 10_000);
    let again = r.agent(deliver());
    assert_eq!(r.state(again), State::AutoApproved, "{:?}", r.engine.intent(again).expect("i").checks());
    assert_eq!(r.tier(again).1, "routine-delivery");
    r.run_out(again);
    // and an hour later it has lapsed: a person again
    later(&mut r, 3_600_000);
    let lapsed = r.agent(deliver());
    assert_eq!(r.state(lapsed), State::PendingApproval, "{:?}", r.engine.intent(lapsed).expect("i").checks());
    assert!(r.codes(lapsed).contains(&"DZ_UNCONFIRMED".to_string()), "{:?}", r.codes(lapsed));
    let report = r.replays();
    assert_eq!(report.rules_version, fleetmesh_decide::rules::RULES_VERSION, "the strict procedure is a setting of rules 2 on, logged with the policy");
}

#[test]
fn deliver_to_a_hub_pad_needs_no_confirmation() {
    let mut r = Rig::new();
    let id = r.agent(deliver_to("dz-crissy-pad"));
    assert_eq!(r.state(id), State::AutoApproved, "{:?}", r.engine.intent(id).expect("i").checks());
}

#[test]
fn confirm_drop_zone_is_always_a_persons_and_stands_for_the_zone_once_approved() {
    let mut r = Rig::new();
    new_point(&mut r.world, "dz-harbour-17");
    let held = r.agent(deliver());
    assert_eq!(r.state(held), State::PendingApproval);
    // an agent's confirmation waits
    let c = r.agent(confirm("dz-harbour-17"));
    assert_eq!(r.state(c), State::PendingApproval);
    assert_eq!(r.tier(c), (Tier::Human, "confirm-is-human".into()));
    assert!(r.engine.inbox().iter().any(|i| i.id() == c));
    // and nothing an agent does approves it: the engine's only ways to approved need a person
    assert_eq!(r.engine.agent_port(AgentId::new("agent-planner")).intent(c).map(Intent::state), Some(State::PendingApproval));
    // the operator's own confirmation is approved at submission (a person decided)
    let own = r.human(confirm("dz-harbour-17"));
    assert_eq!(r.state(own), State::Approved, "{:?}", r.engine.intent(own).expect("i").checks());
    r.run_out(own);
    // the held deliver, checked again, is no longer held for the zone
    let w = r.world.clone();
    let again = r.agent(deliver());
    let _ = w;
    assert_eq!(r.state(again), State::AutoApproved, "{:?}", r.engine.intent(again).expect("i").checks());
    r.replays();
}

#[test]
fn confirm_drop_zone_approves_a_point_with_no_delivery_waiting_and_needs_an_open_zone() {
    // rules 2: it approves a delivery point, which no delivery need wait for
    let mut w = world();
    new_point(&mut w, "dz-harbour-17");
    let c = confirm("dz-harbour-17");
    expect_pass(&run(c.clone(), &w), RuleId::Order);
    di(&mut w).places.drop_zones.get_mut("dz-harbour-17").expect("zone").closed = true;
    expect(&run(c, &w), RuleId::Order, Outcome::Fail, Some("DZ_CLOSED"));
}

#[test]
fn under_the_strict_procedure_and_rules_1_confirm_drop_zone_needs_a_deliver_waiting() {
    for cfg in [strict().rules, rules_v1()] {
        let mut w = world();
        let c = confirm("dz-harbour-17");
        expect(&run_with(c.clone(), &w, &cfg), RuleId::Order, Outcome::Fail, Some("ORDER_NO_DELIVER_WAITING"));
        di(&mut w).waiting_deliver = Some(IntentId(7));
        expect_pass(&run_with(c.clone(), &w, &cfg), RuleId::Order);
        di(&mut w).places.drop_zones.get_mut("dz-harbour-17").expect("zone").closed = true;
        expect(&run_with(c, &w, &cfg), RuleId::Order, Outcome::Fail, Some("DZ_CLOSED"));
    }
}

#[test]
fn a_rejected_confirmation_voids_the_delivery_that_waited() {
    let mut r = Rig::new();
    new_point(&mut r.world, "dz-harbour-17");
    let held = r.agent(deliver());
    let c = r.agent(confirm("dz-harbour-17"));
    let s = r.engine.open_operator_session(OperatorId::new("op-2")).expect("session");
    r.engine.reject(c, s.authority(), Some("a child is playing there".into())).expect("rejected");
    assert_eq!(r.state(c), State::Rejected);
    assert_eq!(r.state(held), State::Voided, "the delivery that waited on it ends too");
    r.replays();
}

#[test]
fn a_confirmation_nobody_decides_expires_and_voids_the_delivery() {
    let mut r = Rig::new();
    new_point(&mut r.world, "dz-harbour-17");
    let held = r.agent(deliver());
    let c = r.agent(confirm("dz-harbour-17"));
    r.clock.advance_ms(121_000);
    r.engine.expire_due().expect("expired");
    assert_eq!(r.state(c), State::Expired, "approval.kind.confirm_drop_zone.expire_s is 120");
    assert_eq!(r.state(held), State::Voided);
    r.replays();
}

#[test]
fn hold_and_return_to_hub_go_at_once() {
    let mut r = Rig::new();
    let h = r.agent(Task::Hold { max_s: Some(120) });
    assert_eq!(r.state(h), State::AutoApproved, "{:?}", r.engine.intent(h).expect("i").checks());
    assert_eq!(r.tier(h).1, "safe-stops");
    let hub_order = r.agent(Task::ReturnToHub { hub: Some(Place { id: Some("hub-marina".into()), at: places().hubs["hub-marina"].at() }) });
    assert_eq!(r.state(hub_order), State::AutoApproved, "{:?}", r.engine.intent(hub_order).expect("i").checks());
    let home = r.agent(Task::ReturnToHub { hub: None });
    assert_eq!(r.state(home), State::AutoApproved);
    r.run_out(h);
    r.replays();
}

#[test]
fn return_to_hub_names_an_open_hub_and_a_point_that_is_not_one_is_a_holds() {
    let w = world();
    let to = |id: Option<&str>, at: LonLat| Task::ReturnToHub { hub: Some(Place { id: id.map(str::to_string), at }) };
    expect_pass(&run(to(Some("hub-marina"), places().hubs["hub-marina"].at()), &w), RuleId::Order);
    expect(&run(to(Some("hub-nowhere"), hub()), &w), RuleId::Order, Outcome::Fail, Some("ORDER_UNKNOWN_SITE"));
    expect(&run(to(None, hub()), &w), RuleId::Order, Outcome::Hold, Some("ORDER_HUB_UNNAMED"));
}

#[test]
fn land_now_at_a_hub_goes_at_once_elsewhere_waits_and_a_persons_own_is_approved_at_submission() {
    let mut r = Rig::new();
    // at the hub (the vehicle is on it)
    let here = r.agent(Task::LandNow { site: None });
    assert_eq!(r.state(here), State::AutoApproved, "{:?}", r.engine.intent(here).expect("i").checks());
    assert_eq!(r.tier(here).1, "land-at-hub");
    // away from any hub: an agent's proposal waits, a person's own is approved when submitted
    let away = LonLat::new(-122.4590, 37.8000);
    let site = Task::LandNow { site: Some(away) };
    let waits = r.agent(site.clone());
    assert_eq!(r.state(waits), State::PendingApproval, "{:?}", r.engine.intent(waits).expect("i").checks());
    assert_eq!(r.tier(waits).1, "land-elsewhere");
    let own = r.human(site);
    assert_eq!(r.state(own), State::Approved, "a person deciding to land is not made to approve it twice");
    r.replays();
}

#[test]
fn land_now_is_allowed_with_no_position_trust_and_for_a_plane_over_people_a_hold_waits() {
    // land_now skips the navigation and energy rules: a vehicle that cannot trust its fix can land
    let mut w = world();
    w.vehicles.get_mut(&sky()).expect("v").gps_fix = Some(Fact::new(0, t0()));
    let rs = run(Task::LandNow { site: None }, &w);
    expect_pass(&rs, RuleId::NavIntegrity);
    assert!(rs.iter().all(|r| r.rule != RuleId::Energy && r.rule != RuleId::Weather && r.rule != RuleId::Payload));
}

#[test]
fn reroute_replaces_a_running_route_within_the_limits_and_waits_beyond_them() {
    let mut r = Rig::new();
    // a delivery to the pad goes at once and is dispatched
    let d = r.agent(deliver_to("dz-crissy-pad"));
    assert_eq!(r.state(d), State::AutoApproved);
    let w = r.world.clone();
    r.engine.dispatch(d, &w).expect("dispatched");
    let pad = places().drop_zones["dz-crissy-pad"].at();
    // a reroute that starts at the vehicle and adds a little distance
    let near = Task::Reroute { route: vec![hub(), pad], cause: RerouteCause::KeepOut };
    let small = r.agent(near);
    assert_eq!(r.state(small), State::AutoApproved, "{:?}", r.engine.intent(small).expect("i").checks());
    assert_eq!(r.tier(small).1, "routine-delivery");
    // a long detour: a person
    let long = r.agent(Task::Reroute { route: vec![hub(), LonLat::new(-122.50, 37.84), pad], cause: RerouteCause::Weather });
    assert_eq!(r.state(long), State::PendingApproval, "{:?}", r.engine.intent(long).expect("i").checks());
    assert_eq!(r.tier(long).1, "everything-else");
    // one that does not start near the vehicle is refused
    let far = r.agent(Task::Reroute { route: vec![LonLat::new(-122.42, 37.81), pad], cause: RerouteCause::Operator });
    assert_eq!(r.state(far), State::Rejected);
    assert!(r.codes(far).contains(&"ORDER_REROUTE_JOIN".to_string()), "{:?}", r.codes(far));
    r.replays();
}

#[test]
fn reroute_needs_a_running_order() {
    let w = world();
    let rs = run(Task::Reroute { route: vec![hub()], cause: RerouteCause::Battery }, &w);
    expect(&rs, RuleId::Order, Outcome::Fail, Some("ORDER_NO_ROUTE_TO_REPLACE"));
}

#[test]
fn abort_away_from_people_goes_at_once_and_over_people_or_at_the_last_moment_waits() {
    let mut r = Rig::new();
    let d = r.agent(deliver_to("dz-crissy-pad"));
    let w = r.world.clone();
    r.engine.dispatch(d, &w).expect("dispatched");
    let abort = |then| Task::Abort { target: d, then };
    // away from people, early, to a safe next order
    let a = r.agent(abort(AbortThen::ReturnToHub));
    assert_eq!(r.state(a), State::AutoApproved, "{:?}", r.engine.intent(a).expect("i").checks());
    assert_eq!(r.tier(a).1, "abort-away-from-people");
    // its `then` puts the vehicle down away from a hub
    r.world.vehicles.get_mut(&sky()).expect("v").position = Some(Fact::new(LonLat::new(-122.4590, 37.8000), t0()));
    let l = r.agent(abort(AbortThen::LandNow));
    assert_eq!(r.state(l), State::PendingApproval, "{:?}", r.engine.intent(l).expect("i").checks());
    assert_eq!(r.tier(l).1, "abort-otherwise");
    // over people: a populated area
    let t = t0();
    let pop = square("crowd", "populated", "", -122.4590, 37.8000, 0.002);
    let ops = OPS;
    r.world.areas = Some(areas_from(&format!("{ops},{pop}")));
    let over = r.agent(abort(AbortThen::Hold));
    assert_eq!(r.state(over), State::PendingApproval, "{:?}", r.engine.intent(over).expect("i").checks());
    // a person's own abort over people is approved when submitted
    let own = r.human(abort(AbortThen::Hold));
    assert_eq!(r.state(own), State::Approved);
    let _ = t;
    r.replays();
}

#[test]
fn abort_in_the_last_seconds_before_the_drop_zone_waits() {
    let mut r = Rig::new();
    let d = r.agent(deliver_to("dz-crissy-pad"));
    let w = r.world.clone();
    r.engine.dispatch(d, &w).expect("dispatched");
    // 40 m from the pad at 4 m/s: 10 s from release, inside the final 20 s
    let pad = places().drop_zones["dz-crissy-pad"].at();
    let v = r.world.vehicles.get_mut(&sky()).expect("v");
    v.position = Some(Fact::new(fleetmesh_decide::geo::offset(pad, 40.0, 0.0), t0()));
    v.velocity_mps = Some(Fact::new([4.0, 0.0], t0()));
    let a = r.agent(Task::Abort { target: d, then: AbortThen::ReturnToHub });
    assert_eq!(r.state(a), State::PendingApproval, "{:?}", r.engine.intent(a).expect("i").checks());
    assert_eq!(r.tier(a).1, "abort-otherwise");
}

#[test]
fn abort_after_release_is_refused_and_needs_a_dispatched_or_queued_target_on_the_vehicle() {
    let mut r = Rig::new();
    let d = r.agent(deliver_to("dz-crissy-pad"));
    // not yet dispatched
    let early = r.agent(Task::Abort { target: d, then: AbortThen::Hold });
    assert_eq!(r.state(early), State::Rejected);
    assert!(r.codes(early).contains(&"ORDER_ABORT_TARGET".to_string()), "{:?}", r.codes(early));
    let w = r.world.clone();
    r.engine.dispatch(d, &w).expect("dispatched");
    // the parcel was released
    di(&mut r.world).parcels.insert("p-1".into(), ParcelFact { state: ParcelState::Released, ..parcel(1.0, "sky-1") });
    let late = r.agent(Task::Abort { target: d, then: AbortThen::ReturnToHub });
    assert_eq!(r.state(late), State::Rejected);
    assert!(r.codes(late).contains(&"ABORT_PARCEL_RELEASED".to_string()), "{:?}", r.codes(late));
    // an unknown target
    let none = r.agent(Task::Abort { target: IntentId(99), then: AbortThen::Hold });
    assert_eq!(r.state(none), State::Rejected);
}

#[test]
fn swap_battery_at_a_hub_goes_at_once_and_a_hub_without_packs_holds() {
    let mut r = Rig::new();
    r.world.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(8.0, t0()));
    di(&mut r.world).parcels.clear();
    let ok = r.agent(Task::SwapBattery { hub: "hub-presidio".into(), pad: Some("pad-1".into()) });
    assert_eq!(r.state(ok), State::AutoApproved, "{:?}", r.engine.intent(ok).expect("i").checks());
    // swapped: a ground order, dispatched and done with no vehicle op
    r.run_out(ok);
    // no packs
    di(&mut r.world).places.hubs.get_mut("hub-presidio").expect("hub").packs = 0;
    let none = r.agent(Task::SwapBattery { hub: "hub-presidio".into(), pad: None });
    assert_eq!(r.state(none), State::PendingApproval, "{:?}", r.engine.intent(none).expect("i").checks());
    assert!(r.codes(none).contains(&"HUB_NO_PACKS".to_string()));
    r.replays();
}

#[test]
fn swap_battery_needs_a_landed_vehicle_at_the_hub_with_no_parcel_aboard() {
    let mut w = world();
    let swap = |hub: &str| Task::SwapBattery { hub: hub.into(), pad: None };
    // the parcel is still aboard
    expect(&run(swap("hub-presidio"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_PARCEL_ABOARD"));
    di(&mut w).parcels.clear();
    expect_pass(&run(swap("hub-presidio"), &w), RuleId::Order);
    expect(&run(swap("hub-marina"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_NOT_AT_HUB"));
    expect(&run(swap("hub-nowhere"), &w), RuleId::Order, Outcome::Fail, Some("ORDER_UNKNOWN_SITE"));
}

#[test]
fn a_new_delivery_order_voids_a_queued_older_one_and_supersedes_a_running_one() {
    let mut r = Rig::new();
    let first = r.agent(deliver_to("dz-crissy-pad"));
    let w = r.world.clone();
    r.engine.dispatch(first, &w).expect("dispatched");
    // a newer order for the vehicle: the older one, running, is superseded by its dispatch
    let hold = r.agent(Task::Hold { max_s: None });
    let w = r.world.clone();
    r.engine.dispatch(hold, &w).expect("dispatched");
    assert_eq!(r.state(hold), State::Dispatched);
    r.replays();
}

#[test]
fn an_approval_that_would_now_fail_voids_the_order() {
    let mut r = Rig::new();
    new_point(&mut r.world, "dz-harbour-17");
    let id = r.agent(deliver());
    assert_eq!(r.state(id), State::PendingApproval);
    // the weather worsens between the proposal and the approval: a hard limit
    let t = r.world.t;
    di(&mut r.world).weather = vec![WxFact { gust_mps: 20.0, ..calm(t) }];
    let s = r.engine.open_operator_session(OperatorId::new("op-2")).expect("session");
    let w = r.world.clone();
    let err = r.engine.approve(id, s.authority(), None, &w, &NoTerrain);
    assert!(matches!(err, Err(EngineError::ChecksFailedAtApproval(_))), "{err:?}");
    assert_eq!(r.state(id), State::Voided);
    r.replays();
}

#[test]
fn hold_outcomes_override_an_auto_tier_and_the_inbox_says_why() {
    let mut r = Rig::new();
    // a delivery to the hub pad is auto when every check is clear ...
    let clear = r.agent(deliver_to("dz-crissy-pad"));
    assert_eq!(r.state(clear), State::AutoApproved);
    // ... and a thin battery margin sends the same order to a person
    let need = need_pct(&r.world, &deliver_to("dz-crissy-pad"));
    r.world.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(need + 2.0, t0()));
    let thin = r.agent(deliver_to("dz-crissy-pad"));
    assert_eq!(r.state(thin), State::PendingApproval, "{:?}", r.engine.intent(thin).expect("i").checks());
    assert!(r.codes(thin).contains(&"ENERGY_MARGIN_THIN".to_string()));
    let p = r.engine.intent(thin).expect("i").policy().expect("decision");
    assert!(p.facts.held.iter().any(|h| h.contains("ENERGY_MARGIN_THIN")), "{:?}", p.facts.held);
    r.replays();
}

// ---- the policy file --------------------------------------------------------------------------

#[test]
fn the_shipped_policy_routes_every_delivery_kind_as_the_design_says() {
    let p = Policy::load(&root().join("sim/policy.toml")).expect("policy");
    let expire = |k: TaskKind| p.approval.kind.get(&k).and_then(|a| a.expire_s);
    assert_eq!(expire(TaskKind::LandNow), Some(30));
    assert_eq!(expire(TaskKind::Abort), Some(30));
    assert_eq!(expire(TaskKind::ConfirmDropZone), Some(120));
    assert_eq!(expire(TaskKind::Deliver), Some(300));
    let q = p.queue();
    for k in [TaskKind::Hold, TaskKind::ReturnToHub, TaskKind::LandNow, TaskKind::Abort] {
        assert!(!q.queues(k), "{} is never queued", k.as_str());
    }
    assert_eq!(q.expire_s(TaskKind::Reroute), 60);
    assert_eq!(q.expire_s(TaskKind::SwapBattery), 900);
    assert!(q.queues(TaskKind::Deliver));
    // the policy's delivery numbers are the code's defaults: one set, not two (it writes out
    // rules 2's standing approvals and rules 3's traffic figures, which the code's default leaves
    // implied)
    let mut defaults = fleetmesh_decide::delivery::DeliveryRules::default();
    defaults.dropzone.standing_approvals = Some(true);
    defaults.separation.traffic = Some(fleetmesh_decide::delivery::TrafficKnobs::default());
    assert_eq!(p.rules.delivery_rules(), defaults);
    assert!(p.rules.standing_approvals());
    // the strict comparison policy is the same but for that one key
    let s = strict();
    assert!(!s.rules.standing_approvals());
    let mut s_rules = s.rules.delivery_rules();
    s_rules.dropzone.standing_approvals = Some(true);
    assert_eq!(s_rules, defaults);
    assert_eq!(s.tiers, p.tiers);
    assert!(p.tiers.iter().any(|t| t.name == "confirm-is-human" && t.locked == Some(true) && t.tier == Tier::Human));
}

#[test]
fn the_policy_loader_refuses_a_file_that_loosens_confirm_drop_zone() {
    let text = std::fs::read_to_string(root().join("sim/policy.toml")).expect("policy");
    // the shipped one loads
    Policy::from_toml(&text).expect("the shipped policy");
    // an auto tier that names it
    let auto = text.replace(
        "[[tier]]\nname = \"confirm-is-human\"\nwhen = { kind = [\"confirm_drop_zone\"] }\ntier = \"human\"",
        "[[tier]]\nname = \"confirm-is-human\"\nwhen = { kind = [\"confirm_drop_zone\"] }\ntier = \"auto\"",
    );
    assert_ne!(auto, text);
    let e = Policy::from_toml(&auto).expect_err("an auto confirm_drop_zone").to_string();
    assert!(e.contains("confirm_drop_zone"), "{e}");
    // an auto tier that matches every kind
    let all = text.replace(
        "[[tier]]\nname = \"everything-else\"\ntier = \"human\"",
        "[[tier]]\nname = \"everything-else\"\ntier = \"auto\"",
    );
    assert_ne!(all, text);
    let e = Policy::from_toml(&all).expect_err("an auto tier with no kinds").to_string();
    assert!(e.contains("confirm_drop_zone"), "{e}");
    // another auto tier that adds it to its kinds
    let added = text.replace("when = { kind = [\"hold\", \"rtl\", \"return_to_hub\"] }", "when = { kind = [\"hold\", \"rtl\", \"return_to_hub\", \"confirm_drop_zone\"] }");
    assert_ne!(added, text);
    assert!(Policy::from_toml(&added).is_err());
    // a locked tier that is not human
    let locked = text.replace("reason = \"a person approves a new delivery point\"\nlocked = true", "reason = \"a person approves a new delivery point\"\nlocked = true\n[[tier]]\nname = \"x\"\ntier = \"auto\"\nlocked = true\nreason = \"x\"\nwhen = { kind = [\"hold\"] }");
    assert_ne!(locked, text);
    let e = Policy::from_toml(&locked).expect_err("a locked auto tier").to_string();
    assert!(e.contains("locked"), "{e}");
}

#[test]
fn even_a_logged_policy_that_routes_it_to_auto_is_held_to_a_person() {
    // the floor holds in `decide` against a policy the loader never saw (a logged copy)
    let mut p = Policy::load(&root().join("sim/policy.toml")).expect("policy");
    p.tiers.insert(
        0,
        TierRule {
            name: "bad".into(),
            when: When::default(),
            tier: Tier::Auto,
            reason: "everything".into(),
            submitter_authorizes: None,
            approvers: None,
            proposer_may_approve: None,
            step_up: None,
            locked: None,
        },
    );
    let mut s = spec(confirm("dz-harbour-17"));
    s.source = Source::Agent { id: AgentId::new("agent-planner") };
    let d = fleetmesh_decide::policy::decide(&s, &world(), &p);
    assert_eq!(d.tier, Tier::Human);
    assert_eq!(d.rule, "(human only)");
}

// ---- geofence ---------------------------------------------------------------------------------

#[test]
fn geofence_refuses_a_no_go_and_the_outside_and_allows_a_clear_route() {
    let mut w = world();
    expect_pass(&run(deliver(), &w), RuleId::Geofence);
    let m = mid();
    let nogo = square("blocked", "no_go", "", m.lon, m.lat, 0.0015);
    w.areas = Some(areas_from(&format!("{OPS},{nogo}")));
    expect(&run(deliver(), &w), RuleId::Geofence, Outcome::Fail, Some("GEOFENCE_NO_GO"));
    // outside every approved area
    w.areas = Some(areas_from(&square("small", "approved", "", hub().lon, hub().lat, 0.002)));
    expect(&run(deliver(), &w), RuleId::Geofence, Outcome::Fail, Some("GEOFENCE_OUTSIDE_APPROVED"));
    // no area set at all
    w.areas = None;
    expect(&run(deliver(), &w), RuleId::Geofence, Outcome::Fail, Some("GEOFENCE_AREAS_UNKNOWN"));
}

#[test]
fn geofence_holds_a_restricted_area_and_a_temporary_one_that_starts_soon_and_ignores_one_that_ended() {
    let mut w = world();
    let m = mid();
    let t = t0();
    let secs = |s: i64| {
        let at = chrono::DateTime::from_timestamp_millis(t.plus_ms(s * 1000).0).expect("time");
        at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    let restricted = square("school", "restricted", "", m.lon, m.lat, 0.0015);
    w.areas = Some(areas_from(&format!("{OPS},{restricted}")));
    expect(&run(deliver(), &w), RuleId::Geofence, Outcome::Hold, Some("GEOFENCE_RESTRICTED"));
    // a temporary no-go that is not in force at the ETA but starts within 600 s of it: hold
    // (in force only briefly: the way back, much later, finds it over)
    let soon = square("fair", "no_go", &format!(",\"valid_from\":\"{}\",\"valid_until\":\"{}\"", secs(300), secs(330)), m.lon, m.lat, 0.0015);
    w.areas = Some(areas_from(&format!("{OPS},{soon}")));
    expect(&run(deliver(), &w), RuleId::Geofence, Outcome::Hold, Some("GEOFENCE_TEMP_SOON"));
    // one that is in force when the route gets there: refuse
    let active = square("fair", "no_go", &format!(",\"valid_from\":\"{}\",\"valid_until\":\"{}\"", secs(-100), secs(5000)), m.lon, m.lat, 0.0015);
    w.areas = Some(areas_from(&format!("{OPS},{active}")));
    expect(&run(deliver(), &w), RuleId::Geofence, Outcome::Fail, Some("GEOFENCE_NO_GO"));
    // one that starts long after the route is done: nothing to say
    let later = square("fair", "no_go", &format!(",\"valid_from\":\"{}\",\"valid_until\":\"{}\"", secs(9000), secs(12000)), m.lon, m.lat, 0.0015);
    w.areas = Some(areas_from(&format!("{OPS},{later}")));
    expect_pass(&run(deliver(), &w), RuleId::Geofence);
    // one that ended before
    let past = square("fair", "no_go", &format!(",\"valid_from\":\"{}\",\"valid_until\":\"{}\"", secs(-5000), secs(-10)), m.lon, m.lat, 0.0015);
    w.areas = Some(areas_from(&format!("{OPS},{past}")));
    expect_pass(&run(deliver(), &w), RuleId::Geofence);
}

#[test]
fn geofence_refuses_a_ceiling_below_the_platforms_route_altitude() {
    let mut w = world();
    let low = r#"{"type":"Feature","properties":{"id":"low-ops","kind":"approved","ceiling_m":60},"geometry":{"type":"Polygon","coordinates":[[[-122.53,37.77],[-122.39,37.77],[-122.39,37.87],[-122.53,37.87],[-122.53,37.77]]]}}"#;
    w.areas = Some(areas_from(low));
    expect(&run(deliver(), &w), RuleId::Geofence, Outcome::Fail, Some("GEOFENCE_CEILING"));
}

#[test]
fn geofence_holds_a_plane_that_would_loiter_over_a_populated_area() {
    let mut w = world();
    w.vehicles.insert(sky(), vehicle("delivery_plane", t0()));
    let pop = square("crowd", "populated", "", hub().lon, hub().lat, 0.003);
    w.areas = Some(areas_from(&format!("{OPS},{pop}")));
    expect(&run(Task::hold(), &w), RuleId::Geofence, Outcome::Hold, Some("GEOFENCE_LOITER_POPULATED"));
    // a copter just stops
    w.vehicles.insert(sky(), vehicle("delivery_drone", t0()));
    expect_pass(&run(Task::hold(), &w), RuleId::Geofence);
}

// ---- energy -----------------------------------------------------------------------------------

/// What the energy rule says the order needs, in percent of a full battery.
fn need_pct(w: &WorldSnapshot, task: &Task) -> f64 {
    let mut w = w.clone();
    w.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(100.0, t0()));
    let rs = run(task.clone(), &w);
    let d = &result(&rs, RuleId::Energy).detail;
    let at = d.find("need ").unwrap_or_else(|| panic!("no need in {d}")) + 5;
    d[at..].split('%').next().expect("number").parse().expect("a number")
}

#[test]
fn energy_allows_holds_and_refuses_by_the_margin_at_its_boundaries() {
    let mut w = world();
    let need = need_pct(&w, &deliver());
    assert!(need > 25.0 && need < 90.0, "{need}");
    let set = |w: &mut WorldSnapshot, b: f64| w.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(b, t0()));
    set(&mut w, 100.0);
    expect_pass(&run(deliver(), &w), RuleId::Energy);
    // just above the hold band
    // (the need is printed to a tenth of a point, so the boundaries are tried 0.2 either side)
    set(&mut w, need + 5.2);
    expect_pass(&run(deliver(), &w), RuleId::Energy);
    // inside it
    set(&mut w, need + 4.8);
    expect(&run(deliver(), &w), RuleId::Energy, Outcome::Hold, Some("ENERGY_MARGIN_THIN"));
    set(&mut w, need + 0.2);
    expect(&run(deliver(), &w), RuleId::Energy, Outcome::Hold, Some("ENERGY_MARGIN_THIN"));
    // below zero
    set(&mut w, need - 0.2);
    expect(&run(deliver(), &w), RuleId::Energy, Outcome::Fail, Some("ENERGY_INSUFFICIENT"));
    // the detail states its numbers
    let rs = run(deliver(), &w);
    assert!(result(&rs, RuleId::Energy).detail.contains("margin") && result(&rs, RuleId::Energy).detail.contains("hold band 5"));
}

#[test]
fn energy_holds_a_stale_or_missing_battery_and_treats_a_slow_links_age_by_its_own_limit() {
    let mut w = world();
    w.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(95.0, t0().plus_ms(-11_000)));
    expect(&run(deliver(), &w), RuleId::Energy, Outcome::Hold, Some("ENERGY_BATTERY_UNKNOWN"));
    // on a slow link the limit is 60 s
    w.vehicles.get_mut(&sky()).expect("v").report_interval_ms = Some(20_000);
    w.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(95.0, t0().plus_ms(-50_000)));
    assert_ne!(result(&run(deliver(), &w), RuleId::Energy).outcome, Outcome::Hold, "50 s is inside the slow limit");
    w.vehicles.get_mut(&sky()).expect("v").battery_pct = None;
    expect(&run(deliver(), &w), RuleId::Energy, Outcome::Hold, Some("ENERGY_BATTERY_UNKNOWN"));
}

#[test]
fn wind_changes_the_cost_in_the_right_direction_and_a_gale_makes_no_headway() {
    let mut w = world();
    let route_east = deliver();
    let calm_need = need_pct(&w, &route_east);
    // the route goes east: a tailwind from the west (positive east) saves, a headwind costs
    let wx = |e: f64| WxFact { wind_e_mps: e, ..calm(t0()) };
    di(&mut w).weather = vec![wx(5.0)];
    let tail = need_pct(&w, &route_east);
    di(&mut w).weather = vec![wx(-5.0)];
    let head = need_pct(&w, &route_east);
    assert!(tail < calm_need && calm_need < head, "{tail} {calm_need} {head}");
    // a headwind at the drone's cruise speed: no ground speed to make good
    di(&mut w).weather = vec![wx(-12.0)];
    expect(&run(deliver(), &w), RuleId::Energy, Outcome::Fail, Some("ENERGY_WIND_NO_HEADWAY"));
}

#[test]
fn an_operators_reserve_only_tightens_and_a_payload_adds_to_the_cost() {
    let w = world();
    let base = need_pct(&w, &deliver());
    let mut s = spec(deliver());
    s.constraints.reserve_pct = Some(40.0);
    let mut w2 = w.clone();
    w2.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(100.0, t0()));
    let tight = rules::evaluate(&s, &w2, &cfg());
    let n40 = result(&tight, RuleId::Energy).detail.clone();
    assert!(n40.contains("reserve 40"), "{n40}");
    // a lower reserve than the policy's (25 for a copter) is not taken
    s.constraints.reserve_pct = Some(5.0);
    let loose = rules::evaluate(&s, &w2, &cfg());
    assert!(result(&loose, RuleId::Energy).detail.contains("reserve 25"), "{}", result(&loose, RuleId::Energy).detail);
    // the same trip with a 2 kg parcel costs more than with 0.5 kg
    let mut heavy = w.clone();
    di(&mut heavy).parcels.insert("p-1".into(), parcel(2.0, "sky-1"));
    let mut light = w.clone();
    di(&mut light).parcels.insert("p-1".into(), parcel(0.5, "sky-1"));
    assert!(need_pct(&heavy, &deliver()) > need_pct(&light, &deliver()));
    assert!(base > 0.0);
}

#[test]
fn energy_holds_when_no_hub_can_be_reached_from_the_route_end() {
    let mut w = world();
    for h in di(&mut w).places.hubs.values_mut() {
        h.open = false;
    }
    expect(&run(deliver(), &w), RuleId::Energy, Outcome::Hold, Some("ENERGY_NO_HUB_REACHABLE"));
}

#[test]
fn the_safety_orders_skip_the_comfort_rules() {
    let mut w = world();
    w.vehicles.get_mut(&sky()).expect("v").battery_pct = Some(Fact::new(3.0, t0()));
    di(&mut w).weather.clear();
    for task in [Task::hold(), Task::ReturnToHub { hub: None }, Task::LandNow { site: None }] {
        let rs = run(task.clone(), &w);
        for rule in [RuleId::Energy, RuleId::Payload, RuleId::Weather] {
            assert!(rs.iter().all(|r| r.rule != rule), "{:?} ran {rule:?}", task.kind());
        }
    }
}

// ---- payload ----------------------------------------------------------------------------------

#[test]
fn payload_is_checked_against_the_platform_with_a_derate_in_gusts() {
    let mut w = world();
    let set = |w: &mut WorldSnapshot, mass: Option<f64>, class: &str| {
        di(w).parcels.insert("p-1".into(), ParcelFact { mass_kg: mass, class: class.into(), ..parcel(1.0, "sky-1") });
    };
    // the drone carries 2.5 kg; the effective maximum is that in calm air
    set(&mut w, Some(1.0), "normal");
    expect_pass(&run(deliver(), &w), RuleId::Payload);
    set(&mut w, Some(2.3), "normal");
    expect(&run(deliver(), &w), RuleId::Payload, Outcome::Hold, Some("PAYLOAD_NEAR_MAX"));
    set(&mut w, Some(2.6), "normal");
    expect(&run(deliver(), &w), RuleId::Payload, Outcome::Fail, Some("PAYLOAD_OVER_MAX"));
    set(&mut w, None, "normal");
    expect(&run(deliver(), &w), RuleId::Payload, Outcome::Hold, Some("PAYLOAD_UNKNOWN"));
    set(&mut w, Some(1.0), "hazmat");
    expect(&run(deliver(), &w), RuleId::Payload, Outcome::Fail, Some("PAYLOAD_CLASS"));
    set(&mut w, Some(1.0), "excluded");
    expect(&run(deliver(), &w), RuleId::Payload, Outcome::Fail, Some("PAYLOAD_CLASS"));
    // a gust of 12 m/s: derated linearly from 7 (1.0) to 14 (0.6): 2.5 x 0.8286 = 2.07 kg
    di(&mut w).weather = vec![WxFact { gust_mps: 12.0, ..calm(t0()) }];
    set(&mut w, Some(2.2), "normal");
    expect(&run(deliver(), &w), RuleId::Payload, Outcome::Fail, Some("PAYLOAD_OVER_DERATED"));
    set(&mut w, Some(1.0), "normal");
    expect_pass(&run(deliver(), &w), RuleId::Payload);
    // a plane does not derate
    w.vehicles.insert(sky(), vehicle("delivery_plane", t0()));
    set(&mut w, Some(4.0), "normal");
    expect_pass(&run(deliver(), &w), RuleId::Payload);
    // a parcel the world does not know
    di(&mut w).parcels.clear();
    expect(&run(deliver(), &w), RuleId::Payload, Outcome::Hold, Some("PAYLOAD_UNKNOWN"));
}

// ---- weather ----------------------------------------------------------------------------------

fn wx_with(w: &mut WorldSnapshot, f: impl FnOnce(&mut WxFact)) {
    let mut s = calm(t0());
    f(&mut s);
    di(w).weather = vec![s];
}

#[test]
fn drone_weather_limits_hold_above_soft_and_refuse_above_hard_at_the_exact_values() {
    let mut w = world();
    // gust: soft 11, hard 14 (a limit is exceeded only above it)
    wx_with(&mut w, |s| s.gust_mps = 11.0);
    expect_pass(&run(deliver(), &w), RuleId::Weather);
    wx_with(&mut w, |s| s.gust_mps = 11.01);
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Hold, Some("WX_GUST_LIMIT_MARGINAL"));
    wx_with(&mut w, |s| s.gust_mps = 14.0);
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Hold, Some("WX_GUST_LIMIT_MARGINAL"));
    wx_with(&mut w, |s| s.gust_mps = 14.01);
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Fail, Some("WX_GUST_LIMIT"));
    // wind: soft 8, hard 10
    wx_with(&mut w, |s| (s.wind_e_mps, s.gust_mps) = (8.0, 8.0));
    expect_pass(&run(deliver(), &w), RuleId::Weather);
    wx_with(&mut w, |s| (s.wind_e_mps, s.gust_mps) = (9.0, 9.0));
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Hold, Some("WX_WIND_LIMIT_MARGINAL"));
    wx_with(&mut w, |s| (s.wind_e_mps, s.gust_mps) = (10.5, 10.5));
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Fail, Some("WX_WIND_LIMIT"));
    // visibility: soft 1500, hard 800 (below)
    wx_with(&mut w, |s| s.visibility_m = Some(1500.0));
    expect_pass(&run(deliver(), &w), RuleId::Weather);
    wx_with(&mut w, |s| s.visibility_m = Some(1200.0));
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Hold, Some("WX_VISIBILITY_MARGINAL"));
    wx_with(&mut w, |s| s.visibility_m = Some(799.0));
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Fail, Some("WX_VISIBILITY"));
    // precipitation: soft 2, hard 5
    wx_with(&mut w, |s| s.precip_mmh = Some(3.0));
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Hold, Some("WX_PRECIP_MARGINAL"));
    wx_with(&mut w, |s| s.precip_mmh = Some(5.5));
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Fail, Some("WX_PRECIP"));
    // several at once all show
    wx_with(&mut w, |s| (s.gust_mps, s.precip_mmh) = (12.0, Some(3.0)));
    let rs = run(deliver(), &w);
    assert!(rs.iter().any(|r| r.codes.contains(&"WX_GUST_LIMIT_MARGINAL".to_string()) && r.codes.contains(&"WX_PRECIP_MARGINAL".to_string())));
}

#[test]
fn missing_or_old_weather_holds_and_the_plane_has_wider_limits() {
    let mut w = world();
    di(&mut w).weather.clear();
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Hold, Some("WX_UNKNOWN"));
    di(&mut w).weather = vec![calm(t0().plus_ms(-121_000))];
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Hold, Some("WX_UNKNOWN"));
    // a plane flies in a 12 m/s gust (soft 13)
    w.vehicles.insert(sky(), vehicle("delivery_plane", t0()));
    wx_with(&mut w, |s| s.gust_mps = 12.0);
    expect_pass(&run(deliver(), &w), RuleId::Weather);
    wx_with(&mut w, |s| s.gust_mps = 16.5);
    expect(&run(deliver(), &w), RuleId::Weather, Outcome::Fail, Some("WX_GUST_LIMIT"));
}

#[test]
fn a_boats_current_and_tide_and_a_cars_flood_are_judged() {
    let mut w = world();
    w.vehicles.insert(sky(), vehicle("delivery_boat", t0()));
    let to_dock = deliver_to("dz-pier-dock");
    // cruise 3 m/s: soft ratio 0.5 is 1.5 m/s, hard 0.8 is 2.4 m/s
    let cur = |s: &mut WxFact, c: f64| (s.current_e_mps, s.current_n_mps) = (Some(c), Some(0.0));
    wx_with(&mut w, |s| cur(s, 1.4));
    expect_pass(&run(to_dock.clone(), &w), RuleId::Weather);
    wx_with(&mut w, |s| cur(s, 1.8));
    expect(&run(to_dock.clone(), &w), RuleId::Weather, Outcome::Hold, Some("WX_CURRENT_MARGINAL"));
    wx_with(&mut w, |s| cur(s, 2.5));
    expect(&run(to_dock.clone(), &w), RuleId::Weather, Outcome::Fail, Some("WX_CURRENT_LIMIT"));
    // tide: draft 0.4 m, clearance 0.5 m (hold below 0.9 m of water), hard 0.2 m (refuse below 0.6 m)
    wx_with(&mut w, |s| s.depth_m = Some(2.0));
    expect_pass(&run(to_dock.clone(), &w), RuleId::Weather);
    wx_with(&mut w, |s| s.depth_m = Some(0.8));
    expect(&run(to_dock.clone(), &w), RuleId::Weather, Outcome::Hold, Some("TIDE_SHALLOW"));
    wx_with(&mut w, |s| s.depth_m = Some(0.5));
    expect(&run(to_dock.clone(), &w), RuleId::Weather, Outcome::Fail, Some("TIDE_AGROUND"));
    // a dock whose deck (2.4 m) the water is within 0.3 m of
    wx_with(&mut w, |s| (s.depth_m, s.tide_m) = (Some(5.0), Some(2.2)));
    expect(&run(to_dock, &w), RuleId::Weather, Outcome::Hold, Some("TIDE_DOCK_LEVEL"));
    // a car: tide near the ground
    w.vehicles.insert(sky(), vehicle("delivery_car", t0()));
    wx_with(&mut w, |s| (s.tide_m, s.ground_m) = (Some(1.0), Some(2.0)));
    expect_pass(&run(deliver_to("dz-marina-kerb"), &w), RuleId::Weather);
    wx_with(&mut w, |s| (s.tide_m, s.ground_m) = (Some(1.8), Some(2.0)));
    expect(&run(deliver_to("dz-marina-kerb"), &w), RuleId::Weather, Outcome::Hold, Some("WX_FLOOD_RISK"));
    // wind does not limit a car
    wx_with(&mut w, |s| (s.wind_e_mps, s.gust_mps) = (20.0, 25.0));
    expect_pass(&run(deliver_to("dz-marina-kerb"), &w), RuleId::Weather);
    wx_with(&mut w, |s| s.precip_mmh = Some(25.0));
    expect(&run(deliver_to("dz-marina-kerb"), &w), RuleId::Weather, Outcome::Fail, Some("WX_PRECIP"));
}

#[test]
fn a_platform_override_moves_the_limits() {
    let mut w = world();
    let mut c = cfg();
    let mut d = fleetmesh_decide::delivery::DeliveryRules::default();
    d.weather.platform.insert(
        "delivery_drone".into(),
        fleetmesh_decide::delivery::WxLimits { gust_soft_mps: Some(13.0), ..Default::default() },
    );
    c.delivery = Some(d);
    wx_with(&mut w, |s| s.gust_mps = 12.0);
    let rs = rules::evaluate(&spec(deliver()), &w, &c);
    expect_pass(&rs, RuleId::Weather);
}

// ---- separation -------------------------------------------------------------------------------

fn other(at: LonLat, t: Time, alt: Option<f64>, domain: Domain) -> TrafficFact {
    TrafficFact {
        position: Fact::new(at, t),
        alt_m: alt,
        velocity_mps: Some([0.0, 0.0]),
        route: Vec::new(),
        speed_mps: Some(0.0),
        domain,
    }
}

#[test]
fn separation_refuses_a_conflict_holds_a_marginal_pass_and_a_hub_zone_and_ignores_distant_traffic() {
    let mut w = world();
    let m = mid();
    // a drone parked on the route at the same height: a conflict
    di(&mut w).traffic.insert(Uid::new("sky-2"), other(m, t0(), Some(80.0), Domain::Air));
    expect(&run(deliver(), &w), RuleId::Separation, Outcome::Fail, Some("SEP_ROUTE_CONFLICT"));
    // 30 m below it: vertical separation is enough
    di(&mut w).traffic.insert(Uid::new("sky-2"), other(m, t0(), Some(40.0), Domain::Air));
    expect_pass(&run(deliver(), &w), RuleId::Separation);
    // beside the route, a little outside the minimum: marginal (between 1.0 and 1.5 times)
    di(&mut w).traffic.insert(Uid::new("sky-2"), other(fleetmesh_decide::geo::offset(m, 0.0, 180.0), t0(), Some(80.0), Domain::Air));
    let rs = run(deliver(), &w);
    assert_eq!(result(&rs, RuleId::Separation).outcome, Outcome::Hold, "{}", result(&rs, RuleId::Separation).detail);
    assert!(result(&rs, RuleId::Separation).codes.contains(&"SEP_MARGINAL".to_string()));
    // far to the side: clear
    di(&mut w).traffic.insert(Uid::new("sky-2"), other(fleetmesh_decide::geo::offset(m, 0.0, 400.0), t0(), Some(80.0), Domain::Air));
    expect_pass(&run(deliver(), &w), RuleId::Separation);
    // a ground vehicle below an aircraft above 20 m: no minimum
    di(&mut w).traffic.insert(Uid::new("car-1"), other(m, t0(), None, Domain::Ground));
    expect_pass(&run(deliver(), &w), RuleId::Separation);
    // another vehicle whose position is old and near the route
    di(&mut w).traffic.clear();
    di(&mut w).traffic.insert(Uid::new("sky-3"), other(m, t0().plus_ms(-30_000), Some(80.0), Domain::Air));
    expect(&run(deliver(), &w), RuleId::Separation, Outcome::Hold, Some("SEP_TRAFFIC_UNKNOWN"));
    // but one more than 5 km from the route is none of its business
    di(&mut w).traffic.insert(Uid::new("sky-3"), other(LonLat::new(-122.30, 37.90), t0().plus_ms(-30_000), Some(80.0), Domain::Air));
    expect_pass(&run(deliver(), &w), RuleId::Separation);
}

#[test]
fn separation_holds_for_a_hub_zone_and_two_routes_head_on_conflict() {
    let mut w = world();
    // two vehicles for one pad: both near the hub
    di(&mut w).traffic.insert(Uid::new("sky-2"), other(hub(), t0(), Some(80.0), Domain::Air));
    let pad = deliver_to("dz-crissy-pad");
    expect(&run(pad, &w), RuleId::Separation, Outcome::Hold, Some("SEP_HUB_ZONE"));
    // head on: the other flies the reverse route towards us
    let mut w = world();
    let (a, b) = (hub(), places().drop_zones["dz-harbour-17"].at());
    let mut t = other(b, t0(), Some(80.0), Domain::Air);
    t.route = vec![a];
    t.speed_mps = Some(12.0);
    di(&mut w).traffic.insert(Uid::new("sky-2"), t);
    expect(&run(deliver(), &w), RuleId::Separation, Outcome::Fail, Some("SEP_ROUTE_CONFLICT"));
    // the same, a layer apart
    let mut t = other(b, t0(), Some(40.0), Domain::Air);
    t.route = vec![a];
    t.speed_mps = Some(12.0);
    di(&mut w).traffic.insert(Uid::new("sky-2"), t);
    expect_pass(&run(deliver(), &w), RuleId::Separation);
}

#[test]
fn separation_gives_the_same_answer_whatever_order_the_traffic_is_listed_in() {
    let mut w = world();
    let m = mid();
    for n in 0..60u32 {
        let at = fleetmesh_decide::geo::offset(m, f64::from(n) * 90.0 - 2500.0, f64::from(n % 7) * 40.0 + 300.0);
        di(&mut w).traffic.insert(Uid::new(format!("sky-{:03}", n + 10)), other(at, t0(), Some(80.0), Domain::Air));
    }
    let first = run(deliver(), &w);
    // the same traffic inserted in the reverse order (a BTreeMap is keyed: the order is the keys')
    let mut w2 = world();
    let keys: Vec<_> = di(&mut w).traffic.keys().cloned().collect();
    for k in keys.iter().rev() {
        let v = di(&mut w).traffic[k].clone();
        di(&mut w2).traffic.insert(k.clone(), v);
    }
    assert_eq!(first, run(deliver(), &w2));
}

// ---- drop zone --------------------------------------------------------------------------------

fn confirmed(w: &mut WorldSnapshot, zone: &str) {
    di(w).confirmed.insert(zone.into(), t0().plus_ms(-60_000));
}

#[test]
fn the_drop_zone_rule_covers_each_branch() {
    let mut w = world();
    // rules 2: a standing approval passes with no confirmation at all
    expect_pass(&run(deliver(), &w), RuleId::Dropzone);
    // a new delivery point holds until a person approves it, and then passes with no time limit
    new_point(&mut w, "dz-harbour-17");
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Hold, Some("DZ_NOT_APPROVED"));
    di(&mut w).confirmed.insert("dz-harbour-17".into(), t0().plus_ms(-3_601_000));
    expect_pass(&run(deliver(), &w), RuleId::Dropzone);
    // the strict procedure and rules 1: unconfirmed (and not a hub pad), then confirmed too long
    // ago, whatever the zone's standing approval says
    for cfg in [strict().rules, rules_v1()] {
        let mut v = world();
        expect(&run_with(deliver(), &v, &cfg), RuleId::Dropzone, Outcome::Hold, Some("DZ_UNCONFIRMED"));
        di(&mut v).confirmed.insert("dz-harbour-17".into(), t0().plus_ms(-3_601_000));
        expect(&run_with(deliver(), &v, &cfg), RuleId::Dropzone, Outcome::Hold, Some("DZ_UNCONFIRMED"));
        di(&mut v).confirmed.insert("dz-harbour-17".into(), t0().plus_ms(-3_599_000));
        let rs = run_with(deliver(), &v, &cfg);
        expect_pass(&rs, RuleId::Dropzone);
        assert_eq!(result(&rs, RuleId::Dropzone).detail, "drop zone dz-harbour-17 is open, confirmed or a hub pad, and clear", "rules 1's words");
        expect_pass(&run_with(deliver_to("dz-crissy-pad"), &v, &cfg), RuleId::Dropzone);
    }
    confirmed(&mut w, "dz-harbour-17");
    // closed
    di(&mut w).places.drop_zones.get_mut("dz-harbour-17").expect("zone").closed = true;
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Fail, Some("DZ_CLOSED"));
    di(&mut w).places.drop_zones.get_mut("dz-harbour-17").expect("zone").closed = false;
    // an unknown zone
    let unknown = Task::Deliver { parcel: "p-1".into(), drop_zone: "dz-nowhere-1".into(), at: hub(), route: vec![], release: Release::Land, window_end: None };
    expect(&run(unknown, &w), RuleId::Dropzone, Outcome::Fail, Some("DZ_CLOSED"));
    // a car cannot go to a water zone, and a boat cannot go to land
    let mut c = w.clone();
    c.vehicles.insert(sky(), vehicle("delivery_car", t0()));
    confirmed(&mut c, "dz-pier-dock");
    expect(&run(deliver_to("dz-pier-dock"), &c), RuleId::Dropzone, Outcome::Fail, Some("DZ_WRONG_SURFACE"));
    let mut b = w.clone();
    b.vehicles.insert(sky(), vehicle("delivery_boat", t0()));
    expect(&run(deliver_to("dz-harbour-17"), &b), RuleId::Dropzone, Outcome::Fail, Some("DZ_WRONG_SURFACE"));
    // slope above 10 degrees
    di(&mut w).places.drop_zones.get_mut("dz-harbour-17").expect("zone").slope_deg = Some(12.0);
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Fail, Some("DZ_SLOPE"));
    di(&mut w).places.drop_zones.get_mut("dz-harbour-17").expect("zone").slope_deg = Some(9.0);
    expect_pass(&run(deliver(), &w), RuleId::Dropzone);
}

#[test]
fn the_drop_zone_gust_limit_and_observations() {
    let mut w = world();
    confirmed(&mut w, "dz-harbour-17");
    // a copter's descent: hold above 8 m/s, refuse above 10
    wx_with(&mut w, |s| (s.at, s.gust_mps) = (places().drop_zones["dz-harbour-17"].at(), 8.5));
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Hold, Some("DZ_GUST_AT_ZONE"));
    wx_with(&mut w, |s| (s.at, s.gust_mps) = (places().drop_zones["dz-harbour-17"].at(), 10.5));
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Fail, Some("DZ_GUST_AT_ZONE"));
    wx_with(&mut w, |s| s.gust_mps = 4.0);
    // the latest look: occupied at a doorstep holds, at a pad refuses
    let occ = |n| DzObservation { t: t0(), state: ObservedState::Occupied, count: n };
    di(&mut w).observations.insert("dz-harbour-17".into(), occ(1));
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Hold, Some("DZ_PEOPLE_NEAR"));
    di(&mut w).observations.insert("dz-crissy-pad".into(), occ(1));
    expect(&run(deliver_to("dz-crissy-pad"), &w), RuleId::Dropzone, Outcome::Fail, Some("DZ_OCCUPIED"));
    // no recent look at a populated zone, or one that could not tell
    di(&mut w).observations.clear();
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Hold, Some("DZ_OBSERVATION_STALE"));
    di(&mut w).observations.insert("dz-harbour-17".into(), DzObservation { t: t0().plus_ms(-31_000), state: ObservedState::Clear, count: 0 });
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Hold, Some("DZ_OBSERVATION_STALE"));
    di(&mut w).observations.insert("dz-harbour-17".into(), DzObservation { t: t0(), state: ObservedState::Unknown, count: 0 });
    expect(&run(deliver(), &w), RuleId::Dropzone, Outcome::Hold, Some("DZ_OBSERVATION_STALE"));
    // an unpopulated zone needs no look
    di(&mut w).observations.clear();
    expect_pass(&run(deliver_to("dz-crissy-pad"), &w), RuleId::Dropzone);
}

// ---- navigation integrity ---------------------------------------------------------------------

fn nav_codes(w: &WorldSnapshot, task: Task) -> (Outcome, Vec<String>) {
    let rs = run(task, w);
    let r = result(&rs, RuleId::NavIntegrity);
    (r.outcome, r.codes.clone())
}

#[test]
fn navigation_trust_follows_the_fix_and_the_table_of_outcomes_by_kind() {
    let mut w = world();
    // trusted: every order allows
    for task in [deliver(), Task::hold(), Task::ReturnToHub { hub: None }, Task::LandNow { site: None }] {
        assert_eq!(nav_codes(&w, task).0, Outcome::Pass);
    }
    // degraded accuracy: a delivery holds, going home and holding allow
    di(&mut w).nav.insert(sky(), NavFacts { h_acc_m: Some(9.0), ..good_nav() });
    let (o, c) = nav_codes(&w, deliver());
    assert_eq!((o, c.contains(&"NAV_DEGRADED_ACCURACY".to_string())), (Outcome::Hold, true), "{c:?}");
    assert_eq!(nav_codes(&w, Task::ReturnToHub { hub: None }).0, Outcome::Pass);
    assert_eq!(nav_codes(&w, Task::hold()).0, Outcome::Pass);
    // too few satellites, or a high HDOP: degraded
    di(&mut w).nav.insert(sky(), NavFacts { sats: Some(5), ..good_nav() });
    assert!(nav_codes(&w, deliver()).1.contains(&"NAV_LOW_SATS".to_string()));
    di(&mut w).nav.insert(sky(), NavFacts { hdop: Some(2.5), ..good_nav() });
    assert!(nav_codes(&w, deliver()).1.contains(&"NAV_HIGH_HDOP".to_string()));
    // untrusted (no fix): a delivery is refused, going home holds, a copter cannot hold, it lands
    di(&mut w).nav.insert(sky(), good_nav());
    w.vehicles.get_mut(&sky()).expect("v").gps_fix = Some(Fact::new(1, t0()));
    let (o, c) = nav_codes(&w, deliver());
    assert_eq!((o, c.contains(&"NAV_NO_FIX".to_string()), c.contains(&"NAV_UNTRUSTED".to_string())), (Outcome::Fail, true, true), "{c:?}");
    assert_eq!(nav_codes(&w, Task::ReturnToHub { hub: None }).0, Outcome::Hold);
    assert_eq!(nav_codes(&w, Task::hold()).0, Outcome::Hold, "a copter without position cannot hold");
    assert_eq!(nav_codes(&w, Task::LandNow { site: None }).0, Outcome::Pass, "land_now needs no horizontal position");
    // a boat or a car just stops
    let mut c = w.clone();
    c.vehicles.insert(sky(), VehicleFacts { gps_fix: Some(Fact::new(1, t0())), ..vehicle("delivery_car", t0()) });
    assert_eq!(nav_codes(&c, Task::hold()).0, Outcome::Pass);
    // abort: allowed when it ends in a land or a hold, held otherwise
    assert_eq!(nav_codes(&w, Task::Abort { target: IntentId(2), then: AbortThen::LandNow }).0, Outcome::Pass);
    assert_eq!(nav_codes(&w, Task::Abort { target: IntentId(2), then: AbortThen::Hold }).0, Outcome::Pass);
    assert_eq!(nav_codes(&w, Task::Abort { target: IntentId(2), then: AbortThen::ReturnToHub }).0, Outcome::Hold);
    // 2D fix, a glitch, dead reckoning
    w.vehicles.get_mut(&sky()).expect("v").gps_fix = Some(Fact::new(2, t0()));
    assert!(nav_codes(&w, deliver()).1.contains(&"NAV_2D_FIX".to_string()));
    w.vehicles.get_mut(&sky()).expect("v").gps_fix = Some(Fact::new(3, t0()));
    w.vehicles.get_mut(&sky()).expect("v").gps_glitch = Some(Fact::new(true, t0()));
    assert!(nav_codes(&w, deliver()).1.contains(&"NAV_GLITCH".to_string()));
    w.vehicles.get_mut(&sky()).expect("v").gps_glitch = Some(Fact::new(false, t0()));
    w.vehicles.get_mut(&sky()).expect("v").nav_source = Some(Fact::new(NavSource::DeadReckoning, t0()));
    assert!(nav_codes(&w, deliver()).1.contains(&"NAV_DEAD_RECKONING".to_string()));
}

#[test]
fn navigation_consistency_checks_catch_a_jump_a_range_mismatch_a_wrong_surface_and_an_impossible_speed() {
    let mut w = world();
    let case = |w: &mut WorldSnapshot, n: NavFacts, code: &str| {
        di(w).nav.insert(sky(), n);
        let (o, c) = nav_codes(w, deliver());
        assert_eq!(o, Outcome::Fail, "{code}: {c:?}");
        assert!(c.contains(&code.to_string()), "{code}: {c:?}");
    };
    // a jump above 30 m plus three times the accuracy (2 m: 36 m)
    case(&mut w, NavFacts { jump_m: Some(37.0), ..good_nav() }, "NAV_JUMP");
    di(&mut w).nav.insert(sky(), NavFacts { jump_m: Some(35.0), ..good_nav() });
    assert_eq!(nav_codes(&w, deliver()).0, Outcome::Pass);
    case(&mut w, NavFacts { range_mismatch_m: Some(200.0), ..good_nav() }, "NAV_RANGE_MISMATCH");
    case(&mut w, NavFacts { wrong_surface: true, ..good_nav() }, "NAV_WRONG_SURFACE");
    // 18 m/s is the drone's top speed; 1.2 times is 21.6
    case(&mut w, NavFacts { speed_mps: Some(22.0), ..good_nav() }, "NAV_IMPOSSIBLE_SPEED");
    // accuracy beyond the degraded limit (15 m for air)
    case(&mut w, NavFacts { h_acc_m: Some(16.0), ..good_nav() }, "NAV_UNTRUSTED");
    // a slow, careful spoof under every threshold passes: the gap the design states
    di(&mut w).nav.insert(sky(), NavFacts { jump_m: Some(5.0), range_mismatch_m: Some(100.0), ..good_nav() });
    assert_eq!(nav_codes(&w, deliver()).0, Outcome::Pass);
}

#[test]
fn navigation_returns_to_trust_only_after_the_recover_time_and_a_stale_position_is_unknown() {
    let mut w = world();
    // it was untrusted and the better state has held 10 s of the 20 it takes
    di(&mut w).nav.insert(sky(), NavFacts { prev_state: Some(NavTrust::Untrusted), good_for_s: Some(10.0), ..good_nav() });
    let (o, c) = nav_codes(&w, deliver());
    assert_eq!(o, Outcome::Fail);
    assert!(c.contains(&"NAV_RECOVERING".to_string()), "{c:?}");
    di(&mut w).nav.insert(sky(), NavFacts { prev_state: Some(NavTrust::Untrusted), good_for_s: Some(25.0), ..good_nav() });
    assert_eq!(nav_codes(&w, deliver()).0, Outcome::Pass);
    // a position older than 5 s, or none: unknown
    w.vehicles.get_mut(&sky()).expect("v").position = Some(Fact::new(hub(), t0().plus_ms(-6_000)));
    let (o, c) = nav_codes(&w, deliver());
    assert_eq!(o, Outcome::Hold);
    assert!(c.contains(&"NAV_TRUST_UNKNOWN".to_string()) && c.contains(&"NAV_STALE_POSITION".to_string()), "{c:?}");
    // a hold for an unknown fix is allowed
    assert_eq!(nav_codes(&w, Task::hold()).0, Outcome::Pass);
    // no navigation facts at all: unknown, never a default
    let mut w = world();
    w.vehicles.get_mut(&sky()).expect("v").gps_fix = None;
    di(&mut w).nav.clear();
    assert_eq!(nav_codes(&w, deliver()).0, Outcome::Hold);
}

// ---- lost link --------------------------------------------------------------------------------

#[test]
fn lost_link_checks_the_failsafe_against_policy_and_the_link_along_the_route() {
    let mut w = world();
    expect_pass(&run(deliver(), &w), RuleId::LostLink);
    // a failsafe that is not what policy wants (land, or another timer): refused, a person is not asked
    let plan = |action, timer| {
        Some(Fact::new(LostLinkPlan { action, timer_s: Some(timer), source: LostLinkSource::Read, circle_m: None }, t0()))
    };
    w.vehicles.get_mut(&sky()).expect("v").lost_link = plan(LostLinkAction::Land, 10.0);
    expect(&run(deliver(), &w), RuleId::LostLink, Outcome::Fail, Some("LINK_FAILSAFE_MISMATCH"));
    w.vehicles.get_mut(&sky()).expect("v").lost_link = plan(LostLinkAction::Rtl, 60.0);
    expect(&run(deliver(), &w), RuleId::LostLink, Outcome::Fail, Some("LINK_FAILSAFE_MISMATCH"));
    // no plan read: a person judges
    w.vehicles.get_mut(&sky()).expect("v").lost_link = None;
    expect(&run(deliver(), &w), RuleId::LostLink, Outcome::Hold, Some("LINK_PLAN_UNKNOWN"));
    w.vehicles.get_mut(&sky()).expect("v").lost_link = plan(LostLinkAction::Rtl, 10.0);
    // the walk along the route
    let link = |trips, gap: Option<f64>, ret, backup| LinkAlong {
        t: t0(),
        trips,
        gap_s: gap,
        return_ok: ret,
        backup_covers: backup,
        on_backup: false,
        backup_rate_low: false,
    };
    di(&mut w).link.insert(sky(), link(false, None, true, false));
    expect_pass(&run(deliver(), &w), RuleId::LostLink);
    di(&mut w).link.insert(sky(), link(true, Some(20.0), false, false));
    expect(&run(deliver(), &w), RuleId::LostLink, Outcome::Fail, Some("LINK_FAILSAFE_UNSAFE"));
    di(&mut w).link.insert(sky(), link(true, Some(45.0), true, false));
    expect(&run(deliver(), &w), RuleId::LostLink, Outcome::Hold, Some("LINK_COVERAGE_GAP"));
    di(&mut w).link.insert(sky(), link(true, Some(45.0), true, true));
    expect_pass(&run(deliver(), &w), RuleId::LostLink);
    di(&mut w).link.insert(sky(), link(true, Some(20.0), true, false));
    expect_pass(&run(deliver(), &w), RuleId::LostLink);
    // on the backup radio: allowed with a note; a slow backup holds a delivery
    let mut l = link(false, None, true, false);
    l.on_backup = true;
    di(&mut w).link.insert(sky(), l.clone());
    let rs = run(deliver(), &w);
    assert_eq!(result(&rs, RuleId::LostLink).outcome, Outcome::Pass);
    assert!(result(&rs, RuleId::LostLink).detail.contains("LINK_ON_BACKUP"));
    l.backup_rate_low = true;
    di(&mut w).link.insert(sky(), l);
    expect(&run(deliver(), &w), RuleId::LostLink, Outcome::Hold, Some("LINK_BACKUP_SLOW"));
}

// ---- replay -----------------------------------------------------------------------------------

#[test]
fn every_kind_replays_and_an_edited_reason_code_or_tier_is_found() {
    let mut r = Rig::new();
    di(&mut r.world).parcels.insert("p-2".into(), ParcelFact { vehicle: None, state: ParcelState::Ready, ..parcel(1.0, "sky-1") });
    // pickup (hub), deliver (held, approved), hold, reroute, return_to_hub, land_now, abort,
    // swap_battery, confirm_drop_zone: all nine
    let site = Place { id: Some("hub-presidio".into()), at: hub() };
    let pickup = r.agent(Task::Pickup { parcel: "p-2".into(), site, load_by_s: Some(120) });
    r.run_out(pickup);
    new_point(&mut r.world, "dz-harbour-17");
    let held = r.agent(deliver());
    r.approve(held);
    r.run_out(held);
    // the order the reroute and the abort name: a delivery to the hub pad, running
    let running = r.agent(deliver_to("dz-crissy-pad"));
    assert_eq!(r.state(running), State::AutoApproved, "{:?}", r.engine.intent(running).expect("i").checks());
    let w = r.world.clone();
    r.engine.dispatch(running, &w).expect("dispatched");
    let pad = places().drop_zones["dz-crissy-pad"].at();
    let reroute = r.agent(Task::Reroute { route: vec![hub(), pad], cause: RerouteCause::Weather });
    let abort = r.agent(Task::Abort { target: running, then: AbortThen::Hold });
    let hold = r.agent(Task::Hold { max_s: Some(30) });
    let home = r.agent(Task::ReturnToHub { hub: None });
    let land = r.agent(Task::LandNow { site: None });
    let confirm_zone = r.human(confirm("dz-harbour-17"));
    for id in [reroute, abort, hold, home, land, confirm_zone] {
        if matches!(r.state(id), State::AutoApproved | State::Approved) {
            let w = r.world.clone();
            r.engine.dispatch(id, &w).expect("dispatched");
        }
    }
    // a person's confirmation has no vehicle op: done as it goes out, as the bridge does it
    assert_eq!(r.state(confirm_zone), State::Dispatched, "rules 2: it approves the point, no delivery need wait");
    r.engine.finish(confirm_zone, TaskOutcome::Done).expect("finished");
    di(&mut r.world).parcels.clear();
    // (a person's own: an agent's order on a vehicle with a human-approved order in play waits)
    let swap = r.human(Task::SwapBattery { hub: "hub-presidio".into(), pad: None });
    assert_eq!(r.state(swap), State::AutoApproved, "{:?}", r.engine.intent(swap).expect("i").policy());
    r.run_out(swap);
    let kinds: std::collections::BTreeSet<_> = r.engine.intents().map(|i| i.spec().kind()).collect();
    for k in TaskKind::DELIVERY {
        assert!(kinds.contains(&k), "no {} in the run", k.as_str());
    }
    let report = r.replays();
    assert!(report.checks_recomputed >= 10 && report.policy_recomputed >= 9, "{report:?}");
    // an edited reason code in a logged check is found
    let text = r.log.text();
    let lines: Vec<&str> = text.lines().collect();
    let i = lines.iter().position(|l| l.contains("\"codes\":[\"DZ_NOT_APPROVED\"")).expect("a check with a reason code");
    let mut edited: Vec<String> = lines.iter().map(ToString::to_string).collect();
    edited[i] = edited[i].replacen("DZ_NOT_APPROVED", "DZ_CLOSED", 1);
    let rep = fleetmesh_decide::replay::replay_str(&edited.join("\n")).expect("parses");
    assert!(!rep.ok(), "an edited reason code goes unnoticed");
    // an edited tier is found
    let j = lines.iter().position(|l| l.contains("\"type\":\"policy.decision\"") && l.contains("\"tier\":\"auto\"")).expect("an auto decision");
    let mut edited: Vec<String> = lines.iter().map(ToString::to_string).collect();
    edited[j] = edited[j].replacen("\"tier\":\"auto\"", "\"tier\":\"human\"", 1);
    assert!(!fleetmesh_decide::replay::replay_str(&edited.join("\n")).expect("parses").ok(), "an edited tier goes unnoticed");
    // an edited delivery input (the weather the checks read) is found
    let k = lines.iter().position(|l| l.contains("\"gust_mps\":4.0")).expect("a check that read the weather");
    let mut edited: Vec<String> = lines.iter().map(ToString::to_string).collect();
    edited[k] = edited[k].replacen("\"gust_mps\":4.0", "\"gust_mps\":40.0", 1);
    assert!(!fleetmesh_decide::replay::replay_str(&edited.join("\n")).expect("parses").ok(), "an edited input goes unnoticed");
}

#[test]
fn a_hold_outcome_logged_as_a_pass_is_found_by_replay() {
    let mut r = Rig::new();
    new_point(&mut r.world, "dz-harbour-17");
    let held = r.agent(deliver());
    assert_eq!(r.state(held), State::PendingApproval);
    let text = r.log.text();
    let edited: Vec<String> = text
        .lines()
        .map(|l| if l.contains("\"outcome\":\"hold\"") { l.replace("\"outcome\":\"hold\"", "\"outcome\":\"pass\"") } else { l.to_string() })
        .collect();
    assert_ne!(edited.join("\n"), text);
    assert!(!fleetmesh_decide::replay::replay_str(&edited.join("\n")).expect("parses").ok());
}

#[test]
fn the_traffic_inputs_a_decision_read_are_logged_by_value() {
    let mut r = Rig::new();
    di(&mut r.world).traffic.insert(Uid::new("sky-2"), other(LonLat::new(-122.30, 37.90), t0(), Some(80.0), Domain::Air));
    let id = r.agent(deliver());
    let _ = id;
    let text = r.log.text();
    assert!(text.contains("\"traffic\""), "the traffic the separation rule read is in the log");
    let report = r.replays();
    assert!(report.ok());
    let _: BTreeMap<u8, u8> = BTreeMap::new();
}

// ---- rules versions: 1 (a person confirms each zone) and 2 (standing approvals) ------------------

/// A policy as a rules 1 build logged it: rules version 1 and no `standing_approvals` key (it did
/// not exist), and places with no standing approvals (nor did they).
fn v1_rig() -> Rig {
    let mut p = Policy::load(&root().join("sim/policy.toml")).expect("the shipped policy");
    p.rules.rules_version = 1;
    if let Some(d) = p.rules.delivery.as_mut() {
        d.dropzone.standing_approvals = None;
        d.separation.traffic = None;
    }
    let mut r = Rig::with_policy(p);
    for z in di(&mut r.world).places.drop_zones.values_mut() {
        z.approval = None;
    }
    r
}

/// A short delivery run on `r`: a delivery to dz-harbour-17 (held for a person under rules 1,
/// at once under rules 2), approved where it waits, flown and done, then a second one.
fn deliveries(r: &mut Rig) -> (IntentId, IntentId) {
    let first = r.agent(deliver());
    if r.state(first) == State::PendingApproval {
        r.approve(first);
    }
    r.run_out(first);
    later(r, 10_000);
    let second = r.agent(deliver());
    if r.state(second) == State::PendingApproval {
        r.approve(second);
    }
    r.run_out(second);
    (first, second)
}

fn replay_any(text: &str) -> fleetmesh_decide::replay::ReplayReport {
    let opts = fleetmesh_decide::replay::ReplayOptions {
        rules: fleetmesh_decide::replay::RulesAccepted::AtLeast(1),
        ..Default::default()
    };
    fleetmesh_decide::replay::replay_str_with(text, &opts).expect("parses")
}

#[test]
fn a_rules_1_log_replays_with_no_mismatch_under_its_own_rules() {
    let mut r = v1_rig();
    let (first, second) = deliveries(&mut r);
    assert!(r.codes(first).contains(&"DZ_UNCONFIRMED".to_string()), "rules 1 asks a person: {:?}", r.codes(first));
    assert_eq!(r.tier(second).1, "routine-delivery", "and the confirmation stood for the hour");
    let text = r.log.text();
    assert!(text.lines().next().is_some_and(|l| l.contains("\"rules_version\":1,") && l.contains("\"policy_schema\":1")), "a rules 1 header");
    assert!(!text.contains("standing_approvals") && !text.contains("\"approval\":\"customer\"") && !text.contains("\"approval\":\"survey\""), "nothing rules 2 added");
    let report = replay_any(&text);
    assert!(report.ok(), "{:?}", report.mismatches);
    assert_eq!(report.rules_version, 1);
    assert!(report.checks_recomputed >= 2, "{report:?}");
    // the default replay accepts the current rules only: an older log needs its version named
    let strict = fleetmesh_decide::replay::replay_str(&text).expect("parses");
    assert!(strict.mismatches.iter().any(|m| m.what.contains("older decision rules")), "{:?}", strict.mismatches);
}

/// A policy as a rules 2 build logged it: rules version 2 and no traffic figures (they did not
/// exist), and a world with no traffic picture (nor did it).
fn v2_rig() -> Rig {
    let mut p = Policy::load(&root().join("sim/policy.toml")).expect("the shipped policy");
    p.rules.rules_version = 2;
    if let Some(d) = p.rules.delivery.as_mut() {
        d.separation.traffic = None;
    }
    let mut r = Rig::with_policy(p);
    di(&mut r.world).picture = None;
    r
}

#[test]
fn a_rules_2_log_replays_with_no_mismatch_and_its_deliveries_needed_no_person() {
    let mut r = v2_rig();
    let (first, second) = deliveries(&mut r);
    for id in [first, second] {
        assert_eq!(r.tier(id).1, "routine-delivery", "{:?}", r.codes(id));
    }
    let text = r.log.text();
    assert!(text.lines().next().is_some_and(|l| l.contains("\"rules_version\":2,") && l.contains("\"policy_schema\":2")), "a rules 2 header");
    assert!(!text.contains("\"picture\"") && !text.contains("ship_cpa_m"), "nothing rules 3 added");
    let report = replay_any(&text);
    assert!(report.ok(), "{:?}", report.mismatches);
    assert_eq!(report.rules_version, fleetmesh_decide::STANDING_APPROVALS_SINCE);
    assert_eq!(report.approvals, 0, "no person was asked");
}

#[test]
fn a_rules_2_log_given_a_rules_1_header_is_found() {
    let mut r = v2_rig();
    deliveries(&mut r);
    let text = r.log.text();
    let mut events = fleetmesh_decide::audit::parse_log(&text).expect("parses");
    let Some(EventBody::RunStarted { rules_version, .. }) = events.first_mut().map(|e| &mut e.body) else {
        panic!("a log starts with run.started");
    };
    *rules_version = Some(1);
    let opts = fleetmesh_decide::replay::ReplayOptions {
        rules: fleetmesh_decide::replay::RulesAccepted::AtLeast(1),
        ..Default::default()
    };
    let rep = fleetmesh_decide::replay::replay_with(&events, &opts);
    assert!(!rep.ok());
    // the markers name the fields rules 1 never had, and the checks recomputed under rules 1 differ
    assert!(rep.mismatches.iter().any(|m| m.what.contains("rules markers") && m.logged.contains("standing_approvals")), "{:?}", rep.mismatches);
    assert!(rep.mismatches.iter().any(|m| m.what.contains("rules markers") && m.logged.contains("drop_zones.approval")), "{:?}", rep.mismatches);
    assert!(rep.mismatches.iter().any(|m| m.what == "check results differ"), "{:?}", rep.mismatches);
}

#[test]
fn a_rules_1_log_given_a_rules_2_header_is_found() {
    let mut r = v1_rig();
    deliveries(&mut r);
    let mut events = fleetmesh_decide::audit::parse_log(&r.log.text()).expect("parses");
    let Some(EventBody::RunStarted { rules_version, .. }) = events.first_mut().map(|e| &mut e.body) else {
        panic!("a log starts with run.started");
    };
    *rules_version = Some(2);
    let rep = fleetmesh_decide::replay::replay(&events);
    // rules 2 reads the zone as a new delivery point, where rules 1 asked for a confirmation
    assert!(rep.mismatches.iter().any(|m| m.what == "check results differ" && m.logged.contains("DZ_UNCONFIRMED") && m.recomputed.contains("DZ_NOT_APPROVED")), "{:?}", rep.mismatches);
}

// ---- rules 3: the traffic the fleet does not control ----------------------------------------------

use fleetmesh_decide::delivery::{PathPoint, PictureState, ReceiverFact, TrackFact, TrafficPicture};

/// A picture at `t` with the network feeds heard, and these tracks.
fn picture(t: Time, tracks: Vec<TrackFact>) -> TrafficPicture {
    let rx = |id: &str, kind: &str| ReceiverFact { id: id.into(), kind: kind.into(), network: true, at: None, range_m: None, up: true };
    TrafficPicture {
        t,
        state: PictureState::Ok,
        reasons: Vec::new(),
        qnh_hpa: Some(1014.5),
        config_sha256: "c0ffee".into(),
        receivers: vec![rx("net-adsb", "adsb_1090"), rx("net-ais", "ais")],
        tracks,
    }
}

/// A track at `e`, `n` metres from the Crissy Field hub, moving at `vel`, with a straight
/// prediction every 10 s to 60 s (300 s for a vessel) and its sigma growing.
fn track(id: &str, class: &str, e: f64, n: f64, alt: Option<f64>, vel: [f64; 2]) -> TrackFact {
    let at = fleetmesh_decide::geo::offset(hub(), e, n);
    let surface = matches!(class, "cargo" | "tanker" | "sailing");
    let (step, count) = if surface { (30.0, 10) } else { (10.0, 6) };
    TrackFact {
        id: id.into(),
        domain: if surface { Domain::Surface } else { Domain::Air },
        class: class.into(),
        cooperative: true,
        label: format!("{id}-label"),
        at,
        alt_m: alt,
        alt_ref: if alt.is_some() { "baro".into() } else { "none".into() },
        h_sigma_m: 20.0,
        v_sigma_m: alt.map(|_| 15.0),
        vel_mps: Some(vel),
        vs_mps: alt.map(|_| 0.0),
        flags: Vec::new(),
        line: None,
        path: (1..=count)
            .map(|i| {
                let dt = f64::from(i) * step;
                PathPoint {
                    dt_s: dt,
                    at: fleetmesh_decide::geo::offset(at, vel[0] * dt, vel[1] * dt),
                    alt_m: alt,
                    h_sigma_m: 20.0 + 2.0 * dt,
                    v_sigma_m: alt.map(|_| 15.0 + dt),
                }
            })
            .collect(),
    }
}

/// A helicopter 400 m east and 800 m north of the hub, coming south at 40 m/s at 150 m: across
/// the drone's route east to dz-harbour-17 within half a minute.
fn low_helicopter() -> TrackFact {
    track("trk-heli", "helicopter", 400.0, 800.0, Some(150.0), [0.0, -40.0])
}

#[test]
fn a_drone_waits_for_a_manned_aircraft_that_will_cross_its_route_low() {
    let mut r = Rig::new();
    let t = r.world.t;
    di(&mut r.world).picture = Some(picture(t, vec![low_helicopter()]));
    let held = r.agent(deliver());
    assert_eq!(r.state(held), State::PendingApproval, "an agent's order the rule holds waits");
    assert!(r.codes(held).contains(&"SEP_TRAFFIC_CONFLICT".to_string()), "{:?}", r.codes(held));
    let sep = r.engine.intent(held).expect("i").checks().iter().find(|c| c.rule == RuleId::Separation).cloned().expect("separation");
    assert_eq!(sep.outcome, Outcome::Hold);
    assert!(sep.detail.contains("helicopter trk-heli-label (trk-heli)") && sep.detail.contains("well clear") && sep.detail.contains("right of way"), "{}", sep.detail);
    // the same helicopter at 1,500 ft passes over: clear, and the order goes at once
    let mut high = low_helicopter();
    high.alt_m = Some(457.0);
    high.path.iter_mut().for_each(|p| p.alt_m = Some(457.0));
    di(&mut r.world).picture = Some(picture(t, vec![high]));
    let clear = r.agent(deliver());
    assert_eq!(r.state(clear), State::AutoApproved, "{:?}", r.engine.intent(clear).expect("i").checks());
    let sep = r.engine.intent(clear).expect("i").checks().iter().find(|c| c.rule == RuleId::Separation).cloned().expect("separation");
    assert!(sep.detail.contains("1 track(s) near the route checked"), "{}", sep.detail);
    r.replays();
}

#[test]
fn only_the_tracks_near_the_order_are_logged_and_replay_decides_on_them() {
    let mut r = Rig::new();
    let t = r.world.t;
    let far = track("trk-far", "airliner", 25_000.0, -20_000.0, Some(3000.0), [-100.0, 0.0]);
    let ship = track("trk-ship", "cargo", 1000.0, 600.0, None, [5.0, 0.0]);
    di(&mut r.world).picture = Some(picture(t, vec![low_helicopter(), far, ship]));
    let id = r.agent(deliver());
    assert!(r.codes(id).contains(&"SEP_TRAFFIC_CONFLICT".to_string()));
    let text = r.log.text();
    let line = text.lines().find(|l| l.contains("\"type\":\"check.result\"") && l.contains("trk-heli")).expect("the decision logged the helicopter");
    assert!(!line.contains("trk-far"), "an airliner 30 km off is not part of the decision");
    assert!(!line.contains("trk-ship"), "a ship is not a drone's traffic");
    assert!(!line.contains("net-ais"), "nor is the AIS feed");
    assert!(line.contains("\"config_sha256\":\"c0ffee\""), "the tracker's configuration is named");
    r.replays();
    // an edited track in the log is found
    let edited: Vec<String> = text.lines().map(|l| l.replace("\"alt_m\":150.0", "\"alt_m\":450.0")).collect();
    assert_ne!(edited.join("\n"), text);
    assert!(!fleetmesh_decide::replay::replay_str(&edited.join("\n")).expect("parses").ok(), "an edited picture goes unnoticed");
}

#[test]
fn a_stale_picture_or_one_that_no_longer_covers_the_route_holds() {
    let mut r = Rig::new();
    let t = r.world.t;
    di(&mut r.world).picture = Some(picture(t.plus_ms(-30_000), Vec::new()));
    let stale = r.agent(deliver());
    assert!(r.codes(stale).contains(&"SEP_TRAFFIC_UNKNOWN".to_string()), "{:?}", r.codes(stale));
    let mut blind = picture(t, Vec::new());
    for rx in &mut blind.receivers {
        rx.up = false;
    }
    blind.state = PictureState::Degraded;
    di(&mut r.world).picture = Some(blind);
    let held = r.agent(deliver());
    assert!(r.codes(held).contains(&"SEP_PICTURE_DEGRADED".to_string()), "{:?}", r.codes(held));
    let sep = r.engine.intent(held).expect("i").checks().iter().find(|c| c.rule == RuleId::Separation).cloned().expect("separation");
    assert!(sep.detail.contains("net-adsb"), "it names the feed not heard: {}", sep.detail);
    r.replays();
}

#[test]
fn with_no_picture_the_rule_says_other_traffic_is_not_checked_and_a_car_has_no_traffic_model() {
    let mut r = Rig::new();
    let id = r.agent(deliver());
    assert_eq!(r.state(id), State::AutoApproved);
    let sep = r.engine.intent(id).expect("i").checks().iter().find(|c| c.rule == RuleId::Separation).cloned().expect("separation");
    assert!(sep.detail.contains("no picture of it in this run"), "{}", sep.detail);
    // a car's delivery: no traffic model, whatever the picture
    let mut w = r.world.clone();
    w.vehicles.insert(Uid::new("car-1"), vehicle("delivery_car", w.t));
    let t = w.t;
    di(&mut w).picture = Some(picture(t, vec![low_helicopter()]));
    let results = fleetmesh_decide::delivery_rules::separation(&spec_for(deliver_to("dz-marina-kerb"), "car-1"), &w, &r.engine.policy().rules);
    assert!(results.iter().all(|c| c.outcome == Outcome::Pass && c.detail.contains("no traffic model for a car")), "{results:?}");
}

fn spec_for(task: Task, uid: &str) -> IntentSpec {
    let mut s = spec(task);
    s.uids = vec![Uid::new(uid)];
    s
}

/// A policy as a rules 3 build logged it: rules version 3 (the traffic figures and the picture,
/// no home bases).
fn v3_rig() -> Rig {
    let mut p = Policy::load(&root().join("sim/policy.toml")).expect("the shipped policy");
    p.rules.rules_version = fleetmesh_decide::TRAFFIC_SEPARATION_SINCE;
    Rig::with_policy(p)
}

#[test]
fn a_rules_3_log_replays_with_no_mismatch() {
    let mut r = v3_rig();
    let t = r.world.t;
    di(&mut r.world).picture = Some(picture(t, Vec::new()));
    let (first, second) = deliveries(&mut r);
    for id in [first, second] {
        assert_eq!(r.tier(id).1, "routine-delivery", "{:?}", r.codes(id));
    }
    let text = r.log.text();
    assert!(text.lines().next().is_some_and(|l| l.contains("\"rules_version\":3,") && l.contains("\"policy_schema\":3")), "a rules 3 header");
    assert!(text.contains("\"picture\"") && text.contains("ship_cpa_m"), "the picture and the traffic figures are logged");
    let report = replay_any(&text);
    assert!(report.ok(), "{:?}", report.mismatches);
    assert_eq!(report.rules_version, fleetmesh_decide::TRAFFIC_SEPARATION_SINCE);
}

#[test]
fn a_rules_3_log_given_a_rules_2_header_is_found() {
    let mut r = Rig::new();
    let t = r.world.t;
    di(&mut r.world).picture = Some(picture(t, vec![low_helicopter()]));
    r.agent(deliver());
    let mut events = fleetmesh_decide::audit::parse_log(&r.log.text()).expect("parses");
    let Some(EventBody::RunStarted { rules_version, .. }) = events.first_mut().map(|e| &mut e.body) else {
        panic!("a log starts with run.started");
    };
    *rules_version = Some(2);
    let opts = fleetmesh_decide::replay::ReplayOptions { rules: fleetmesh_decide::replay::RulesAccepted::AtLeast(1), ..Default::default() };
    let rep = fleetmesh_decide::replay::replay_with(&events, &opts);
    assert!(rep.mismatches.iter().any(|m| m.what.contains("rules markers") && m.logged.contains("separation.traffic")), "{:?}", rep.mismatches);
    assert!(rep.mismatches.iter().any(|m| m.what.contains("rules markers") && m.logged.contains("delivery.picture")), "{:?}", rep.mismatches);
    // rules 2 never read the picture: the held order recomputes as clear
    assert!(rep.mismatches.iter().any(|m| m.what == "check results differ"), "{:?}", rep.mismatches);
}

#[test]
fn a_rules_2_log_given_a_rules_3_header_is_found() {
    let mut r = v2_rig();
    deliveries(&mut r);
    let mut events = fleetmesh_decide::audit::parse_log(&r.log.text()).expect("parses");
    let Some(EventBody::RunStarted { rules_version, .. }) = events.first_mut().map(|e| &mut e.body) else {
        panic!("a log starts with run.started");
    };
    *rules_version = Some(3);
    let rep = fleetmesh_decide::replay::replay(&events);
    // rules 3's separation says what it checked of other traffic; rules 2's said nothing of it
    assert!(rep.mismatches.iter().any(|m| m.what == "check results differ"), "{:?}", rep.mismatches);
}

// ---- rules 4: home bases in the assigner (docs/design/fleet-organisation.md) -----------------

/// The assigner's input for one parcel at the Presidio hub, bound for the Marina kerb, and two
/// drones waiting there for it: "drone-1" based at the Pier hub (a visitor) and "drone-2" based at
/// the Presidio. `based`: whether the candidates name their home bases (rules 4 logs them).
fn assign_input(t: Time, based: bool) -> fleetmesh_decide::dispatch::AssignInput {
    use fleetmesh_decide::dispatch::{AssignInput, Candidate, DispatchParams, OpenParcel};
    let drone = catalog().get("delivery_drone").cloned().expect("platform");
    let cand = |uid: &str, home: &str| Candidate {
        uid: Uid::new(uid),
        hub: "hub-presidio".into(),
        platform: drone.clone(),
        kind: AutopilotKind::Copter,
        battery_pct: 95.0,
        home: based.then(|| home.to_string()),
    };
    AssignInput {
        t,
        params: DispatchParams::default(),
        energy: Default::default(),
        places: places(),
        // still air: the costs are the distances
        weather: vec![],
        parcels: vec![OpenParcel {
            id: "p-9".into(),
            hub: "hub-presidio".into(),
            zone: "dz-marina-kerb".into(),
            mass_kg: 1.0,
            class: "normal".into(),
            priority: 1,
            due: t.plus_ms(1_800_000),
        }],
        vehicles: vec![cand("drone-1", "hub-pier"), cand("drone-2", "hub-presidio")],
    }
}

#[test]
fn a_rules_4_dispatch_plan_replays_and_sends_the_parcel_with_the_drone_based_at_its_hub() {
    let mut r = Rig::new();
    let input = assign_input(r.world.t, true);
    let plan = fleetmesh_decide::dispatch::assign(&input);
    assert_eq!(plan.assignments[0].vehicle.as_str(), "drone-2", "the drone based at the Presidio: {plan:?}");
    r.engine.record_dispatch_plan(1, input, plan).expect("logged");
    let text = r.log.text();
    assert!(text.contains("\"home\":\"hub-pier\""), "each candidate's home base is logged");
    let report = r.replays();
    assert_eq!(report.rules_version, fleetmesh_decide::HOME_BASE_SINCE);
}

#[test]
fn a_rules_3_dispatch_plan_replays_under_its_own_rules() {
    let mut r = v3_rig();
    let input = assign_input(r.world.t, false);
    let rules3 = r.engine.policy().rules.clone();
    let plan = fleetmesh_decide::dispatch::assign_under(&input, &rules3);
    // rules 3 costs both back to the hub nearest the kerb: a tie, the lower uid takes it
    assert_eq!(plan.assignments[0].vehicle.as_str(), "drone-1", "{plan:?}");
    r.engine.record_dispatch_plan(1, input, plan).expect("logged");
    let text = r.log.text();
    assert!(!text.contains("\"home\":\"hub-"), "nothing rules 4 added");
    let report = replay_any(&text);
    assert!(report.ok(), "{:?}", report.mismatches);
    assert_eq!(report.rules_version, fleetmesh_decide::TRAFFIC_SEPARATION_SINCE);
}

#[test]
fn a_rules_4_dispatch_plan_given_a_rules_3_header_is_found() {
    let mut r = Rig::new();
    let input = assign_input(r.world.t, true);
    let plan = fleetmesh_decide::dispatch::assign(&input);
    r.engine.record_dispatch_plan(1, input, plan).expect("logged");
    let mut events = fleetmesh_decide::audit::parse_log(&r.log.text()).expect("parses");
    let Some(EventBody::RunStarted { rules_version, .. }) = events.first_mut().map(|e| &mut e.body) else {
        panic!("a log starts with run.started");
    };
    *rules_version = Some(fleetmesh_decide::TRAFFIC_SEPARATION_SINCE);
    let opts = fleetmesh_decide::replay::ReplayOptions { rules: fleetmesh_decide::replay::RulesAccepted::AtLeast(1), ..Default::default() };
    let rep = fleetmesh_decide::replay::replay_with(&events, &opts);
    assert!(rep.mismatches.iter().any(|m| m.what.contains("rules markers") && m.logged.contains("vehicles.home")), "{:?}", rep.mismatches);
    // rules 3 costs the way back to the nearest hub: the visitor wins, not the plan logged
    assert!(rep.mismatches.iter().any(|m| m.what.contains("dispatch plan differs")), "{:?}", rep.mismatches);
}
