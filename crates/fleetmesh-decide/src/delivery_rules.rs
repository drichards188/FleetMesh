//! The delivery rules (docs/design/delivery-rules.md): pure functions of an intent, the world
//! snapshot (with its [`DeliveryInputs`]) and the rules config. Each returns one
//! [`CheckResult`] per vehicle with an outcome (`Pass`, `Hold`, `Fail`) and the reason codes
//! behind a hold or a refusal. All rules run and none stops at the first failure, so the inbox
//! card and the proposer see every reason.
//!
//! | rule | applies to |
//! | --- | --- |
//! | `order` | every delivery order: the parcel, the place, the order it names |
//! | `geofence` | pickup, deliver, reroute, return_to_hub, land_now (site), hold (a plane) |
//! | `domain` | pickup, deliver, reroute, return_to_hub (terrain: land or water) |
//! | `energy` | pickup, deliver, reroute |
//! | `payload` | pickup, deliver |
//! | `weather` | pickup, deliver, reroute |
//! | `separation` | pickup, deliver, reroute, return_to_hub |
//! | `dropzone` | deliver, confirm_drop_zone |
//! | `nav_integrity` | all vehicle orders, outcomes by kind (section 9) |
//! | `lost_link` | pickup, deliver, reroute |
//!
//! `hold`, `return_to_hub`, `land_now` and `abort` skip the energy, payload and weather rules:
//! a vehicle that cannot take a delivery can still be told to stop.
//!
//! Gaps, named where the input is missing (each is read from [`DeliveryInputs`], never
//! defaulted): the bridge fills weather, navigation fix, traffic and link predictions as far as
//! the stand-ins and the link emulator give them today; a route's altitude is the platform's
//! cruise altitude; a `reroute` is not checked for entering an area the old route did not cross.

use std::collections::BTreeSet;

use crate::areas::{Area, AreaKind};
use crate::catalog::{AutopilotKind, Domain, LandNowBehavior, PlatformFacts};
use crate::delivery::{
    DeliveryInputs, DeliveryRules, DropZoneKind, NavFacts, NavTrust, ObservedState, ParcelState, StandingApproval,
    Surface2, WxFact, WxLimits,
};
use crate::geo::{LonLat, boundary_distance, enu, haversine_m};
use crate::ids::Uid;
use crate::intent::{AbortThen, IntentSpec, Task, TaskKind};
use crate::route::{LEG_MAX_M, along, route_for};
use crate::rules::{CheckResult, Outcome, RuleId, RulesConfig, per_vehicle};
use crate::world::{NavSource, VehicleFacts, WorldSnapshot};

/// Whether the delivery rule set judges this intent: a delivery order, or a hold in a world that
/// gives delivery inputs (a hold elsewhere keeps the inherited rules).
pub fn applies(spec: &IntentSpec, world: &WorldSnapshot) -> bool {
    let k = spec.kind();
    matches!(
        k,
        TaskKind::Pickup
            | TaskKind::Deliver
            | TaskKind::Reroute
            | TaskKind::ReturnToHub
            | TaskKind::LandNow
            | TaskKind::Abort
            | TaskKind::SwapBattery
            | TaskKind::ConfirmDropZone
    ) || (k == TaskKind::Hold && world.delivery.is_some())
}

/// All the delivery rules, in order.
pub fn evaluate(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let kind = spec.kind();
    let mut out = Vec::new();
    out.extend(crate::rules::capability(spec, world, cfg));
    out.extend(crate::rules::readiness(spec, world, cfg));
    out.extend(order(spec, world, cfg));
    if matches!(kind, TaskKind::Pickup | TaskKind::Deliver | TaskKind::Reroute | TaskKind::ReturnToHub) {
        out.extend(crate::rules::domain(spec, world, cfg));
    }
    if matches!(
        kind,
        TaskKind::Pickup | TaskKind::Deliver | TaskKind::Reroute | TaskKind::ReturnToHub | TaskKind::LandNow | TaskKind::Hold
    ) {
        out.extend(geofence(spec, world, cfg));
    }
    if matches!(kind, TaskKind::Pickup | TaskKind::Deliver | TaskKind::Reroute) {
        out.extend(energy(spec, world, cfg));
        out.extend(weather(spec, world, cfg));
    }
    if matches!(kind, TaskKind::Pickup | TaskKind::Deliver) {
        out.extend(payload(spec, world, cfg));
    }
    if matches!(kind, TaskKind::Pickup | TaskKind::Deliver | TaskKind::Reroute | TaskKind::ReturnToHub) {
        out.extend(separation(spec, world, cfg));
    }
    if matches!(kind, TaskKind::Deliver | TaskKind::ConfirmDropZone) {
        out.extend(dropzone(spec, world, cfg));
    }
    if !kind.is_ground_order() {
        out.extend(nav_integrity(spec, world, cfg));
    }
    if matches!(kind, TaskKind::Pickup | TaskKind::Deliver | TaskKind::Reroute) {
        out.extend(lost_link(spec, world, cfg));
    }
    out
}

// ---- helpers ----------------------------------------------------------------------------------

/// A rule's verdict on one vehicle, built up from every reason it finds: the worst outcome wins,
/// and every code and sentence is kept.
struct Verdict {
    rule: RuleId,
    uid: Uid,
    outcome: Outcome,
    codes: Vec<String>,
    why: Vec<String>,
    notes: Vec<String>,
}

fn rank(o: Outcome) -> u8 {
    match o {
        Outcome::Pass => 0,
        Outcome::NotEvaluated | Outcome::Unverified | Outcome::Hold => 1,
        Outcome::Fail => 2,
    }
}

impl Verdict {
    fn new(rule: RuleId, uid: &Uid) -> Verdict {
        Verdict { rule, uid: uid.clone(), outcome: Outcome::Pass, codes: Vec::new(), why: Vec::new(), notes: Vec::new() }
    }

    fn add(&mut self, outcome: Outcome, code: &str, text: String) {
        if rank(outcome) > rank(self.outcome) {
            self.outcome = outcome;
        }
        if !self.codes.iter().any(|c| c == code) {
            self.codes.push(code.to_string());
            self.why.push(text);
        }
    }

    fn hold(&mut self, code: &str, text: impl Into<String>) {
        self.add(Outcome::Hold, code, text.into());
    }

    fn refuse(&mut self, code: &str, text: impl Into<String>) {
        self.add(Outcome::Fail, code, text.into());
    }

    fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }

    fn has(&self, code: &str) -> bool {
        self.codes.iter().any(|c| c == code)
    }

    /// The result: the reasons if there are any, else `pass` (with the notes).
    fn done(self, pass: impl Into<String>) -> CheckResult {
        let mut detail = if self.why.is_empty() { pass.into() } else { self.why.join("; ") };
        for n in &self.notes {
            if !detail.is_empty() {
                detail.push_str("; ");
            }
            detail.push_str(n);
        }
        CheckResult { codes: self.codes, ..CheckResult::new(self.rule, Some(&self.uid), self.outcome, detail) }
    }
}

fn inputs(world: &WorldSnapshot) -> Option<&DeliveryInputs> {
    world.delivery.as_ref()
}

fn unknown_inputs(rule: RuleId, uid: &Uid, what: &str) -> CheckResult {
    CheckResult::new(rule, Some(uid), Outcome::Hold, format!("no {what} given: it cannot be verified, a person judges it"))
        .with_codes(&["INPUT_UNKNOWN"])
}

fn kind_of(v: &VehicleFacts) -> Option<AutopilotKind> {
    crate::world::platform_autopilot(v)
}

fn secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

/// Ground speed along a track `(de, dn)` (a unit vector) for a vehicle that moves at `c` through a
/// medium (air or water) that itself moves at `flow` (east, north): the speed made good when it
/// aims to hold the track. None when the flow across the track is at least `c`.
pub fn ground_speed(c: f64, flow: (f64, f64), track: (f64, f64)) -> Option<f64> {
    let along_flow = flow.0 * track.0 + flow.1 * track.1;
    let across = flow.0 * -track.1 + flow.1 * track.0;
    if across.abs() >= c {
        return None;
    }
    let gs = along_flow + (c * c - across * across).sqrt();
    (gs > 0.0).then_some(gs)
}

/// The weather samples fresh at `world.t`.
fn fresh_wx<'a>(world: &'a WorldSnapshot, d: &DeliveryRules) -> Vec<&'a WxFact> {
    let Some(di) = inputs(world) else { return Vec::new() };
    di.weather
        .iter()
        .filter(|w| world.t.ms_since(w.t) as f64 / 1000.0 <= d.weather.max_age_s && w.t <= world.t.plus_ms(1000))
        .collect()
}

/// The nearest fresh sample to `p` (ties: the earlier in the list).
fn nearest_wx<'a>(wx: &[&'a WxFact], p: LonLat) -> Option<&'a WxFact> {
    let mut best: Option<(&WxFact, f64)> = None;
    for w in wx {
        let d = haversine_m(p, w.at);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((w, d));
        }
    }
    best.map(|(w, _)| w)
}

/// The points of the order's outbound route in travel order: the vehicle's position, the planned
/// waypoints, the task path.
fn outbound(route: &crate::route::Route) -> Vec<LonLat> {
    let mut pts: Vec<LonLat> = Vec::new();
    if let Some(t) = &route.transit {
        pts.push(t.from);
        pts.extend(route.transit_via.iter().copied());
    }
    if let Some(p) = route.task_routed() {
        pts.extend(p.points);
    } else if let Some(t) = &route.transit {
        pts.push(t.to);
    }
    pts.dedup_by(|a, b| a.e7() == b.e7());
    pts
}

fn polyline_m(pts: &[LonLat]) -> f64 {
    pts.windows(2).map(|w| haversine_m(w[0], w[1])).sum()
}

/// One leg of a trip with the time its far end is reached.
#[derive(Clone, Debug)]
pub struct TripLeg {
    pub gs_mps: Option<f64>,
    /// seconds from now at the leg's start and end
    pub t0: f64,
    pub t1: f64,
}

/// The legs of a trip along `pts` by `platform` through the flow `wx` gives (the energy rule's
/// own model, which dispatch costs a candidate with).
pub fn trip_legs(pts: &[LonLat], platform: &PlatformFacts, wx: &[&WxFact], min_frac: f64) -> Vec<TripLeg> {
    trip(pts, platform, wx, min_frac)
}

