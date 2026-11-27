//! Rules 8 ([`crate::rules::PLAYS_SINCE`], docs/design/incidents.md): a person's standing decision
//! for a pattern, a play. Where one cause makes many held orders alike (gusts over the drones'
//! soft limit at one hub), the person answers the pattern once instead of each order:
//!
//! - a **ground stop** at a hub: no agent's pickup or delivery leaves it without a person;
//! - **within limits**: an order held only for the codes the play names, whose trip, parcel and
//!   battery are within its limits, goes as the person decided.
//!
//! A parcel at or above the play's `except_priority` is outside it: its order is decided as if there
//! were no play, so an urgent parcel still asks the person. Only a person adopts a play
//! (`play.adopted`, with their session); it ends when the bridge's end condition clears, when a
//! person lifts it, or at its `until_t`. The engine fills the plays in force into each decision's
//! inputs ([`crate::WorldSnapshot::plays`]) and [`route`] reads them; replay checks that each
//! decision read exactly the plays in force and re-derives its routing.

use serde::{Deserialize, Serialize};

use crate::geo::{LonLat, haversine_m};
use crate::ids::OperatorId;
use crate::intent::{IntentSpec, Task, TaskKind};
use crate::policy::{PolicyDecision, Tier};
use crate::rules::{CheckResult, Outcome};
use crate::time::Time;
use crate::world::WorldSnapshot;

/// What a play does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayKind {
    /// No agent's pickup or delivery leaves the hub without a person.
    GroundStop,
    /// An order held only for the play's codes, within its limits, goes as the person decided.
    Limits,
}

impl PlayKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PlayKind::GroundStop => "ground_stop",
            PlayKind::Limits => "limits",
        }
    }
}

/// A limits play's bounds; one left out does not bound.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayLimits {
    /// The parcel's trip: from the vehicle (or the hub it is picked up at) to its drop zone, m.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_trip_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_mass_kg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_battery_pct: Option<f64>,
}

impl PlayLimits {
    pub fn is_empty(&self) -> bool {
        self.max_trip_m.is_none() && self.max_mass_kg.is_none() && self.min_battery_pct.is_none()
    }
}

/// A play in force, as adopted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Play {
    pub id: u64,
    pub kind: PlayKind,
    /// The hub whose departures it answers.
    pub hub: String,
    /// A limits play: the held codes it answers (each among the policy's `[plays] codes`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codes: Vec<String>,
    #[serde(default, skip_serializing_if = "PlayLimits::is_empty")]
    pub limits: PlayLimits,
    /// Parcels at or above this priority are outside the play (an urgent one still asks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub except_priority: Option<u8>,
    /// When it ends at the latest.
    pub until_t: Time,
    /// Its end condition in words, as the bridge watches it ("gusts under 11 m/s for 5 min").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifts: Option<String>,
    /// The person who adopted it.
    pub by: OperatorId,
}

/// What a person asks for: the engine gives the play its id, its operator and its `until_t`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayRequest {
    pub kind: PlayKind,
    pub hub: String,
    #[serde(default)]
    pub codes: Vec<String>,
    #[serde(default)]
    pub limits: PlayLimits,
    #[serde(default)]
    pub except_priority: Option<u8>,
    /// How long at most, s (at most `[plays] max_s`).
    pub for_s: u32,
    #[serde(default)]
    pub lifts: Option<String>,
}

/// `[plays]` in the policy file (rules 8, policy schema 5). Absent: no play may be adopted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaysPolicy {
    /// The held codes a limits play may answer: soft-band holds a person may decide for a pattern
    /// (never a refusal: a play sends nothing the checks refuse).
    pub codes: Vec<String>,
    /// A play's longest life, s.
    pub max_s: u32,
}

impl PlaysPolicy {
    /// Why a request is refused under this policy, if it is.
    pub fn refuse(&self, r: &PlayRequest) -> Option<String> {
        if r.hub.trim().is_empty() {
            return Some("a play names its hub".into());
        }
        if r.for_s == 0 || r.for_s > self.max_s {
            return Some(format!("a play lasts 1 to {} s ([plays] max_s), not {}", self.max_s, r.for_s));
        }
        match r.kind {
            PlayKind::GroundStop => {
                if !r.codes.is_empty() || !r.limits.is_empty() {
                    return Some("a ground stop takes no codes and no limits".into());
                }
            }
            PlayKind::Limits => {
                if r.codes.is_empty() {
                    return Some("a limits play names the held codes it answers".into());
                }
                if let Some(c) = r.codes.iter().find(|c| !self.codes.contains(c)) {
                    return Some(format!("{c} is not among the codes a play may answer ([plays] codes: {:?})", self.codes));
                }
                if r.limits.is_empty() {
                    return Some("a limits play names its limits (trip, mass or battery)".into());
                }
                let bad = |v: Option<f64>| v.is_some_and(|x| !x.is_finite() || x < 0.0);
                if bad(r.limits.max_trip_m) || bad(r.limits.max_mass_kg) || bad(r.limits.min_battery_pct) || r.limits.min_battery_pct.is_some_and(|b| b > 100.0) {
                    return Some("a limit is a finite number, 0 or more (a battery 100% at most)".into());
                }
            }
        }
        None
    }
}

