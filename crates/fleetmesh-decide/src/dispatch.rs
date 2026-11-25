//! Dispatch v1 (docs/design/dispatch.md): the deterministic greedy assigner.
//!
//! [`assign`] is a pure function of its [`AssignInput`]: no clock, no randomness, no I/O, no
//! hash-map iteration; every sort is stable and ties break by id. The bridge's dispatcher
//! builds the input from the world as it is, logs it by value with the plan (`dispatch.plan`),
//! and proposes the orders the plan names as an agent (`dispatcher`) through the same port as
//! any other proposer. Replay re-runs [`assign`] on the logged input and compares the plan.
//!
//! What it decides: for each open parcel, best priority and earliest deadline first, which of
//! the vehicles waiting at the parcel's origin hub carries it. A candidate is refused for a
//! platform that cannot carry the mass, a drop zone on the wrong surface, a battery that does
//! not cover the trip (out to the zone and back to the nearest hub) over its reserve, or a wind
//! (or current) that makes a leg impossible. The rest are costed:
//!
//! ```text
//! cost = w_energy * energy_pct + w_time * eta_s / 60 + w_late * max(0, eta_s - slack_s) / 60
//! ```
//!
//! with the energy and the legs' ground speed taken from the same trip model the energy rule
//! uses ([`crate::delivery_rules::trip_legs`]), so a headwind changes the winner. Not in v1:
//! batching for cars and boats, the expected-hold penalty (it needs a dry run) and wear.
//!
//! Rules 4 ([`HOME_BASE_SINCE`]): every vehicle belongs to a home base (its home hub,
//! [`Candidate::home`]) and goes back there after a delivery, so the way back a trip is costed
//! with goes to its home hub, or to the hub nearest the zone when the battery would not cover the
//! way home (where automation would send it then). A vehicle away from its base is therefore
//! dearer than one based where the parcel waits, unless the parcel lies on its way home. Under
//! rules 3 the way back went to the hub nearest the zone always ([`assign_under`] decides under
//! the rules it is given; replay gives it the log's).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::catalog::{AutopilotKind, Domain, PlatformFacts};
use crate::delivery::{DropZone, EnergyKnobs, Places, Surface2, WxFact};
use crate::geo::LonLat;
use crate::ids::Uid;
use crate::rules::{HOME_BASE_SINCE, RulesConfig};
use crate::time::Time;

/// The assigner's knobs (`[dispatch]` of a scenario's dispatch file), logged with every plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DispatchParams {
    /// Seconds of scenario time between plans.
    pub tick_s: u32,
    /// A trip may use at most this share of a full battery.
    pub max_range_frac: f64,
    /// An order still waiting in the inbox after this long is withdrawn and the parcel
    /// reassigned.
    pub hold_reassign_s: u32,
    pub max_proposals_per_min: u32,
    pub w_energy: f64,
    pub w_time: f64,
    pub w_late: f64,
    pub w_idle: f64,
}

impl Default for DispatchParams {
    fn default() -> Self {
        DispatchParams {
            tick_s: 10,
            max_range_frac: 0.7,
            hold_reassign_s: 120,
            max_proposals_per_min: 60,
            w_energy: 1.0,
            w_time: 0.5,
            w_late: 8.0,
            w_idle: 0.2,
        }
    }
}

/// A parcel the assigner may place: ready and not yet assigned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenParcel {
    pub id: String,
    pub hub: String,
    pub zone: String,
    pub mass_kg: f64,
    /// `normal`, `fragile`, `hazmat` (v1 refuses it) or `excluded`.
    pub class: String,
    /// 0 low, 1 normal, 2 high, 3 urgent.
    pub priority: u8,
    /// The delivery deadline.
    pub due: Time,
}