/// The legs of `pts` flown by `platform` through the flow the weather gives at each leg's middle
/// (wind for air, the water's current for a boat, none for a car).
fn trip(pts: &[LonLat], platform: &PlatformFacts, wx: &[&WxFact], min_frac: f64) -> Vec<TripLeg> {
    let c = platform.cruise_mps.unwrap_or(0.0);
    let mut t = 0.0;
    let mut out = Vec::new();
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = haversine_m(a, b);
        let mid = LonLat::new((a.lon + b.lon) / 2.0, (a.lat + b.lat) / 2.0);
        let flow = nearest_wx(wx, mid).map_or((0.0, 0.0), |s| match platform.domain {
            Domain::Air => (s.wind_e_mps, s.wind_n_mps),
            Domain::Surface => (s.current_e_mps.unwrap_or(0.0), s.current_n_mps.unwrap_or(0.0)),
            Domain::Ground => (0.0, 0.0),
        });
        let (e, n) = enu(a, b);
        let norm = e.hypot(n);
        let track = if norm > 0.0 { (e / norm, n / norm) } else { (0.0, 1.0) };
        let gs = ground_speed(c, flow, track).filter(|g| *g >= min_frac * c);
        let dt = match gs {
            Some(g) if len > 0.0 => len / g,
            _ => 0.0,
        };
        out.push(TripLeg { gs_mps: if len == 0.0 { Some(c) } else { gs }, t0: t, t1: t + dt });
        t += dt;
    }
    out
}

fn platform_of(v: &VehicleFacts) -> Option<&PlatformFacts> {
    v.platform.as_ref()
}

/// The platform's payload multiplier on its energy draw: `1 + payload_load * mass / max`.
fn payload_mult(d: &DeliveryRules, kind: AutopilotKind, platform: &PlatformFacts, mass_kg: f64) -> f64 {
    match platform.payload_kg {
        Some(max) if max > 0.0 => 1.0 + d.energy.payload_load.get(kind) * (mass_kg / max).clamp(0.0, 1.0),
        _ => 1.0,
    }
}

fn parcel_of(spec: &IntentSpec) -> Option<&str> {
    match &spec.task {
        Task::Pickup { parcel, .. } | Task::Deliver { parcel, .. } | Task::ConfirmDropZone { parcel, .. } => Some(parcel),
        _ => None,
    }
}

// ---- order: what the order itself needs -------------------------------------------------------

/// Preconditions of each kind (delivery-orders.md section 5), before the rules read the world.
pub fn order(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::Order;
    let kind = spec.kind();
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let Some(di) = inputs(world) else {
            // an order that names a parcel, a hub or a zone cannot be checked without the world's
            // places; a hold or a land_now names none
            return if matches!(kind, TaskKind::Hold | TaskKind::LandNow | TaskKind::ReturnToHub) && !matches!(&spec.task, Task::ReturnToHub { hub: Some(_) })
            {
                CheckResult::new(r, Some(uid), Outcome::Pass, format!("no precondition for {}", kind.as_str()))
            } else {
                unknown_inputs(r, uid, "places, parcels or orders (delivery inputs)").with_codes(&["ORDER_WORLD_UNKNOWN"])
            };
        };
        let pos = v.position.as_ref().map(|p| p.value);
        // rules 5: a pickup or a return may name a point within the hub (a vehicle's stand, or the
        // pad automation gave it); before, the hub's own point
        let stands = cfg.at_least(crate::rules::HUB_SEQUENCING_SINCE);
        let agrees = |id: &Option<String>, at: LonLat, x: &mut Verdict, what: &str| {
            let Some(i) = id else { return };
            match di.places.hubs.get(i) {
                None => x.refuse("ORDER_UNKNOWN_SITE", format!("{what} {i} is not a hub of this world")),
                Some(h) if !h.open => x.refuse("ORDER_HUB_CLOSED", format!("hub {i} is closed")),
                Some(h) if stands && haversine_m(h.at(), at) > h.radius_m.max(5.0) => x.refuse(
                    "ORDER_SITE_MISMATCH",
                    format!("{what} {i}'s point is {:.0} m from where the order puts it, outside its {:.0} m radius", haversine_m(h.at(), at), h.radius_m),
                ),
                Some(h) if !stands && haversine_m(h.at(), at) > 5.0 => x.refuse(
                    "ORDER_SITE_MISMATCH",
                    format!("{what} {i} is {:.0} m from where the order puts it", haversine_m(h.at(), at)),
                ),
                Some(_) => {}
            }
        };
        match &spec.task {
            Task::Pickup { parcel, site, .. } => {
                match di.parcels.get(parcel) {
                    None => x.refuse("ORDER_PARCEL_UNKNOWN", format!("parcel {parcel} is unknown")),
                    Some(p) if p.state != ParcelState::Ready => x.refuse(
                        "ORDER_PARCEL_NOT_READY",
                        format!("parcel {parcel} is {:?}, not ready", p.state).to_lowercase(),
                    ),
                    Some(p) if p.vehicle.as_ref().is_some_and(|u| u != uid) => x.refuse(
                        "ORDER_PARCEL_ASSIGNED",
                        format!("parcel {parcel} is assigned to another vehicle"),
                    ),
                    Some(_) => {}
                }
                agrees(&site.id, site.at, &mut x, "site");
            }
            Task::Deliver { parcel, drop_zone, at, window_end, .. } => {
                match di.parcels.get(parcel) {
                    None => x.refuse("ORDER_PARCEL_UNKNOWN", format!("parcel {parcel} is unknown")),
                    Some(p) if p.state != ParcelState::Loaded || p.vehicle.as_ref() != Some(uid) => x.refuse(
                        "ORDER_PARCEL_NOT_ABOARD",
                        format!("parcel {parcel} is not loaded on {uid}"),
                    ),
                    Some(_) => {}
                }
                match di.places.drop_zones.get(drop_zone) {
                    None => x.refuse("ORDER_UNKNOWN_DROP_ZONE", format!("drop zone {drop_zone} is unknown")),
                    Some(z) if haversine_m(z.at(), *at) > 5.0 => x.refuse(
                        "ORDER_SITE_MISMATCH",
                        format!("drop zone {drop_zone} is {:.0} m from where the order puts it", haversine_m(z.at(), *at)),
                    ),
                    Some(_) => {}
                }
                if let Some(w) = window_end
                    && let Ok(end) = chrono::DateTime::parse_from_rfc3339(w)
                    && world.t.0 > end.timestamp_millis()
                {
                    x.refuse("ORDER_WINDOW_PASSED", format!("the delivery window ended at {w}"));
                }
            }
            Task::Reroute { route, .. } => {
                match di.reroute_base.get(uid) {
                    None => x.refuse(
                        "ORDER_NO_ROUTE_TO_REPLACE",
                        format!("{uid} has no running deliver, pickup or return_to_hub to reroute"),
                    ),
                    Some(_) => {
                        if let (Some(p), Some(first)) = (pos, route.first())
                            && haversine_m(p, *first) > d.reroute.join_m
                        {
                            x.refuse(
                                "ORDER_REROUTE_JOIN",
                                format!(
                                    "the new route starts {:.0} m from {uid}, more than the {:.0} m it may join within",
                                    haversine_m(p, *first),
                                    d.reroute.join_m
                                ),
                            );
                        }
                    }
                }
            }
            Task::ReturnToHub { hub: Some(h) } => {
                agrees(&h.id, h.at, &mut x, "hub");
                if h.id.is_none() {
                    x.hold("ORDER_HUB_UNNAMED", "return_to_hub names a point that is not a hub");
                }
            }
            Task::ReturnToHub { hub: None } => {}
            Task::LandNow { .. } | Task::Hold { .. } | Task::Rtl {} => {}
            Task::Abort { target, then } => {
                match di.targets.get(&target.to_string()) {
                    None => x.refuse("ORDER_ABORT_TARGET", format!("intent {target} is unknown")),
                    Some(t) => {
                        if !t.uids.contains(uid) {
                            x.refuse("ORDER_ABORT_TARGET", format!("intent {target} is not on {uid}"));
                        } else if !matches!(t.state, crate::intent::State::Dispatched | crate::intent::State::Queued) {
                            x.refuse(
                                "ORDER_ABORT_TARGET",
                                format!("intent {target} is {}, not dispatched or queued", format!("{:?}", t.state).to_lowercase()),
                            );
                        } else if t.kind == TaskKind::Deliver && t.released {
                            x.refuse("ABORT_PARCEL_RELEASED", format!("the parcel of intent {target} is released: it cannot be un-released"));
                        }
                    }
                }
                let _ = then;
            }
            Task::SwapBattery { hub, .. } => match di.places.hubs.get(hub) {
                None => x.refuse("ORDER_UNKNOWN_SITE", format!("hub {hub} is not a hub of this world")),
                Some(h) if !h.open => x.refuse("ORDER_HUB_CLOSED", format!("hub {hub} is closed")),
                Some(h) => {
                    match pos {
                        Some(p) if haversine_m(p, h.at()) <= h.radius_m => {}
                        Some(p) => x.refuse(
                            "ORDER_NOT_AT_HUB",
                            format!("{uid} is {:.0} m from hub {hub}, outside its {:.0} m pad area", haversine_m(p, h.at()), h.radius_m),
                        ),
                        None => x.refuse("ORDER_NOT_AT_HUB", format!("{uid} has no position: it cannot be shown at hub {hub}")),
                    }
                    if di.parcels.values().any(|p| p.vehicle.as_ref() == Some(uid) && p.state == ParcelState::Loaded) {
                        x.refuse("ORDER_PARCEL_ABOARD", format!("{uid} still carries a parcel: unload it first"));
                    }
                    if h.packs == 0 {
                        x.hold("HUB_NO_PACKS", format!("hub {hub} has no charged pack"));
                    }
                }
            },
            Task::ConfirmDropZone { drop_zone, parcel, .. } => {
                match di.places.drop_zones.get(drop_zone) {
                    None => x.refuse("ORDER_UNKNOWN_DROP_ZONE", format!("drop zone {drop_zone} is unknown")),
                    Some(z) if z.closed => x.refuse("DZ_CLOSED", format!("drop zone {drop_zone} is closed")),
                    Some(_) => {}
                }
                // rules 2: a confirmation approves a delivery point, which no delivery need wait
                // for; under rules 1 (and standing approvals off) it confirms a waiting delivery
                if di.waiting_deliver.is_none() && !cfg.standing_approvals() {
                    x.refuse(
                        "ORDER_NO_DELIVER_WAITING",
                        format!("no deliver for parcel {parcel} to {drop_zone} is queued or waiting at the zone"),
                    );
                }
            }
            _ => {}
        }
        x.done(format!("{} preconditions hold", kind.as_str()))
    })
}