/// The plays in force at `t`: adopted, not ended, before their `until_t`.
pub fn in_force(plays: &[Play], t: Time) -> Vec<Play> {
    plays.iter().filter(|p| t < p.until_t).cloned().collect()
}

/// How far from a hub's centre a vehicle is at it: the larger of three times its radius and
/// 150 m (as the bridge's catchment).
fn catchment_m(radius_m: f64) -> f64 {
    (radius_m * 3.0).max(150.0)
}

/// Whether `spec` is a departure from `hub`: a pickup at it, or a delivery whose vehicles are all
/// at it.
pub fn departs_from(spec: &IntentSpec, world: &WorldSnapshot, hub: &str) -> bool {
    let Some(di) = world.delivery.as_ref() else { return false };
    let Some(h) = di.places.hubs.get(hub) else { return false };
    match &spec.task {
        Task::Pickup { site, .. } => site.id.as_deref() == Some(hub),
        Task::Deliver { .. } => {
            !spec.uids.is_empty()
                && spec.uids.iter().all(|u| {
                    world
                        .vehicles
                        .get(u)
                        .and_then(|v| v.position.as_ref())
                        .is_some_and(|p| haversine_m(p.value, LonLat::new(h.lon, h.lat)) <= catchment_m(h.radius_m))
                })
        }
        _ => false,
    }
}

fn parcel_of(spec: &IntentSpec) -> Option<&str> {
    match &spec.task {
        Task::Pickup { parcel, .. } | Task::Deliver { parcel, .. } => Some(parcel),
        _ => None,
    }
}

/// Whether the order's parcel is outside the play: at or above its `except_priority`.
pub fn excepted(p: &Play, spec: &IntentSpec, world: &WorldSnapshot) -> bool {
    let Some(min) = p.except_priority else { return false };
    let prio = parcel_of(spec).and_then(|id| world.delivery.as_ref()?.parcels.get(id)).and_then(|f| f.priority);
    prio.is_some_and(|x| x >= min)
}

/// What a limits play reads of an order: its trip, its parcel's mass, its vehicles' lowest
/// battery. None where the inputs do not say.
pub fn order_facts(spec: &IntentSpec, world: &WorldSnapshot) -> (Option<f64>, Option<f64>, Option<f64>) {
    let di = world.delivery.as_ref();
    let parcel = parcel_of(spec).and_then(|id| di?.parcels.get(id));
    let mass = parcel.and_then(|f| f.mass_kg);
    let battery = spec
        .uids
        .iter()
        .map(|u| world.vehicles.get(u).and_then(|v| v.battery_pct.as_ref()).map(|f| f.value))
        .collect::<Option<Vec<f64>>>()
        .and_then(|b| b.into_iter().reduce(f64::min));
    let zone_at = |id: &str| di.and_then(|d| d.places.drop_zones.get(id)).map(|z| z.at());
    let trip = match &spec.task {
        Task::Deliver { at, .. } => spec
            .uids
            .first()
            .and_then(|u| world.vehicles.get(u))
            .and_then(|v| v.position.as_ref())
            .map(|p| haversine_m(p.value, *at)),
        Task::Pickup { site, .. } => {
            parcel.and_then(|f| f.to.as_deref()).and_then(zone_at).map(|z| haversine_m(site.at, z))
        }
        _ => None,
    };
    (trip, mass, battery)
}

/// Whether the order is within a limits play's limits: every limit it names is met, by facts the
/// inputs give (a fact they do not give meets no limit).
pub fn within(l: &PlayLimits, (trip, mass, battery): (Option<f64>, Option<f64>, Option<f64>)) -> bool {
    l.max_trip_m.is_none_or(|m| trip.is_some_and(|t| t <= m))
        && l.max_mass_kg.is_none_or(|m| mass.is_some_and(|x| x <= m))
        && l.min_battery_pct.is_none_or(|m| battery.is_some_and(|b| b >= m))
}