/// A vehicle the assigner may use: idle at a hub with its battery known and its navigation
/// trusted (the bridge filters on those and says which are not available).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub uid: Uid,
    pub hub: String,
    pub platform: PlatformFacts,
    pub kind: AutopilotKind,
    pub battery_pct: f64,
    /// Rules 4: its home base, the hub it belongs to and returns to (the fleet file's
    /// `home_hub`, else the hub it started at). Absent before rules 4, and for a vehicle with no
    /// base (then the way back goes to the hub nearest the zone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssignInput {
    pub t: Time,
    pub params: DispatchParams,
    pub energy: EnergyKnobs,
    pub places: Places,
    pub weather: Vec<WxFact>,
    pub parcels: Vec<OpenParcel>,
    pub vehicles: Vec<Candidate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Terms {
    pub energy_pct: f64,
    pub time: f64,
    pub late: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    pub vehicle: Uid,
    pub parcel: String,
    pub hub: String,
    pub zone: String,
    pub eta_s: f64,
    pub cost: f64,
    pub terms: Terms,
    /// The candidates it beat, cheapest first, at most three.
    pub beat: Vec<(Uid, f64)>,
    /// The candidates refused, with the first reason.
    pub rejected: Vec<(Uid, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub assignments: Vec<Assignment>,
    /// Parcels left open, each with what stopped every candidate.
    pub unassigned: Vec<(String, Vec<String>)>,
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Where a vehicle goes back to from `zone` when it has no home base to go to: the open hub
/// nearest the zone (its origin hub when there is none).
fn nearest_back(inp: &AssignInput, zone: &DropZone, hub_at: LonLat) -> LonLat {
    inp.places.nearest_hub(zone.at()).map_or(hub_at, |(h, _)| h.at())
}

/// One candidate's trip for one parcel: the energy it costs (out to the zone, then back to
/// `back` with nothing aboard) and the time to the zone, or why not.
fn trip_cost(inp: &AssignInput, v: &Candidate, zone: &DropZone, hub_at: LonLat, back: LonLat, mass_kg: f64) -> Result<(f64, f64), String> {
    let platform = &v.platform;
    let (Some(cruise), Some(endurance_h)) = (
        platform.cruise_mps.filter(|c| *c > 0.0),
        platform.endurance_h.filter(|e| *e > 0.0),
    ) else {
        return Err("ENERGY_ENDURANCE_UNKNOWN".into());
    };
    let wx: Vec<&WxFact> = inp.weather.iter().collect();
    let min_frac = inp.energy.min_ground_speed_frac;
    let per_h = 100.0 / endurance_h;
    let mult = match platform.payload_kg {
        Some(max) if max > 0.0 => 1.0 + inp.energy.payload_load.get(v.kind) * (mass_kg / max).clamp(0.0, 1.0),
        _ => 1.0,
    };
    let out = crate::delivery_rules::trip_legs(&[hub_at, zone.at()], platform, &wx, min_frac);
    let mut energy = 0.0;
    let mut eta = 0.0;
    for l in &out {
        if l.gs_mps.is_none() {
            return Err(format!("ENERGY_WIND_NO_HEADWAY: a leg cannot be made good against the flow at {cruise:.1} m/s"));
        }
        energy += (l.t1 - l.t0) / 3600.0 * per_h * mult;
        eta += l.t1 - l.t0;
    }
    if matches!(v.kind, AutopilotKind::Copter) {
        energy += inp.energy.drop_hover_s / 3600.0 * per_h * mult;
    }
    // back with nothing aboard
    let ret = crate::delivery_rules::trip_legs(&[zone.at(), back], platform, &wx, min_frac);
    for l in &ret {
        if l.gs_mps.is_none() {
            return Err("ENERGY_WIND_NO_HEADWAY: the way back cannot be made good against the flow".into());
        }
        energy += (l.t1 - l.t0) / 3600.0 * per_h;
    }
    Ok((energy, eta))
}

/// The plan for `inp` under this build's rules: pure and deterministic.
pub fn assign(inp: &AssignInput) -> Plan {
    assign_under(inp, &RulesConfig::default())
}

/// The plan for `inp` under `rules` (a run's, or the version a replayed log was made under):
/// pure and deterministic.
pub fn assign_under(inp: &AssignInput, rules: &RulesConfig) -> Plan {
    // rules 4: the way back goes to the vehicle's home base
    let home_base = rules.at_least(HOME_BASE_SINCE);
    let mut plan = Plan::default();
    let mut order: Vec<&OpenParcel> = inp.parcels.iter().collect();
    order.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(a.due.cmp(&b.due))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut vehicles: Vec<&Candidate> = inp.vehicles.iter().collect();
    vehicles.sort_by(|a, b| a.uid.cmp(&b.uid));
    let mut used: BTreeSet<&Uid> = BTreeSet::new();
    for p in order {
        let Some(hub) = inp.places.hubs.get(&p.hub) else {
            plan.unassigned.push((p.id.clone(), vec![format!("hub {} is unknown", p.hub)]));
            continue;
        };
        let Some(zone) = inp.places.drop_zones.get(&p.zone) else {
            plan.unassigned.push((p.id.clone(), vec![format!("drop zone {} is unknown", p.zone)]));
            continue;
        };
        if zone.closed {
            plan.unassigned.push((p.id.clone(), vec![format!("drop zone {} is closed", p.zone)]));
            continue;
        }
        if p.class != "normal" && p.class != "fragile" {
            plan.unassigned.push((p.id.clone(), vec![format!("class {} is not carried", p.class)]));
            continue;
        }
        let slack_s = (p.due.0 - inp.t.0) as f64 / 1000.0;
        let mut scored: Vec<(&Candidate, f64, f64, Terms)> = Vec::new();
        let mut rejected: Vec<(Uid, String)> = Vec::new();
        let mut reasons: BTreeSet<String> = BTreeSet::new();
        for v in vehicles.iter().filter(|v| !used.contains(&v.uid) && v.hub == p.hub) {
            let fail = |why: String, rejected: &mut Vec<(Uid, String)>, reasons: &mut BTreeSet<String>| {
                reasons.insert(why.clone());
                rejected.push((v.uid.clone(), why));
            };
            if v.platform.payload_kg.is_none_or(|max| max < p.mass_kg) {
                fail("PAYLOAD_OVER_MAX".into(), &mut rejected, &mut reasons);
                continue;
            }
            let wrong = matches!(
                (v.platform.domain, zone.surface),
                (Domain::Ground, Surface2::Water) | (Domain::Surface, Surface2::Land)
            );
            if wrong {
                fail("DZ_WRONG_SURFACE".into(), &mut rejected, &mut reasons);
                continue;
            }
            let reserve = inp.energy.reserve_pct.get(v.kind);
            let fits = |energy: f64| energy <= inp.params.max_range_frac * 100.0 && v.battery_pct - energy >= reserve;
            let nearest = nearest_back(inp, zone, hub.at());
            let home = if home_base {
                v.home.as_ref().and_then(|h| inp.places.hubs.get(h)).filter(|h| h.open).map(|h| h.at())
            } else {
                None
            };
            let costed = match home {
                // home, unless the battery would not cover the way there: then the hub nearest the
                // zone, where automation would send it
                Some(home_at) => match trip_cost(inp, v, zone, hub.at(), home_at, p.mass_kg) {
                    Ok((energy, eta)) if fits(energy) => Ok((energy, eta)),
                    _ => trip_cost(inp, v, zone, hub.at(), nearest, p.mass_kg),
                },
                None => trip_cost(inp, v, zone, hub.at(), nearest, p.mass_kg),
            };
            match costed {
                Err(why) => fail(why.split(':').next().unwrap_or("ENERGY").to_string(), &mut rejected, &mut reasons),
                Ok((energy, eta)) => {
                    if !fits(energy) {
                        fail("ENERGY_INSUFFICIENT".into(), &mut rejected, &mut reasons);
                        continue;
                    }
                    let late = (eta - slack_s).max(0.0) / 60.0;
                    let terms = Terms { energy_pct: r3(energy), time: r3(eta / 60.0), late: r3(late) };
                    let cost = inp.params.w_energy * energy + inp.params.w_time * eta / 60.0 + inp.params.w_late * late;
                    scored.push((v, eta, cost, terms));
                }
            }
        }
        scored.sort_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.uid.cmp(&b.0.uid)));
        match scored.first() {
            None => {
                let mut r: Vec<String> = reasons.into_iter().collect();
                if r.is_empty() {
                    r.push(format!("no idle vehicle at {}", p.hub));
                }
                plan.unassigned.push((p.id.clone(), r));
            }
            Some((v, eta, cost, terms)) => {
                used.insert(&v.uid);
                plan.assignments.push(Assignment {
                    vehicle: v.uid.clone(),
                    parcel: p.id.clone(),
                    hub: p.hub.clone(),
                    zone: p.zone.clone(),
                    eta_s: r3(*eta),
                    cost: r3(*cost),
                    terms: terms.clone(),
                    beat: scored.iter().skip(1).take(3).map(|(c, _, k, _)| (c.uid.clone(), r3(*k))).collect(),
                    rejected,
                });
            }
        }
    }
    plan
}