// ---- geofence ---------------------------------------------------------------------------------

/// Areas, airspace and temporary restrictions along the route at the times it reaches them
/// (delivery-rules.md section 4).
pub fn geofence(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::Geofence;
    let kind = spec.kind();
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let Some(platform) = platform_of(v) else {
            x.refuse("GEOFENCE_AREAS_UNKNOWN", "cannot evaluate: platform not in the catalog");
            return x.done("");
        };
        let Some(areas) = &world.areas else {
            x.refuse("GEOFENCE_AREAS_UNKNOWN", "no area set loaded: the route cannot be placed");
            return x.done("");
        };
        let kind_ap = kind_of(v);
        // a hold: only a plane over people matters
        if kind == TaskKind::Hold {
            let loiters = kind_ap.is_some_and(AutopilotKind::is_fixed_wing);
            if loiters
                && let Some(p) = v.position.as_ref().map(|p| p.value)
                && areas.areas.iter().any(|a| a.kind == AreaKind::Populated && a.active_at(world.t) && a.contains(p))
            {
                x.hold("GEOFENCE_LOITER_POPULATED", format!("{uid} would loiter over a populated area"));
            }
            return x.done("a hold needs no route");
        }
        let route = match route_for(spec, v, world, cfg) {
            Ok(route) => route,
            Err(e) => {
                if kind.is_safety_order() {
                    return CheckResult::new(r, Some(uid), Outcome::NotEvaluated, format!("route not checked ({} is a safety order): {e}", kind.as_str()));
                }
                x.refuse("GEOFENCE_AREAS_UNKNOWN", e);
                return x.done("");
            }
        };
        if route.is_empty() {
            return x.done(format!("no path for {}", kind.as_str()));
        }
        let h_acc = inputs(world).and_then(|di| di.nav.get(uid)).and_then(|n| n.h_acc_m).unwrap_or(0.0);
        let buffer = d.geofence.buffer_m.get(platform.domain) + h_acc;
        let wx = fresh_wx(world, &d);
        let min_frac = d.energy.min_ground_speed_frac;
        // every leg of the route in travel order, the way home included, with the time its start
        // is reached
        let mut legs: Vec<(LonLat, LonLat, f64)> = Vec::new();
        let mut t = 0.0;
        for leg in route.legs() {
            let tl = trip(&[leg.from, leg.to], platform, &wx, min_frac);
            let one = tl.first();
            legs.push((leg.from, leg.to, t));
            t += one.map_or(0.0, |l| l.t1 - l.t0);
        }
        let start = v.position.as_ref().map(|p| p.value);
        let active_or_soon: Vec<&Area> = areas.areas.iter().collect();
        let approved: Vec<&Area> = areas.approved().filter(|a| a.active_at(world.t) || a.valid_from.is_some()).collect();
        let mut outside_reported = false;
        let mut ceiling_done = false;
        for (i, (from, to, t0)) in legs.iter().enumerate() {
            let Some(pts) = along(*from, *to) else {
                x.refuse("GEOFENCE_AREAS_UNKNOWN", format!("a leg of {:.0} km is longer than the {:.0} km the rule samples", haversine_m(*from, *to) / 1000.0, LEG_MAX_M / 1000.0));
                continue;
            };
            let len = haversine_m(*from, *to).max(1.0);
            let leg_t = trip(&[*from, *to], platform, &wx, min_frac).first().map_or(0.0, |l| l.t1 - l.t0);
            let mut samples: Vec<(LonLat, f64)> = Vec::with_capacity(pts.len() + 2);
            samples.push((*from, 0.0));
            samples.extend(pts.iter().map(|(p, dist)| (*p, *dist)));
            samples.push((*to, len));
            // sample no finer than the knob: keep every k-th point
            let k = ((d.geofence.sample_m / crate::route::LEG_STEP_M).round() as usize).max(1);
            for (j, (p, along_m)) in samples.iter().enumerate() {
                if j % k != 0 && j + 1 != samples.len() {
                    continue;
                }
                let eta = world.t.plus_ms(((t0 + leg_t * (along_m / len)) * 1000.0) as i64);
                for a in &active_or_soon {
                    match a.kind {
                        AreaKind::NoGo | AreaKind::Restricted => {
                            let near = a.contains(*p) || boundary_distance(*p, a) <= buffer;
                            // a vehicle that starts inside leaves it: its first leg is allowed
                            if i == 0 && start.is_some_and(|s| a.contains(s)) {
                                continue;
                            }
                            if a.active_at(eta) {
                                if a.kind == AreaKind::NoGo && near {
                                    x.refuse(
                                        "GEOFENCE_NO_GO",
                                        format!("{} comes within {buffer:.0} m of no-go {} ({:.0} m along it)", leg_name(i, legs.len()), a.id, along_m),
                                    );
                                } else if a.kind == AreaKind::Restricted && a.contains(*p) {
                                    x.hold(
                                        "GEOFENCE_RESTRICTED",
                                        format!("{} enters restricted {} ({:.0} m along it): a person decides", leg_name(i, legs.len()), a.id, along_m),
                                    );
                                }
                            } else if let Some(from_t) = a.valid_from
                                && eta < from_t
                                && from_t.ms_since(eta) <= i64::from(d.geofence.soon_s) * 1000
                                && (a.contains(*p) || (a.kind == AreaKind::NoGo && near))
                            {
                                x.hold(
                                    "GEOFENCE_TEMP_SOON",
                                    format!(
                                        "{} {} becomes active {:.0} s after the route reaches it: legal now, not for long",
                                        a.kind.as_str(),
                                        a.id,
                                        secs(from_t.ms_since(eta))
                                    ),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                if !outside_reported {
                    let inside_approved = approved
                        .iter()
                        .any(|a| a.active_at(eta) && a.contains(*p) && boundary_distance(*p, a) >= buffer);
                    if !inside_approved {
                        outside_reported = true;
                        x.refuse(
                            "GEOFENCE_OUTSIDE_APPROVED",
                            format!(
                                "{} leaves the approved area at ({:.5}, {:.5}), {:.0} m along it, counting a {buffer:.0} m buffer",
                                leg_name(i, legs.len()),
                                p.lon,
                                p.lat,
                                along_m
                            ),
                        );
                    }
                }
                if !ceiling_done
                    && platform.domain == Domain::Air
                    && let Some(alt) = platform.typical_alt_m
                    && let Some(c) = approved
                        .iter()
                        .filter(|a| a.contains(*p) && a.active_at(eta))
                        .filter_map(|a| a.ceiling_m)
                        .min_by(f64::total_cmp)
                    && alt > c
                {
                    ceiling_done = true;
                    x.refuse("GEOFENCE_CEILING", format!("{} flies at {alt:.0} m, above the {c:.0} m ceiling there", platform.id));
                }
            }
        }
        // land_now and a plane loiter over people
        if kind == TaskKind::LandNow {
            let site = match &spec.task {
                Task::LandNow { site: Some(s) } => Some(*s),
                _ => start,
            };
            if let Some(s) = site
                && areas.areas.iter().any(|a| a.kind == AreaKind::Populated && a.active_at(world.t) && a.contains(s))
            {
                x.note("the landing site is in a populated area");
            }
        }
        x.done(format!("clear of the areas ({} legs, {buffer:.0} m buffer)", legs.len()))
    })
}

fn leg_name(i: usize, n: usize) -> String {
    if i + 1 == n && n > 1 { "the return leg".into() } else { format!("leg {}", i + 1) }
}

// ---- energy -----------------------------------------------------------------------------------

/// Battery and range reserve with wind (delivery-rules.md section 5).
pub fn energy(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::Energy;
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let (Some(platform), Some(ap)) = (platform_of(v), kind_of(v)) else {
            x.refuse("ENERGY_BATTERY_UNKNOWN", "cannot evaluate: platform or autopilot unknown");
            return x.done("");
        };
        // the battery fact and its age
        let slow = v.report_interval_ms.is_some_and(|i| i >= 5000);
        let limit = if slow { d.energy.max_age_slow_s } else { d.energy.max_age_s };
        let battery = match &v.battery_pct {
            Some(b) if world.t.ms_since(b.t) as f64 / 1000.0 <= limit => Some(b.value),
            Some(b) => {
                x.hold(
                    "ENERGY_BATTERY_UNKNOWN",
                    format!("battery fact is {:.1} s old (limit {limit:.0} s)", secs(world.t.ms_since(b.t))),
                );
                None
            }
            None => {
                x.hold("ENERGY_BATTERY_UNKNOWN", "no battery level reported");
                None
            }
        };
        let (Some(cruise), Some(endurance_h)) = (platform.cruise_mps.filter(|c| *c > 0.0), platform.endurance_h.filter(|e| *e > 0.0)) else {
            x.hold("ENERGY_ENDURANCE_UNKNOWN", format!("no cruise speed or endurance figure for {}", platform.id));
            return x.done("");
        };
        let route = match route_for(spec, v, world, cfg) {
            Ok(r) => r,
            Err(e) => {
                x.refuse("ENERGY_INSUFFICIENT", e);
                return x.done("");
            }
        };
        let _ = cruise;
        let pts = outbound(&route);
        if pts.len() < 2 {
            return x.done("no distance to fly");
        }
        let wx = fresh_wx(world, &d);
        let mass = parcel_of(spec)
            .and_then(|p| inputs(world).and_then(|di| di.parcels.get(p)))
            .and_then(|p| p.mass_kg)
            .unwrap_or(0.0);
        let mult = payload_mult(&d, ap, platform, mass);
        let min_frac = d.energy.min_ground_speed_frac;
        let per_h = 100.0 / endurance_h;
        let cost = |legs: &[TripLeg]| -> Option<f64> {
            let mut pct = 0.0;
            for l in legs {
                l.gs_mps?;
                pct += (l.t1 - l.t0) / 3600.0 * per_h * mult;
            }
            Some(pct)
        };
        let out_legs = trip(&pts, platform, &wx, min_frac);
        let Some(mut out_pct) = cost(&out_legs) else {
            x.refuse(
                "ENERGY_WIND_NO_HEADWAY",
                format!("a leg cannot be made good against the flow (ground speed under {:.0}% of {:.1} m/s cruise)", min_frac * 100.0, cruise),
            );
            return x.done("");
        };
        // a hover at the drop zone (copter only)
        if matches!(spec.task, Task::Deliver { .. }) && matches!(ap, AutopilotKind::Copter) {
            out_pct += d.energy.drop_hover_s / 3600.0 * per_h * mult;
        }
        // the return to the nearest hub reachable from the route's end
        let end = *pts.last().unwrap_or(&pts[0]);
        let places = inputs(world).map(|di| &di.places);
        let mut ret_pct: Option<f64> = None;
        if let Some(pl) = places {
            let mut hubs: Vec<_> = pl.hubs.values().filter(|h| h.open).collect();
            hubs.sort_by(|a, b| haversine_m(end, a.at()).total_cmp(&haversine_m(end, b.at())).then_with(|| a.id.cmp(&b.id)));
            for h in hubs {
                let legs = trip(&[end, h.at()], platform, &wx, min_frac);
                if let Some(c) = cost(&legs) {
                    ret_pct = Some(c);
                    break;
                }
            }
        }
        let reserve = ap_reserve(&d, ap, spec.constraints.reserve_pct);
        let Some(battery) = battery else {
            return x.done("");
        };
        let Some(ret) = ret_pct else {
            x.hold(
                "ENERGY_NO_HUB_REACHABLE",
                format!("no open hub can be reached from the route's end (route {out_pct:.1}%, battery {battery:.0}%)"),
            );
            // with no hub the margin is judged against the route and the reserve alone
            let need = out_pct * (1.0 + d.energy.wind_margin) + reserve;
            let margin = battery - need;
            if margin < 0.0 {
                x.refuse("ENERGY_INSUFFICIENT", format!("battery {battery:.0}%, need {need:.1}% for the route and reserve {reserve:.0}%"));
            }
            return x.done("");
        };
        let need = (out_pct + ret) * (1.0 + d.energy.wind_margin) + reserve;
        let margin = battery - need;
        let text = format!(
            "battery {battery:.0}%, need {need:.1}% (route {out_pct:.1} plus return {ret:.1}, times {:.2} for wind margin, equals {:.1}, plus reserve {reserve:.0}), margin {margin:.1} points, hold band {:.0}",
            1.0 + d.energy.wind_margin,
            (out_pct + ret) * (1.0 + d.energy.wind_margin),
            d.energy.hold_band_pct
        );
        if margin < 0.0 {
            x.refuse("ENERGY_INSUFFICIENT", text);
        } else if margin < d.energy.hold_band_pct {
            x.hold("ENERGY_MARGIN_THIN", text);
        } else {
            x.note(text);
        }
        x.done("")
    })
}

/// The reserve for one order: the platform kind's, raised (never lowered) by the operator's.
fn ap_reserve(d: &DeliveryRules, ap: AutopilotKind, asked: Option<f64>) -> f64 {
    let base = d.energy.reserve_pct.get(ap);
    asked.map_or(base, |c| c.max(base))
}

// ---- payload ----------------------------------------------------------------------------------

/// The parcel against the platform's payload limit, derated in gusts (section 6).
pub fn payload(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::Payload;
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let (Some(platform), Some(ap)) = (platform_of(v), kind_of(v)) else {
            x.hold("PAYLOAD_UNKNOWN", "cannot evaluate: platform unknown");
            return x.done("");
        };
        let parcel = parcel_of(spec).and_then(|p| inputs(world).and_then(|di| di.parcels.get(p)));
        let Some(parcel) = parcel else {
            x.hold("PAYLOAD_UNKNOWN", "the parcel is not known to the world");
            return x.done("");
        };
        if matches!(parcel.class.as_str(), "hazmat" | "excluded") {
            x.refuse("PAYLOAD_CLASS", format!("parcel class {} is not carried (v1 carries no dangerous goods)", parcel.class));
            return x.done("");
        }
        let Some(mass) = parcel.mass_kg else {
            x.hold("PAYLOAD_UNKNOWN", "the parcel's mass is unknown");
            return x.done("");
        };
        let Some(max) = platform.payload_kg else {
            x.hold("PAYLOAD_UNKNOWN", format!("{} has no payload limit in the catalog", platform.id));
            return x.done("");
        };
        let gust = payload_gust(world, &d);
        let effective = effective_payload(&d, platform, ap, max, gust);
        let text = format!("parcel {mass:.2} kg, platform maximum {max:.2} kg, effective maximum {effective:.2} kg");
        if mass > max {
            x.refuse("PAYLOAD_OVER_MAX", text);
        } else if mass > effective {
            x.refuse("PAYLOAD_OVER_DERATED", text);
        } else if mass > d.payload.hold_frac * effective {
            x.hold("PAYLOAD_NEAR_MAX", text);
        } else {
            x.note(text);
        }
        x.done("")
    })
}