/// A limits play's word on an order the policy file sent to a person because a check held it
/// (`(hold)`, with `checks` its checks' results): one in force sends it when it is a departure from
/// the play's hub, held only for the play's codes and within its limits (`(play)`). An order for a
/// parcel the play excepts is left to the person.
pub fn route_limits(spec: &IntentSpec, world: &WorldSnapshot, checks: &[CheckResult], d: &mut PolicyDecision) {
    if world.plays.is_empty() || d.rule != "(hold)" || !matches!(spec.kind(), TaskKind::Pickup | TaskKind::Deliver) {
        return;
    }
    if checks.iter().any(|r| r.outcome == Outcome::Fail) {
        return;
    }
    let held: Vec<&str> = checks.iter().filter(|r| r.outcome == Outcome::Hold).flat_map(|r| r.codes.iter().map(String::as_str)).collect();
    if held.is_empty() {
        return;
    }
    for p in world.plays.iter().filter(|p| p.kind == PlayKind::Limits) {
        if held.iter().all(|c| p.codes.iter().any(|x| x == c))
            && departs_from(spec, world, &p.hub)
            && !excepted(p, spec, world)
            && within(&p.limits, order_facts(spec, world))
        {
            d.tier = Tier::Auto;
            d.rule = "(play)".into();
            d.reason = format!("play {} by {}: held only for {} and within its limits, it goes as the person decided", p.id, p.by, held.join(", "));
            d.submitter_authorizes = false;
            return;
        }
    }
}

/// A ground stop's word on an order the policy routed as `d`: an agent's pickup or delivery
/// leaving a hub under a ground stop goes to a person (`(ground stop)`), whatever `d` said; one
/// for a parcel the play excepts is left as `d` said.
pub fn route_ground_stop(spec: &IntentSpec, world: &WorldSnapshot, d: &mut PolicyDecision) {
    if world.plays.is_empty() || !spec.source.is_agent() || !matches!(spec.kind(), TaskKind::Pickup | TaskKind::Deliver) {
        return;
    }
    if let Some(p) = world.plays.iter().find(|p| p.kind == PlayKind::GroundStop && departs_from(spec, world, &p.hub) && !excepted(p, spec, world)) {
        d.tier = Tier::Human;
        d.rule = "(ground stop)".into();
        d.reason = format!("play {} by {}: a ground stop at {}: no agent's departure leaves it without a person", p.id, p.by, p.hub);
        d.submitter_authorizes = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PlaysPolicy {
        PlaysPolicy { codes: vec!["WX_GUST_LIMIT_MARGINAL".into()], max_s: 3600 }
    }

    fn req(kind: PlayKind) -> PlayRequest {
        PlayRequest { kind, hub: "hub-crissy".into(), codes: vec![], limits: PlayLimits::default(), except_priority: Some(3), for_s: 1800, lifts: None }
    }

    #[test]
    fn a_request_is_refused_outside_the_policy() {
        let p = policy();
        assert_eq!(p.refuse(&req(PlayKind::GroundStop)), None);
        assert!(p.refuse(&PlayRequest { for_s: 7200, ..req(PlayKind::GroundStop) }).is_some(), "longer than max_s");
        assert!(p.refuse(&req(PlayKind::Limits)).is_some(), "a limits play names its codes");
        let limits = PlayLimits { max_trip_m: Some(1500.0), max_mass_kg: Some(1.0), min_battery_pct: Some(60.0) };
        let ok = PlayRequest { codes: vec!["WX_GUST_LIMIT_MARGINAL".into()], limits: limits.clone(), ..req(PlayKind::Limits) };
        assert_eq!(p.refuse(&ok), None);
        assert!(p.refuse(&PlayRequest { codes: vec!["WX_GUST_LIMIT".into()], ..ok.clone() }).is_some(), "a refusal's code is never a play's");
        assert!(p.refuse(&PlayRequest { limits: PlayLimits::default(), ..ok.clone() }).is_some(), "limits named");
        assert!(p.refuse(&PlayRequest { limits: PlayLimits { min_battery_pct: Some(120.0), ..limits }, ..ok }).is_some());
    }

    #[test]
    fn within_its_limits_means_every_named_limit_met_by_a_known_fact() {
        let l = PlayLimits { max_trip_m: Some(1500.0), max_mass_kg: Some(1.0), min_battery_pct: Some(60.0) };
        assert!(within(&l, (Some(900.0), Some(0.8), Some(90.0))));
        assert!(!within(&l, (Some(1600.0), Some(0.8), Some(90.0))), "too far");
        assert!(!within(&l, (Some(900.0), None, Some(90.0))), "a mass not known meets no limit");
        assert!(within(&PlayLimits { max_trip_m: Some(1500.0), ..PlayLimits::default() }, (Some(900.0), None, None)), "one named, one checked");
    }
}