/// The vehicles by hub, for the bridge's reading of which ones are free.
pub fn by_hub(vehicles: &[Candidate]) -> BTreeMap<&str, usize> {
    let mut m = BTreeMap::new();
    for v in vehicles {
        *m.entry(v.hub.as_str()).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::{DropZoneKind, Hub};

    fn platform(id: &str, domain: Domain, cruise: f64, payload: f64) -> PlatformFacts {
        let d = match domain {
            Domain::Air => "air",
            Domain::Ground => "ground",
            Domain::Surface => "surface",
        };
        serde_json::from_value(serde_json::json!({
            "id": id, "domain": d, "cruise_mps": cruise, "endurance_h": 0.4, "payload_kg": payload,
            "min_turn_radius_m": null, "autopilot": null
        }))
        .expect("platform")
    }

    fn places() -> Places {
        let mut p = Places::default();
        p.hubs.insert(
            "h".into(),
            Hub { id: "h".into(), name: String::new(), lon: -122.46, lat: 37.80, radius_m: 50.0, pads: 2, packs: 2, open: true },
        );
        // a second base 3.3 km north, farther than "h" from every zone below
        p.hubs.insert(
            "k".into(),
            Hub { id: "k".into(), name: String::new(), lon: -122.46, lat: 37.83, radius_m: 50.0, pads: 2, packs: 2, open: true },
        );
        for (id, lon, lat, surface) in [("near", -122.455, 37.80, Surface2::Land), ("far", -122.40, 37.80, Surface2::Land), ("wet", -122.455, 37.81, Surface2::Water)] {
            p.drop_zones.insert(
                id.into(),
                DropZone {
                    id: id.into(),
                    kind: DropZoneKind::Doorstep,
                    lon,
                    lat,
                    radius_m: 4.0,
                    surface,
                    elevation_m: None,
                    closed: false,
                    populated: false,
                    hub: None,
                    slope_deg: None,
                    dock_level_m: None,
                    approval: None,
                },
            );
        }
        p
    }

    fn input(parcels: Vec<OpenParcel>, vehicles: Vec<Candidate>) -> AssignInput {
        AssignInput {
            t: Time(0),
            params: DispatchParams::default(),
            energy: EnergyKnobs::default(),
            places: places(),
            weather: vec![],
            parcels,
            vehicles,
        }
    }

    fn cand(uid: &str, plat: PlatformFacts, battery: f64) -> Candidate {
        Candidate { uid: Uid::new(uid), hub: "h".into(), platform: plat, kind: AutopilotKind::Copter, battery_pct: battery, home: None }
    }

    /// `c` based at `home` (rules 4).
    fn based(mut c: Candidate, home: &str) -> Candidate {
        c.home = Some(home.into());
        c
    }

    fn rules(version: u32) -> RulesConfig {
        RulesConfig { rules_version: version, ..RulesConfig::default() }
    }

    fn parcel(id: &str, zone: &str, mass: f64, priority: u8, due_s: i64) -> OpenParcel {
        OpenParcel { id: id.into(), hub: "h".into(), zone: zone.into(), mass_kg: mass, class: "normal".into(), priority, due: Time(due_s * 1000) }
    }

    #[test]
    fn the_cheaper_vehicle_wins_and_ties_go_to_the_lower_uid() {
        let d = platform("drone", Domain::Air, 12.0, 2.5);
        let inp = input(vec![parcel("p1", "near", 1.0, 1, 1800)], vec![cand("2", d.clone(), 90.0), cand("1", d, 90.0)]);
        let plan = assign(&inp);
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].vehicle.as_str(), "1");
        assert_eq!(plan.assignments[0].beat.len(), 1);
        assert_eq!(plan, assign(&inp), "deterministic");
    }

    #[test]
    fn priority_then_deadline_then_id_order_the_parcels_and_a_vehicle_is_taken_once() {
        let d = platform("drone", Domain::Air, 12.0, 2.5);
        let inp = input(
            vec![parcel("a", "near", 1.0, 1, 900), parcel("b", "near", 1.0, 3, 3000), parcel("c", "near", 1.0, 3, 1000)],
            vec![cand("1", d.clone(), 90.0), cand("2", d, 90.0)],
        );
        let plan = assign(&inp);
        let got: Vec<(&str, &str)> = plan.assignments.iter().map(|a| (a.parcel.as_str(), a.vehicle.as_str())).collect();
        assert_eq!(got, vec![("c", "1"), ("b", "2")]);
        assert_eq!(plan.unassigned.len(), 1);
        assert_eq!(plan.unassigned[0].0, "a");
    }

    #[test]
    fn a_heavy_parcel_never_goes_to_a_drone_and_hazmat_goes_nowhere() {
        let d = platform("drone", Domain::Air, 12.0, 2.5);
        let car = platform("car", Domain::Ground, 6.0, 40.0);
        let mut c = cand("2", car, 90.0);
        c.kind = AutopilotKind::Ugv;
        let mut inp = input(vec![parcel("p", "near", 5.0, 1, 3000)], vec![cand("1", d, 90.0), c]);
        let plan = assign(&inp);
        assert_eq!(plan.assignments[0].vehicle.as_str(), "2");
        assert!(plan.assignments[0].rejected.iter().any(|(u, w)| u.as_str() == "1" && w == "PAYLOAD_OVER_MAX"));
        inp.parcels[0].class = "hazmat".into();
        assert!(assign(&inp).assignments.is_empty());
    }

    #[test]
    fn a_wet_zone_is_not_for_a_car_and_a_flat_battery_is_not_for_a_far_trip() {
        let car = platform("car", Domain::Ground, 6.0, 40.0);
        let mut c = cand("1", car, 90.0);
        c.kind = AutopilotKind::Ugv;
        let plan = assign(&input(vec![parcel("p", "wet", 1.0, 1, 3000)], vec![c]));
        assert!(plan.assignments.is_empty());
        assert!(plan.unassigned[0].1.contains(&"DZ_WRONG_SURFACE".to_string()));
        let d = platform("drone", Domain::Air, 12.0, 2.5);
        let plan = assign(&input(vec![parcel("p", "far", 1.0, 1, 3000)], vec![cand("1", d, 30.0)]));
        assert!(plan.assignments.is_empty());
        assert!(plan.unassigned[0].1.contains(&"ENERGY_INSUFFICIENT".to_string()));
    }

    #[test]
    fn a_strong_headwind_changes_the_winner() {
        // calm, the long-endurance platform is cheaper; against a 9 m/s headwind it makes 3 m/s
        // good, under 30% of its cruise, so the faster one carries the parcel
        let mut slow = platform("slow", Domain::Air, 12.0, 2.5);
        slow.endurance_h = Some(1.0);
        let fast = platform("fast", Domain::Air, 14.0, 2.5);
        let mut inp = input(vec![parcel("p", "near", 1.0, 1, 100_000)], vec![cand("1", slow, 95.0), cand("2", fast, 95.0)]);
        // a zone due east of the hub: a westerly wind blowing toward the west is a headwind
        inp.weather = vec![WxFact {
            at: LonLat::new(-122.457, 37.80),
            t: Time(0),
            wind_e_mps: 0.0,
            wind_n_mps: 0.0,
            gust_mps: 0.0,
            visibility_m: None,
            precip_mmh: None,
            current_e_mps: None,
            current_n_mps: None,
            tide_m: None,
            depth_m: None,
            ground_m: None,
            source: "t".into(),
        }];
        let calm = assign(&inp);
        assert_eq!(calm.assignments[0].vehicle.as_str(), "1", "{calm:?}");
        inp.weather[0].wind_e_mps = -9.0;
        let windy = assign(&inp);
        assert_eq!(windy.assignments[0].vehicle.as_str(), "2", "{windy:?}");
        assert!(windy.assignments[0].rejected.iter().any(|(u, w)| u.as_str() == "1" && w == "ENERGY_WIND_NO_HEADWAY"));
    }

    #[test]
    fn from_rules_4_the_way_back_goes_home_so_the_vehicle_based_at_the_parcels_hub_wins() {
        // two drones waiting at "h" for a parcel near it: "2" is based there, "1" is visiting from
        // "k". Rules 3 costs both back to the hub nearest the zone, a tie the lower uid takes;
        // rules 4 costs the visitor's way home to "k", and the one based at "h" carries it
        let d = platform("drone", Domain::Air, 12.0, 2.5);
        let inp = input(vec![parcel("p", "near", 1.0, 1, 3000)], vec![based(cand("1", d.clone(), 95.0), "k"), based(cand("2", d, 95.0), "h")]);
        let old = assign_under(&inp, &rules(crate::rules::TRAFFIC_SEPARATION_SINCE));
        assert_eq!(old.assignments[0].vehicle.as_str(), "1", "{old:?}");
        let new = assign_under(&inp, &rules(HOME_BASE_SINCE));
        assert_eq!(new.assignments[0].vehicle.as_str(), "2", "{new:?}");
        assert!(new.assignments[0].beat[0].1 > new.assignments[0].cost, "the visitor costs more: {new:?}");
        assert_eq!(assign(&inp), new, "this build decides under rules 4");
        // with no base named, rules 4 decides as rules 3 did
        let mut none = inp.clone();
        none.vehicles.iter_mut().for_each(|v| v.home = None);
        assert_eq!(assign_under(&none, &rules(HOME_BASE_SINCE)), assign_under(&none, &rules(crate::rules::TRAFFIC_SEPARATION_SINCE)));
    }

    #[test]
    fn the_way_back_is_the_nearest_hub_when_the_battery_would_not_cover_the_way_home() {
        let d = platform("drone", Domain::Air, 12.0, 2.5);
        let mut inp = input(vec![parcel("p", "near", 1.0, 1, 3000)], vec![based(cand("1", d, 95.0), "k")]);
        let to_nearest = assign_under(&inp, &rules(crate::rules::TRAFFIC_SEPARATION_SINCE)).assignments[0].terms.energy_pct;
        let to_home = assign_under(&inp, &rules(HOME_BASE_SINCE)).assignments[0].terms.energy_pct;
        assert!(to_home > to_nearest + 5.0, "home is farther: {to_home} against {to_nearest}");
        // a battery that covers the trip back to "h" but not home to "k"
        let reserve = inp.energy.reserve_pct.get(AutopilotKind::Copter);
        inp.vehicles[0].battery_pct = reserve + (to_nearest + to_home) / 2.0;
        let plan = assign_under(&inp, &rules(HOME_BASE_SINCE));
        assert_eq!(plan.assignments.len(), 1, "{plan:?}");
        assert_eq!(plan.assignments[0].terms.energy_pct, to_nearest, "costed back to the nearest hub");
    }

    #[test]
    fn a_candidate_with_no_base_serializes_as_before_rules_4() {
        let d = platform("drone", Domain::Air, 12.0, 2.5);
        let v = serde_json::to_value(cand("1", d.clone(), 90.0)).expect("json");
        assert!(v.get("home").is_none(), "{v}");
        let v = serde_json::to_value(based(cand("1", d, 90.0), "h")).expect("json");
        assert_eq!(v["home"], "h");
        let back: Candidate = serde_json::from_value(v).expect("round trip");
        assert_eq!(back.home.as_deref(), Some("h"));
    }
}