/// The gust the payload rule derates by: the strongest of the fresh weather samples (NaN with
/// none). The dispatcher reads the same figure to offer a parcel only to a vehicle with room.
pub fn payload_gust(world: &WorldSnapshot, d: &DeliveryRules) -> f64 {
    fresh_wx(world, d).iter().map(|w| w.gust_mps).fold(f64::NAN, f64::max)
}

/// A platform's payload limit `max` derated in `gust` (section 6): a multirotor's falls linearly
/// from 1.0 at `payload.derate_from_mps` to `payload.derate_floor` at its hard gust limit; a plane,
/// a car and a boat do not derate; with no gust known, `max`.
pub fn effective_payload(d: &DeliveryRules, platform: &PlatformFacts, ap: AutopilotKind, max: f64, gust: f64) -> f64 {
    if matches!(ap, AutopilotKind::Copter | AutopilotKind::Blimp) && gust.is_finite() {
        let hard = weather_limits(d, platform, ap).gust_hard_mps.unwrap_or(14.0);
        let from = d.payload.derate_from_mps;
        let f = if gust <= from {
            1.0
        } else if gust >= hard {
            d.payload.derate_floor
        } else {
            1.0 - (1.0 - d.payload.derate_floor) * (gust - from) / (hard - from)
        };
        max * f
    } else {
        max
    }
}

// ---- weather ----------------------------------------------------------------------------------

/// A platform's weather limits: its autopilot kind's first-draft limits, under what the policy sets
/// for the platform. The weather rule reads them; so does the bridge's view of each hub's wind.
pub fn weather_limits(d: &DeliveryRules, platform: &PlatformFacts, ap: AutopilotKind) -> WxLimits {
    let base = WxLimits::defaults(ap);
    d.weather.platform.get(&platform.id).map_or(base.clone(), |o| o.over(&base))
}

/// Wind, gusts, visibility, precipitation, tide and current (section 7). The worst of the fresh
/// samples counts; no fresh sample is `WX_UNKNOWN`.
pub fn weather(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::Weather;
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let (Some(platform), Some(ap)) = (platform_of(v), kind_of(v)) else {
            x.hold("WX_UNKNOWN", "cannot evaluate: platform unknown");
            return x.done("");
        };
        let wx = fresh_wx(world, &d);
        if wx.is_empty() {
            x.hold("WX_UNKNOWN", format!("no weather sample within {:.0} s at the places {uid} will be", d.weather.max_age_s));
            return x.done("");
        }
        let l = weather_limits(&d, platform, ap);
        // rules 6: the ground and the water the order's vehicle meets are read where it goes (its
        // position and its order's points), the wind and the current anywhere fresh, as before
        let along: Option<Vec<LonLat>> = cfg.at_least(crate::rules::FLOOD_ALONG_ROUTE_SINCE).then(|| order_points(spec, v, world, cfg));
        let near_route = |w: &WxFact| along.as_ref().is_none_or(|pts| pts.iter().any(|p| haversine_m(*p, w.at) <= crate::rules::FLOOD_NEAR_M));
        let wind = wx.iter().map(|w| w.wind_mps()).fold(0.0, f64::max);
        let gust = wx.iter().map(|w| w.gust_mps).fold(0.0, f64::max);
        let vis = wx.iter().filter_map(|w| w.visibility_m).fold(f64::INFINITY, f64::min);
        let precip = wx.iter().filter_map(|w| w.precip_mmh).fold(0.0, f64::max);
        let above = |x: &mut Verdict, value: f64, soft: Option<f64>, hard: Option<f64>, code: &str, what: &str, unit: &str| {
            if hard.is_some_and(|h| value > h) {
                x.refuse(code, format!("{what} {value:.1} {unit} is above the hard limit {:.1}", hard.unwrap_or(0.0)));
            } else if soft.is_some_and(|s| value > s) {
                x.hold(&format!("{code}_MARGINAL"), format!("{what} {value:.1} {unit} is above the soft limit {:.1}", soft.unwrap_or(0.0)));
            }
        };
        let below = |x: &mut Verdict, value: f64, soft: Option<f64>, hard: Option<f64>, code: &str, what: &str, unit: &str| {
            if !value.is_finite() {
                return;
            }
            if hard.is_some_and(|h| value < h) {
                x.refuse(code, format!("{what} {value:.0} {unit} is below the hard limit {:.0}", hard.unwrap_or(0.0)));
            } else if soft.is_some_and(|s| value < s) {
                x.hold(&format!("{code}_MARGINAL"), format!("{what} {value:.0} {unit} is below the soft limit {:.0}", soft.unwrap_or(0.0)));
            }
        };
        above(&mut x, wind, l.wind_soft_mps, l.wind_hard_mps, "WX_WIND_LIMIT", "sustained wind", "m/s");
        above(&mut x, gust, l.gust_soft_mps, l.gust_hard_mps, "WX_GUST_LIMIT", "gust", "m/s");
        below(&mut x, vis, l.vis_soft_m, l.vis_hard_m, "WX_VISIBILITY", "visibility", "m");
        above(&mut x, precip, l.precip_soft_mmh, l.precip_hard_mmh, "WX_PRECIP", "precipitation", "mm/h");
        // a boat: the current against its cruise speed, the tide against its draft
        if platform.domain == Domain::Surface {
            let cruise = platform.cruise_mps.filter(|c| *c > 0.0);
            let cur = wx
                .iter()
                .filter_map(|w| Some(w.current_e_mps?.hypot(w.current_n_mps?)))
                .fold(0.0, f64::max);
            if let Some(c) = cruise {
                let ratio = cur / c;
                if l.current_hard_ratio.is_some_and(|h| ratio > h) {
                    x.refuse("WX_CURRENT_LIMIT", format!("current {cur:.2} m/s is {ratio:.2} of cruise, above the hard {:.2}", l.current_hard_ratio.unwrap_or(0.0)));
                } else if l.current_soft_ratio.is_some_and(|s| ratio > s) {
                    x.hold("WX_CURRENT_MARGINAL", format!("current {cur:.2} m/s is {ratio:.2} of cruise, above the soft {:.2}", l.current_soft_ratio.unwrap_or(0.0)));
                }
            }
            // rules 6: the water under the boat where it goes, not the shallowest anywhere
            let depth = wx.iter().filter(|w| near_route(w)).filter_map(|w| w.depth_m).fold(f64::INFINITY, f64::min);
            if depth.is_finite() {
                let draft = platform.draft_m.unwrap_or(0.0);
                if depth < draft + d.weather.keel_hard_m {
                    x.refuse("TIDE_AGROUND", format!("water {depth:.2} m deep, draft {draft:.2} m, hard clearance {:.2} m", d.weather.keel_hard_m));
                } else if depth < draft + d.weather.keel_clear_m {
                    x.hold("TIDE_SHALLOW", format!("water {depth:.2} m deep, draft {draft:.2} m, clearance {:.2} m", d.weather.keel_clear_m));
                }
            }
            // a dock's deck against the water at the arrival
            if let Task::Deliver { drop_zone, .. } = &spec.task
                && let Some(z) = inputs(world).and_then(|di| di.places.drop_zones.get(drop_zone))
                && let Some(deck) = z.dock_level_m
                && let Some(s) = nearest_wx(&wx, z.at())
                && let Some(tide) = s.tide_m
                && tide >= deck - d.weather.dock_level_band_m
            {
                x.hold("TIDE_DOCK_LEVEL", format!("water {tide:.2} m is within {:.2} m of the dock deck at {deck:.2} m", d.weather.dock_level_band_m));
            }
        }
        // a car: ground the tide can flood (rules 6: where it goes)
        if platform.domain == Domain::Ground {
            for w in wx.iter().filter(|w| near_route(w)) {
                if let (Some(tide), Some(ground)) = (w.tide_m, w.ground_m)
                    && tide > ground - d.weather.flood_clear_m
                {
                    x.hold("WX_FLOOD_RISK", format!("tide {tide:.2} m reaches within {:.2} m of the ground at {ground:.2} m", d.weather.flood_clear_m));
                    break;
                }
            }
        }
        x.done(format!(
            "wind {wind:.1} m/s, gust {gust:.1} m/s, visibility {}, precipitation {precip:.1} mm/h are within limits ({} samples)",
            if vis.is_finite() { format!("{vis:.0} m") } else { "not reported".into() },
            wx.len()
        ))
    })
}

/// Rules 6: where an order takes its vehicle, for the checks of the ground and the water there:
/// its position, the route's points and the way back's end; without a route (no position known)
/// the task's own points.
fn order_points(spec: &IntentSpec, v: &VehicleFacts, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<LonLat> {
    match route_for(spec, v, world, cfg) {
        Ok(r) => {
            let mut pts = outbound(&r);
            if let Some(ret) = &r.ret {
                pts.push(ret.to);
            }
            if pts.is_empty()
                && let Some(p) = &v.position
            {
                pts.push(p.value);
            }
            pts
        }
        Err(_) => {
            let mut pts: Vec<LonLat> = v.position.iter().map(|p| p.value).collect();
            pts.extend(spec.task.path().map(|p| p.points).unwrap_or_default());
            pts
        }
    }
}

// ---- separation -------------------------------------------------------------------------------

fn along_poly(pts: &[LonLat], speed: f64, t: f64) -> LonLat {
    let mut left = speed * t;
    for w in pts.windows(2) {
        let len = haversine_m(w[0], w[1]);
        if left <= len || len == 0.0 {
            let f = if len == 0.0 { 0.0 } else { left / len };
            return LonLat::new(w[0].lon + (w[1].lon - w[0].lon) * f, w[0].lat + (w[1].lat - w[0].lat) * f);
        }
        left -= len;
    }
    pts.last().copied().unwrap_or(LonLat::new(0.0, 0.0))
}

/// Route conflicts and minimum distance to other traffic (section 8). Pending orders in the
/// inbox are not included.
pub fn separation(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::Separation;
    let k = &d.separation;
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let Some(platform) = platform_of(v) else {
            x.hold("SEP_TRAFFIC_UNKNOWN", "cannot evaluate: platform unknown");
            return x.done("");
        };
        let route = match route_for(spec, v, world, cfg) {
            Ok(r) => r,
            Err(e) => {
                x.hold("SEP_TRAFFIC_UNKNOWN", e);
                return x.done("");
            }
        };
        let pts = outbound(&route);
        let Some(di) = inputs(world) else {
            return unknown_inputs(r, uid, "traffic (delivery inputs)").with_codes(&["SEP_TRAFFIC_UNKNOWN"]);
        };
        // rules 5: the hub sequences its own traffic, and the fleet's vehicles are predicted as
        // they will move (a pickup where the vehicle stands moves nothing)
        let seq = cfg.at_least(crate::rules::HUB_SEQUENCING_SINCE);
        if seq && spec.kind() == TaskKind::Pickup && polyline_m(&pts) < PICKUP_IN_PLACE_M {
            return x.done("no route to separate: it loads where it stands");
        }
        if pts.len() < 2 {
            return x.done("no route to separate");
        }
        let speed = platform.cruise_mps.unwrap_or(0.0).max(0.1);
        let my_alt = if platform.domain == Domain::Air { platform.typical_alt_m.unwrap_or(0.0) } else { 0.0 };
        let mut closest: Option<(f64, String, bool)> = None;
        let mut pairs: Vec<String> = Vec::new();
        for (other, tf) in &di.traffic {
            if spec.uids.contains(other) {
                continue;
            }
            let near = pts.iter().any(|p| haversine_m(*p, tf.position.value) <= k.relevant_km * 1000.0);
            if !near {
                continue;
            }
            if world.t.ms_since(tf.position.t) as f64 / 1000.0 > k.max_age_s {
                x.hold("SEP_TRAFFIC_UNKNOWN", format!("{other}'s position is {:.0} s old and it is within {:.0} km of the route", secs(world.t.ms_since(tf.position.t)), k.relevant_km));
                continue;
            }
            // the minimum for this pair
            let (min_h, min_v) = match (platform.domain, tf.domain) {
                (Domain::Air, Domain::Air) => (k.air_air_h_m, Some(k.air_air_v_m)),
                (Domain::Ground, Domain::Ground) => (k.ground_ground_m, None),
                (Domain::Surface, Domain::Surface) => (k.surface_surface_m, None),
                (Domain::Air, _) | (_, Domain::Air) => {
                    // rules 5: an aircraft of ours with a route to fly is taken at the height it
                    // will fly it (its cruise), not the height it is leaving its stand at
                    let theirs = match world.vehicles.get(other).and_then(platform_of).and_then(|p| p.typical_alt_m) {
                        Some(cruise)
                            if seq
                                && tf.domain == Domain::Air
                                && polyline_m(&std::iter::once(tf.position.value).chain(tf.route.iter().copied()).collect::<Vec<_>>()) >= PICKUP_IN_PLACE_M =>
                        {
                            tf.alt_m.unwrap_or(0.0).max(cruise)
                        }
                        _ => tf.alt_m.unwrap_or(0.0),
                    };
                    let air_alt = if platform.domain == Domain::Air { my_alt } else { theirs };
                    if air_alt > k.air_over_ground_m {
                        continue;
                    }
                    (k.ground_ground_m, None)
                }
                _ => continue,
            };
            let other_speed = tf.speed_mps.or_else(|| tf.velocity_mps.map(|[e, n]| e.hypot(n))).unwrap_or(0.0);
            // rules 5: one with a route to fly flies it at its cruise speed (or faster, as it goes)
            // and height, and is at rest at its end; one that is still (on the ground, or slower
            // than its velocity estimate's noise) with no route to fly stays where it is. One
            // parked on a hub's stand (still, not in the air, inside the hub's radius) is the
            // hub's while this order's vehicle is on the same hub's stands: the hub's layout and
            // sequencing keep a vehicle leaving or reaching its stand clear of the parked ones
            let other_platform = world.vehicles.get(other).and_then(platform_of);
            let landed = tf.domain == Domain::Air && tf.alt_m.is_some_and(|a| a <= LANDED_M);
            let route_m = polyline_m(&std::iter::once(tf.position.value).chain(tf.route.iter().copied()).collect::<Vec<_>>());
            let flies = seq && route_m >= PICKUP_IN_PLACE_M;
            let still = seq && !flies && (landed || other_speed < crate::rules::STILL_MPS);
            let parked_at = (still && tf.domain != Domain::Air || landed && still)
                .then(|| di.places.hubs.values().find(|hb| haversine_m(tf.position.value, hb.at()) <= hb.radius_m))
                .flatten();
            // ... and for the whole of an order that leaves or reaches that hub (its way out of the
            // hub or into it passes the hub's parked vehicles by its layout, not by our minimum)
            let leaves_or_reaches = |hb: &crate::delivery::Hub| {
                [pts.first(), pts.last()].into_iter().flatten().any(|p| haversine_m(*p, hb.at()) <= hb.radius_m)
            };
            let other_pts: Vec<LonLat> = if still {
                vec![tf.position.value, tf.position.value]
            } else if tf.route.is_empty() {
                let [e, n] = tf.velocity_mps.unwrap_or([0.0, 0.0]);
                vec![tf.position.value, crate::geo::offset(tf.position.value, e * k.horizon_s, n * k.horizon_s)]
            } else {
                std::iter::once(tf.position.value).chain(tf.route.iter().copied()).collect()
            };
            let other_speed = if flies {
                other_speed.max(other_platform.and_then(|p| p.cruise_mps).unwrap_or(0.0))
            } else if tf.route.is_empty() && tf.velocity_mps.is_some() {
                other_speed
            } else {
                other_speed.max(0.0)
            };
            // ... at its cruise height when it flies a route (it climbs to it), else where it is
            let other_alt = match (flies, tf.domain, other_platform.and_then(|p| p.typical_alt_m)) {
                (true, Domain::Air, Some(cruise)) => Some(tf.alt_m.unwrap_or(0.0).max(cruise)),
                _ => tf.alt_m,
            };
            // rules 5: the uncertainty grows while either of the two moves; once both are at rest (this
            // order's vehicle at its route's end, the other still or at its own route's end) neither
            // drifts towards the other any more
            let moving_s = if seq {
                let mine = polyline_m(&pts) / speed;
                let theirs = if still {
                    0.0
                } else if !tf.route.is_empty() {
                    route_m / other_speed.max(0.1)
                } else {
                    f64::INFINITY
                };
                mine.max(theirs)
            } else {
                f64::INFINITY
            };
            // ... and one arriving at a hub's stand is parked there once it has arrived (its route
            // ends inside the hub's radius), for an order on, leaving or reaching that hub
            let arrives_at = flies
                .then(|| tf.route.last().and_then(|end| di.places.hubs.values().find(|hb| haversine_m(*end, hb.at()) <= hb.radius_m)))
                .flatten();
            let arrived_s = route_m / other_speed.max(0.1);
            let steps = (k.horizon_s / k.step_s).floor() as usize;
            let mut worst: Option<(f64, f64, bool)> = None; // ratio, time, in hub zone
            for i in 0..=steps {
                let t = i as f64 * k.step_s;
                let a = along_poly(&pts, speed, t);
                let the_hubs = |hb: &crate::delivery::Hub| haversine_m(a, hb.at()) <= hb.radius_m || leaves_or_reaches(hb);
                if parked_at.is_some_and(the_hubs) || arrives_at.is_some_and(|hb| t >= arrived_s && the_hubs(hb)) {
                    continue;
                }
                let b = if tf.route.is_empty() && tf.velocity_mps.is_some() && !still {
                    along_poly(&other_pts, other_speed.max(0.001), t)
                } else {
                    along_poly(&other_pts, other_speed, t)
                };
                let h = haversine_m(a, b);
                let drift = k.drift_mps * t.min(moving_s);
                let v_ok = match (min_v, other_alt) {
                    (Some(mv), Some(alt)) => (my_alt - alt).abs() < mv,
                    (Some(_), None) => true,
                    _ => true,
                };
                if !v_ok {
                    continue;
                }
                let ratio = h / (min_h + drift);
                if worst.is_none_or(|(wr, _, _)| ratio < wr) {
                    let hub_zone = di.places.hubs.values().any(|hb| {
                        haversine_m(a, hb.at()) <= k.hub_zone_m && haversine_m(b, hb.at()) <= k.hub_zone_m
                    });
                    worst = Some((ratio, t, hub_zone));
                }
            }
            if let Some((ratio, t, hub_zone)) = worst {
                let text = format!("{other}: closest approach {:.2} of the {min_h:.0} m minimum at +{t:.0} s", ratio);
                if closest.as_ref().is_none_or(|(c, _, _)| ratio < *c) {
                    closest = Some((ratio, text.clone(), hub_zone));
                }
                pairs.push(text.clone());
                if ratio < 1.0 {
                    if hub_zone && seq {
                        x.hold(
                            "SEP_HUB_ZONE",
                            format!("{other} and {uid} meet within {:.0} m of a hub at +{t:.0} s: the hub sequences them (automation holds one until the other is clear)", k.hub_zone_m),
                        );
                    } else if hub_zone {
                        x.hold("SEP_HUB_ZONE", format!("{other} and {uid} meet within {:.0} m of a hub at +{t:.0} s: a person decides who waits", k.hub_zone_m));
                    } else {
                        x.refuse(
                            "SEP_ROUTE_CONFLICT",
                            format!("{other}: predicted loss of separation at +{t:.0} s (delay about {:.0} s until clear, or take another layer)", (min_h / my_alt_speed(speed)).max(k.step_s)),
                        );
                    }
                } else if ratio < k.marginal_factor {
                    x.hold("SEP_MARGINAL", text);
                }
            }
        }
        // rules 3: the traffic the fleet does not control, as the company's picture shows it
        if cfg.at_least(crate::rules::TRAFFIC_SEPARATION_SINCE) {
            traffic_separation(&mut x, uid, v, platform, &pts, speed, spec, world, di, &d);
        }
        match closest {
            Some((_, text, _)) => x.done(format!("clear of {} vehicles (closest: {text})", pairs.len())),
            None => x.done("no other vehicle within reach of the route"),
        }
    })
}

/// Rules 3 ([`crate::rules::TRAFFIC_SEPARATION_SINCE`], section 8.1): the order's route against
/// the picture of other traffic ([`crate::traffic`]). A drone keeps well clear of manned aircraft
/// (they have the right of way) and clear of other drones and of air tracks that broadcast
/// nothing; a boat keeps its closest approach from ships under way and out of their way ahead,
/// and clear of other vessels; a car has no traffic model. Each holds (`SEP_TRAFFIC_CONFLICT`,
/// `SEP_TRAFFIC_NONCOOP`, `SEP_SHIP_CPA`, `SEP_SHIP_AHEAD`): automation waits it out or takes
/// another route, and a person's own order waits for that person. A route the receivers heard no
/// longer cover holds `SEP_PICTURE_DEGRADED`, a stale picture `SEP_TRAFFIC_UNKNOWN`.
#[allow(clippy::too_many_arguments)]
fn traffic_separation(
    x: &mut Verdict,
    uid: &Uid,
    v: &VehicleFacts,
    platform: &PlatformFacts,
    pts: &[LonLat],
    speed: f64,
    spec: &IntentSpec,
    world: &WorldSnapshot,
    di: &DeliveryInputs,
    d: &DeliveryRules,
) {
    let k = d.separation.traffic_knobs();
    if platform.domain == Domain::Ground {
        x.note("other traffic: no traffic model for a car");
        return;
    }
    let Some(pic) = &di.picture else {
        x.note("other traffic: no picture of it in this run (aircraft and vessels the fleet does not control are not checked)");
        return;
    };
    let age = secs(world.t.ms_since(pic.t));
    if age > k.max_age_s {
        x.hold("SEP_TRAFFIC_UNKNOWN", format!("the picture of other traffic is {age:.0} s old (over {:.0} s): it cannot be relied on", k.max_age_s));
        return;
    }
    if let Some(gap) = crate::traffic::coverage_gap(pic, platform.domain, pts) {
        x.hold("SEP_PICTURE_DEGRADED", gap);
    }
    // our altitude above mean sea level: the cruise height above home, and home's height (where
    // the vehicle's own altitudes say it); the surface has none
    let alt = (platform.domain == Domain::Air).then(|| {
        let home = match (v.alt_msl_m.as_ref(), di.traffic.get(uid).and_then(|t| t.alt_m)) {
            (Some(msl), Some(rel)) => msl.value - rel,
            _ => 0.0,
        };
        platform.typical_alt_m.unwrap_or(0.0) + home
    });
    let horizon = if platform.domain == Domain::Surface { k.surface_horizon_s.max(k.horizon_s) } else { k.horizon_s };
    let samples = crate::traffic::trajectory(pts, speed, alt, d.separation.step_s, horizon);
    let refs = crate::traffic::reference_points(spec, world);
    let relevant_km = d.separation.relevant_km;
    // under way (in the air, or a boat making way): inside a minimum and leaving it is not a conflict
    let own = di.traffic.get(uid);
    let under_way = match platform.domain {
        Domain::Air => own.and_then(|t| t.alt_m).is_some_and(|a| a > 5.0),
        _ => own.and_then(|t| t.speed_mps).is_some_and(|s| s > 0.5),
    };
    let a = crate::traffic::assess(&samples, pts, speed, platform.domain, under_way, pic, world.t, &k, d.separation.surface_surface_m, |t| {
        crate::traffic::relevant(t, &refs, relevant_km)
    });
    // one sentence per reason code: its first conflict, and how many more
    let mut by_code: Vec<(&'static str, Vec<&crate::traffic::Conflict>)> = Vec::new();
    for c in &a.conflicts {
        match by_code.iter_mut().find(|(code, _)| *code == c.code) {
            Some((_, v)) => v.push(c),
            None => by_code.push((c.code, vec![c])),
        }
    }
    for (code, cs) in by_code {
        let more = if cs.len() > 1 { format!(" (and {} more)", cs.len() - 1) } else { String::new() };
        let then = match code {
            "SEP_SHIP_AHEAD" | "SEP_SHIP_CPA" => "the boat waits or takes another way",
            "SEP_TRAFFIC_NONCOOP" => "what it is is not known: the vehicle waits or takes another route",
            _ => "it has the right of way: the vehicle waits or takes another route",
        };
        x.hold(code, format!("{}{more}; {then}", cs[0].words()));
    }
    let closest = a.closest.as_ref().map(|c| {
        format!("; closest {} at {:.1} of its widened minimum at +{:.0} s", c.who(), c.ratio, c.dt_s)
    });
    let qnh = if pic.qnh_hpa.is_none() { "; QNH not known: pressure altitudes taken at standard pressure, their uncertainty widened" } else { "" };
    x.note(format!(
        "other traffic: {} track(s) near the route checked{}{qnh}",
        a.checked,
        closest.unwrap_or_default()
    ));
}

/// Rules 5: a pickup whose route is shorter than this, m, loads where the vehicle stands (the task
/// layer flies it nothing: `crates/fleetmesh-bridge` tasks, within its home tolerance or 10 m).
const PICKUP_IN_PLACE_M: f64 = 10.0;

/// Rules 5: an aircraft this low above its home, m, is on the ground (it is predicted at rest).
const LANDED_M: f64 = 2.0;

fn my_alt_speed(speed: f64) -> f64 {
    speed.max(0.1)
}

// ---- drop zone --------------------------------------------------------------------------------

/// The drop zone's state and conditions (section 10).
pub fn dropzone(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::Dropzone;
    let zone_id = match &spec.task {
        Task::Deliver { drop_zone, .. } | Task::ConfirmDropZone { drop_zone, .. } => drop_zone.clone(),
        _ => return Vec::new(),
    };
    let confirming = matches!(spec.task, Task::ConfirmDropZone { .. });
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let Some(di) = inputs(world) else {
            return unknown_inputs(r, uid, "drop zones (delivery inputs)").with_codes(&["DZ_CLOSED"]);
        };
        let Some(z) = di.places.drop_zones.get(&zone_id) else {
            x.refuse("DZ_CLOSED", format!("drop zone {zone_id} is unknown"));
            return x.done("");
        };
        if z.closed {
            x.refuse("DZ_CLOSED", format!("drop zone {zone_id} is closed"));
            return x.done("");
        }
        if confirming {
            // a confirmation is a person's judgement: the conditions are shown, not tested
            return x.done(format!("drop zone {zone_id} is open"));
        }
        let Some(platform) = platform_of(v) else {
            x.refuse("DZ_WRONG_SURFACE", "cannot evaluate: platform unknown");
            return x.done("");
        };
        let ap = kind_of(v);
        let wrong = matches!(
            (platform.domain, z.surface),
            (Domain::Ground, Surface2::Water) | (Domain::Surface, Surface2::Land)
        );
        if wrong {
            x.refuse("DZ_WRONG_SURFACE", format!("{} cannot reach a {} drop zone", platform.id, if z.surface == Surface2::Water { "water" } else { "land" }));
        }
        // the gust at the zone: a copter descends to a low hover
        let wx = fresh_wx(world, &d);
        if matches!(ap, Some(AutopilotKind::Copter))
            && let Some(s) = nearest_wx(&wx, z.at())
        {
            if s.gust_mps > d.dropzone.descent_gust_hard_mps {
                x.refuse("DZ_GUST_AT_ZONE", format!("gust {:.1} m/s at the zone is above {:.1}", s.gust_mps, d.dropzone.descent_gust_hard_mps));
            } else if s.gust_mps > d.dropzone.descent_gust_mps {
                x.hold("DZ_GUST_AT_ZONE", format!("gust {:.1} m/s at the zone is above the descent limit {:.1}", s.gust_mps, d.dropzone.descent_gust_mps));
            }
        }
        // the latest look
        let obs = di.observations.get(&zone_id).filter(|o| world.t.ms_since(o.t) as f64 / 1000.0 <= d.dropzone.obs_max_age_s);
        match obs {
            Some(o) if o.state == ObservedState::Occupied => {
                if z.kind == DropZoneKind::Pad {
                    x.refuse("DZ_OCCUPIED", format!("the pad is occupied ({} seen)", o.count));
                } else {
                    x.hold("DZ_PEOPLE_NEAR", format!("{} person(s) seen at the {:?}: a person could judge it a passer-by", o.count, z.kind).to_lowercase());
                }
            }
            Some(o) if o.state == ObservedState::Unknown && z.populated => {
                x.hold("DZ_OBSERVATION_STALE", "the latest look could not tell, and the zone is populated");
            }
            Some(_) => {}
            None if z.populated => {
                x.hold("DZ_OBSERVATION_STALE", format!("no look within {:.0} s, and the zone is populated", d.dropzone.obs_max_age_s));
            }
            None => {}
        }
        let standing = cfg.standing_approvals();
        if standing {
            // rules 2: an approved delivery point, ahead of time (a survey, the customer's own
            // spot, a hub pad) or by a person once (`confirm_drop_zone`): no time limit
            if !approved_point(z, di) {
                let why = match z.approval {
                    Some(StandingApproval::Withdrawn) => "its standing approval was withdrawn",
                    _ => "it is not on the approved list",
                };
                x.hold(
                    "DZ_NOT_APPROVED",
                    format!("drop zone {zone_id} is not an approved delivery point ({why}): a person approves it once and it stands"),
                );
            }
        } else {
            // confirmed lately, or a pad at a hub
            let confirmed = di
                .confirmed
                .get(&zone_id)
                .is_some_and(|t| (0..=i64::from(d.dropzone.confirm_valid_s) * 1000).contains(&world.t.ms_since(*t)));
            let hub_pad = z.kind == DropZoneKind::Pad && z.hub.is_some();
            if !confirmed && !hub_pad {
                x.hold("DZ_UNCONFIRMED", format!("drop zone {zone_id} was not confirmed within {} s", d.dropzone.confirm_valid_s));
            }
        }
        // slope for a release on land
        if z.surface == Surface2::Land
            && let Some(s) = z.slope_deg
            && s > d.dropzone.max_slope_deg
        {
            x.refuse("DZ_SLOPE", format!("slope {s:.1} degrees is above {:.1}", d.dropzone.max_slope_deg));
        }
        if standing {
            x.done(format!("drop zone {zone_id} is open, an approved delivery point, and clear"))
        } else {
            x.done(format!("drop zone {zone_id} is open, confirmed or a hub pad, and clear"))
        }
    })
}

/// Rules 2: a zone a delivery may go to without a person: approved ahead of time
/// ([`crate::delivery::DropZone::standing_approved`]) or approved by a person once (the engine's
/// `confirmed`, from an approved `confirm_drop_zone` or a held `deliver`), with no time limit.
pub fn approved_point(z: &crate::delivery::DropZone, di: &DeliveryInputs) -> bool {
    z.standing_approved() || di.confirmed.contains_key(&z.id)
}

// ---- navigation integrity ---------------------------------------------------------------------

/// A vehicle's trust state with the causes, from the facts it reported and the bridge's checks.
pub fn nav_state(v: &VehicleFacts, nav: Option<&NavFacts>, world: &WorldSnapshot, d: &DeliveryRules) -> (NavTrust, Vec<(&'static str, String)>) {
    let k = &d.nav;
    let mut causes: Vec<(&'static str, String)> = Vec::new();
    let Some(pos) = &v.position else {
        return (NavTrust::Unknown, vec![("NAV_STALE_POSITION", "no position reported".into())]);
    };
    let age = world.t.ms_since(pos.t) as f64 / 1000.0;
    if age > k.max_age_s {
        return (NavTrust::Unknown, vec![("NAV_STALE_POSITION", format!("position is {age:.1} s old (limit {:.0} s)", k.max_age_s))]);
    }
    let domain = v.platform.as_ref().map_or(Domain::Air, |p| p.domain);
    let fix = v.gps_fix.as_ref().map(|f| f.value);
    if fix.is_none() && nav.is_none() {
        return (NavTrust::Unknown, vec![("NAV_TRUST_UNKNOWN", "no fix type or navigation facts reported".into())]);
    }
    // untrusted
    let mut untrusted = false;
    match fix {
        Some(f) if f < 2 => {
            untrusted = true;
            causes.push(("NAV_NO_FIX", format!("GPS fix type {f}")));
        }
        Some(2) => {
            untrusted = true;
            causes.push(("NAV_2D_FIX", "only a 2D fix".into()));
        }
        _ => {}
    }
    if v.gps_glitch.as_ref().is_some_and(|g| g.value) {
        untrusted = true;
        causes.push(("NAV_GLITCH", "the navigation filter flags the GPS as glitching".into()));
    }
    match v.nav_source.as_ref().map(|s| s.value) {
        Some(NavSource::DeadReckoning) => {
            untrusted = true;
            causes.push(("NAV_DEAD_RECKONING", "navigating on dead reckoning".into()));
        }
        Some(NavSource::None) => {
            untrusted = true;
            causes.push(("NAV_NO_FIX", "no position source".into()));
        }
        _ => {}
    }
    let h_acc = nav.and_then(|n| n.h_acc_m);
    let degraded_acc = k.degraded_acc_m.get(domain);
    let trusted_acc = k.trusted_acc_m.get(domain);
    if let Some(h) = h_acc
        && h > degraded_acc
    {
        untrusted = true;
        causes.push(("NAV_UNTRUSTED", format!("horizontal accuracy {h:.1} m is above {degraded_acc:.0} m")));
    }
    if let Some(n) = nav {
        if let Some(j) = n.jump_m
            && j > k.jump_m + 3.0 * h_acc.unwrap_or(0.0)
        {
            untrusted = true;
            causes.push(("NAV_JUMP", format!("the position jumped {j:.0} m from where its velocity put it")));
        }
        if let Some(m) = n.range_mismatch_m
            && m > k.range_tol_m
        {
            untrusted = true;
            causes.push(("NAV_RANGE_MISMATCH", format!("the range to the serving station disagrees by {m:.0} m")));
        }
        if n.wrong_surface {
            untrusted = true;
            causes.push(("NAV_WRONG_SURFACE", "the position is on the wrong surface for the platform".into()));
        }
        if let (Some(s), Some(max)) = (n.speed_mps, v.platform.as_ref().and_then(|p| p.max_mps))
            && s > max * k.speed_factor
        {
            untrusted = true;
            causes.push(("NAV_IMPOSSIBLE_SPEED", format!("reported speed {s:.1} m/s is above {:.1}", max * k.speed_factor)));
        }
    }
    let mut state = if untrusted {
        NavTrust::Untrusted
    } else {
        // degraded
        let mut degraded = false;
        if let Some(s) = nav.and_then(|n| n.sats)
            && s < k.min_sats
        {
            degraded = true;
            causes.push(("NAV_LOW_SATS", format!("{s} satellites tracked, fewer than {}", k.min_sats)));
        }
        if let Some(h) = nav.and_then(|n| n.hdop)
            && h > k.max_hdop
        {
            degraded = true;
            causes.push(("NAV_HIGH_HDOP", format!("HDOP {h:.1} is above {:.1}", k.max_hdop)));
        }
        match h_acc {
            Some(h) if h > trusted_acc => {
                degraded = true;
                causes.push(("NAV_DEGRADED_ACCURACY", format!("horizontal accuracy {h:.1} m is above {trusted_acc:.0} m")));
            }
            None => {
                return (NavTrust::Unknown, vec![("NAV_TRUST_UNKNOWN", "no horizontal accuracy reported".into())]);
            }
            _ => {}
        }
        if degraded { NavTrust::Degraded } else { NavTrust::Trusted }
    };
    // hysteresis: a better state only after the conditions have held for `recover_s`
    if let Some(n) = nav
        && let Some(prev) = n.prev_state
        && prev > state
        && prev != NavTrust::Unknown
        && n.good_for_s.is_none_or(|g| g < k.recover_s)
    {
        causes.push((
            "NAV_RECOVERING",
            format!("was {}, better for {:.0} s of the {:.0} s it takes to trust again", prev.as_str(), n.good_for_s.unwrap_or(0.0), k.recover_s),
        ));
        state = prev;
    }
    (state, causes)
}

/// Position fix trust, by order kind (section 9).
pub fn nav_integrity(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::NavIntegrity;
    let kind = spec.kind();
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let nav = inputs(world).and_then(|di| di.nav.get(uid));
        let (state, causes) = nav_state(v, nav, world, &d);
        let air = v.platform.as_ref().is_none_or(|p| p.domain == Domain::Air);
        let then = match &spec.task {
            Task::Abort { then, .. } => Some(*then),
            _ => None,
        };
        let cause_text = causes.iter().map(|(c, t)| format!("{c}: {t}")).collect::<Vec<_>>().join(", ");
        let cause_codes: Vec<&str> = causes.iter().map(|(c, _)| *c).collect();
        let text = format!("navigation is {} ({})", state.as_str(), if cause_text.is_empty() { "fix, satellites and accuracy within limits".into() } else { cause_text });
        let mut apply = |outcome: Outcome, code: &str| {
            match outcome {
                Outcome::Fail => x.refuse(code, text.clone()),
                Outcome::Hold => x.hold(code, text.clone()),
                _ => {}
            }
            for c in &cause_codes {
                if !x.has(c) && outcome != Outcome::Pass {
                    x.codes.push((*c).to_string());
                }
            }
        };
        match kind {
            TaskKind::Pickup | TaskKind::Deliver | TaskKind::Reroute => match state {
                NavTrust::Trusted => {}
                NavTrust::Degraded => apply(Outcome::Hold, "NAV_DEGRADED_ACCURACY"),
                NavTrust::Untrusted => apply(Outcome::Fail, "NAV_UNTRUSTED"),
                NavTrust::Unknown => apply(Outcome::Hold, "NAV_TRUST_UNKNOWN"),
            },
            TaskKind::ReturnToHub => match state {
                NavTrust::Trusted | NavTrust::Degraded => {}
                NavTrust::Untrusted => apply(Outcome::Hold, "NAV_UNTRUSTED"),
                NavTrust::Unknown => apply(Outcome::Hold, "NAV_TRUST_UNKNOWN"),
            },
            TaskKind::Hold => {
                if state == NavTrust::Untrusted && air {
                    apply(Outcome::Hold, "NAV_UNTRUSTED");
                }
            }
            TaskKind::Abort if state == NavTrust::Untrusted && !matches!(then, Some(AbortThen::LandNow | AbortThen::Hold)) => {
                apply(Outcome::Hold, "NAV_UNTRUSTED");
            }
            // land_now needs no horizontal position
            _ => {}
        }
        x.done(text.clone())
    })
}

// ---- lost link --------------------------------------------------------------------------------

/// The failsafe plan and the link along the route (section 11).
pub fn lost_link(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let d = cfg.delivery_rules();
    let r = RuleId::LostLink;
    per_vehicle(spec, world, r, |uid, v| {
        let mut x = Verdict::new(r, uid);
        let (Some(platform), Some(ap)) = (platform_of(v), kind_of(v)) else {
            x.hold("LINK_PLAN_UNKNOWN", "cannot evaluate: platform unknown");
            return x.done("");
        };
        let want = d.lost_link.expected(&platform.id, ap);
        match &v.lost_link {
            None => x.hold("LINK_PLAN_UNKNOWN", format!("{uid}'s failsafe plan has not been read")),
            Some(p) => {
                let plan = &p.value;
                let timer_ok = match plan.timer_s {
                    Some(t) => want.timer_s.iter().any(|w| (w - t).abs() < 0.5),
                    None => false,
                };
                if plan.action != want.action || !timer_ok {
                    x.refuse(
                        "LINK_FAILSAFE_MISMATCH",
                        format!(
                            "{uid}'s failsafe is {:?} after {} s; policy wants {:?} after {} s",
                            plan.action,
                            plan.timer_s.map_or("no timer".to_string(), |t| format!("{t:.0}")),
                            want.action,
                            want.timer_s.iter().map(|t| format!("{t:.0}")).collect::<Vec<_>>().join(" or ")
                        ),
                    );
                }
            }
        }
        match inputs(world).and_then(|di| di.link.get(uid)) {
            None => x.note("no link prediction along the route was given: the coverage walk is not evaluated"),
            Some(l) => {
                if l.trips && !l.return_ok {
                    x.refuse("LINK_FAILSAFE_UNSAFE", "the failsafe trips on the route and the return from there exceeds the energy available");
                } else if l.trips && l.gap_s.is_some_and(|g| g > d.lost_link.gap_hold_s) && !l.backup_covers {
                    x.hold(
                        "LINK_COVERAGE_GAP",
                        format!("the link is predicted down for {:.0} s on the route (limit {:.0} s) and no backup covers it", l.gap_s.unwrap_or(0.0), d.lost_link.gap_hold_s),
                    );
                }
                if l.on_backup {
                    x.note("LINK_ON_BACKUP: the main link is down and the backup carries");
                    if matches!(spec.task, Task::Deliver { .. }) && l.backup_rate_low {
                        x.hold("LINK_BACKUP_SLOW", "the backup radio carries a low data rate: a person judges a delivery on it");
                    }
                }
            }
        }
        x.done(format!("failsafe {:?} as policy wants", want.action))
    })
}

// ---- policy facts -----------------------------------------------------------------------------

/// What the policy conditions of the delivery kinds read (delivery-orders.md section 9), computed
/// from the same logged inputs as the rules.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeliveryPolicyFacts {
    /// `land_now`: the landing point is in a hub's landing zone or an approved landing area.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub site_planned: bool,
    /// `abort`: the vehicle is over, or within the people buffer of, a populated area.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub over_people: bool,
    /// `abort`: the target is a `deliver` in its last seconds before the drop zone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub final_approach: bool,
    /// `abort`: what the order does next puts the vehicle down away from a hub.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unplanned_landing: bool,
    /// The routine tier: a pickup from a hub, a deliver to a confirmed zone or a hub pad (from
    /// rules 2: to an approved delivery point, [`approved_point`]), a reroute within the
    /// added-distance limit, a battery swap at a hub.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hub_or_confirmed: bool,
}

/// The facts for `spec` (every vehicle of the order must satisfy a "planned"/"routine" fact).
pub fn policy_facts(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> DeliveryPolicyFacts {
    let d = cfg.delivery_rules();
    let mut f = DeliveryPolicyFacts::default();
    let Some(di) = inputs(world) else { return f };
    let all = |pred: &dyn Fn(&Uid, &VehicleFacts) -> bool| -> bool {
        !spec.uids.is_empty() && spec.uids.iter().all(|u| world.vehicles.get(u).is_some_and(|v| pred(u, v)))
    };
    let pos_of = |v: &VehicleFacts| v.position.as_ref().map(|p| p.value);
    let planned_at = |p: LonLat| {
        di.places.hub_landing_at(p).is_some()
            || world.areas.as_ref().is_some_and(|a| a.areas.iter().any(|ar| ar.kind == AreaKind::Landing && ar.active_at(world.t) && ar.contains(p)))
    };
    let people = |p: LonLat| {
        world.areas.as_ref().is_some_and(|a| {
            a.areas.iter().any(|ar| {
                ar.kind == AreaKind::Populated && ar.active_at(world.t) && (ar.contains(p) || boundary_distance(p, ar) <= d.abort.people_buffer_m)
            })
        })
    };
    match &spec.task {
        Task::LandNow { site } => {
            f.site_planned = match site {
                Some(s) => planned_at(*s),
                None => all(&|_, v| pos_of(v).is_some_and(planned_at)),
            };
        }
        Task::Abort { target, then } => {
            f.over_people = !all(&|_, v| pos_of(v).is_some_and(|p| !people(p)));
            let tf = di.targets.get(&target.to_string());
            f.final_approach = tf.is_some_and(|t| {
                t.kind == TaskKind::Deliver
                    && t.drop_zone.as_ref().and_then(|z| di.places.drop_zones.get(z)).is_some_and(|z| {
                        spec.uids.iter().any(|u| {
                            world.vehicles.get(u).is_some_and(|v| {
                                let speed = v.velocity_mps.as_ref().map_or(0.0, |f| f.value[0].hypot(f.value[1])).max(0.5);
                                pos_of(v).is_some_and(|p| haversine_m(p, z.at()) / speed <= d.abort.final_approach_s)
                            })
                        })
                    })
            });
            f.unplanned_landing = *then == AbortThen::LandNow && !all(&|_, v| pos_of(v).is_some_and(planned_at));
        }
        Task::Pickup { site, .. } => {
            f.hub_or_confirmed = site.id.as_ref().is_some_and(|i| di.places.hubs.contains_key(i));
        }
        Task::Deliver { drop_zone, .. } if cfg.standing_approvals() => {
            // rules 2: an approved delivery point, with no time limit
            f.hub_or_confirmed = di.places.drop_zones.get(drop_zone).is_some_and(|z| approved_point(z, di));
        }
        Task::Deliver { drop_zone, .. } => {
            let confirmed = di
                .confirmed
                .get(drop_zone)
                .is_some_and(|t| (0..=i64::from(d.dropzone.confirm_valid_s) * 1000).contains(&world.t.ms_since(*t)));
            let pad = di.places.drop_zones.get(drop_zone).is_some_and(|z| z.kind == DropZoneKind::Pad && z.hub.is_some());
            f.hub_or_confirmed = confirmed || pad;
        }
        Task::Reroute { route, .. } => {
            f.hub_or_confirmed = spec.uids.iter().all(|u| {
                let (Some(base), Some(v)) = (di.reroute_base.get(u), world.vehicles.get(u)) else { return false };
                let Some(p) = pos_of(v) else { return false };
                let mut pts = vec![p];
                pts.extend(route.iter().copied());
                let new_m = polyline_m(&pts);
                base.route_m > 0.0 && (new_m - base.route_m) / base.route_m * 100.0 <= d.reroute.auto_added_pct
            });
        }
        Task::SwapBattery { hub, .. } => {
            f.hub_or_confirmed = di.places.hubs.contains_key(hub);
        }
        _ => {}
    }
    f
}

/// The platform's own `land_now` (the catalog's field, else its autopilot kind's).
pub fn land_now_behavior(v: &VehicleFacts) -> Option<LandNowBehavior> {
    v.platform.as_ref().and_then(|p| p.land_now).or_else(|| kind_of(v).map(LandNowBehavior::default_for))
}

/// Every uid an order's rules named in a result, for the console (a convenience for tests).
pub fn rule_codes(results: &[CheckResult]) -> BTreeSet<String> {
    results.iter().flat_map(|r| r.codes.iter().cloned()).collect()
}
