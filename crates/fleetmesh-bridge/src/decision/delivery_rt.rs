//! The delivery runtime: what makes deliveries happen in a run (docs/design/dispatch.md).
//!
//! A scenario (`FLEETMESH_SCENARIO_DIR`: `scenario.toml`, `dispatch.toml`, `events.toml`) gives the
//! run its parcels and its scripted conditions. Once a second [`Decision::delivery_step`]
//!
//! 1. releases the parcels whose time has come, and feeds the rules what the bridge does not
//!    measure itself: the weather at the hubs, the drop zones and the vehicles (the weather crate's
//!    archive and scripted gusts, tidewater's tide and current; each sample is a [`WxFact`] and
//!    the decisions log it by value), a drop zone's observation (scripted, or seeded per zone),
//!    and the link predicted along a vehicle's trip, main and backup;
//! 2. follows the orders it has proposed (a pickup, a deliver, a return to the hub, a battery
//!    swap) and moves the parcels and the vehicles on: loaded, delivered, free again;
//! 3. runs the greedy assigner ([`fleetmesh_decide::dispatch::assign_under`]) every `tick_s` and
//!    proposes the first order of each assignment as the agent `dispatcher`, through the same port
//!    as any agent's proposal: policy routes it, a person decides what policy holds. From rules 4
//!    ([`fleetmesh_decide::HOME_BASE_SINCE`], docs/design/fleet-organisation.md) every vehicle has
//!    a home base: the assigner costs the way back to it, a vehicle goes home after a delivery
//!    ([`Decision::return_target`]), and one idle at another base goes home
//!    ([`Decision::send_visitors_home`]);
//! 4. watches every vehicle's navigation trust (`nav-monitor`): a fall to untrusted proposes the
//!    safe order for its kind (delivery-rules.md section 9), through the engine.
//!
//! Management by exception (rules 2, docs/DECISIONS.md 2026-11-25): routine deliveries need no
//! person, and automation waits out what real operations automate before anyone is asked.
//!
//! - **Approved delivery points.** A parcel for a zone that is not an approved delivery point (no
//!   standing approval, none by a person) is not assigned; the dispatcher asks a person once per
//!   zone (`confirm_drop_zone`), and the parcels go when it is approved. Under the strict
//!   procedure (`standing_approvals = false`) a delivery waits for a person's confirmation of its
//!   zone as in rules 1, asked once per zone while one is pending.
//! - **The look before departure.** A loaded vehicle whose drop zone does not look clear waits at
//!   the hub and looks again; after `[automation] depart_wait_s` the parcel stays at the hub for a
//!   later try and the person is told (an issue alert).
//! - **The drop-time look.** Over the zone the vehicle looks again before it lets go (the task
//!   layer's [`crate::tasks::DropLook`]); a zone that stays not clear for `hover_wait_s` ends the
//!   delivery with the parcel aboard, and the dispatcher brings it home and tells the person.
//! - **Holds automation waits out** ([`WAIT_OUT`]): another vehicle near the hub or the route, a
//!   stale fact, a failsafe not read yet. The dispatcher dry-runs each order first and, when its
//!   only holds are these, waits and tries again instead of sending it to the inbox; after
//!   `hold_retry_max_s` it sends it (the hold is then an issue). A hub with no charged pack is
//!   waited out the same way. Limits (weather, energy, navigation accuracy, link gaps, geofences,
//!   payload) are never waited out: they go to a person as before.
//!
//! Nothing here approves anything. Every fact it supplies is part of a decision's logged inputs
//! or of a `delivery.event` or `dispatch.plan` of the audit log.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fleetmesh_decide::catalog::AutopilotKind;
use fleetmesh_decide::delivery::{
    DzObservation, LinkAlong, NavTrust, ObservedState, ParcelFact, ParcelState, WxFact,
};
use fleetmesh_decide::dispatch::{AssignInput, Candidate, DispatchParams, OpenParcel};
use fleetmesh_decide::{AgentId, IntentId, LonLat, State, Time, Uid as DUid};
use serde::Deserialize;
use fleetmesh_decide::Clock as _;
use serde_json::{Value, json};

use super::snapshot::bridge_uid;
use super::{Decision, IntentRequest};
use crate::tasks::{SiteParam, TaskParams};

/// The dispatcher's and the navigation monitor's agent ids on the roster.
pub const DISPATCHER: &str = "dispatcher";
pub const NAV_MONITOR: &str = "nav-monitor";

// ---- the scenario's files ---------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ScenarioToml {
    name: String,
    title: String,
    seed: u64,
    duration_s: f64,
    dispatch: String,
    events: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ParcelToml {
    id: String,
    from: String,
    to: String,
    mass_kg: f64,
    #[serde(default)]
    ready_s: f64,
    /// Seconds after it is ready.
    #[serde(default = "default_due")]
    due_s: f64,
    #[serde(default = "default_priority")]
    priority: u8,
    #[serde(default = "default_class")]
    class: String,
}

fn default_due() -> f64 {
    1800.0
}
fn default_priority() -> u8 {
    1
}
fn default_class() -> String {
    "normal".into()
}

#[derive(Debug, Clone, Deserialize)]
struct DemandToml {
    rate_per_min: f64,
    #[serde(default)]
    from_s: f64,
    /// Until this scenario second (default: the scenario's duration).
    #[serde(default)]
    until_s: Option<f64>,
    hubs: Vec<String>,
    zones: Vec<String>,
    /// A hub listed here draws its parcels' zones from its own list (a car hub's kerbs, a boat
    /// dock's docks) instead of `zones`.
    #[serde(default)]
    zones_by_hub: BTreeMap<String, Vec<String>>,
    #[serde(default = "default_mass")]
    mass_kg: [f64; 2],
    /// A hub listed here draws its parcels' masses from its own range (a drone base takes what its
    /// drones carry, a car kerb or a boat dock heavier ones) instead of `mass_kg`.
    #[serde(default)]
    mass_by_hub: BTreeMap<String, [f64; 2]>,
    #[serde(default = "default_mix")]
    priority_mix: [f64; 4],
    #[serde(default = "default_due")]
    due_s: f64,
    /// A hub listed here gives its parcels this long to their due time instead of `due_s` (a
    /// boat's cross-bay run takes most of an hour).
    #[serde(default)]
    due_by_hub: BTreeMap<String, f64>,
}

fn default_mass() -> [f64; 2] {
    [0.2, 2.0]
}
fn default_mix() -> [f64; 4] {
    [0.1, 0.6, 0.25, 0.05]
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ObsToml {
    /// The share of a kind of zone that is busy (people there in turns), by kind.
    busy: BTreeMap<String, f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DispatchToml {
    dispatch: Option<DispatchParams>,
    parcel: Vec<ParcelToml>,
    demand: Option<DemandToml>,
    observations: ObsToml,
    automation: Automation,
    site: Vec<Site>,
}

/// A site: bases (hubs) that work together at one place, as a Marina's drone pads and its car kerb
/// (`[[site]]` of a scenario's dispatch file; docs/design/fleet-organisation.md). The console groups
/// the bases under it; nothing the rules or the dispatcher read.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    pub id: String,
    pub name: String,
    pub hubs: Vec<String>,
}

/// What automation does on its own before a person is told (`[automation]` of a scenario's
/// dispatch file). The bridge's, not a rule's: no decision reads it, and what it does is logged as
/// `delivery.event`s and proposals.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Automation {
    /// Loaded at the hub: how long a vehicle waits for its drop zone to look clear before the
    /// parcel stays at the hub for a later try (and the person is told), s.
    pub depart_wait_s: f64,
    /// Over the drop zone: how long a vehicle waits for a clear look before it keeps the parcel
    /// and comes home (and the person is told), s.
    pub hover_wait_s: f64,
    /// A parcel that came back (or stayed) undelivered is tried again after this, s.
    pub retry_after_s: f64,
    /// A parcel is tried this many times before it is given up (failed).
    pub max_tries: u32,
    /// A dispatcher's order whose only holds automation waits out is tried again after this, s.
    pub hold_retry_s: f64,
    /// ... and sent to the person after this much waiting (the hold is then an issue), s.
    pub hold_retry_max_s: f64,
    /// How long an issue alert stays in the inbox after automation raised it, s.
    pub issue_show_s: f64,
    /// Rules 3: an order that has waited this long for other traffic (a conflict the separation
    /// rule predicts) is offered a route round it, s; after `hold_retry_max_s` the person is told
    /// (an issue alert) and automation goes on waiting, never sending the order to the inbox.
    pub traffic_replan_after_s: f64,
}

impl Default for Automation {
    fn default() -> Self {
        Automation {
            depart_wait_s: 120.0,
            // longer than a busy doorstep's turn (the scenarios' 90 s): a passer-by waits out,
            // a crowd that stays does not
            hover_wait_s: 120.0,
            retry_after_s: 600.0,
            max_tries: 2,
            hold_retry_s: 15.0,
            hold_retry_max_s: 300.0,
            issue_show_s: 300.0,
            traffic_replan_after_s: 30.0,
        }
    }
}

/// Hold reasons automation waits out and tries again rather than ask a person (real operations
/// sequence departures and re-read a stale fact): a drop zone that does not look clear yet,
/// another vehicle near the hub or the route, and a fact that is stale or not read yet. Never a
/// limit: weather, energy, navigation accuracy, link gaps, geofences, payload and the drop zone's
/// approval stay a person's.
pub const WAIT_OUT: &[&str] = &[
    "DZ_PEOPLE_NEAR",
    "DZ_OBSERVATION_STALE",
    "SEP_HUB_ZONE",
    "SEP_MARGINAL",
    "SEP_TRAFFIC_UNKNOWN",
    "ENERGY_BATTERY_UNKNOWN",
    "WX_UNKNOWN",
    "NAV_TRUST_UNKNOWN",
    "LINK_PLAN_UNKNOWN",
    "INPUT_UNKNOWN",
    // rules 3: other traffic (TRAFFIC_CODES): waited out on the ground, held or re-planned in flight
    "SEP_TRAFFIC_CONFLICT",
    "SEP_TRAFFIC_NONCOOP",
    "SEP_SHIP_CPA",
    "SEP_SHIP_AHEAD",
    "SEP_PICTURE_DEGRADED",
];

/// Rules 3: the holds for other traffic. Automation waits them out (a vehicle on the ground stays
/// there, one under way holds), offers a route round a conflict after `traffic_replan_after_s`,
/// and never sends the order to the inbox for them: a conflict that outlasts `hold_retry_max_s` is
/// an issue alert (`traffic_conflict`), a picture that no longer covers a route another
/// (`traffic_picture_degraded`).
pub const TRAFFIC_CODES: &[&str] = &["SEP_TRAFFIC_CONFLICT", "SEP_TRAFFIC_NONCOOP", "SEP_SHIP_CPA", "SEP_SHIP_AHEAD", "SEP_PICTURE_DEGRADED"];

/// Rules 5 ([`fleetmesh_decide::HUB_SEQUENCING_SINCE`]): the meetings of the fleet's own vehicles
/// automation sequences, as a hub or a vertiport does its arrivals and departures: one waits until
/// the other is clear. Waited out and never sent to a person for; a wait that outlasts
/// `hold_retry_max_s` is an issue alert (`fleet_conflict`), and automation goes on waiting.
pub const SEQUENCE_CODES: &[&str] = &["SEP_HUB_ZONE", "SEP_MARGINAL"];

/// Rules 5: the refusal that only says another of the fleet's vehicles' routes is in the way
/// (its own fix is a short delay): a dispatcher's order it would refuse waits, as a hold does.
const ROUTE_CONFLICT: &str = "SEP_ROUTE_CONFLICT";

/// Whether `codes` hold for other traffic.
fn for_traffic(codes: &[String]) -> bool {
    codes.iter().any(|c| TRAFFIC_CODES.contains(&c.as_str()))
}

/// The holds that are the drop zone's look (waited out at the hub up to `depart_wait_s`).
const LOOK_CODES: &[&str] = &["DZ_PEOPLE_NEAR", "DZ_OBSERVATION_STALE"];

/// The holds a person's approval of the zone clears (rules 2's `DZ_NOT_APPROVED`, the strict
/// procedure's `DZ_UNCONFIRMED`).
const APPROVAL_CODES: &[&str] = &["DZ_NOT_APPROVED", "DZ_UNCONFIRMED"];

#[derive(Debug, Clone, Deserialize)]
struct ZoneEventToml {
    zone: String,
    at_s: f64,
    duration_s: f64,
    #[serde(default = "default_count")]
    count: u32,
}
fn default_count() -> u32 {
    3
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WeatherToml {
    wind_east_mps: Option<f64>,
    wind_north_mps: Option<f64>,
    visibility_m: Option<f64>,
    archive: Option<String>,
    timeline: Vec<fleetmesh_weather::Frame>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EventsToml {
    weather: WeatherToml,
    dropzone_event: Vec<ZoneEventToml>,
}

/// A parcel of the scenario.
#[derive(Debug, Clone)]
pub struct ParcelSpec {
    pub id: String,
    pub from: String,
    pub to: String,
    pub mass_kg: f64,
    pub ready_s: f64,
    pub due_s: f64,
    pub priority: u8,
    pub class: String,
}

/// The scenario as the bridge runs it.
pub struct Scenario {
    pub name: String,
    pub title: String,
    pub seed: u64,
    pub duration_s: f64,
    pub params: DispatchParams,
    pub parcels: Vec<ParcelSpec>,
    busy: BTreeMap<String, f64>,
    zone_events: Vec<ZoneEventToml>,
    wx: WeatherToml,
    store: fleetmesh_weather::Store,
    pub automation: Automation,
    /// The sites its bases are grouped under (display only).
    pub sites: Vec<Site>,
}

/// SplitMix64: a small seeded generator whose stream is the same on every host.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn hash_str(seed: u64, s: &str) -> u64 {
    let mut r = Rng(seed ^ 0xA5A5_5A5A_1234_5678);
    for b in s.bytes() {
        r.0 ^= u64::from(b);
        r.next();
    }
    r.next()
}

impl Scenario {
    /// Load `dir` (`FLEETMESH_SCENARIO_DIR`); `scale` multiplies the demand's rate
    /// (`FLEETMESH_SCENARIO_SCALE`), as the fleet's scale multiplies its vehicles.
    pub fn load(dir: &Path, root: &Path, scale: f64) -> Result<Scenario, String> {
        let read = |name: &str| -> Result<String, String> {
            std::fs::read_to_string(dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        let sc: ScenarioToml = toml::from_str(&read("scenario.toml")?).map_err(|e| format!("scenario.toml: {e}"))?;
        let dname = if sc.dispatch.is_empty() { "dispatch.toml".to_string() } else { sc.dispatch.clone() };
        let d: DispatchToml = toml::from_str(&read(&dname)?).map_err(|e| format!("{dname}: {e}"))?;
        let ename = if sc.events.is_empty() { "events.toml".to_string() } else { sc.events.clone() };
        let ev: EventsToml = match read(&ename) {
            Ok(t) => toml::from_str(&t).map_err(|e| format!("{ename}: {e}"))?,
            Err(_) => EventsToml::default(),
        };
        let duration_s = if sc.duration_s > 0.0 { sc.duration_s } else { 3600.0 };
        let mut parcels: Vec<ParcelSpec> = d
            .parcel
            .iter()
            .map(|p| ParcelSpec {
                id: p.id.clone(),
                from: p.from.clone(),
                to: p.to.clone(),
                mass_kg: p.mass_kg,
                ready_s: p.ready_s,
                due_s: p.ready_s + p.due_s,
                priority: p.priority.min(3),
                class: p.class.clone(),
            })
            .collect();
        if let Some(g) = &d.demand {
            let mut rng = Rng(hash_str(sc.seed, "demand"));
            let rate = (g.rate_per_min * scale / 60.0).max(1e-9);
            let until = g.until_s.unwrap_or(duration_s);
            let mut t = g.from_s;
            let mut n = 0;
            loop {
                t += -(1.0 - rng.unit()).ln() / rate;
                if t >= until || g.hubs.is_empty() || g.zones.is_empty() {
                    break;
                }
                n += 1;
                let hub = g.hubs[(rng.next() % g.hubs.len() as u64) as usize].clone();
                let zones = g.zones_by_hub.get(&hub).filter(|z| !z.is_empty()).unwrap_or(&g.zones);
                let zone = zones[(rng.next() % zones.len() as u64) as usize].clone();
                let range = g.mass_by_hub.get(&hub).copied().unwrap_or(g.mass_kg);
                let due = g.due_by_hub.get(&hub).copied().unwrap_or(g.due_s);
                let mass = range[0] + rng.unit() * (range[1] - range[0]);
                let u = rng.unit();
                let mut acc = 0.0;
                let mut priority = 0u8;
                for (i, w) in g.priority_mix.iter().enumerate() {
                    acc += w;
                    if u <= acc {
                        priority = i as u8;
                        break;
                    }
                    priority = i as u8;
                }
                parcels.push(ParcelSpec {
                    id: format!("p-{n:04}"),
                    from: hub,
                    to: zone,
                    mass_kg: (mass * 10.0).round() / 10.0,
                    ready_s: t.round(),
                    due_s: t.round() + due,
                    priority,
                    class: "normal".into(),
                });
            }
        }
        parcels.sort_by(|a, b| a.ready_s.total_cmp(&b.ready_s).then_with(|| a.id.cmp(&b.id)));
        let mut store = fleetmesh_weather::Store::default();
        let use_archive = std::env::var("FLEETMESH_WEATHER_ARCHIVE").map_or(true, |v| v != "off");
        let archive = ev.weather.archive.clone().filter(|a| use_archive && root.join(a).exists());
        let cfg = fleetmesh_weather::Config { archive, archive_sha256: None, timeline: ev.weather.timeline.clone() };
        store.configure(Some(&cfg), Some(root));
        Ok(Scenario {
            name: if sc.name.is_empty() { dir.file_name().map_or("scenario".into(), |n| n.to_string_lossy().into()) } else { sc.name },
            title: sc.title,
            seed: sc.seed,
            duration_s,
            params: d.dispatch.unwrap_or_default(),
            parcels,
            busy: d.observations.busy,
            zone_events: ev.dropzone_event,
            wx: ev.weather,
            store,
            automation: d.automation,
            sites: d.site,
        })
    }

    /// What a look at a drop zone `elapsed_s` into the run sees: a scripted crowd, else the zone's
    /// own seeded pattern (some zones are busy in turns of 90 s out of 300).
    pub fn observe(&self, zone: &str, kind: &str, elapsed_s: f64) -> (ObservedState, u32) {
        if let Some(e) = self.zone_events.iter().find(|e| e.zone == zone && elapsed_s >= e.at_s && elapsed_s < e.at_s + e.duration_s) {
            return (ObservedState::Occupied, e.count);
        }
        let p = self.busy.get(kind).copied().unwrap_or(0.0);
        let h = hash_str(self.seed, zone);
        let is_busy = (h % 1000) as f64 / 1000.0 < p;
        if is_busy {
            let phase = (h >> 16) % 300;
            if ((elapsed_s as u64 + phase) % 300) < 90 {
                return (ObservedState::Occupied, 1 + (h % 3) as u32);
            }
        }
        (ObservedState::Clear, 0)
    }

    /// The weather at a point `elapsed_s` into the run (scenario time `t_unix`): the archive's
    /// mean wind where it covers the point, else the scenario's constant wind, over which the
    /// scripted gust ramp lies. None where nothing says what the wind is.
    pub fn weather_at(&self, at: LonLat, now: Time, elapsed_s: f64, t_unix: f64) -> Option<WxFact> {
        let s = self.store.at(at.lat, at.lon, t_unix, elapsed_s);
        let v = s.values;
        let (e, n) = match (v.wind_east_mps, v.wind_north_mps, self.wx.wind_east_mps, self.wx.wind_north_mps) {
            (Some(e), Some(n), _, _) => (e, n),
            (_, _, Some(e), Some(n)) => (e, n),
            _ => return None,
        };
        let source = if s.synthetic {
            "scripted scenario weather".to_string()
        } else if v.wind_east_mps.is_some() {
            s.source.clone()
        } else {
            "scenario baseline".to_string()
        };
        let mean = e.hypot(n);
        Some(WxFact {
            at,
            t: now,
            wind_e_mps: e,
            wind_n_mps: n,
            gust_mps: v.gust_mps.unwrap_or(mean).max(mean),
            visibility_m: v.visibility_m.or(self.wx.visibility_m),
            precip_mmh: v.precipitation_mm,
            current_e_mps: None,
            current_n_mps: None,
            tide_m: None,
            depth_m: None,
            ground_m: None,
            source,
        })
    }
}

// ---- parcels and jobs -------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PState {
    Waiting,
    Ready,
    Assigned,
    Loaded,
    Delivered,
    Failed,
}

impl PState {
    pub fn name(self) -> &'static str {
        match self {
            PState::Waiting => "waiting",
            PState::Ready => "ready",
            PState::Assigned => "assigned",
            PState::Loaded => "loaded",
            PState::Delivered => "delivered",
            PState::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParcelRec {
    pub spec: ParcelSpec,
    pub state: PState,
    pub vehicle: Option<DUid>,
    pub assigned_ms: Option<i64>,
    pub loaded_ms: Option<i64>,
    pub delivered_ms: Option<i64>,
    pub tries: u32,
    /// Times automation kept it undelivered (its zone did not look clear) and brought it back.
    pub returns: u32,
    /// Not assigned before this (a parcel that came back waits before it is tried again), ms.
    pub not_before_ms: i64,
    /// How each attempt that fell short ended, oldest first (at most [`ENDS_KEPT`]).
    pub ends: Vec<TryEnd>,
    /// Why it failed: the end that made automation give it up (none while it has not failed).
    pub failure: Option<TryEnd>,
}

impl ParcelRec {
    fn new(spec: ParcelSpec) -> ParcelRec {
        ParcelRec {
            spec,
            state: PState::Waiting,
            vehicle: None,
            assigned_ms: None,
            loaded_ms: None,
            delivered_ms: None,
            tries: 0,
            returns: 0,
            not_before_ms: 0,
            ends: Vec::new(),
            failure: None,
        }
    }

    /// Keep how an attempt ended (the oldest dropped beyond [`ENDS_KEPT`]).
    fn ended(&mut self, end: TryEnd) {
        self.ends.push(end);
        if self.ends.len() > ENDS_KEPT {
            self.ends.remove(0);
        }
    }

    /// Failed, for `end` (also kept as the last attempt's end).
    fn fail(&mut self, end: TryEnd) {
        self.state = PState::Failed;
        self.failure = Some(end.clone());
        if self.ends.last() != Some(&end) {
            self.ended(end);
        }
    }
}

/// How many attempts' ends a parcel keeps for `GET /api/parcels`.
const ENDS_KEPT: usize = 6;

/// How one attempt at a parcel fell short, as automation saw it: which stage, how the order ended,
/// and why (the refusing or holding checks' codes and words, or the task layer's failure). What
/// `GET /api/parcels` shows for a failed parcel (`failure`) and for its earlier tries (`ends`), and
/// what the `parcel` delivery event of its failure carries. Display and diagnostics only: no
/// decision reads it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TryEnd {
    /// `pickup`, `deliver`, `look` (the look before departure or over the zone kept it) or `return`
    pub stage: &'static str,
    /// how it ended: `rejected` (a check refused it), `failed` (the vehicle or the task layer),
    /// `expired` (nobody decided in time), `voided`, `aborted`, or `kept` (the look kept it aboard
    /// or at the hub)
    pub ended: &'static str,
    /// the reason codes of the checks that refused or held it (none for a task-layer failure)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codes: Vec<String>,
    /// in words: the refusing checks' details, or the failed step's
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<u64>,
    /// seconds into the scenario
    pub at_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Pickup,
    Deliver,
    Return,
    Swap,
}

#[derive(Clone, Debug)]
struct Job {
    parcel: Option<String>,
    stage: Stage,
    intent: Option<IntentId>,
    /// A `confirm_drop_zone` proposed for this job's held deliver.
    confirm: Option<IntentId>,
    proposed_ms: i64,
    /// Not before this time (a pause after a failure).
    not_before_ms: i64,
    /// Since when automation has been waiting out this job's holds (none: not waiting), ms, and
    /// the codes it waits on.
    waiting_since_ms: Option<i64>,
    waiting_on: Vec<String>,
    /// Its holds were waited out as long as automation waits: the next proposal goes as it is.
    escalated: bool,
    /// Rules 3: the route round other traffic the next deliver takes (its waypoints).
    via: Vec<LonLat>,
    /// Rules 3: tried a route round other traffic during this wait.
    replanned: bool,
}

impl Job {
    fn new(parcel: Option<String>, stage: Stage, intent: Option<IntentId>, now_ms: i64) -> Job {
        Job {
            parcel,
            stage,
            intent,
            confirm: None,
            proposed_ms: now_ms,
            not_before_ms: 0,
            waiting_since_ms: None,
            waiting_on: Vec::new(),
            escalated: false,
            via: Vec::new(),
            replanned: false,
        }
    }
}

/// A person asked to approve a zone as a delivery point (rules 2: once per zone).
#[derive(Clone, Debug)]
struct ZoneAsk {
    intent: Option<IntentId>,
}

/// An issue automation raised for the person: what happened, what it did, what the person might
/// do. Shown in the inbox as an alert (never an approval) for `issue_show_s`, or while its cause
/// lasts.
#[derive(Clone, Debug)]
pub struct Issue {
    pub kind: &'static str,
    pub uid: Option<DUid>,
    pub problem: String,
    pub did: String,
    pub recommend: String,
    pub at_wall_s: f64,
    /// shown until this time on the decision clock (none: while its cause lasts)
    pub until_ms: Option<i64>,
    pub intent: Option<IntentId>,
    /// Rules 3: the traffic tracks it is about (the broadcast first, then what the sensors see)
    pub track_ids: Vec<String>,
}

/// Everything the runtime keeps between steps.
#[derive(Default)]
pub struct Runtime {
    pub scenario: Option<Scenario>,
    pub parcels: BTreeMap<String, ParcelRec>,
    t0: Option<Time>,
    jobs: BTreeMap<DUid, Job>,
    last_plan_ms: i64,
    last_feed_ms: i64,
    last_obs: BTreeMap<String, ObservedState>,
    plan_tick: u64,
    last_unassigned: BTreeSet<String>,
    proposals: Vec<i64>,
    /// The swaps whose pack was taken (by intent), and the packs coming back: (ready ms, hub).
    swaps_taken: BTreeSet<IntentId>,
    charging: Vec<(i64, String)>,
    /// Each vehicle's navigation trust as last seen, and since when it has been better.
    pub nav: BTreeMap<DUid, NavSeen>,
    /// What the navigation monitor did for a vehicle that fell to untrusted (an alert while it stays so).
    nav_proposed: BTreeMap<DUid, NavAction>,
    pub delivered: u32,
    /// A vehicle whose pickup was refused or failed waits before it is offered another parcel.
    cooldown: BTreeMap<DUid, i64>,
    /// Rules 2: the zones a person was asked to approve as delivery points, once each.
    zone_asks: BTreeMap<String, ZoneAsk>,
    /// The issues automation raised for the person, by id.
    pub issues: BTreeMap<String, Issue>,
    /// What the dispatcher last said it waits on for a vehicle (logged when it changes).
    waits_said: BTreeMap<DUid, String>,
    /// Returns to a hub that ended short of it in a row, by vehicle (a failed upload is tried again).
    return_failures: BTreeMap<DUid, u32>,
    /// Rules 3: the vehicles automation is keeping clear of other traffic (on the ground, or held
    /// under way), and what for.
    pub traffic_waits: BTreeMap<DUid, TrafficWait>,
    /// Rules 3: when the traffic monitor last looked.
    last_traffic_ms: i64,
    /// Rules 3: where each non-cooperative aircraft's issue last saw it, and when (ms).
    noncoop_seen: BTreeMap<String, (LonLat, i64)>,
    /// Rules 3: when each non-cooperative track was first near a drone (ms).
    noncoop_first: BTreeMap<String, i64>,
    /// Each vehicle's state as the fleet view last gave it (`super::fleet`), and since when (ms).
    pub(super) fleet_since: BTreeMap<DUid, (&'static str, i64)>,
    /// Rules 4: since when a vehicle has been idle at a hub other than its home base (ms).
    visiting_since: BTreeMap<DUid, i64>,
    /// Since when each parcel no vehicle of its site can carry within the payload margin has
    /// waited (ms), and the ones whose wait was logged.
    short_since: BTreeMap<String, i64>,
    short_said: BTreeSet<String>,
}

/// Rules 3: a vehicle automation keeps clear of other traffic.
#[derive(Clone, Debug)]
pub struct TrafficWait {
    pub since_ms: i64,
    /// the holding checks' words
    pub why: String,
    pub codes: Vec<String>,
    /// what automation did, in words (waits at the hub, holds where it is, a route round it)
    pub did: String,
    /// under way (held where it is) rather than on the ground
    pub under_way: bool,
    pub parcel: Option<String>,
    /// shown until then (a pickup that waits has no job to follow), ms
    pub until_ms: Option<i64>,
}

/// A return to a hub that fails (a mission upload with no answer, a link that drops mid-exchange)
/// is proposed again this many times before the person is told.
const RETURN_TRIES: u32 = 3;

/// Rules 5: a visitor pad lies at least this far from every stand, every vehicle on the ground and
/// every return bound for the hub, m.
const PAD_CLEAR_M: f64 = 12.0;

/// A parcel that no vehicle of its site carries within the payload margin for this long, s, is
/// an issue for the person (once per parcel); before, it waits with no one asked.
const NO_ROOM_ISSUE_S: f64 = 600.0;

/// A vehicle at a hub with its battery under this, percent, gets a battery swap before its next
/// parcel (and the fleet view counts it as charging).
pub const SWAP_BELOW_PCT: f64 = 45.0;

/// Rules 4: a vehicle idle this long at a hub other than its home base, with no parcel waiting
/// there, is sent home, s (Zipline's software sends each aircraft to the dock that suits it).
pub const REBASE_AFTER_S: i64 = 60;

/// Rules 4: at most this many vehicles are sent home in one plan tick.
const REBASE_PER_TICK: usize = 4;

impl Runtime {
    pub fn new(scenario: Option<Scenario>) -> Runtime {
        Runtime { scenario, ..Default::default() }
    }
}

/// An order the navigation monitor proposed for a vehicle whose navigation fell to untrusted.
#[derive(Clone, Debug)]
pub struct NavAction {
    pub intent: IntentId,
    /// the order proposed: `land_now` or `hold`
    pub task: &'static str,
    /// the bridge's wall clock, s, when it acted
    pub at_wall_s: f64,
    /// the causes the navigation rule gave, one line
    pub why: String,
}

#[derive(Clone, Debug)]
pub struct NavSeen {
    pub state: NavTrust,
    /// Seconds the better state's conditions have held (for the hysteresis).
    pub good_since_ms: Option<i64>,
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default()
}

fn terminal(s: State) -> bool {
    matches!(s, State::Done | State::Failed | State::Rejected | State::Expired | State::Voided | State::Aborted)
}

impl Decision {
    /// Load the scenario named by `FLEETMESH_SCENARIO_DIR` (None without one).
    pub(super) fn load_scenario(root: &Path) -> Result<Option<Scenario>, String> {
        let Ok(dir) = std::env::var("FLEETMESH_SCENARIO_DIR") else { return Ok(None) };
        let dir = PathBuf::from(dir);
        let dir = if dir.is_absolute() { dir } else { root.join(dir) };
        let scale = std::env::var("FLEETMESH_SCENARIO_SCALE").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
        Scenario::load(&dir, root, scale).map(Some)
    }

    fn rt(&self) -> std::sync::MutexGuard<'_, Runtime> {
        self.runtime.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Seconds into the run, once it has begun.
    fn elapsed(&self, rt: &Runtime, now: Time) -> f64 {
        rt.t0.map_or(0.0, |t0| now.ms_since(t0) as f64 / 1000.0)
    }

    fn log_delivery(&self, kind: &str, uid: Option<&DUid>, parcel: Option<&str>, detail: &str, data: Value) {
        let mut core = self.lock();
        if let Err(e) = core.engine.record_delivery_event(kind, uid, parcel, detail, data) {
            eprintln!("delivery: log {kind}: {e}");
        }
    }

    /// One second of the delivery runtime. Never approves anything: it feeds, follows, assigns
    /// and proposes.
    pub async fn delivery_step(&self) {
        let now = self.clock.now();
        {
            let mut rt = self.rt();
            if rt.t0.is_none() {
                rt.t0 = Some(now);
                if rt.scenario.is_none() && self.places.is_empty() {
                    return;
                }
                let specs: Vec<ParcelSpec> = rt.scenario.as_ref().map(|s| s.parcels.clone()).unwrap_or_default();
                for p in specs {
                    rt.parcels.insert(p.id.clone(), ParcelRec::new(p));
                }
            }
        }
        self.release_parcels(now);
        self.refresh_routes();
        self.refresh_feed(now).await;
        self.follow_jobs(now).await;
        self.watch_navigation(now).await;
        // rules 3: other traffic (the monitor under way, and the picture's own issues)
        self.watch_traffic(now).await;
        self.recharge(now);
        self.dispatch_parcels(now).await;
        self.publish_parcels();
    }

    fn publish_parcels(&self) {
        // the parcels the engine's rules read, as the feed gives them
        let rt = self.rt();
        let mut feed = self.feed();
        feed.parcels = rt
            .parcels
            .iter()
            .filter_map(|(id, p)| {
                let state = match p.state {
                    PState::Waiting | PState::Failed => return None,
                    PState::Ready | PState::Assigned => ParcelState::Ready,
                    PState::Loaded => ParcelState::Loaded,
                    PState::Delivered => ParcelState::Released,
                };
                Some((
                    id.clone(),
                    ParcelFact {
                        mass_kg: Some(p.spec.mass_kg),
                        class: p.spec.class.clone(),
                        state,
                        vehicle: p.vehicle.clone(),
                    },
                ))
            })
            .collect();
    }

    fn release_parcels(&self, now: Time) {
        let mut events = Vec::new();
        {
            let mut rt = self.rt();
            let el = self.elapsed(&rt, now);
            for p in rt.parcels.values_mut() {
                if p.state == PState::Waiting && p.spec.ready_s <= el {
                    p.state = PState::Ready;
                    events.push((p.spec.id.clone(), format!("parcel {} ready at {}: to {} ({} kg, priority {})", p.spec.id, p.spec.from, p.spec.to, p.spec.mass_kg, p.spec.priority)));
                }
            }
        }
        for (id, d) in events {
            self.log_delivery("parcel", None, Some(&id), &d, json!({ "state": "ready" }));
        }
    }

    // ---- the feed ---------------------------------------------------------------------------

    /// Rules 5: each vehicle's dispatched route (a pickup that flies, a delivery, a return, a
    /// reroute, a transit), for the separation rule's prediction of the fleet's own vehicles.
    fn refresh_routes(&self) {
        if !self.rules.at_least(fleetmesh_decide::HUB_SEQUENCING_SINCE) {
            return;
        }
        use fleetmesh_decide::TaskKind as K;
        let routes: BTreeMap<DUid, Vec<LonLat>> = {
            let core = self.lock();
            core.engine
                .intents()
                .filter(|i| i.state() == State::Dispatched && matches!(i.spec().kind(), K::Pickup | K::Deliver | K::ReturnToHub | K::Reroute | K::Transit))
                .filter_map(|i| Some((i.spec().uids.clone(), i.spec().task.path()?.points)))
                .flat_map(|(uids, pts)| uids.into_iter().map(move |u| (u, pts.clone())))
                .collect()
        };
        self.feed().routes = routes;
    }

    /// Rules 5: where at `hub` vehicle `uid` parks. At its home base, its own stand (where it was
    /// launched, when that lies within the hub's radius), as a nest's drone has its own pad and a
    /// depot's car its own bay; at another hub (or with no stand of its own), the first free
    /// visitor pad: the hub's point, else a ring of pads round it, each clear of every vehicle's
    /// stand there, of every vehicle on the ground there and of every return already bound
    /// there. `None`: the hub's own point (no free pad, or before rules 5).
    pub(super) fn stand_at(&self, uid: &DUid, hub: &str) -> Option<LonLat> {
        if !self.rules.at_least(fleetmesh_decide::HUB_SEQUENCING_SINCE) {
            return None;
        }
        let h = self.places.hubs.get(hub)?;
        // (its home first: home_hub reads the manifest too, and its lock is not re-entrant)
        let home = self.home_hub(uid, None);
        let manifest = self.world.manifest();
        let own = bridge_uid(uid).and_then(|b| manifest.bases.get(&b)).and_then(|b| b.launched).map(|(lon, lat)| LonLat::new(lon, lat));
        if let Some(st) = own
            && home.as_deref() == Some(hub)
            && fleetmesh_decide::geo::haversine_m(st, h.at()) <= h.radius_m
        {
            drop(manifest);
            return Some(st);
        }
        // a visitor pad: clear of the stands, of what is parked there and of the returns bound there
        let stands: Vec<LonLat> = manifest.bases.values().filter_map(|b| b.launched).map(|(lon, lat)| LonLat::new(lon, lat)).collect();
        drop(manifest);
        let me = bridge_uid(uid);
        let parked: Vec<LonLat> = self
            .world
            .vehicles()
            .into_iter()
            .filter(|c| Some(c.uid) != me)
            .filter_map(|c| c.read(|v| v.pos).map(|(lat, lon)| LonLat::new(lon, lat)))
            .collect();
        let bound: Vec<LonLat> = {
            let core = self.lock();
            core.engine
                .intents()
                .filter(|i| matches!(i.state(), State::Dispatched | State::Approved | State::AutoApproved | State::PendingApproval) && !i.spec().uids.contains(uid))
                .filter_map(|i| match &i.spec().task {
                    fleetmesh_decide::Task::ReturnToHub { hub: Some(p) } if p.id.as_deref() == Some(hub) => Some(p.at),
                    _ => None,
                })
                .collect()
        };
        let clear = |p: LonLat| {
            let d = |q: &LonLat| fleetmesh_decide::geo::haversine_m(p, *q);
            stands.iter().chain(parked.iter()).chain(bound.iter()).all(|q| d(q) >= PAD_CLEAR_M)
        };
        let ring = (h.radius_m * 0.5).clamp(PAD_CLEAR_M, 40.0);
        let n = h.pads.clamp(4, 12);
        let pads = std::iter::once(h.at()).chain((0..n).map(|k| {
            let a = (f64::from(k) * 360.0 / f64::from(n)).to_radians();
            fleetmesh_decide::geo::offset(h.at(), ring * a.sin(), ring * a.cos())
        }));
        pads.into_iter().find(|p| clear(*p))
    }

    /// Rules 5: a pickup at `hub` by a vehicle on the hub's stands loads where it stands (the
    /// parcel is brought to its stand), so the order names the vehicle's own position; a vehicle
    /// elsewhere is sent the hub's point, as before.
    fn pickup_point(&self, uid: &DUid, hub: &str) -> Option<LonLat> {
        if !self.rules.at_least(fleetmesh_decide::HUB_SEQUENCING_SINCE) {
            return None;
        }
        let h = self.places.hubs.get(hub)?;
        let b = bridge_uid(uid)?;
        let (lat, lon) = self.world.vehicles().into_iter().find(|c| c.uid == b).and_then(|c| c.read(|v| v.pos))?;
        let pos = LonLat::new(lon, lat);
        (fleetmesh_decide::geo::haversine_m(pos, h.at()) <= h.radius_m).then_some(pos)
    }

    /// Weather at the hubs, the drop zones and the vehicles (every 5 s), and a drop zone's
    /// observation (every 5 s for the zones with a parcel on the way).
    async fn refresh_feed(&self, now: Time) {
        let (due, el, t_unix) = {
            let mut rt = self.rt();
            let due = now.ms_since(Time(rt.last_feed_ms)) >= 5000 || rt.last_feed_ms == 0;
            if due {
                rt.last_feed_ms = now.0;
            }
            let el = self.elapsed(&rt, now);
            let t_unix = scenario_unix(now, el);
            (due, el, t_unix)
        };
        if !due {
            return;
        }
        // where the weather is read: hubs, zones with a parcel not yet delivered, vehicles
        let mut sites: Vec<LonLat> = self.places.hubs.values().map(|h| h.at()).collect();
        let (zones, kinds): (Vec<String>, BTreeMap<String, String>) = {
            let rt = self.rt();
            let open: BTreeSet<String> = rt
                .parcels
                .values()
                .filter(|p| matches!(p.state, PState::Ready | PState::Assigned | PState::Loaded))
                .map(|p| p.spec.to.clone())
                .collect();
            let kinds = open
                .iter()
                .filter_map(|z| self.places.drop_zones.get(z).map(|d| (z.clone(), format!("{:?}", d.kind).to_lowercase())))
                .collect();
            (open.into_iter().collect(), kinds)
        };
        for z in &zones {
            if let Some(d) = self.places.drop_zones.get(z) {
                sites.push(d.at());
            }
        }
        let vehicle_sites: Vec<LonLat> = self
            .world
            .vehicles()
            .iter()
            .filter_map(|c| c.read(|v| v.pos).map(|(lat, lon)| LonLat::new(lon, lat)))
            .take(80)
            .collect();
        sites.extend(vehicle_sites);
        let mut facts: Vec<WxFact> = {
            let rt = self.rt();
            match &rt.scenario {
                Some(s) => sites.iter().filter_map(|p| s.weather_at(*p, now, el, t_unix)).collect(),
                None => Vec::new(),
            }
        };
        // tidewater: the water's level and current, the depth and the ground, and the wind where
        // no scenario says it
        if let Some(extra) = self.tidewater_sample(&sites).await {
            for (i, x) in extra.iter().enumerate() {
                let Some(site) = sites.get(i) else { break };
                let f = match facts.iter_mut().find(|f| f.at == *site) {
                    Some(f) => f,
                    None => {
                        let Some(w) = x.wind else { continue };
                        facts.push(WxFact {
                            at: *site,
                            t: now,
                            wind_e_mps: w[0],
                            wind_n_mps: w[1],
                            gust_mps: w[0].hypot(w[1]),
                            visibility_m: None,
                            precip_mmh: None,
                            current_e_mps: None,
                            current_n_mps: None,
                            tide_m: None,
                            depth_m: None,
                            ground_m: None,
                            source: "tidewater".into(),
                        });
                        facts.last_mut().expect("just pushed")
                    }
                };
                if let Some(c) = x.current {
                    f.current_e_mps = Some(c[0]);
                    f.current_n_mps = Some(c[1]);
                }
                f.tide_m = x.level;
                f.depth_m = x.depth.filter(|d| *d > 0.0);
                f.ground_m = x.ground;
            }
        }
        // a drop zone's look (the vehicle reports it on arrival; before that the zone's own feed)
        let mut seen = Vec::new();
        {
            let rt = self.rt();
            if let Some(s) = &rt.scenario {
                for z in &zones {
                    let kind = kinds.get(z).map_or("doorstep", String::as_str);
                    let (state, count) = s.observe(z, kind, el);
                    seen.push((z.clone(), state, count));
                }
            }
        }
        let mut changed = Vec::new();
        {
            let mut rt = self.rt();
            let mut feed = self.feed();
            feed.weather = facts;
            for (z, state, count) in seen {
                feed.observations.insert(z.clone(), DzObservation { t: now, state, count });
                if rt.last_obs.insert(z.clone(), state) != Some(state) {
                    changed.push((z, state, count));
                }
            }
        }
        for (z, state, count) in changed {
            self.log_delivery(
                "observation",
                None,
                None,
                &format!("drop zone {z} looked at: {state:?}, {count} seen"),
                json!({ "zone": z, "state": format!("{state:?}").to_lowercase(), "count": count }),
            );
        }
    }

    async fn tidewater_sample(&self, sites: &[LonLat]) -> Option<Vec<TwSample>> {
        let tw = super::terrain::Tidewater::from_env()?;
        let raw = tw.conditions(sites).await.ok()?;
        Some(
            raw.iter()
                .map(|s| TwSample {
                    wind: pair(s, "wind_east", "wind_north"),
                    current: pair(s, "current_east", "current_north"),
                    level: s["water_level_m"].as_f64(),
                    depth: s["water_depth_m"].as_f64(),
                    ground: s["elevation_m"].as_f64(),
                })
                .collect(),
        )
    }

    /// The predicted link along `from` to `to` for a vehicle: its own link as the order
    /// answers, and its backup radio along the same route (the coverage predictor).
    async fn link_along(&self, uid: &DUid, from: LonLat, to: LonLat, backup: Option<&str>, now: Time) -> Option<LinkAlong> {
        let buid = bridge_uid(uid)?;
        let mut q = json!({ "uid": buid, "route": [[from.lon, from.lat], [to.lon, to.lat]], "from_vehicle": false });
        q["speed_mps"] = json!(12.0);
        let main = crate::coverage::query(&self.world, self, &q).await.ok()?;
        if main["predicted"] != json!(true) {
            return None;
        }
        let holds = main["holds"].as_bool().unwrap_or(true);
        let gap = main["trip_down"]["down"]["seconds"].as_f64().or(main["first_down"]["down"]["seconds"].as_f64());
        let backup_covers = match backup {
            Some(b) => {
                let q = json!({ "class": b, "route": [[from.lon, from.lat], [to.lon, to.lat]], "from_vehicle": false });
                crate::coverage::query(&self.world, self, &q).await.ok().and_then(|a| a["holds"].as_bool()).unwrap_or(false)
            }
            None => false,
        };
        let on_backup = main["active_profile"].is_string() && main["active_profile"] != main["profile"];
        Some(LinkAlong {
            t: now,
            trips: !holds,
            gap_s: gap,
            return_ok: true,
            backup_covers,
            on_backup,
            backup_rate_low: backup.is_some_and(|b| b.contains("iridium")),
        })
    }

    /// The QNH now, hPa, for the traffic picture: the weather archive's pressure at the hubs (on
    /// the shore, a few metres above sea level: their surface pressure is the sea-level pressure
    /// within half a hectopascal), the middle of them. None where the weather gives none (no
    /// archive, a point it does not cover): the picture then says standard pressure was used.
    pub(super) fn qnh_now(&self) -> Option<f64> {
        let now = self.clock.now();
        let rt = self.rt();
        let s = rt.scenario.as_ref()?;
        let el = self.elapsed(&rt, now);
        let t_unix = scenario_unix(now, el);
        let mut p: Vec<f64> = self
            .places
            .hubs
            .values()
            .filter_map(|h| s.store.at(h.lat, h.lon, t_unix, el).values.pressure_hpa)
            .filter(|p| (900.0..=1100.0).contains(p))
            .collect();
        if p.is_empty() {
            return None;
        }
        p.sort_by(f64::total_cmp);
        Some(p[p.len() / 2])
    }

    // ---- the vehicles' state ---------------------------------------------------------------

    /// Each vehicle that is idle at a hub, with its battery and what it is.
    fn free_vehicles(&self, world: &fleetmesh_decide::WorldSnapshot, busy: &BTreeSet<DUid>, threshold_pct: f64) -> Vec<Candidate> {
        let mut out = Vec::new();
        for (uid, v) in &world.vehicles {
            if busy.contains(uid) {
                continue;
            }
            let (Some(platform), Some(ap), Some(pos), Some(batt)) =
                (v.platform.clone(), v.autopilot.as_ref().map(|a| a.value), v.position.as_ref().map(|p| p.value), v.battery_pct.as_ref())
            else {
                continue;
            };
            if world.t.ms_since(batt.t) > 30_000 || batt.value < threshold_pct {
                continue;
            }
            if self.rt().cooldown.get(uid).is_some_and(|until| world.t.0 < *until) {
                continue;
            }
            // an aircraft just down on its stand and still armed disarms in a moment: an order
            // started now races its disarm (the motors stop under a mission that is starting)
            if let Some(b) = bridge_uid(uid)
                && let Some(cell) = self.world.vehicles().into_iter().find(|c| c.uid == b)
                && cell.read(|v| v.armed && v.kind.is_some_and(|k| k.flying()) && !v.airborne())
            {
                continue;
            }
            // trusted navigation, not flying away from a hub
            let trust = self.rt().nav.get(uid).map(|n| n.state);
            if matches!(trust, Some(NavTrust::Untrusted | NavTrust::Degraded)) {
                continue;
            }
            let Some((hub, d)) = self.places.nearest_hub(pos) else { continue };
            if d > hub_catchment(hub.radius_m) {
                continue;
            }
            // rules 4: its home base, which the assigner costs the way back to
            let home = if self.rules.at_least(fleetmesh_decide::HOME_BASE_SINCE) {
                self.home_hub(uid, v.home.as_ref().map(|h| h.value))
            } else {
                None
            };
            out.push(Candidate { uid: uid.clone(), hub: hub.id.clone(), platform, kind: ap, battery_pct: batt.value, home });
        }
        out
    }

    /// An intent's state and the reason codes of its checks that did not pass.
    fn intent_state(&self, id: IntentId) -> Option<(State, Vec<String>)> {
        let core = self.lock();
        core.engine.intent(id).map(|i| {
            (i.state(), i.checks().iter().filter(|c| c.outcome != fleetmesh_decide::Outcome::Pass).flat_map(|c| c.codes.clone()).collect())
        })
    }

    fn vehicle_payload(&self, uid: &DUid) -> Option<&'static str> {
        let b = bridge_uid(uid)?;
        self.world.vehicles().into_iter().find(|c| c.uid == b).and_then(|c| c.read(|v| v.payload()))
    }

    // ---- following the orders ---------------------------------------------------------------

    async fn follow_jobs(&self, now: Time) {
        let snapshot: Vec<(DUid, Job)> = self.rt().jobs.iter().map(|(u, j)| (u.clone(), j.clone())).collect();
        for (uid, job) in snapshot {
            let Some(id) = job.intent else {
                continue;
            };
            let Some((state, codes)) = self.intent_state(id) else {
                self.rt().jobs.remove(&uid);
                continue;
            };
            if state == State::PendingApproval && job.stage == Stage::Deliver {
                // a delivery held for its zone's approval: a person is asked about the zone, once
                // (rules 2: a new delivery point; the strict procedure: a confirmation, one
                // pending at a time per zone)
                if codes.iter().any(|c| APPROVAL_CODES.contains(&c.as_str()))
                    && let Some(p) = job.parcel.clone()
                {
                    self.ask_about_zone(&uid, &p).await;
                }
                // ... and once the zone is approved, the held delivery goes again (its other
                // holds, if any, are ones automation waits out or a person's)
                if let Some(zone) = job.parcel.as_ref().and_then(|p| self.rt().parcels.get(p).map(|r| r.spec.to.clone()))
                    && codes.iter().all(|c| APPROVAL_CODES.contains(&c.as_str()) || WAIT_OUT.contains(&c.as_str()))
                    && codes.iter().any(|c| APPROVAL_CODES.contains(&c.as_str()))
                    && self.zone_approved_now(&zone, now)
                    && self.agent_withdraw(&AgentId::new(DISPATCHER), id.0, "its drop zone is approved now: the delivery goes again", Some("pending_approval")).is_ok()
                {
                    self.log_delivery("withdrawn", Some(&uid), job.parcel.as_deref(), &format!("{id} withdrawn: {zone} is approved now, the delivery is proposed again"), json!({ "intent": id.0, "zone": zone }));
                    if let Some(j) = self.rt().jobs.get_mut(&uid) {
                        j.intent = None;
                        j.not_before_ms = now.0;
                    }
                    continue;
                }
            }
            // a held order that nobody has decided for too long is withdrawn, and the parcel
            // goes back to the assigner
            if state == State::PendingApproval && matches!(job.stage, Stage::Pickup | Stage::Deliver) {
                let (limit, waited) = {
                    let rt = self.rt();
                    let limit = rt.scenario.as_ref().map_or(120, |s| s.params.hold_reassign_s);
                    (limit, now.ms_since(Time(job.proposed_ms)) / 1000)
                };
                if job.stage == Stage::Pickup && waited >= i64::from(limit) {
                    if self.agent_withdraw(&AgentId::new(DISPATCHER), id.0, "held in the inbox too long: the parcel is reassigned", Some("pending_approval")).is_ok() {
                        self.log_delivery("withdrawn", Some(&uid), job.parcel.as_deref(), &format!("{id} withdrawn after {waited} s unanswered"), json!({ "intent": id.0 }));
                    }
                    continue;
                }
            }
            if !terminal(state) {
                continue;
            }
            self.job_ended(&uid, &job, id, state, now).await;
        }
    }

    async fn job_ended(&self, uid: &DUid, job: &Job, id: IntentId, state: State, now: Time) {
        let ok = state == State::Done;
        match job.stage {
            Stage::Pickup => {
                let p = job.parcel.clone().unwrap_or_default();
                let payload = self.vehicle_payload(uid);
                if ok && matches!(payload, Some("loaded") | None) {
                    {
                        let mut rt = self.rt();
                        if let Some(r) = rt.parcels.get_mut(&p) {
                            r.state = PState::Loaded;
                            r.loaded_ms = Some(now.0);
                        }
                        rt.jobs.remove(uid);
                    }
                    self.log_delivery("parcel", Some(uid), Some(&p), &format!("parcel {p} loaded on {uid}"), json!({ "state": "loaded", "payload": payload }));
                    self.propose_deliver(uid, &p, now).await;
                } else {
                    let end = self.order_end("pickup", uid, id, state, now);
                    {
                        let mut rt = self.rt();
                        if let Some(r) = rt.parcels.get_mut(&p) {
                            r.state = PState::Ready;
                            r.vehicle = None;
                            r.ended(end);
                        }
                        rt.jobs.remove(uid);
                    }
                    self.rt().cooldown.insert(uid.clone(), now.0 + 120_000);
                    self.log_delivery("parcel", Some(uid), Some(&p), &format!("pickup of {p} by {uid} ended {state:?}: the parcel is open again"), json!({ "state": "ready" }));
                }
            }
            Stage::Deliver => {
                let p = job.parcel.clone().unwrap_or_default();
                let payload = self.vehicle_payload(uid);
                if ok && matches!(payload, Some("released") | None) {
                    {
                        let mut rt = self.rt();
                        if let Some(r) = rt.parcels.get_mut(&p) {
                            r.state = PState::Delivered;
                            r.delivered_ms = Some(now.0);
                        }
                        rt.delivered += 1;
                        rt.jobs.remove(uid);
                    }
                    self.log_delivery("parcel", Some(uid), Some(&p), &format!("parcel {p} delivered by {uid}"), json!({ "state": "delivered", "payload": payload }));
                    self.propose_return(uid, now, None).await;
                    return;
                }
                // the drop-time look: the zone stayed not clear, the parcel is still aboard. The
                // safe thing is done already (nothing was let go); bring it home and tell the person
                if let Some(why) = self.op_failure(id).filter(|d| d.starts_with("drop zone not clear"))
                    && state == State::Failed
                {
                    let zone = self.rt().parcels.get(&p).map(|r| r.spec.to.clone()).unwrap_or_default();
                    let mut end = self.order_end("look", uid, id, state, now);
                    end.ended = "kept";
                    end.codes = vec!["DROP_NOT_CLEAR".into()];
                    self.parcel_ended(&p, end);
                    self.log_delivery(
                        "look",
                        Some(uid),
                        Some(&p),
                        &format!("{uid} kept {p}: {why}; it comes home with it"),
                        json!({ "zone": zone, "where": "over the zone", "kept": true }),
                    );
                    self.rt().jobs.remove(uid);
                    let home = self.propose_return(uid, now, Some(&p)).await;
                    let a = self.auto();
                    let again = self.rt().parcels.get(&p).is_some_and(|r| r.returns + 1 < a.max_tries);
                    self.raise(
                        format!("delivery:{p}"),
                        Issue {
                            kind: "drop_not_clear",
                            uid: Some(uid.clone()),
                            problem: format!("{p} was not delivered: over {zone}, {}", why.trim_start_matches("drop zone not clear: ")),
                            did: format!(
                                "{uid} did not let it go and is bringing it back{}",
                                home.map_or(String::new(), |h| format!(" to {h}")),
                            ),
                            recommend: if again {
                                format!("nothing is needed for safety; automation tries {p} again in {:.0} min. If {zone} is not fit, have the customer choose another spot (a new spot needs your approval)", a.retry_after_s / 60.0)
                            } else {
                                format!("{p} has been tried {} times and will not be sent again: contact the customer, or order it to another approved spot", a.max_tries)
                            },
                            at_wall_s: crate::clock::wall(),
                            until_ms: Some(now.0 + (a.issue_show_s * 1000.0) as i64),
                            intent: Some(id),
                            track_ids: Vec::new(),
                        },
                    );
                    return;
                }
                // not released: try again twice, then bring it home. A refusal is final, except
                // one for a predicted conflict with moving traffic, whose fix is a short delay
                // (delivery-rules.md section 8: the dispatcher re-proposes with the fix)
                let end = self.order_end("deliver", uid, id, state, now);
                let codes = self.intent_state(id).map(|(_, c)| c).unwrap_or_default();
                let sequencing = state == State::Rejected
                    && !codes.is_empty()
                    && codes.iter().all(|c| c == "SEP_ROUTE_CONFLICT" || WAIT_OUT.contains(&c.as_str()));
                // rules 5: a refusal only for another of the fleet's vehicles' route is sequencing,
                // not a try: the next proposal waits it out (its dry run)
                let counts = !(sequencing && self.rules.at_least(fleetmesh_decide::HUB_SEQUENCING_SINCE));
                let tries = {
                    let mut rt = self.rt();
                    let r = rt.parcels.get_mut(&p);
                    r.map_or(9, |r| {
                        if counts {
                            r.tries += 1;
                        }
                        r.ended(end.clone());
                        r.tries
                    })
                };
                self.log_delivery("parcel", Some(uid), Some(&p), &format!("delivery of {p} by {uid} ended {state:?} before the parcel was released (try {tries})"), json!({ "state": "loaded", "ended": format!("{state:?}") }));
                if tries < 3 && (state != State::Rejected || sequencing) {
                    let pause = if sequencing { (self.auto().hold_retry_s * 1000.0) as i64 } else { 30_000 };
                    let mut rt = self.rt();
                    if let Some(j) = rt.jobs.get_mut(uid) {
                        j.intent = None;
                        j.not_before_ms = now.0 + pause;
                    }
                } else {
                    {
                        let mut rt = self.rt();
                        if let Some(r) = rt.parcels.get_mut(&p) {
                            r.fail(end.clone());
                        }
                        rt.jobs.remove(uid);
                    }
                    self.log_delivery(
                        "parcel",
                        Some(uid),
                        Some(&p),
                        &format!("parcel {p} failed: not delivered after {tries} tries, the last {} ({}{}), {uid} is sent home", end.ended, if end.codes.is_empty() { String::new() } else { format!("{}: ", end.codes.join(", ")) }, end.detail),
                        json!({ "state": "failed", "reason": end }),
                    );
                    let why = self.op_failure(id).unwrap_or_else(|| format!("its order ended {state:?}").to_lowercase());
                    let home = self.propose_return(uid, now, None).await;
                    let a = self.auto();
                    self.raise(
                        format!("delivery:{p}"),
                        Issue {
                            kind: "delivery_failed",
                            uid: Some(uid.clone()),
                            problem: format!("{p} was not delivered after {tries} tries: the last ended {} ({why})", format!("{state:?}").to_lowercase()),
                            did: format!("automation tried it again after each failure, then gave it up and sent {uid} back{}", home.map_or(String::new(), |h| format!(" to {h}"))),
                            recommend: format!("look at why it failed (the vehicle's link, the route's checks); order {p} again yourself if it should still go"),
                            at_wall_s: crate::clock::wall(),
                            until_ms: Some(now.0 + (a.issue_show_s * 1000.0) as i64),
                            intent: Some(id),
                            track_ids: Vec::new(),
                        },
                    );
                }
            }
            Stage::Return => {
                self.rt().jobs.remove(uid);
                // a return that failed on its way out (an upload with no answer, a link that
                // dropped mid-exchange) is tried again: going home is the safe default
                if matches!(state, State::Failed | State::Voided) && self.vehicle_hub(uid).is_none() && !self.heard(uid) {
                    // its link is down: its own failsafe has it; the return is proposed again once
                    // it is heard (not a try)
                    self.log_delivery("retry", Some(uid), job.parcel.as_deref(), &format!("{uid}'s return ended {state:?} while it is not heard: proposed again once it is"), json!({ "order": "return_to_hub", "not_heard": true }));
                    let mut j = Job::new(job.parcel.clone(), Stage::Return, None, now.0);
                    j.not_before_ms = now.0 + (self.auto().hold_retry_s * 1000.0) as i64;
                    self.rt().jobs.insert(uid.clone(), j);
                    return;
                }
                if matches!(state, State::Failed | State::Voided) && self.vehicle_hub(uid).is_none() {
                    let n = {
                        let mut rt = self.rt();
                        let n = rt.return_failures.entry(uid.clone()).or_insert(0);
                        *n += 1;
                        *n
                    };
                    let why = self.op_failure(id).unwrap_or_else(|| format!("it ended {state:?}").to_lowercase());
                    if n < RETURN_TRIES {
                        self.log_delivery("retry", Some(uid), job.parcel.as_deref(), &format!("{uid}'s return failed ({why}): proposed again (try {})", n + 1), json!({ "order": "return_to_hub", "try": n + 1 }));
                        // at once: the vehicle holds where it is meanwhile
                        let j = Job::new(job.parcel.clone(), Stage::Return, None, now.0);
                        self.rt().jobs.insert(uid.clone(), j);
                        return;
                    }
                    self.rt().return_failures.remove(uid);
                    let a = self.auto();
                    self.raise(
                        format!("return:{uid}"),
                        Issue {
                            kind: "return_failed",
                            uid: Some(uid.clone()),
                            problem: format!("{uid} could not be sent home: its return failed {RETURN_TRIES} times ({why})"),
                            did: "automation proposed the return again after each failure and has stopped trying; the vehicle holds where it is, and its own lost-link failsafe brings it home if its link drops".into(),
                            recommend: "look at its link and its position; order a return to a hub or a landing yourself".into(),
                            at_wall_s: crate::clock::wall(),
                            until_ms: Some(now.0 + (a.issue_show_s * 1000.0) as i64),
                            intent: Some(id),
                            track_ids: Vec::new(),
                        },
                    );
                    // a parcel aboard stays aboard: the person decides where the vehicle goes
                    return;
                }
                if state == State::Done {
                    self.rt().return_failures.remove(uid);
                }
                // a parcel it brought back: off the vehicle at the hub, tried again later
                if let Some(p) = job.parcel.clone() {
                    let hub = self.vehicle_hub(uid);
                    self.parcel_back(&p, hub.as_deref(), now, "brought back");
                }
            }
            Stage::Swap => {
                self.rt().jobs.remove(uid);
            }
        }
    }

    /// A parcel that was not delivered and is at a hub again (brought back, or never set off):
    /// open for a later try after `retry_after_s`, or failed after `max_tries`.
    fn parcel_back(&self, p: &str, hub: Option<&str>, now: Time, how: &str) {
        let a = self.auto();
        let failed = {
            let mut rt = self.rt();
            let Some(r) = rt.parcels.get_mut(p) else { return };
            r.returns += 1;
            r.vehicle = None;
            if let Some(h) = hub {
                r.spec.from = h.to_string();
            }
            if r.returns >= a.max_tries {
                let end = r.ends.last().cloned().unwrap_or_else(|| TryEnd {
                    stage: "return",
                    ended: "kept",
                    codes: Vec::new(),
                    detail: how.to_string(),
                    vehicle: None,
                    intent: None,
                    at_s: 0.0,
                });
                r.fail(end);
                true
            } else {
                r.state = PState::Ready;
                r.not_before_ms = now.0 + (a.retry_after_s * 1000.0) as i64;
                false
            }
        };
        let at = hub.map_or(String::new(), |h| format!(" at {h}"));
        if failed {
            let reason = self.rt().parcels.get(p).and_then(|r| r.failure.clone());
            let why = reason.as_ref().map_or(String::new(), |e| format!(" (the last: {}{})", if e.codes.is_empty() { String::new() } else { format!("{}: ", e.codes.join(", ")) }, e.detail));
            self.log_delivery("parcel", None, Some(p), &format!("parcel {p} {how}{at}: tried {} times, not sent again{why}", a.max_tries), json!({ "state": "failed", "reason": reason }));
        } else {
            self.log_delivery("parcel", None, Some(p), &format!("parcel {p} {how}{at}: tried again in {:.0} s", a.retry_after_s), json!({ "state": "ready", "retry_s": a.retry_after_s }));
        }
    }

    /// Whether a vehicle is heard now (its telemetry within its link's stale limit, as the inbox's
    /// `lost_link` alert reads it). One not heard is left to its own lost-link failsafe: the
    /// dispatcher proposes it nothing until it is heard again (each order would be decided on
    /// stale facts, and voided when the bridge calls it lost).
    fn heard(&self, uid: &DUid) -> bool {
        let Some(b) = bridge_uid(uid) else { return true };
        let Some(cell) = self.world.vehicles().into_iter().find(|c| c.uid == b) else { return true };
        let link = cell.link_snapshot(crate::clock::mono());
        let wall = crate::clock::wall();
        // its position too: a link that still carries heartbeats but no position for longer than
        // the rules' fact age decides every order on stale facts
        let fresh = cell.read(|v| v.seen.position).is_none_or(|t| wall - t <= f64::from(self.rules.max_fact_age_s));
        link.last_rx_t.is_none_or(|t| wall - t <= link.stale_after_s()) && fresh
    }

    /// The hub a vehicle is at, if it is at one.
    fn vehicle_hub(&self, uid: &DUid) -> Option<String> {
        let b = bridge_uid(uid)?;
        let pos = self.world.vehicles().into_iter().find(|c| c.uid == b).and_then(|c| c.read(|v| v.pos))?;
        let (hub, d) = self.places.nearest_hub(LonLat::new(pos.1, pos.0))?;
        (d <= hub_catchment(hub.radius_m)).then(|| hub.id.clone())
    }

    /// The detail of the failed op of an intent, if one failed.
    fn op_failure(&self, id: IntentId) -> Option<String> {
        let core = self.lock();
        let t = core.taskings.get(&id)?.snapshot();
        t.ops.into_iter().find(|o| o.state == crate::tasks::OpState::Failed).map(|o| o.detail)
    }

    /// How one of the dispatcher's orders for a parcel ended, for the parcel's record
    /// ([`TryEnd`]): its end in a word, the codes of the checks that refused it (or, for one that
    /// expired or was withdrawn, held it), and why in words (those checks' details, the task
    /// layer's failed step, or the state change's own reason).
    fn order_end(&self, stage: &'static str, uid: &DUid, id: IntentId, state: State, now: Time) -> TryEnd {
        let ended = match state {
            State::Rejected => "rejected",
            State::Failed => "failed",
            State::Expired => "expired",
            State::Voided => "voided",
            State::Aborted => "aborted",
            _ => "ended",
        };
        let (codes, words, reason) = {
            let core = self.lock();
            core.engine.intent(id).map_or_else(Default::default, |i| {
                let counts = |o: fleetmesh_decide::Outcome| match state {
                    State::Rejected => o == fleetmesh_decide::Outcome::Fail,
                    State::Failed | State::Aborted => false,
                    _ => o != fleetmesh_decide::Outcome::Pass,
                };
                let bad: Vec<&fleetmesh_decide::CheckResult> = i.checks().iter().filter(|c| counts(c.outcome)).collect();
                let mut codes: Vec<String> = Vec::new();
                for c in bad.iter().flat_map(|c| c.codes.iter()) {
                    if !codes.contains(c) {
                        codes.push(c.clone());
                    }
                }
                let words = bad.iter().map(|c| c.detail.clone()).filter(|d| !d.is_empty()).collect::<Vec<_>>().join("; ");
                (codes, words, i.history().last().map(|h| h.reason.clone()).unwrap_or_default())
            })
        };
        let failed_step = if state == State::Failed { self.op_failure(id) } else { None };
        let detail = failed_step.or((!words.is_empty()).then_some(words)).unwrap_or(reason);
        let at_s = self.elapsed(&self.rt(), now);
        TryEnd { stage, ended, codes, detail, vehicle: Some(uid.to_string()), intent: Some(id.0), at_s }
    }

    /// Keep `end` on parcel `p`'s record.
    fn parcel_ended(&self, p: &str, end: TryEnd) {
        if let Some(r) = self.rt().parcels.get_mut(p) {
            r.ended(end);
        }
    }

    // ---- automation: what it waits out, what it asks, what it tells -------------------------

    fn auto(&self) -> Automation {
        self.rt().scenario.as_ref().map(|s| s.automation.clone()).unwrap_or_default()
    }

    /// What a zone's latest look says: `Ok(what was seen)` when it is clear enough, else
    /// `Err(why not)`. `at_zone`: the drop-time look, which needs a fresh clear look whatever the
    /// zone. Else the look before departure, read as the drop-zone rule reads it: people there, or
    /// a populated zone with no recent look that could tell.
    pub(super) fn zone_look(&self, zone: &str, now: Time, at_zone: bool) -> Result<String, String> {
        let max_age = self.rules.delivery_rules().dropzone.obs_max_age_s;
        let populated = self.places.drop_zones.get(zone).is_some_and(|z| z.populated);
        let obs = self.feed().observations.get(zone).cloned();
        let fresh = obs.filter(|o| now.ms_since(o.t) as f64 / 1000.0 <= max_age);
        match fresh {
            Some(o) if o.state == ObservedState::Clear => Ok("clear".into()),
            Some(o) if o.state == ObservedState::Occupied => Err(format!("occupied, {} seen", o.count)),
            Some(_) if at_zone || populated => Err("the look could not tell".into()),
            Some(_) => Ok("the look could not tell, and the zone is not populated".into()),
            None if at_zone || populated => Err(format!("no look within {max_age:.0} s")),
            None => Ok("no recent look, and the zone is not populated".into()),
        }
    }

    /// The drop-time look of a delivery the task layer flies ([`crate::tasks::DropLook`]): the
    /// zone's observation feed, read while the vehicle is over it, for `hover_wait_s` at most.
    pub(super) fn drop_look_hook(&self, task: &fleetmesh_decide::Task) -> Option<crate::tasks::DropLook> {
        let fleetmesh_decide::Task::Deliver { drop_zone, .. } = task else { return None };
        let zone = drop_zone.clone();
        let me = self.me.clone();
        let wait = std::time::Duration::from_secs_f64(self.auto().hover_wait_s.max(0.0));
        Some(crate::tasks::DropLook {
            look: std::sync::Arc::new(move || match me.upgrade() {
                Some(d) => d.zone_look(&zone, d.clock.now(), true),
                None => Err("the decision layer is gone".into()),
            }),
            wait,
        })
    }

    /// A pickup at a hub by a vehicle already inside the hub's landing zone (on one of its pads)
    /// is loaded where the vehicle stands: the task layer flies it no route (a parcel is brought to
    /// the pad). Without this a vehicle a few metres from the hub's centre point took off and
    /// hovered over that point to load, and loaded drones bunched there at cruise height, each
    /// holding the others' departures (`SEP_HUB_ZONE`). The decision is unchanged: only where
    /// the task layer has it load.
    pub(super) fn pickup_in_place(&self, task: &fleetmesh_decide::Task, order: &mut crate::tasks::TaskOrder) {
        let fleetmesh_decide::Task::Pickup { site, .. } = task else { return };
        let Some(hub) = site.id.as_ref().and_then(|h| self.places.hubs.get(h)) else { return };
        let [cell] = order.vehicles.as_slice() else { return };
        let Some((lat, lon)) = cell.read(|v| v.pos) else { return };
        if fleetmesh_decide::geo::haversine_m(LonLat::new(lon, lat), hub.at()) <= hub.radius_m {
            order.params.route = Some(vec![[lon, lat]]);
        }
    }

    /// Whether a delivery to `zone` may go with no person: rules 2's approved delivery point (a
    /// standing approval, a hub pad, a person's approval once); under the strict procedure, a
    /// confirmation within `confirm_valid_s` or a hub pad.
    pub(super) fn zone_approved_now(&self, zone: &str, now: Time) -> bool {
        let Some(z) = self.places.drop_zones.get(zone) else { return false };
        let core = self.lock();
        let seen = core.engine.drop_zone_confirmations().get(zone).copied();
        if self.rules.standing_approvals() {
            z.standing_approved() || seen.is_some()
        } else {
            z.hub_pad() || seen.is_some_and(|t| now.ms_since(t) <= i64::from(self.rules.delivery_rules().dropzone.confirm_valid_s) * 1000)
        }
    }

    /// Ask a person about a parcel's zone (`confirm_drop_zone`): rules 2, to approve it as a new
    /// delivery point, once per zone; the strict procedure, to confirm it for a delivery that
    /// waits, while no other request for the zone is pending.
    async fn ask_about_zone(&self, uid: &DUid, parcel: &str) {
        let Some(zone) = self.rt().parcels.get(parcel).map(|p| p.spec.to.clone()) else { return };
        let standing = self.rules.standing_approvals();
        if let Some(ask) = self.rt().zone_asks.get(&zone).cloned() {
            let open = ask.intent.and_then(|i| self.intent_state(i)).is_some_and(|(s, _)| !terminal(s));
            if open || standing {
                return;
            }
        }
        let obs = self.feed().observations.get(&zone).cloned();
        let seen = match &obs {
            Some(o) => format!("{:?} ({} seen) at {}", o.state, o.count, rfc3339(o.t.0)),
            None => "no look yet".into(),
        };
        let how = match self.places.drop_zones.get(&zone).and_then(|z| z.approval) {
            Some(fleetmesh_decide::delivery::StandingApproval::Withdrawn) => "its standing approval was withdrawn",
            _ => "it is not on the approved list",
        };
        let rationale = if standing {
            format!("dispatcher: {zone} is a new delivery point ({how}): approving it lets {parcel} and every later delivery there go with no one asked; the latest look: {seen}")
        } else {
            format!("dispatcher: {zone} has not been confirmed for {parcel}; the latest look: {seen}")
        };
        let params = TaskParams {
            parcel: Some(parcel.into()),
            drop_zone: Some(zone.clone()),
            evidence: Some(obs.iter().map(|o| format!("obs:{zone}:{}", o.t.0)).collect()),
            ..TaskParams::default()
        };
        let req = IntentRequest {
            task: "confirm_drop_zone".into(),
            uids: bridge_uid(uid).into_iter().collect(),
            params,
            constraints: Default::default(),
            rationale: Some(rationale),
            group: None,
        };
        let id = self.propose(DISPATCHER, req, "confirm_drop_zone").await;
        self.rt().zone_asks.insert(zone, ZoneAsk { intent: id });
        if let Some(j) = self.rt().jobs.get_mut(uid) {
            j.confirm = Some(id.unwrap_or(IntentId(0)));
        }
    }

    /// Rules 2: the open parcels whose zone is not an approved delivery point are not assigned;
    /// a person is asked about each such zone once, before any parcel is loaded for it (the
    /// request names an idle vehicle at the first parcel's hub, the one that would carry it: an
    /// order names a vehicle). A zone whose request ended unapproved keeps its parcels at their
    /// hubs, and the person is told.
    async fn hold_for_unapproved_zones(
        &self,
        open: Vec<OpenParcel>,
        world: &fleetmesh_decide::WorldSnapshot,
        busy: &BTreeSet<DUid>,
        now: Time,
    ) -> Vec<OpenParcel> {
        if !self.rules.standing_approvals() {
            return open;
        }
        let mut keep = Vec::new();
        let mut waiting: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut hub_of: BTreeMap<String, String> = BTreeMap::new();
        for p in open {
            if self.zone_approved_now(&p.zone, now) {
                keep.push(p);
            } else {
                hub_of.entry(p.zone.clone()).or_insert_with(|| p.hub.clone());
                waiting.entry(p.zone.clone()).or_default().push(p.id.clone());
            }
        }
        let carrier = |hub: &str| -> Option<DUid> {
            world
                .vehicles
                .iter()
                .filter(|(u, _)| !busy.contains(*u))
                .find(|(_, v)| {
                    v.position.as_ref().is_some_and(|p| {
                        self.places.nearest_hub(p.value).is_some_and(|(h, d)| h.id == hub && d <= hub_catchment(h.radius_m))
                    })
                })
                .map(|(u, _)| u.clone())
        };
        for (zone, parcels) in &waiting {
            let ask = self.rt().zone_asks.get(zone).cloned();
            match ask {
                None => {
                    let parcel = parcels[0].clone();
                    let Some(by) = hub_of.get(zone).and_then(|h| carrier(h)).and_then(|u| bridge_uid(&u)) else { continue };
                    let req = IntentRequest {
                        task: "confirm_drop_zone".into(),
                        uids: vec![by],
                        params: TaskParams {
                            parcel: Some(parcel.clone()),
                            drop_zone: Some(zone.clone()),
                            evidence: Some(self.feed().observations.get(zone).map(|o| format!("obs:{zone}:{}", o.t.0)).into_iter().collect()),
                            ..TaskParams::default()
                        },
                        constraints: Default::default(),
                        rationale: Some(format!(
                            "dispatcher: {zone} is a new delivery point (it is not on the approved list): {} parcel(s) wait for it ({}); approving it lets them and every later delivery there go with no one asked",
                            parcels.len(),
                            parcels.join(", ")
                        )),
                        group: None,
                    };
                    let id = self.propose(DISPATCHER, req, "confirm_drop_zone").await;
                    self.log_delivery("ask", None, Some(&parcel), &format!("{zone} is not an approved delivery point: a person is asked once ({} parcel(s) wait)", parcels.len()), json!({ "zone": zone, "intent": id.map(|i| i.0) }));
                    self.rt().zone_asks.insert(zone.clone(), ZoneAsk { intent: id });
                }
                Some(a) => {
                    let ended = a.intent.and_then(|i| self.intent_state(i)).is_none_or(|(s, _)| terminal(s));
                    let key = format!("zone:{zone}");
                    if ended && !self.rt().issues.contains_key(&key) {
                        let a2 = a.intent.and_then(|i| self.intent_state(i)).map(|(s, _)| s);
                        let how = match a2 {
                            Some(State::Rejected) => "its approval was rejected",
                            Some(State::Expired) => "nobody decided on its approval",
                            _ => "it was not approved",
                        };
                        self.raise(
                            key,
                            Issue {
                                kind: "zone_not_approved",
                                uid: None,
                                problem: format!("{} parcel(s) for {zone} cannot go: it is not an approved delivery point and {how}", parcels.len()),
                                did: format!("automation keeps them at their hubs ({}) and sends nothing to {zone}; it does not ask again", parcels.join(", ")),
                                recommend: format!("if {zone} is fit, approve it as a delivery point (a confirm_drop_zone of your own) and they go; if not, have the customers choose another spot"),
                                at_wall_s: crate::clock::wall(),
                                until_ms: None,
                                intent: a.intent,
                                track_ids: Vec::new(),
                            },
                        );
                    }
                }
            }
        }
        // an issue about a zone lasts while parcels wait for it
        self.rt().issues.retain(|k, _| k.strip_prefix("zone:").is_none_or(|z| waiting.contains_key(z)));
        keep
    }

    /// Raise an issue for the person (logged as a `delivery.event`, shown as an alert).
    pub(super) fn raise(&self, id: String, issue: Issue) {
        self.log_delivery(
            "issue",
            issue.uid.as_ref(),
            None,
            &format!("{}: {}; {}; {}", issue.kind, issue.problem, issue.did, issue.recommend),
            json!({ "id": id, "kind": issue.kind, "problem": issue.problem, "did": issue.did, "recommend": issue.recommend }),
        );
        self.rt().issues.insert(id, issue);
    }

    /// What a dispatcher's order would get now, dry run: `None` to send it as it is (auto, a
    /// refusal, or a hold that is a person's), or the hold codes automation waits out.
    async fn wait_out(&self, req: &IntentRequest) -> Option<Vec<String>> {
        self.wait_out_why(req).await.map(|(codes, _)| codes)
    }

    /// [`Self::wait_out`], and what the holding checks said (one line).
    async fn wait_out_why(&self, req: &IntentRequest) -> Option<(Vec<String>, String)> {
        let p = self.agent_dry_run(&AgentId::new(DISPATCHER), req).await.ok()?;
        if p.would_reach != State::PendingApproval {
            return None;
        }
        let holding: Vec<&fleetmesh_decide::CheckResult> = p
            .checks
            .iter()
            .filter(|c| matches!(c.outcome, fleetmesh_decide::Outcome::Hold | fleetmesh_decide::Outcome::Unverified))
            .collect();
        let held: Vec<String> = holding
            .iter()
            .flat_map(|c| if c.codes.is_empty() { vec![format!("{}:{:?}", c.rule.as_str(), c.outcome)] } else { c.codes.clone() })
            .collect();
        let why = holding.iter().map(|c| c.detail.clone()).collect::<Vec<_>>().join("; ");
        (!held.is_empty() && held.iter().all(|c| WAIT_OUT.contains(&c.as_str()))).then_some((held, why))
    }

    /// Rules 5: what a dispatcher's order would get now, dry run, when what stops it is the hub's
    /// or another of the fleet's vehicles' (`SEQUENCE_CODES`, other traffic, a refusal only for
    /// another vehicle's route), or, for a return, anything automation waits out: the codes and
    /// what the stopping checks said. `None`: send it as it is (or wait as before rules 5).
    async fn sequenced_why(&self, req: &IntentRequest, stage: Stage) -> Option<(Vec<String>, String)> {
        if !self.rules.at_least(fleetmesh_decide::HUB_SEQUENCING_SINCE) {
            return None;
        }
        let p = self.agent_dry_run(&AgentId::new(DISPATCHER), req).await.ok()?;
        let stopping: Vec<&fleetmesh_decide::CheckResult> = p.checks.iter().filter(|c| c.outcome != fleetmesh_decide::Outcome::Pass).collect();
        let refused: Vec<String> = stopping.iter().filter(|c| c.outcome == fleetmesh_decide::Outcome::Fail).flat_map(|c| c.codes.clone()).collect();
        let held: Vec<String> = stopping
            .iter()
            .filter(|c| matches!(c.outcome, fleetmesh_decide::Outcome::Hold | fleetmesh_decide::Outcome::Unverified))
            .flat_map(|c| if c.codes.is_empty() { vec![format!("{}:{:?}", c.rule.as_str(), c.outcome)] } else { c.codes.clone() })
            .collect();
        let wait_out = |c: &String| WAIT_OUT.contains(&c.as_str());
        // what else may stand with a meeting the hub sequences: other traffic, and another
        // vehicle's position or the picture not fresh (read again in a moment)
        let fleet = |c: &String| SEQUENCE_CODES.contains(&c.as_str()) || TRAFFIC_CODES.contains(&c.as_str()) || c == "SEP_TRAFFIC_UNKNOWN";
        let ok = match p.would_reach {
            // refused only because another of our vehicles' route is in the way (a check that
            // refuses names its holds too: other traffic, the hub, a stale reading)
            State::Rejected => refused.iter().any(|c| c == ROUTE_CONFLICT) && refused.iter().all(|c| c == ROUTE_CONFLICT || wait_out(c)) && held.iter().all(wait_out),
            State::PendingApproval => !held.is_empty() && held.iter().all(wait_out) && (stage == Stage::Return || held.iter().any(|c| SEQUENCE_CODES.contains(&c.as_str())) && held.iter().all(fleet)),
            _ => false,
        };
        if !ok {
            return None;
        }
        let mut codes: Vec<String> = refused;
        for c in held {
            if !codes.contains(&c) {
                codes.push(c);
            }
        }
        let why = stopping.iter().map(|c| c.detail.clone()).filter(|d| !d.is_empty()).collect::<Vec<_>>().join("; ");
        Some((codes, why))
    }

    /// Rules 5: one of the dispatcher's orders waits for the hub or for another of the fleet's
    /// vehicles (or a return waits out what automation waits out): the vehicle stays where it is
    /// (at the hub, or held where it is) and the order is tried again every `hold_retry_s`. It is
    /// never sent to a person for it; one that waits longer than `hold_retry_max_s` is an issue
    /// alert (`fleet_conflict`, or `return_held` for a return held by something else), while
    /// automation goes on waiting.
    #[allow(clippy::too_many_arguments)]
    fn wait_for_fleet(&self, uid: &DUid, parcel: Option<&str>, stage: Stage, codes: &[String], why: &str, since: i64, now: Time) {
        let a = self.auto();
        let waited_s = (now.0 - since) as f64 / 1000.0;
        let hub = self.vehicle_hub(uid);
        let did = match &hub {
            Some(h) => format!("{uid} waits at {h}"),
            None => format!("{uid} holds where it is"),
        };
        {
            let mut rt = self.rt();
            let j = rt.jobs.entry(uid.clone()).or_insert_with(|| Job::new(parcel.map(str::to_string), stage, None, now.0));
            j.stage = stage;
            j.waiting_since_ms = Some(since);
            j.waiting_on = codes.to_vec();
            j.intent = None;
            j.not_before_ms = now.0 + (a.hold_retry_s * 1000.0) as i64;
        }
        let what = match stage {
            Stage::Return => "its return",
            Stage::Pickup => "its pickup",
            _ => "its delivery",
        };
        let fleet = codes.iter().any(|c| c == ROUTE_CONFLICT || SEQUENCE_CODES.contains(&c.as_str()));
        let for_what = if fleet { "another of the fleet's vehicles (the hub sequences them)" } else { "what automation waits out" };
        self.say_wait(
            uid,
            parcel,
            format!("{did}: {what} waits for {for_what} ({}): automation tries again", codes.join(", ")),
            json!({ "codes": codes, "order": match stage { Stage::Return => "return_to_hub", Stage::Pickup => "pickup", _ => "deliver" }, "sequenced": true, "why": why }),
        );
        if waited_s >= a.hold_retry_max_s {
            let kind = if fleet { "fleet_conflict" } else { "return_held" };
            self.keep_issue(
                format!("{kind}:{uid}"),
                Issue {
                    kind,
                    uid: Some(uid.clone()),
                    problem: format!("{uid}'s {} has waited {:.0} min for {for_what}: {why}", what.trim_start_matches("its "), waited_s / 60.0),
                    did: format!("{did} and tries {what} again every {:.0} s; it is never sent to you for this", a.hold_retry_s),
                    recommend: "nothing is needed for safety: it goes when the way is clear. If it should not wait, order it another route, a return or a landing yourself".into(),
                    at_wall_s: crate::clock::wall(),
                    until_ms: None,
                    intent: None,
                    track_ids: Vec::new(),
                },
                30_000,
                now,
            );
        }
    }

    /// Whether an order of the dispatcher's would go at once (auto), dry run.
    async fn goes_auto(&self, req: &IntentRequest) -> bool {
        self.agent_dry_run(&AgentId::new(DISPATCHER), req).await.is_ok_and(|p| p.would_reach == State::AutoApproved)
    }

    /// Log what automation waits on for a vehicle, when it changes.
    fn say_wait(&self, uid: &DUid, parcel: Option<&str>, what: String, data: Value) {
        let changed = self.rt().waits_said.insert(uid.clone(), what.clone()).as_ref() != Some(&what);
        if changed {
            self.log_delivery("wait", Some(uid), parcel, &what, data);
        }
    }

    // ---- dispatch ---------------------------------------------------------------------------

    async fn dispatch_parcels(&self, now: Time) {
        // a held job whose pause is over: propose its deliver again
        let retry: Vec<(DUid, String)> = self
            .rt()
            .jobs
            .iter()
            .filter(|(_, j)| j.stage == Stage::Deliver && j.intent.is_none() && now.0 >= j.not_before_ms)
            .filter_map(|(u, j)| j.parcel.clone().map(|p| (u.clone(), p)))
            .collect();
        // one not heard is left to its own failsafe until it is heard again
        let retry: Vec<(DUid, String)> = retry.into_iter().filter(|(u, _)| self.heard(u)).collect();
        for (u, p) in retry {
            self.propose_deliver(&u, &p, now).await;
        }
        // a return that waited out a hold: propose it again
        let homes: Vec<(DUid, Option<String>)> = self
            .rt()
            .jobs
            .iter()
            .filter(|(_, j)| j.stage == Stage::Return && j.intent.is_none() && now.0 >= j.not_before_ms)
            .map(|(u, j)| (u.clone(), j.parcel.clone()))
            .collect();
        let homes: Vec<(DUid, Option<String>)> = homes.into_iter().filter(|(u, _)| self.heard(u)).collect();
        for (u, p) in homes {
            self.propose_return(&u, now, p.as_deref()).await;
        }
        let (params, due) = {
            let mut rt = self.rt();
            let params = rt.scenario.as_ref().map_or_else(DispatchParams::default, |s| s.params.clone());
            let due = now.ms_since(Time(rt.last_plan_ms)) >= i64::from(params.tick_s) * 1000 || rt.last_plan_ms == 0;
            if due {
                rt.last_plan_ms = now.0;
            }
            (params, due)
        };
        if !due {
            return;
        }
        let world = self.snapshot();
        let (open, busy): (Vec<OpenParcel>, BTreeSet<DUid>) = {
            let rt = self.rt();
            let open = rt
                .parcels
                .values()
                .filter(|p| p.state == PState::Ready && p.vehicle.is_none() && now.0 >= p.not_before_ms)
                .map(|p| OpenParcel {
                    id: p.spec.id.clone(),
                    hub: p.spec.from.clone(),
                    zone: p.spec.to.clone(),
                    mass_kg: p.spec.mass_kg,
                    class: p.spec.class.clone(),
                    priority: p.spec.priority,
                    due: Time(self.t0_ms(&rt) + (p.spec.due_s * 1000.0) as i64),
                })
                .collect();
            (open, rt.jobs.keys().cloned().collect())
        };
        let busy = self.with_open_orders(busy);
        // rules 2: nothing goes to a zone that is not an approved delivery point
        let open = self.hold_for_unapproved_zones(open, &world, &busy, now).await;
        // a swap for a vehicle that came home low, before the next parcel
        let swap_below = SWAP_BELOW_PCT;
        self.propose_swaps(&world, &busy, swap_below, now).await;
        // rules 4: a vehicle idle at another base than its own, with no parcel waiting there, goes home
        let busy = self.send_visitors_home(&world, busy, &open, swap_below, now).await;
        if open.is_empty() {
            return;
        }
        let vehicles = self.free_vehicles(&world, &busy, swap_below);
        let weather = self.feed().weather.clone();
        let places = {
            let mut p = self.places.clone();
            for (id, n) in &self.feed().hub_packs {
                if let Some(h) = p.hubs.get_mut(id) {
                    h.packs = *n;
                }
            }
            p
        };
        // each parcel is offered only to the vehicles that carry it inside the payload rule's hold
        // band as the wind is now (a drone's limit derated in the gust, as the rule derates it): a
        // parcel near a drone's limit goes by a vehicle with room. None free at its hub: one free at
        // another base of the same site whose platform reaches its zone takes it (the parcel is
        // carried across the site); none: it waits, and if no vehicle of its site could carry it
        // within the margin at all, the person hears of it once (an issue), never as a pickup to
        // approve. The heaviest go first, to the few that can take them; each group is its own plan
        let mut groups = self.payload_groups(open, &vehicles, &world, &places, now);
        groups.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
        let mut taken: BTreeSet<DUid> = BTreeSet::new();
        let mut plans: Vec<(AssignInput, fleetmesh_decide::dispatch::Plan)> = Vec::new();
        for (key, parcels) in groups {
            let input = AssignInput {
                t: now,
                params: params.clone(),
                energy: self.rules.delivery_rules().energy.clone(),
                places: places.clone(),
                weather: weather.clone(),
                parcels,
                vehicles: vehicles.iter().filter(|v| key.contains(&v.uid) && !taken.contains(&v.uid)).cloned().collect(),
            };
            let plan = fleetmesh_decide::dispatch::assign_under(&input, &self.rules);
            taken.extend(plan.assignments.iter().map(|a| a.vehicle.clone()));
            plans.push((input, plan));
        }
        if plans.iter().all(|(_, p)| p.assignments.is_empty() && p.unassigned.is_empty()) {
            return;
        }
        // a plan with nothing new (the same parcels left over for the same reasons) is not logged again
        let sig: BTreeSet<String> = plans.iter().flat_map(|(_, p)| p.unassigned.iter().map(|(p, r)| format!("{p}:{}", r.join(",")))).collect();
        {
            let mut rt = self.rt();
            let fresh = plans.iter().any(|(_, p)| !p.assignments.is_empty()) || sig != rt.last_unassigned;
            rt.last_unassigned = sig;
            if !fresh {
                return;
            }
        }
        for (input, plan) in &plans {
            if plan.assignments.is_empty() && plan.unassigned.is_empty() {
                continue;
            }
            let tick = {
                let mut rt = self.rt();
                rt.plan_tick += 1;
                rt.plan_tick
            };
            let mut core = self.lock();
            if let Err(e) = core.engine.record_dispatch_plan(tick, input.clone(), plan.clone()) {
                eprintln!("dispatch: log plan: {e}");
            }
        }
        for (input, plan) in &plans {
            for a in &plan.assignments {
                {
                    let mut rt = self.rt();
                    if !self.proposal_allowed(&mut rt, now, params.max_proposals_per_min) {
                        return;
                    }
                    if let Some(p) = rt.parcels.get_mut(&a.parcel) {
                        p.state = PState::Assigned;
                        p.vehicle = Some(a.vehicle.clone());
                        p.assigned_ms = Some(now.0);
                    }
                }
                let why = format!(
                    "{} to {}: {} {:.0}% battery, {:.0} min, energy {:.1}%{}",
                    a.parcel,
                    a.zone,
                    a.vehicle,
                    input.vehicles.iter().find(|v| v.uid == a.vehicle).map_or(0.0, |v| v.battery_pct),
                    a.terms.time,
                    a.terms.energy_pct,
                    a.beat.first().map_or(String::new(), |(u, c)| format!("; next best {u} cost {c}")),
                );
                self.propose_pickup(&a.vehicle, &a.parcel, &a.hub, &why, now).await;
            }
        }
    }

    /// The open parcels grouped by the free vehicles that carry each with room (see
    /// [`Self::dispatch_parcels`]): transfers within a site, the parcels that wait for room, and the
    /// issue for one no vehicle of its site can carry within the margin for
    /// [`NO_ROOM_ISSUE_S`].
    fn payload_groups(
        &self,
        open: Vec<OpenParcel>,
        vehicles: &[Candidate],
        world: &fleetmesh_decide::WorldSnapshot,
        places: &fleetmesh_decide::delivery::Places,
        now: Time,
    ) -> Vec<(Vec<DUid>, Vec<OpenParcel>)> {
        use fleetmesh_decide::Domain;
        use fleetmesh_decide::delivery::Surface2;
        let d = self.rules.delivery_rules();
        let hold_frac = d.payload.hold_frac;
        let gust = fleetmesh_decide::delivery_rules::payload_gust(world, &d);
        // what a platform carries within the margin now (none: no limit known, as the rule says)
        let room = |plat: &fleetmesh_decide::PlatformFacts, kind: AutopilotKind| {
            plat.payload_kg.map(|max| fleetmesh_decide::delivery_rules::effective_payload(&d, plat, kind, max, gust) * hold_frac)
        };
        let reaches = |dom: Domain, zone: &str| {
            !matches!((dom, places.drop_zones.get(zone).map(|z| z.surface)), (Domain::Ground, Some(Surface2::Water)) | (Domain::Surface, Some(Surface2::Land)))
        };
        let fits = |plat: &fleetmesh_decide::PlatformFacts, kind: AutopilotKind, p: &OpenParcel| {
            room(plat, kind).is_none_or(|r| p.mass_kg <= r) && reaches(plat.domain, &p.zone)
        };
        // the bases of each parcel's site (its own hub alone when it is in none)
        let sites: Vec<Vec<String>> = self.rt().scenario.as_ref().map(|s| s.sites.iter().map(|x| x.hubs.clone()).collect()).unwrap_or_default();
        let site_of = |hub: &str| sites.iter().find(|h| h.iter().any(|x| x == hub)).cloned().unwrap_or_else(|| vec![hub.to_string()]);
        // every vehicle of the fleet by its base, whatever it is doing (could anyone carry it?)
        let fleet: Vec<(Option<String>, fleetmesh_decide::PlatformFacts, AutopilotKind)> = world
            .vehicles
            .iter()
            .filter_map(|(u, v)| Some((self.home_hub(u, v.home.as_ref().map(|h| h.value)), v.platform.clone()?, v.autopilot.as_ref()?.value)))
            .collect();
        let mut groups: Vec<(Vec<DUid>, Vec<OpenParcel>)> = Vec::new();
        let mut short: BTreeSet<String> = BTreeSet::new();
        for mut p in open {
            let mut key: Vec<DUid> = vehicles.iter().filter(|v| v.hub == p.hub && fits(&v.platform, v.kind, &p)).map(|v| v.uid.clone()).collect();
            if key.is_empty() {
                // a vehicle with room free at another base of the same site: the parcel goes there
                let site = site_of(&p.hub);
                let other = site.iter().filter(|h| **h != p.hub).find(|h| vehicles.iter().any(|v| v.hub == **h && fits(&v.platform, v.kind, &p))).cloned();
                if let Some(to) = other {
                    let from = std::mem::replace(&mut p.hub, to.clone());
                    if let Some(r) = self.rt().parcels.get_mut(&p.id) {
                        r.spec.from = to.clone();
                    }
                    self.log_delivery(
                        "transfer",
                        None,
                        Some(&p.id),
                        &format!("{} ({} kg) moves from {from} to {to}: no free vehicle at {from} carries it with room now, and one at {to} does", p.id, p.mass_kg),
                        json!({ "from": from, "to": to, "mass_kg": p.mass_kg, "gust_mps": gust.is_finite().then_some(gust) }),
                    );
                    key = vehicles.iter().filter(|v| v.hub == p.hub && fits(&v.platform, v.kind, &p)).map(|v| v.uid.clone()).collect();
                }
            }
            if key.is_empty() {
                // it waits for room; if no vehicle of its site could carry it now, it is short
                let site = site_of(&p.hub);
                let can = fleet.iter().any(|(base, plat, kind)| base.as_ref().is_some_and(|b| site.contains(b)) && fits(plat, *kind, &p));
                if !can {
                    short.insert(p.id.clone());
                    self.parcel_short(&p, &site, gust, now);
                }
                continue;
            }
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, ps)) => ps.push(p),
                None => groups.push((key, vec![p])),
            }
        }
        // a parcel that has room again is not short
        let cleared: Vec<String> = {
            let mut rt = self.rt();
            let gone: Vec<String> = rt.short_since.keys().filter(|k| !short.contains(*k)).cloned().collect();
            for k in &gone {
                rt.short_since.remove(k);
            }
            gone
        };
        for k in cleared {
            self.rt().issues.remove(&format!("no_vehicle:{k}"));
        }
        groups
    }

    /// A parcel no vehicle of its site carries within the payload margin now: it waits, and after
    /// [`NO_ROOM_ISSUE_S`] the person is told, once per parcel (an issue alert while it lasts).
    fn parcel_short(&self, p: &OpenParcel, site: &[String], gust: f64, now: Time) {
        let since = *self.rt().short_since.entry(p.id.clone()).or_insert(now.0);
        let waited_s = (now.0 - since) as f64 / 1000.0;
        let first = self.rt().short_said.insert(p.id.clone());
        if first {
            self.log_delivery(
                "wait",
                None,
                Some(&p.id),
                &format!("{} ({} kg) waits at {}: no vehicle of {} carries it within the payload margin now", p.id, p.mass_kg, p.hub, site.join(", ")),
                json!({ "codes": ["PAYLOAD_NO_ROOM"], "order": "assign", "mass_kg": p.mass_kg, "gust_mps": gust.is_finite().then_some(gust) }),
            );
        }
        if waited_s < NO_ROOM_ISSUE_S {
            return;
        }
        let d = self.rules.delivery_rules();
        let wind = if gust.is_finite() { format!(" in a {gust:.1} m/s gust") } else { String::new() };
        self.keep_issue(
            format!("no_vehicle:{}", p.id),
            Issue {
                kind: "parcel_no_vehicle",
                uid: None,
                problem: format!(
                    "{} ({} kg, to {}) has waited {:.0} min at {}: no vehicle of {} carries it within the payload margin{wind} (each limit, derated for the wind, times {:.0}%)",
                    p.id,
                    p.mass_kg,
                    p.zone,
                    waited_s / 60.0,
                    p.hub,
                    site.join(" or "),
                    d.payload.hold_frac * 100.0
                ),
                did: "automation keeps it at the hub and gives it to the first vehicle that carries it with room (a calmer wind, a bigger vehicle back); it has asked no one to judge a drone's margin".into(),
                recommend: "if it should go now, send it another way or split it; to fly it on a drone at the margin, order its pickup yourself (the payload check will ask you). It stays open until it is carried".into(),
                at_wall_s: crate::clock::wall(),
                until_ms: None,
                intent: None,
                track_ids: Vec::new(),
            },
            30_000,
            now,
        );
    }

    fn t0_ms(&self, rt: &Runtime) -> i64 {
        rt.t0.map_or(0, |t| t.0)
    }

    fn proposal_allowed(&self, rt: &mut Runtime, now: Time, per_min: u32) -> bool {
        rt.proposals.retain(|t| now.0 - *t < 60_000);
        if rt.proposals.len() as u32 >= per_min {
            return false;
        }
        rt.proposals.push(now.0);
        true
    }

    /// Vehicles with an order of any kind that is not over.
    fn with_open_orders(&self, mut busy: BTreeSet<DUid>) -> BTreeSet<DUid> {
        let core = self.lock();
        for i in core.engine.intents() {
            if matches!(i.state(), State::Proposed | State::Checked | State::AutoApproved | State::PendingApproval | State::Approved | State::Queued | State::Dispatched) {
                busy.extend(i.spec().uids.iter().cloned());
            }
        }
        busy
    }

    async fn propose(&self, agent: &str, req: IntentRequest, what: &str) -> Option<IntentId> {
        // the rules read the parcels as they are now
        self.publish_parcels();
        match self.agent_propose(&AgentId::new(agent), req).await {
            Ok(o) => Some(o.id),
            Err(e) => {
                eprintln!("{agent}: {what}: {}", e.detail);
                self.log_delivery("refused", None, None, &format!("{agent}: {what}: {}", e.detail), Value::Null);
                None
            }
        }
    }

    async fn propose_pickup(&self, uid: &DUid, parcel: &str, hub: &str, why: &str, now: Time) {
        let Some(b) = bridge_uid(uid) else { return };
        // the link along the trip, main and backup, before the rules read it
        let spec = self.rt().parcels.get(parcel).map(|p| p.spec.clone());
        if let (Some(h), Some(spec)) = (self.places.hubs.get(hub), spec)
            && let Some(z) = self.places.drop_zones.get(&spec.to)
        {
            {
                let backup = self.world.vehicles().into_iter().find(|c| c.uid == b).and_then(|c| c.read(|v| v.identity.as_ref().and_then(|i| i.str("backup_link").map(str::to_string))));
                if let Some(l) = self.link_along(uid, h.at(), z.at(), backup.as_deref(), now).await {
                    self.feed().link.insert(uid.clone(), l);
                }
            }
        }
        // rules 5: on the hub's stands it loads where it stands
        let at = self.pickup_point(uid, hub).map(|p| [p.lon, p.lat]);
        let params = TaskParams { parcel: Some(parcel.into()), site: Some(SiteParam::Id(hub.into())), at, ..TaskParams::default() };
        let req = IntentRequest {
            task: "pickup".into(),
            uids: vec![b],
            params,
            constraints: Default::default(),
            rationale: Some(format!("dispatcher: {why}")),
            group: None,
        };
        // a hold automation waits out (another vehicle at the hub, a stale fact): the parcel goes
        // back to the assigner and the vehicle waits a moment, with no one asked
        self.publish_parcels();
        if let Some(codes) = self.wait_out(&req).await {
            let a = self.auto();
            {
                let mut rt = self.rt();
                if let Some(p) = rt.parcels.get_mut(parcel) {
                    p.state = PState::Ready;
                    p.vehicle = None;
                }
                rt.cooldown.insert(uid.clone(), now.0 + (a.hold_retry_s * 1000.0) as i64);
            }
            self.say_wait(uid, Some(parcel), format!("{uid} waits before it loads {parcel} ({}): automation tries again", codes.join(", ")), json!({ "codes": codes, "order": "pickup" }));
            // rules 3: shown with what automation does for other traffic, while it waits
            if for_traffic(&codes) {
                let why = self.wait_out_why(&req).await.map(|(_, w)| w).unwrap_or_default();
                let hub = self.vehicle_hub(uid).unwrap_or_else(|| "its hub".into());
                self.rt().traffic_waits.insert(
                    uid.clone(),
                    TrafficWait {
                        since_ms: now.0,
                        why,
                        codes: codes.clone(),
                        did: format!("{uid} waits on the ground at {hub} before it loads {parcel}"),
                        under_way: false,
                        parcel: Some(parcel.to_string()),
                        until_ms: Some(now.0 + ((a.hold_retry_s + 5.0) * 1000.0) as i64),
                    },
                );
            }
            return;
        }
        let id = self.propose(DISPATCHER, req, "pickup").await;
        let mut rt = self.rt();
        rt.waits_said.remove(uid);
        match id {
            Some(id) => {
                rt.jobs.insert(uid.clone(), Job::new(Some(parcel.into()), Stage::Pickup, Some(id), now.0));
            }
            None => {
                if let Some(p) = rt.parcels.get_mut(parcel) {
                    p.state = PState::Ready;
                    p.vehicle = None;
                }
            }
        }
    }

    async fn propose_deliver(&self, uid: &DUid, parcel: &str, now: Time) {
        let Some(b) = bridge_uid(uid) else { return };
        // not heard: proposed once it is (its own failsafe has it meanwhile)
        if !self.heard(uid) {
            let mut rt = self.rt();
            let j = rt.jobs.entry(uid.clone()).or_insert_with(|| Job::new(Some(parcel.into()), Stage::Deliver, None, now.0));
            j.stage = Stage::Deliver;
            j.intent = None;
            j.not_before_ms = now.0 + 15_000;
            return;
        }
        let (zone, due_ms) = {
            let rt = self.rt();
            match rt.parcels.get(parcel) {
                Some(p) => (p.spec.to.clone(), self.t0_ms(&rt) + (p.spec.due_s * 1000.0) as i64),
                None => return,
            }
        };
        {
            let mut rt = self.rt();
            rt.jobs.entry(uid.clone()).or_insert_with(|| Job::new(Some(parcel.into()), Stage::Deliver, None, now.0)).stage = Stage::Deliver;
        }
        let a = self.auto();
        let job = self.rt().jobs.get(uid).cloned();
        let since = job.as_ref().and_then(|j| j.waiting_since_ms);
        let escalated = job.as_ref().is_some_and(|j| j.escalated);
        // the look before departure: a zone that does not look clear makes the loaded vehicle
        // wait at the hub and look again; after depart_wait_s the parcel stays there for later
        // (one under way, held for other traffic, looks again over the zone)
        let at_hub = self.vehicle_hub(uid).is_some();
        if at_hub && let Err(why) = self.zone_look(&zone, now, false) {
            let since = since.unwrap_or(now.0);
            if (now.0 - since) as f64 / 1000.0 >= a.depart_wait_s {
                self.rt().jobs.remove(uid);
                self.rt().waits_said.remove(uid);
                self.log_delivery("look", Some(uid), Some(parcel), &format!("{zone} has not looked clear for {:.0} s ({why}): {parcel} stays at the hub, {uid} is free", a.depart_wait_s), json!({ "zone": zone, "where": "before departure", "kept": true }));
                let hub = self.vehicle_hub(uid);
                let again = self.rt().parcels.get(parcel).is_some_and(|r| r.returns + 1 < a.max_tries);
                let at_s = self.elapsed(&self.rt(), now);
                self.parcel_ended(
                    parcel,
                    TryEnd {
                        stage: "look",
                        ended: "kept",
                        codes: vec!["DROP_NOT_CLEAR".into()],
                        detail: format!("{zone} did not look clear for {:.0} s before departure ({why}): the parcel stayed at the hub", a.depart_wait_s),
                        vehicle: Some(uid.to_string()),
                        intent: None,
                        at_s,
                    },
                );
                self.parcel_back(parcel, hub.as_deref(), now, "kept at the hub");
                self.raise(
                    format!("delivery:{parcel}"),
                    Issue {
                        kind: "drop_not_clear",
                        uid: Some(uid.clone()),
                        problem: format!("{parcel} did not set off: {zone} has not looked clear for {:.0} s ({why})", a.depart_wait_s),
                        did: format!("{uid} did not leave {}; the parcel stays there and {uid} takes other work", hub.as_deref().unwrap_or("the hub")),
                        recommend: if again {
                            format!("nothing is needed for safety; automation tries {parcel} again in {:.0} min. If {zone} stays busy, have the customer choose another spot (a new spot needs your approval)", a.retry_after_s / 60.0)
                        } else {
                            format!("{parcel} has been tried {} times and will not be sent again: contact the customer, or order it to another approved spot", a.max_tries)
                        },
                        at_wall_s: crate::clock::wall(),
                        until_ms: Some(now.0 + (a.issue_show_s * 1000.0) as i64),
                        intent: None,
                        track_ids: Vec::new(),
                    },
                );
                return;
            }
            {
                let mut rt = self.rt();
                if let Some(j) = rt.jobs.get_mut(uid) {
                    j.waiting_since_ms = Some(since);
                    j.intent = None;
                    j.not_before_ms = now.0 + 5_000;
                }
            }
            self.say_wait(uid, Some(parcel), format!("{zone} does not look clear ({why}): {uid} waits at the hub with {parcel} and looks again"), json!({ "zone": zone, "look": why }));
            return;
        }
        let release = self
            .world
            .vehicles()
            .into_iter()
            .find(|c| c.uid == b)
            .and_then(|c| c.read(|v| v.kind))
            .map_or("land", |k| match k {
                crate::mav::Kind::Copter | crate::mav::Kind::Blimp => "winch",
                crate::mav::Kind::Plane | crate::mav::Kind::Vtol => "slip",
                crate::mav::Kind::Boat => "dock",
                _ => "slip",
            });
        let via = job.as_ref().map(|j| j.via.clone()).unwrap_or_default();
        let params = TaskParams {
            parcel: Some(parcel.into()),
            drop_zone: Some(zone.clone()),
            release: Some(release.into()),
            window_end: Some(rfc3339(due_ms + 3_600_000)),
            route: (!via.is_empty()).then(|| via.iter().map(|p| [p.lon, p.lat]).collect()),
            ..TaskParams::default()
        };
        let req = IntentRequest {
            task: "deliver".into(),
            uids: vec![b],
            params,
            constraints: Default::default(),
            rationale: Some(if via.is_empty() {
                format!("dispatcher: {parcel} is aboard {uid}: take it to {zone}")
            } else {
                format!("dispatcher: {parcel} is aboard {uid}: take it to {zone} by a route round other traffic")
            }),
            group: None,
        };
        // a hold automation waits out: the vehicle waits at the hub with the parcel and tries
        // again; after hold_retry_max_s the order goes as it is and a person decides
        self.publish_parcels();
        let held = self.wait_out_why(&req).await;
        // rules 3: other traffic is automation's to wait out, or to go round; the person hears of a
        // conflict that outlasts that, and is never sent the order for it (even one an earlier
        // hold already escalated)
        if let Some((codes, why)) = &held
            && for_traffic(codes)
            && !codes.iter().any(|c| SEQUENCE_CODES.contains(&c.as_str()) && self.rules.at_least(fleetmesh_decide::HUB_SEQUENCING_SINCE))
        {
            let since = since.unwrap_or(now.0);
            self.wait_for_traffic(uid, Some(parcel), Stage::Deliver, &req, codes, why, since, now).await;
            return;
        }
        // rules 5: the hub, or another of the fleet's vehicles, is in the way: sequenced, never a
        // person's
        if let Some((codes, why)) = self.sequenced_why(&req, Stage::Deliver).await {
            let since = since.unwrap_or(now.0);
            self.wait_for_fleet(uid, Some(parcel), Stage::Deliver, &codes, &why, since, now);
            return;
        }
        if !escalated && let Some((codes, _)) = held {
            let since = since.unwrap_or(now.0);
            if (now.0 - since) as f64 / 1000.0 < a.hold_retry_max_s {
                {
                    let mut rt = self.rt();
                    if let Some(j) = rt.jobs.get_mut(uid) {
                        j.waiting_since_ms = Some(since);
                        j.waiting_on = codes.clone();
                        j.intent = None;
                        j.not_before_ms = now.0 + (a.hold_retry_s * 1000.0) as i64;
                    }
                }
                let look = codes.iter().all(|c| LOOK_CODES.contains(&c.as_str()));
                let what = if look { format!("{zone} does not look clear") } else { "a hold automation waits out".into() };
                self.say_wait(uid, Some(parcel), format!("{what} ({}): {uid} waits at the hub with {parcel} and tries again", codes.join(", ")), json!({ "codes": codes, "order": "deliver" }));
                return;
            }
            self.log_delivery("wait", Some(uid), Some(parcel), &format!("{uid} waited {:.0} s on {} for {parcel}: the delivery goes to a person", a.hold_retry_max_s, codes.join(", ")), json!({ "codes": codes, "escalated": true }));
            if let Some(j) = self.rt().jobs.get_mut(uid) {
                j.escalated = true;
            }
        }
        let id = self.propose(DISPATCHER, req, "deliver").await;
        let mut rt = self.rt();
        rt.waits_said.remove(uid);
        rt.traffic_waits.remove(uid);
        let job = rt.jobs.entry(uid.clone()).or_insert_with(|| Job::new(Some(parcel.into()), Stage::Deliver, None, now.0));
        job.stage = Stage::Deliver;
        job.intent = id;
        job.confirm = None;
        job.proposed_ms = now.0;
        job.waiting_since_ms = None;
        job.waiting_on.clear();
        job.via.clear();
        job.replanned = false;
        if id.is_none() {
            job.not_before_ms = now.0 + 30_000;
        }
    }

    /// Send a vehicle back to a hub: from rules 4 its home base ([`Self::return_target`]), before
    /// them the nearest; with `parcel`, one it is bringing back undelivered. Returns the hub. A
    /// hold automation waits out holds it where it is (a safety order is never delayed for long:
    /// after hold_retry_max_s it goes as it is).
    async fn propose_return(&self, uid: &DUid, now: Time, parcel: Option<&str>) -> Option<String> {
        let b = bridge_uid(uid)?;
        // not heard: its own lost-link failsafe has it; the return is proposed once it is heard
        if !self.heard(uid) {
            let mut rt = self.rt();
            let j = rt.jobs.entry(uid.clone()).or_insert_with(|| Job::new(parcel.map(str::to_string), Stage::Return, None, now.0));
            j.stage = Stage::Return;
            j.intent = None;
            j.not_before_ms = now.0 + 15_000;
            return None;
        }
        let (pos, ap_home) = self.world.vehicles().into_iter().find(|c| c.uid == b).map(|c| c.read(|v| (v.pos, v.home)))?;
        let pos = LonLat::new(pos?.1, pos?.0);
        let (hub, home) = self.return_target(uid, pos, ap_home.map(|h| LonLat::new(h.1, h.0))).await?;
        // rules 5: its own stand at its base, else a free pad
        let at = self.stand_at(uid, &hub).map(|p| [p.lon, p.lat]);
        let params = TaskParams { hub: Some(hub.clone()), at, ..TaskParams::default() };
        let here = self.vehicle_hub(uid).filter(|h| *h != hub);
        let why = match (parcel, &here) {
            (Some(p), _) => format!("dispatcher: {uid} could not deliver {p}: back to {hub} with it"),
            (None, Some(h)) => format!("dispatcher: {uid} is idle at {h}, away from its base: home to {hub}"),
            (None, None) if home => format!("dispatcher: {uid} is done: home to {hub}"),
            (None, None) => format!("dispatcher: {uid} is done: back to {hub}"),
        };
        let req = IntentRequest {
            task: "return_to_hub".into(),
            uids: vec![b],
            params,
            constraints: Default::default(),
            rationale: Some(why),
            group: None,
        };
        let a = self.auto();
        let job = self.rt().jobs.get(uid).filter(|j| j.stage == Stage::Return).cloned();
        let since = job.as_ref().and_then(|j| j.waiting_since_ms);
        let held = self.wait_out_why(&req).await;
        if let Some((codes, why)) = &held
            && for_traffic(codes)
        {
            let since = since.unwrap_or(now.0);
            if job.is_none() {
                self.rt().jobs.insert(uid.clone(), Job::new(parcel.map(str::to_string), Stage::Return, None, now.0));
            }
            self.wait_for_traffic(uid, parcel, Stage::Return, &req, codes, why, since, now).await;
            return Some(hub);
        }
        // rules 5: a routine return never waits for a person: what automation waits out (the hub's
        // sequencing above all) is waited out for as long as it lasts, and a long wait is an issue
        if let Some((codes, why)) = self.sequenced_why(&req, Stage::Return).await {
            let since = since.unwrap_or(now.0);
            self.wait_for_fleet(uid, parcel, Stage::Return, &codes, &why, since, now);
            return Some(hub);
        }
        if !job.as_ref().is_some_and(|j| j.escalated)
            && let Some((codes, _)) = held
        {
            let since = since.unwrap_or(now.0);
            if (now.0 - since) as f64 / 1000.0 < a.hold_retry_max_s {
                let mut j = job.unwrap_or_else(|| Job::new(parcel.map(str::to_string), Stage::Return, None, now.0));
                j.waiting_since_ms = Some(since);
                j.waiting_on = codes.clone();
                j.intent = None;
                j.not_before_ms = now.0 + (a.hold_retry_s * 1000.0) as i64;
                self.rt().jobs.insert(uid.clone(), j);
                self.say_wait(uid, parcel, format!("{uid} holds before it heads to {hub} ({}): automation tries again", codes.join(", ")), json!({ "codes": codes, "order": "return_to_hub" }));
                return Some(hub);
            }
        }
        let id = self.propose(DISPATCHER, req, "return_to_hub").await;
        self.rt().waits_said.remove(uid);
        self.rt().traffic_waits.remove(uid);
        match id {
            Some(id) => {
                self.rt().jobs.insert(uid.clone(), Job::new(parcel.map(str::to_string), Stage::Return, Some(id), now.0));
            }
            None => {
                // refused: the parcel, if it had one, is off the job's books where it is
                if let Some(p) = parcel {
                    let at_s = self.elapsed(&self.rt(), now);
                    self.parcel_ended(
                        p,
                        TryEnd {
                            stage: "return",
                            ended: "rejected",
                            codes: Vec::new(),
                            detail: format!("{uid}'s return to {hub} with it was refused"),
                            vehicle: Some(uid.to_string()),
                            intent: None,
                            at_s,
                        },
                    );
                    self.parcel_back(p, None, now, "could not be brought back by order");
                }
                self.rt().jobs.remove(uid);
            }
        }
        Some(hub)
    }

    /// Where a vehicle goes back to, and whether that is its home base. From rules 4
    /// ([`fleetmesh_decide::HOME_BASE_SINCE`]) its home base, unless the way there would not go at
    /// once (its dry run is refused, or held for a limit a person judges: the battery, the
    /// weather): then the hub nearest it, as before rules 4.
    async fn return_target(&self, uid: &DUid, pos: LonLat, ap_home: Option<LonLat>) -> Option<(String, bool)> {
        let nearest = self.places.nearest_hub(pos).map(|(h, _)| h.id.clone());
        if !self.rules.at_least(fleetmesh_decide::HOME_BASE_SINCE) {
            return nearest.map(|h| (h, false));
        }
        let Some(home) = self.home_hub(uid, ap_home).filter(|h| self.places.hubs.get(h).is_some_and(|x| x.open)) else {
            return nearest.map(|h| (h, false));
        };
        if nearest.as_deref() == Some(home.as_str()) {
            return Some((home, true));
        }
        let b = bridge_uid(uid)?;
        let req = IntentRequest {
            task: "return_to_hub".into(),
            uids: vec![b],
            params: TaskParams { hub: Some(home.clone()), at: self.stand_at(uid, &home).map(|p| [p.lon, p.lat]), ..TaskParams::default() },
            constraints: Default::default(),
            rationale: Some(format!("dispatcher: {uid} home to {home}")),
            group: None,
        };
        self.publish_parcels();
        // rules 5: a way home that the hub or another of ours only delays is still the way home (it
        // waits, and goes): not a reason to send it to another base
        if self.sequenced_why(&req, Stage::Return).await.is_some() {
            return Some((home, true));
        }
        let goes = match self.agent_dry_run(&AgentId::new(DISPATCHER), &req).await {
            Ok(p) if p.would_reach == State::AutoApproved => true,
            // held only for what automation waits out (traffic near the hub, a stale reading)
            Ok(p) if p.would_reach == State::PendingApproval => p
                .checks
                .iter()
                .filter(|c| matches!(c.outcome, fleetmesh_decide::Outcome::Hold | fleetmesh_decide::Outcome::Unverified))
                .all(|c| !c.codes.is_empty() && c.codes.iter().all(|x| WAIT_OUT.contains(&x.as_str()))),
            _ => false,
        };
        if goes {
            return Some((home, true));
        }
        let back = nearest?;
        self.log_delivery(
            "home",
            Some(uid),
            None,
            &format!("{uid} cannot go home to {home} at once (its checks would not let it): back to {back} instead"),
            json!({ "home": home, "hub": back }),
        );
        Some((back, false))
    }

    /// Rules 4: a vehicle idle at a hub other than its home base, with no parcel waiting at that
    /// hub, for [`REBASE_AFTER_S`], is sent home (a `return_to_hub` the dispatcher proposes, routed
    /// by policy like any return). Returns `busy` with the ones sent.
    async fn send_visitors_home(
        &self,
        world: &fleetmesh_decide::WorldSnapshot,
        mut busy: BTreeSet<DUid>,
        open: &[OpenParcel],
        below: f64,
        now: Time,
    ) -> BTreeSet<DUid> {
        if !self.rules.at_least(fleetmesh_decide::HOME_BASE_SINCE) {
            return busy;
        }
        let waiting: BTreeSet<&str> = open.iter().map(|p| p.hub.as_str()).collect();
        let visiting: BTreeSet<DUid> = self
            .free_vehicles(world, &busy, below)
            .into_iter()
            .filter(|v| {
                v.home.as_ref().is_some_and(|h| *h != v.hub && self.places.hubs.get(h).is_some_and(|x| x.open)) && !waiting.contains(v.hub.as_str())
            })
            .map(|v| v.uid)
            .collect();
        let due: Vec<DUid> = {
            let mut rt = self.rt();
            rt.visiting_since.retain(|u, _| visiting.contains(u));
            let mut due = Vec::new();
            for u in &visiting {
                let since = *rt.visiting_since.entry(u.clone()).or_insert(now.0);
                if now.0 - since >= REBASE_AFTER_S * 1000 {
                    due.push(u.clone());
                }
            }
            due
        };
        for u in due.into_iter().take(REBASE_PER_TICK) {
            // tried again after another wait, whatever comes of it
            self.rt().visiting_since.insert(u.clone(), now.0);
            let Some(b) = bridge_uid(&u) else { continue };
            let Some((pos, ap_home)) = self.world.vehicles().into_iter().find(|c| c.uid == b).map(|c| c.read(|v| (v.pos, v.home))) else { continue };
            let Some(pos) = pos.map(|p| LonLat::new(p.1, p.0)) else { continue };
            // only when it can go home now (else it stays where it is, ready, and is tried again)
            if !matches!(self.return_target(&u, pos, ap_home.map(|h| LonLat::new(h.1, h.0))).await, Some((_, true))) {
                continue;
            }
            if self.propose_return(&u, now, None).await.is_some() {
                busy.insert(u);
            }
        }
        busy
    }

    /// The dispatcher's work on each vehicle and what automation waits out for it, for the fleet
    /// view ([`super::fleet`]).
    pub(super) fn fleet_work(&self, now: Time) -> (BTreeMap<DUid, super::fleet::Work>, BTreeMap<DUid, String>) {
        let (jobs, parcels, mut holds) = {
            let rt = self.rt();
            let jobs: Vec<(DUid, Job)> = rt.jobs.iter().map(|(u, j)| (u.clone(), j.clone())).collect();
            let parcels: BTreeMap<String, (String, String)> = rt.parcels.iter().map(|(id, p)| (id.clone(), (p.spec.from.clone(), p.spec.to.clone()))).collect();
            let mut holds: BTreeMap<DUid, String> = BTreeMap::new();
            // rules 3: kept clear of other traffic, on the ground or under way
            for (u, w) in &rt.traffic_waits {
                if w.until_ms.is_none_or(|t| now.0 < t) {
                    holds.insert(u.clone(), w.did.clone());
                }
            }
            // a vehicle waiting a moment before it loads (a hold automation waits out)
            for (u, until) in &rt.cooldown {
                if now.0 < *until
                    && let Some(said) = rt.waits_said.get(u)
                {
                    holds.entry(u.clone()).or_insert_with(|| said.clone());
                }
            }
            (jobs, parcels, holds)
        };
        let mut work = BTreeMap::new();
        for (u, j) in jobs {
            let stage = match j.stage {
                Stage::Pickup => super::fleet::Stage::Pickup,
                Stage::Deliver => super::fleet::Stage::Deliver,
                Stage::Return => super::fleet::Stage::Return,
                Stage::Swap => super::fleet::Stage::Swap,
            };
            let (running, hub) = match j.intent {
                Some(id) => {
                    let core = self.lock();
                    core.engine.intent(id).map_or((false, None), |i| {
                        let hub = match &i.spec().task {
                            fleetmesh_decide::Task::ReturnToHub { hub: Some(h) } => h.id.clone(),
                            fleetmesh_decide::Task::SwapBattery { hub, .. } => Some(hub.clone()),
                            _ => None,
                        };
                        (i.state() == State::Dispatched, hub)
                    })
                }
                None => (false, None),
            };
            let route = j.parcel.as_ref().and_then(|p| parcels.get(p)).cloned();
            let to = match stage {
                super::fleet::Stage::Deliver => route.as_ref().map(|r| r.1.clone()),
                super::fleet::Stage::Pickup => route.as_ref().map(|r| r.0.clone()),
                _ => hub,
            };
            let zone_at = (stage == super::fleet::Stage::Deliver)
                .then(|| to.as_ref().and_then(|z| self.places.drop_zones.get(z)).map(|z| z.at()))
                .flatten();
            if j.waiting_since_ms.is_some() && !j.escalated {
                let look = !j.waiting_on.is_empty() && j.waiting_on.iter().all(|c| LOOK_CODES.contains(&c.as_str()));
                let what = match (look, &to) {
                    (true, Some(z)) => format!("waits for {z} to look clear"),
                    _ if for_traffic(&j.waiting_on) => "keeps clear of other traffic".to_string(),
                    _ if j.waiting_on.is_empty() => "waits a moment and tries again".to_string(),
                    _ => format!("waits out {}", j.waiting_on.join(", ")),
                };
                holds.entry(u.clone()).or_insert(what);
            }
            work.insert(u, super::fleet::Work { stage, running, parcel: j.parcel.clone(), to, zone_at });
        }
        (work, holds)
    }

    /// A vehicle at a hub with its battery under `below` gets a swap, if the hub has a pack (a
    /// hub with none: the vehicle waits there for one to charge, with no one asked).
    async fn propose_swaps(&self, world: &fleetmesh_decide::WorldSnapshot, busy: &BTreeSet<DUid>, below: f64, now: Time) {
        let low: Vec<(DUid, String)> = world
            .vehicles
            .iter()
            .filter(|(u, _)| !busy.contains(*u))
            .filter_map(|(u, v)| {
                let (pos, batt) = (v.position.as_ref()?.value, v.battery_pct.as_ref()?);
                let (hub, d) = self.places.nearest_hub(pos)?;
                (batt.value < below && d <= hub.radius_m * 1.5 + 30.0).then(|| (u.clone(), hub.id.clone()))
            })
            .collect();
        for (uid, hub) in low {
            let Some(b) = bridge_uid(&uid) else { continue };
            let packs = self.feed().hub_packs.get(&hub).copied().unwrap_or_else(|| self.places.hubs.get(&hub).map_or(0, |h| h.packs));
            if packs == 0 {
                self.say_wait(&uid, None, format!("{uid} waits at {hub} for a charged pack (none in stock): automation swaps it when one is ready"), json!({ "hub": hub, "order": "swap_battery" }));
                continue;
            }
            let params = TaskParams { hub: Some(hub.clone()), ..TaskParams::default() };
            let req = IntentRequest {
                task: "swap_battery".into(),
                uids: vec![b],
                params,
                constraints: Default::default(),
                rationale: Some(format!("dispatcher: {uid} is under {below:.0}% at {hub}: swap its battery")),
                group: None,
            };
            let id = self.propose(DISPATCHER, req, "swap_battery").await;
            self.rt().waits_said.remove(&uid);
            if let Some(id) = id {
                self.rt().jobs.insert(uid.clone(), Job::new(None, Stage::Swap, Some(id), now.0));
            }
        }
    }

    /// Packs: a swap that went out took one from its hub, which charges the spent one again.
    fn recharge(&self, now: Time) {
        let started: Vec<(IntentId, String, DUid)> = {
            let core = self.lock();
            core.engine
                .intents()
                .filter(|i| i.state() == State::Dispatched || i.state() == State::Done)
                .filter_map(|i| match &i.spec().task {
                    fleetmesh_decide::Task::SwapBattery { hub, .. } => Some((i.id(), hub.clone(), i.spec().uids.first()?.clone())),
                    _ => None,
                })
                .collect()
        };
        let mut notes = Vec::new();
        {
            let mut rt = self.rt();
            let charge_s = self.rules.delivery_rules().hub.charge_s;
            for (id, hub, uid) in started {
                if rt.swaps_taken.insert(id) {
                    let mut feed = self.feed();
                    let n = feed.hub_packs.entry(hub.clone()).or_insert_with(|| self.places.hubs.get(&hub).map_or(0, |h| h.packs));
                    *n = n.saturating_sub(1);
                    notes.push((hub.clone(), uid, *n));
                    drop(feed);
                    rt.charging.push((now.0 + i64::from(charge_s) * 1000, hub));
                }
            }
            let (ready, wait): (Vec<_>, Vec<_>) = rt.charging.drain(..).partition(|(t, _)| *t <= now.0);
            rt.charging = wait;
            for (_, hub) in ready {
                let mut feed = self.feed();
                let n = feed.hub_packs.entry(hub.clone()).or_insert_with(|| self.places.hubs.get(&hub).map_or(0, |h| h.packs));
                *n += 1;
                notes.push((hub, DUid::new("-"), *n));
            }
        }
        for (hub, uid, n) in notes {
            self.log_delivery("hub", if uid.as_str() == "-" { None } else { Some(&uid) }, None, &format!("hub {hub} has {n} charged pack(s)"), json!({ "hub": hub, "packs": n }));
        }
    }

    // ---- the navigation monitor -------------------------------------------------------------

    /// Every vehicle's navigation trust, each second (the hysteresis the rules read is kept here),
    /// and a safe order proposed for a vehicle with a running order that falls to untrusted.
    async fn watch_navigation(&self, now: Time) {
        let world = self.snapshot();
        let d = self.rules.delivery_rules();
        let Some(di) = world.delivery.as_ref() else { return };
        let mut falls: Vec<Fall> = Vec::new();
        {
            let mut rt = self.rt();
            for (uid, v) in &world.vehicles {
                // the conditions as they are now, without the hysteresis the rules add to them
                let mut bare = di.nav.get(uid).cloned();
                if let Some(n) = bare.as_mut() {
                    n.prev_state = None;
                    n.good_for_s = None;
                }
                let (raw, causes) = fleetmesh_decide::delivery_rules::nav_state(v, bare.as_ref(), &world, &d);
                let seen = rt.nav.entry(uid.clone()).or_insert(NavSeen { state: NavTrust::Unknown, good_since_ms: None });
                if raw >= seen.state || seen.state == NavTrust::Unknown {
                    // a worse (or the same) state at once; the first reading as it is
                    if raw != seen.state && matches!(raw, NavTrust::Degraded | NavTrust::Untrusted) {
                        falls.push((uid.clone(), raw, causes));
                    }
                    *seen = NavSeen { state: raw, good_since_ms: None };
                } else {
                    // a better one only after the conditions hold for recover_s
                    let since = *seen.good_since_ms.get_or_insert(now.0);
                    if now.0 - since >= (d.nav.recover_s * 1000.0) as i64 {
                        *seen = NavSeen { state: raw, good_since_ms: None };
                    }
                }
            }
        }
        for (uid, state, causes) in falls {
            let why: Vec<String> = causes.iter().map(|(c, t)| format!("{c}: {t}")).collect();
            if state == NavTrust::Untrusted {
                // a new fall: what was done for an earlier one is not this one's
                self.rt().nav_proposed.remove(&uid);
            }
            self.log_delivery(
                "nav",
                Some(&uid),
                None,
                &format!("{uid} navigation is {}: {}", state.as_str(), why.join("; ")),
                json!({ "state": state.as_str(), "causes": causes.iter().map(|(c, _)| *c).collect::<Vec<_>>() }),
            );
            if state != NavTrust::Untrusted {
                continue;
            }
            // only a vehicle with an order running (or about to) needs a safe one
            let running = {
                let core = self.lock();
                core.engine.intents().any(|i| {
                    matches!(i.state(), State::Dispatched | State::Queued | State::Approved | State::AutoApproved | State::PendingApproval)
                        && i.spec().uids.contains(&uid)
                        && matches!(i.spec().kind(), fleetmesh_decide::TaskKind::Pickup | fleetmesh_decide::TaskKind::Deliver | fleetmesh_decide::TaskKind::ReturnToHub | fleetmesh_decide::TaskKind::Reroute)
                })
            };
            if !running {
                continue;
            }
            let Some(b) = bridge_uid(&uid) else { continue };
            let air_copter = matches!(world.vehicles.get(&uid).and_then(|v| v.autopilot.as_ref()).map(|a| a.value), Some(AutopilotKind::Copter));
            let (task, what) = if air_copter { ("land_now", "put down") } else { ("hold", "held where it is") };
            let req = IntentRequest {
                task: task.into(),
                uids: vec![b],
                params: TaskParams::default(),
                constraints: Default::default(),
                rationale: Some(format!("nav-monitor: {uid}'s position fix is untrusted ({}) while an order runs: {what}", why.join("; "))),
                group: None,
            };
            if let Some(id) = self.propose(NAV_MONITOR, req, task).await {
                let action = NavAction { intent: id, task, at_wall_s: crate::clock::wall(), why: why.join("; ") };
                self.rt().nav_proposed.insert(uid, action);
            }
        }
        // what the rules read of each vehicle's trust (the hysteresis)
        let seen: BTreeMap<DUid, NavSeen> = self.rt().nav.clone();
        let mut feed = self.feed();
        feed.nav_hysteresis = seen
            .into_iter()
            .map(|(u, s)| (u, (s.state, s.good_since_ms.map_or(0.0, |t| (now.0 - t) as f64 / 1000.0))))
            .collect();
    }

    // ---- other traffic (rules 3) -------------------------------------------------------------

    /// An issue that lasts while its cause does: raised (and logged) the first time, kept quietly
    /// after, shown until `tail_ms` after the last time it was kept.
    fn keep_issue(&self, id: String, mut issue: Issue, tail_ms: i64, now: Time) {
        issue.until_ms = Some(now.0 + tail_ms);
        let first = self.rt().issues.get(&id).map(|i| i.at_wall_s);
        match first {
            Some(at) => {
                issue.at_wall_s = at;
                self.rt().issues.insert(id, issue);
            }
            None => self.raise(id, issue),
        }
    }

    /// Rules 3: one of the dispatcher's orders held for other traffic. The vehicle stays where it
    /// is (on the ground at its hub, or held under way) and the order is tried again every
    /// `hold_retry_s`; a delivery that has waited `traffic_replan_after_s` for a conflict is offered
    /// a route round it once (it goes when the rules pass it); a conflict that outlasts
    /// `hold_retry_max_s` is the person's to hear of (`traffic_conflict`), and the order is never
    /// sent to the inbox for it.
    #[allow(clippy::too_many_arguments)]
    async fn wait_for_traffic(&self, uid: &DUid, parcel: Option<&str>, stage: Stage, req: &IntentRequest, codes: &[String], why: &str, since: i64, now: Time) {
        let a = self.auto();
        let waited_s = (now.0 - since) as f64 / 1000.0;
        let hub = self.vehicle_hub(uid);
        let under_way = hub.is_none();
        let conflict = codes.iter().any(|c| c != "SEP_PICTURE_DEGRADED" && TRAFFIC_CODES.contains(&c.as_str()));
        let mut did = match &hub {
            Some(h) => format!("{uid} waits on the ground at {h}"),
            None => format!("{uid} holds where it is"),
        };
        // a route round it, once in a wait (a delivery: a return names only its hub)
        let replanned = self.rt().jobs.get(uid).is_some_and(|j| j.replanned);
        if stage == Stage::Deliver && conflict && waited_s >= a.traffic_replan_after_s && !replanned {
            if let Some(j) = self.rt().jobs.get_mut(uid) {
                j.replanned = true;
            }
            let dest = req.params.drop_zone.as_ref().and_then(|z| self.places.drop_zones.get(z)).map(|z| z.at());
            if let Some(dest) = dest
                && let Some(wps) = self.traffic_detour(uid, dest)
            {
                let mut alt = req.clone();
                alt.params.route = Some(wps.iter().map(|p| [p.lon, p.lat]).collect());
                if self.goes_auto(&alt).await {
                    if let Some(j) = self.rt().jobs.get_mut(uid) {
                        j.via = wps.clone();
                        j.intent = None;
                        j.not_before_ms = now.0;
                    }
                    self.log_delivery(
                        "traffic",
                        Some(uid),
                        parcel,
                        &format!("{uid} waited {waited_s:.0} s for other traffic ({}): it goes by a route round it, by {}", codes.join(", "), wps.iter().map(|p| format!("{:.5},{:.5}", p.lat, p.lon)).collect::<Vec<_>>().join(" ")),
                        json!({ "response": "replan", "codes": codes, "via": wps.iter().map(|p| [p.lon, p.lat]).collect::<Vec<_>>(), "waited_s": waited_s }),
                    );
                    return;
                }
            }
            did.push_str("; no route round it that the rules pass was found");
        }
        {
            let mut rt = self.rt();
            if let Some(j) = rt.jobs.get_mut(uid) {
                j.waiting_since_ms = Some(since);
                j.waiting_on = codes.to_vec();
                j.intent = None;
                j.not_before_ms = now.0 + (a.hold_retry_s * 1000.0) as i64;
            }
            rt.traffic_waits.insert(
                uid.clone(),
                TrafficWait { since_ms: since, why: why.to_string(), codes: codes.to_vec(), did: did.clone(), under_way, parcel: parcel.map(str::to_string), until_ms: None },
            );
        }
        self.say_wait(uid, parcel, format!("{did} for other traffic ({}): automation tries again", codes.join(", ")), json!({ "codes": codes, "order": if stage == Stage::Return { "return_to_hub" } else { "deliver" }, "traffic": true }));
        if conflict && waited_s >= a.hold_retry_max_s {
            let what = if stage == Stage::Return { "its return" } else { "its delivery" };
            self.keep_issue(
                format!("traffic_conflict:{uid}"),
                Issue {
                    kind: "traffic_conflict",
                    uid: Some(uid.clone()),
                    problem: format!("{uid}'s predicted conflict with other traffic has lasted {:.0} min: {why}", waited_s / 60.0),
                    did: format!("{did} and tries {what} again every {:.0} s{}", a.hold_retry_s, if replanned { "; it looked for a route round it and none clears" } else { "" }),
                    recommend: "nothing is needed for safety: it goes when the traffic has passed. If it should not wait, order it another route or a return yourself".into(),
                    at_wall_s: crate::clock::wall(),
                    until_ms: None,
                    intent: None,
                    track_ids: tracks_named(why),
                },
                30_000,
                now,
            );
        }
    }

    /// Rules 3: a route for `uid` from where it is to `dest` round the first conflict a straight one
    /// meets ([`fleetmesh_decide::traffic::detour`], within the reroute's automatic limit).
    fn traffic_detour(&self, uid: &DUid, dest: LonLat) -> Option<Vec<LonLat>> {
        let b = self.traffic.built()?;
        let world = self.snapshot();
        let v = world.vehicles.get(uid)?;
        let platform = v.platform.as_ref()?;
        let pos = v.position.as_ref()?.value;
        let d = self.rules.delivery_rules();
        let k = d.separation.traffic_knobs();
        let alt = self.cruise_msl(&world, uid);
        let refs = [pos, dest];
        let km = d.separation.relevant_km;
        fleetmesh_decide::traffic::detour(
            pos,
            dest,
            platform.cruise_mps.unwrap_or(0.0).max(0.1),
            alt,
            platform.domain,
            self.vehicle_hub(uid).is_none(),
            &b.picture,
            world.t,
            &k,
            d.separation.step_s,
            d.separation.surface_surface_m,
            d.reroute.auto_added_pct / 100.0,
            |t| fleetmesh_decide::traffic::relevant(t, &refs, km),
        )
    }

    /// A vehicle's cruise altitude above mean sea level, as the separation rule takes it (its
    /// platform's cruise height above home, and home's height), none on the surface.
    fn cruise_msl(&self, world: &fleetmesh_decide::WorldSnapshot, uid: &DUid) -> Option<f64> {
        let v = world.vehicles.get(uid)?;
        let p = v.platform.as_ref()?;
        if p.domain != fleetmesh_decide::Domain::Air {
            return None;
        }
        let rel = world.delivery.as_ref().and_then(|d| d.traffic.get(uid)).and_then(|t| t.alt_m);
        let home = match (v.alt_msl_m.as_ref(), rel) {
            (Some(msl), Some(rel)) => msl.value - rel,
            _ => 0.0,
        };
        Some(p.typical_alt_m.unwrap_or(0.0) + home)
    }

    /// Rules 3, every 2 s: the traffic monitor. A delivery or return under way whose route ahead
    /// the picture predicts a conflict on is held where it is (a drone hovers, a boat stops: a
    /// `hold`, which goes at once) and tried again as soon as it is clear; when the traffic comes
    /// at it where it is, a delivery takes a route round it, or holds and the person hears of it.
    /// And the issues the picture itself raises: a feed or receiver not heard, a broadcast our
    /// sensors contradict, an aircraft that broadcasts nothing near a drone.
    async fn watch_traffic(&self, now: Time) {
        if !self.rules.at_least(fleetmesh_decide::TRAFFIC_SEPARATION_SINCE) || !self.traffic.active() {
            return;
        }
        {
            let mut rt = self.rt();
            if now.0 - rt.last_traffic_ms < 2_000 {
                return;
            }
            rt.last_traffic_ms = now.0;
            // a wait that ended (its order went, or the job is over) is not a wait
            let live: BTreeSet<DUid> = rt.jobs.iter().filter(|(_, j)| j.intent.is_none()).map(|(u, _)| u.clone()).collect();
            rt.traffic_waits.retain(|u, w| live.contains(u) || w.until_ms.is_some_and(|t| now.0 < t));
        }
        let Some(b) = self.traffic.built() else { return };
        let world = self.snapshot();
        self.picture_issues(&b, &world, now);
        let d = self.rules.delivery_rules();
        let k = d.separation.traffic_knobs();
        let a = self.auto();
        let running: Vec<(DUid, Job, IntentId)> = self
            .rt()
            .jobs
            .iter()
            .filter(|(_, j)| matches!(j.stage, Stage::Deliver | Stage::Return))
            .filter_map(|(u, j)| Some((u.clone(), j.clone(), j.intent?)))
            .collect();
        for (uid, job, id) in running {
            let task = {
                let core = self.lock();
                match core.engine.intent(id) {
                    Some(i) if i.state() == State::Dispatched => i.spec().task.clone(),
                    _ => continue,
                }
            };
            let Some(v) = world.vehicles.get(&uid) else { continue };
            let Some(platform) = v.platform.as_ref() else { continue };
            if platform.domain == fleetmesh_decide::Domain::Ground || self.vehicle_hub(&uid).is_some() {
                continue;
            }
            let Some(pos) = v.position.as_ref().map(|p| p.value) else { continue };
            let Some(path) = task.path() else { continue };
            let Some(dest) = path.points.last().copied() else { continue };
            if fleetmesh_decide::geo::haversine_m(pos, dest) < 60.0 {
                continue;
            }
            let ahead = remaining_route(pos, &path.points);
            let alt = match platform.domain {
                fleetmesh_decide::Domain::Air => v.alt_msl_m.as_ref().map(|a| a.value).or_else(|| self.cruise_msl(&world, &uid)),
                _ => None,
            };
            let speed = platform.cruise_mps.unwrap_or(0.0).max(0.1);
            let horizon = if platform.domain == fleetmesh_decide::Domain::Surface { k.surface_horizon_s.max(k.horizon_s) } else { k.horizon_s };
            let mut refs = ahead.clone();
            refs.push(pos);
            let km = d.separation.relevant_km;
            let relevant = |t: &fleetmesh_decide::delivery::TrackFact| fleetmesh_decide::traffic::relevant(t, &refs, km);
            let samples = fleetmesh_decide::traffic::trajectory(&ahead, speed, alt, d.separation.step_s, horizon);
            let found = fleetmesh_decide::traffic::assess(&samples, &ahead, speed, platform.domain, true, &b.picture, world.t, &k, d.separation.surface_surface_m, relevant);
            let Some(c) = found.conflicts.first().cloned() else { continue };
            let here = fleetmesh_decide::traffic::trajectory(&[pos], 0.0, alt, d.separation.step_s, horizon);
            let hover_ok = fleetmesh_decide::traffic::assess(&here, &[pos], 0.0, platform.domain, true, &b.picture, world.t, &k, d.separation.surface_surface_m, relevant).conflicts.is_empty();
            let stop = if platform.domain == fleetmesh_decide::Domain::Surface { "stops and stands off" } else { "holds where it is" };
            // a route round it, when holding here does not keep it clear
            if !hover_ok
                && job.stage == Stage::Deliver
                && let Some(wps) = fleetmesh_decide::traffic::detour(pos, dest, speed, alt, platform.domain, true, &b.picture, world.t, &k, d.separation.step_s, d.separation.surface_surface_m, d.reroute.auto_added_pct / 100.0, relevant)
            {
                {
                    let mut rt = self.rt();
                    if let Some(j) = rt.jobs.get_mut(&uid) {
                        j.via = wps.clone();
                        j.intent = None;
                        j.not_before_ms = now.0;
                        j.waiting_since_ms = Some(now.0);
                        j.replanned = true;
                    }
                }
                self.log_delivery(
                    "traffic",
                    Some(&uid),
                    job.parcel.as_deref(),
                    &format!("{uid} under way: {}: holding there would not keep it clear, so it takes a route round it", c.words()),
                    json!({ "response": "replan", "track": c.track, "code": c.code, "dt_s": c.dt_s, "ratio": c.ratio, "via": wps.iter().map(|p| [p.lon, p.lat]).collect::<Vec<_>>() }),
                );
                continue;
            }
            let Some(b_uid) = bridge_uid(&uid) else { continue };
            let req = IntentRequest {
                task: "hold".into(),
                uids: vec![b_uid],
                params: TaskParams::default(),
                constraints: Default::default(),
                rationale: Some(format!("dispatcher: {}: {uid} {stop} until it is clear (it gives way)", c.words())),
                group: None,
            };
            let Some(hold) = self.propose(DISPATCHER, req, "hold").await else { continue };
            let did = if hover_ok { format!("{uid} {stop} (i{})", hold.0) } else { format!("{uid} {stop} (i{}); it is coming its way and no route round it clears", hold.0) };
            {
                let mut rt = self.rt();
                if let Some(j) = rt.jobs.get_mut(&uid) {
                    j.intent = None;
                    j.waiting_since_ms = Some(now.0);
                    j.waiting_on = vec![c.code.to_string()];
                    j.not_before_ms = now.0 + (a.hold_retry_s * 1000.0) as i64;
                    j.replanned = !hover_ok;
                }
                rt.traffic_waits.insert(uid.clone(), TrafficWait { since_ms: now.0, why: c.words(), codes: vec![c.code.to_string()], did: did.clone(), under_way: true, parcel: job.parcel.clone(), until_ms: None });
            }
            self.log_delivery(
                "traffic",
                Some(&uid),
                job.parcel.as_deref(),
                &format!("{uid} under way: {}: {did}", c.words()),
                json!({ "response": "hold", "hold": hold.0, "track": c.track, "code": c.code, "dt_s": c.dt_s, "ratio": c.ratio, "hover_clear": hover_ok }),
            );
            // the person hears of it if it lasts: the order's own wait raises traffic_conflict
        }
    }

    /// Rules 3: the issues the picture itself raises, each while it lasts (and a minute after).
    fn picture_issues(&self, b: &super::traffic::Built, world: &fleetmesh_decide::WorldSnapshot, now: Time) {
        let d = self.rules.delivery_rules();
        let k = d.separation.traffic_knobs();
        let reach = d.separation.relevant_km * 1000.0;
        let fleet_at: Vec<(DUid, LonLat, bool)> = world
            .vehicles
            .iter()
            .filter_map(|(u, v)| {
                let air = v.platform.as_ref().is_some_and(|p| p.domain == fleetmesh_decide::Domain::Air);
                Some((u.clone(), v.position.as_ref()?.value, air))
            })
            .collect();
        let ops: Vec<LonLat> = fleet_at.iter().map(|(_, p, _)| *p).chain(self.places.hubs.values().map(|h| h.at())).collect();
        let near_ops = |p: LonLat| ops.iter().any(|o| fleetmesh_decide::geo::haversine_m(*o, p) <= reach);
        // the picture degraded: what automation holds for it. Not while it is starting (its feeds
        // connecting: no one is told), and cleared as soon as it is whole again
        let degraded = b.picture.state == fleetmesh_decide::delivery::PictureState::Degraded;
        if !degraded && self.rt().issues.remove("traffic_picture").is_some() {
            self.log_delivery("issue", None, None, "traffic_picture_degraded cleared: the traffic picture is whole again", json!({ "id": "traffic_picture", "kind": "traffic_picture_degraded", "cleared": true }));
        }
        if degraded && !self.traffic.starting() {
            let held: Vec<DUid> = self.rt().traffic_waits.iter().filter(|(_, w)| w.codes.iter().any(|c| c == "SEP_PICTURE_DEGRADED")).map(|(u, _)| u.clone()).collect();
            let did = if held.is_empty() {
                "other receivers or the network feed still cover the fleet's routes: automation holds nothing for it".to_string()
            } else {
                format!(
                    "automation holds the departures whose routes depend on that coverage ({} now: {}) and sends each when a receiver heard covers its route again",
                    held.len(),
                    held.iter().map(DUid::as_str).collect::<Vec<_>>().join(", ")
                )
            };
            self.keep_issue(
                "traffic_picture".into(),
                Issue {
                    kind: "traffic_picture_degraded",
                    uid: None,
                    problem: format!("the traffic picture is degraded: {}", b.picture.reasons.join("; ")),
                    did,
                    recommend: "check the receiver or the feed named; nothing else is needed".into(),
                    at_wall_s: crate::clock::wall(),
                    until_ms: None,
                    intent: None,
                    track_ids: Vec::new(),
                },
                20_000,
                now,
            );
        }
        // a broadcast our own sensors contradict, near operations
        for dis in &b.disagreements {
            let Some(t) = b.raw.iter().find(|t| t.id == dis.track) else { continue };
            // an aircraft's: a marine radar's disagreements with AIS are mostly vessels that move
            // alike, one seen and the other not (the tracker's limit: no detection model), and the
            // separation rule keeps clear of both tracks anyway
            if t.domain != fleetmesh_tracker::Domain::Air || !near_ops(LonLat::new(t.lon, t.lat)) {
                continue;
            }
            let label = t.ids.callsign.clone().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).or_else(|| t.ids.icao24.clone()).unwrap_or_else(|| t.id.clone());
            let class = serde_json::to_value(t.class).ok().and_then(|v| v.as_str().map(|s| s.replace('_', " "))).unwrap_or_default();
            self.keep_issue(
                format!("traffic_disagree:{label}"),
                Issue {
                    kind: "traffic_broadcast_disagrees",
                    uid: None,
                    problem: format!(
                        "{class} {label} broadcasts a position {:.0} m from where our sensor ({}) sees an aircraft moving as it does, and the sensor sees nothing where it says it is: a faulty or spoofed broadcast",
                        dis.offset_m, dis.sensor
                    ),
                    did: "automation keeps well clear of both: the broadcast position and the one our sensors see each count as a manned aircraft, for departures and for flights under way".into(),
                    recommend: "nothing is needed for safety; if it lasts, report the faulty broadcast (to the aircraft's operator or air traffic control)".into(),
                    at_wall_s: crate::clock::wall(),
                    until_ms: None,
                    intent: None,
                    track_ids: vec![dis.track.clone(), dis.sensed.clone()],
                },
                30_000,
                now,
            );
        }
        // an aircraft that broadcasts nothing near one of our drones (one issue per object: a track
        // that reappears near where an earlier one was is the same issue)
        for t in &b.picture.tracks {
            if fleetmesh_decide::traffic::kind_of(t, &k) != Some(fleetmesh_decide::traffic::TrackKind::Noncoop) {
                continue;
            }
            let line = t.line.as_ref();
            let at = line.map_or(t.at, |l| l.from);
            let near = fleet_at
                .iter()
                .filter(|(_, _, air)| *air)
                .map(|(u, p, _)| (u, fleetmesh_decide::geo::haversine_m(*p, at)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some((drone, dist)) = near.filter(|(_, d)| *d <= 2.0 * k.noncoop_h_m) else { continue };
            // seen for a while (a sensor's false alarm, and a crossing of two of them, does not last)
            let first = *self.rt().noncoop_first.entry(t.id.clone()).or_insert(now.0);
            if now.0 - first < 10_000 {
                continue;
            }
            let id = {
                let mut rt = self.rt();
                let found = rt
                    .noncoop_seen
                    .iter()
                    .find(|(_, (p, last))| now.0 - *last < 60_000 && fleetmesh_decide::geo::haversine_m(*p, at) <= 500.0)
                    .map(|(id, _)| id.clone());
                let id = found.unwrap_or_else(|| format!("traffic_noncoop:{}", t.id));
                rt.noncoop_seen.insert(id.clone(), (at, now.0));
                rt.noncoop_seen.retain(|_, (_, last)| now.0 - *last < 120_000);
                let live: BTreeSet<String> = b.picture.tracks.iter().map(|x| x.id.clone()).collect();
                rt.noncoop_first.retain(|k, _| live.contains(k));
                id
            };
            let seen = match line {
                Some(l) => format!(
                    "seen only as a bearing ({:.0} degrees from {drone}'s detect-and-avoid, somewhere between {:.0} and {:.0} m out)",
                    l.bearing_deg, l.range_m[0], l.range_m[1]
                ),
                None => format!("{} ({:.0} m from {drone})", t.id, dist),
            };
            self.keep_issue(
                id,
                Issue {
                    kind: "traffic_noncooperative_near",
                    uid: Some(drone.clone()),
                    problem: format!("an aircraft that broadcasts nothing (no ADS-B, no Remote ID) near {drone}: {seen}"),
                    did: format!(
                        "automation keeps drones {:.0} m clear of it and of its uncertainty (a bearing-only one along its whole line): departures and flights whose routes pass near it wait, hold or go round it",
                        k.noncoop_h_m
                    ),
                    recommend: "nothing is needed for safety; if it stays near a hub or the routes, look at it (a drone's camera) and report it if it is a hazard".into(),
                    at_wall_s: crate::clock::wall(),
                    until_ms: None,
                    intent: None,
                    track_ids: vec![t.id.clone()],
                },
                60_000,
                now,
            );
        }
    }

    // ---- reads ------------------------------------------------------------------------------

    /// `GET /api/traffic` and the console's `traffic` message: the picture ([`super::traffic`])
    /// and what automation is doing for other traffic now (`automation`: each vehicle it keeps
    /// clear, since when, why and how), shown and never asked about.
    pub fn traffic_json(&self, bounds: Option<[f64; 4]>) -> Value {
        let mut v = self.traffic.api_json(bounds);
        let now = self.clock.now();
        let waits: Vec<Value> = self
            .rt()
            .traffic_waits
            .iter()
            .map(|(u, w)| {
                json!({
                    "uid": u.as_str(), "since_s": ((now.0 - w.since_ms) as f64 / 1000.0).max(0.0).round(),
                    "codes": w.codes, "why": w.why, "did": w.did, "under_way": w.under_way, "parcel": w.parcel,
                })
            })
            .collect();
        if let Some(o) = v.as_object_mut() {
            o.insert("automation".into(), json!(waits));
        }
        v
    }

    /// `GET /api/parcels` and MCP `get_parcels`.
    pub fn parcels_json(&self) -> Value {
        let rt = self.rt();
        let now = self.clock.now();
        let el = self.elapsed(&rt, now);
        let list: Vec<Value> = rt
            .parcels
            .values()
            .map(|p| {
                json!({
                    "id": p.spec.id,
                    "state": p.state.name(),
                    "from": p.spec.from,
                    "to": p.spec.to,
                    "mass_kg": p.spec.mass_kg,
                    "priority": p.spec.priority,
                    "class": p.spec.class,
                    "ready_s": p.spec.ready_s,
                    "due_s": p.spec.due_s,
                    "vehicle": p.vehicle,
                    "tries": p.tries,
                    "returns": p.returns,
                    // why it failed, and how its earlier tries ended (display only)
                    "failure": p.failure,
                    "ends": p.ends,
                })
            })
            .collect();
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        for p in rt.parcels.values() {
            *counts.entry(p.state.name()).or_insert(0) += 1;
        }
        json!({
            "scenario": rt.scenario.as_ref().map(|s| json!({"name": s.name, "title": s.title, "seed": s.seed, "duration_s": s.duration_s})),
            "elapsed_s": el,
            "counts": counts,
            "parcels": list,
        })
    }

    /// `GET /api/places`: the hubs (with the packs each has now) and the drop zones, each with the
    /// latest look at it (what the scenario's feed saw, with its age), for the console's map.
    pub fn places_json(&self) -> Value {
        let feed = self.feed();
        let now = self.clock.now();
        let hubs: Vec<Value> = self
            .places
            .hubs
            .values()
            .map(|h| {
                json!({
                    "id": h.id, "name": h.name, "at": [h.lon, h.lat], "radius_m": h.radius_m, "pads": h.pads,
                    "packs": feed.hub_packs.get(&h.id).copied().unwrap_or(h.packs), "open": h.open,
                })
            })
            .collect();
        let zones: Vec<Value> = self
            .places
            .drop_zones
            .values()
            .map(|z| {
                let seen = feed.observations.get(&z.id).map(|o| {
                    json!({
                        "state": match o.state {
                            ObservedState::Clear => "clear",
                            ObservedState::Occupied => "occupied",
                            ObservedState::Unknown => "unknown",
                        },
                        "count": o.count,
                        "age_s": (now.ms_since(o.t) as f64 / 1000.0).max(0.0),
                    })
                });
                json!({
                    "id": z.id, "kind": z.kind, "at": [z.lon, z.lat], "radius_m": z.radius_m, "surface": z.surface,
                    "closed": z.closed, "populated": z.populated, "hub": z.hub, "observation": seen,
                })
            })
            .collect();
        json!({ "hubs": hubs, "drop_zones": zones })
    }

    /// `GET /api/weather/grid`: the weather the rules read, in the shape the console's weather
    /// panel takes. It is the scenario's own sampler (archive, baseline wind and visibility, the
    /// scripted ramp) as `weather_at` uses it, so the panel and the rules never disagree. None
    /// when the run has no delivery scenario.
    pub fn weather_grid_json(&self, bounds: [f64; 4]) -> Option<Value> {
        let now = self.clock.now();
        let rt = self.rt();
        let el = self.elapsed(&rt, now);
        let t_unix = scenario_unix(now, el);
        let s = rt.scenario.as_ref()?;
        let mut regions = s.store.regions(t_unix, el);
        let fill = |v: &mut fleetmesh_weather::Values| {
            if v.wind_east_mps.is_none() || v.wind_north_mps.is_none() {
                v.wind_east_mps = s.wx.wind_east_mps;
                v.wind_north_mps = s.wx.wind_north_mps;
            }
            if v.visibility_m.is_none() {
                v.visibility_m = s.wx.visibility_m;
            }
        };
        fill(&mut regions.outside);
        regions.points.iter_mut().for_each(|p| fill(&mut p.values));
        let sample = s.store.at((bounds[1] + bounds[3]) / 2.0, (bounds[0] + bounds[2]) / 2.0, t_unix, el);
        let known = regions.outside.wind_east_mps.is_some() || regions.points.iter().any(|p| p.values.wind_east_mps.is_some());
        let source = if sample.synthetic || sample.model.is_some() { sample.source } else { "scenario baseline".to_string() };
        Some(json!({
            "available": known,
            "bounds": bounds,
            "nlon": 0, "nlat": 0, "cells": [],
            "t": t_unix, "age_s": 0.0, "stale": false,
            "source": source,
            "model": sample.model,
            "synthetic": sample.synthetic,
            "regions": regions,
        }))
    }

    /// `GET /api/weather` and MCP `get_weather`: the weather the rules read at one point, now
    /// (`Scenario::weather_at`). None when the run has no delivery scenario or no source says.
    pub fn weather_point_json(&self, lat: f64, lon: f64) -> Option<Value> {
        let now = self.clock.now();
        let rt = self.rt();
        let el = self.elapsed(&rt, now);
        let f = rt.scenario.as_ref()?.weather_at(LonLat::new(lon, lat), now, el, scenario_unix(now, el))?;
        Some(json!({
            "available": true, "lat": lat, "lon": lon,
            "wind_east_mps": f.wind_e_mps, "wind_north_mps": f.wind_n_mps, "gust_mps": f.gust_mps,
            "visibility_m": f.visibility_m, "precipitation_mm": f.precip_mmh, "source": f.source,
        }))
    }

    /// `GET /api/nav`: each vehicle's navigation trust as the monitor holds it (`trusted`,
    /// `degraded`, `untrusted`, `unknown`), with what the monitor did for it, if it acted.
    pub fn nav_json(&self) -> Value {
        let rt = self.rt();
        let vehicles: serde_json::Map<String, Value> = rt
            .nav
            .iter()
            .map(|(u, s)| {
                let acted = rt.nav_proposed.get(u).filter(|_| s.state == NavTrust::Untrusted).map(|a| {
                    json!({ "intent": a.intent.0, "task": a.task, "at_wall_s": a.at_wall_s, "why": a.why })
                });
                (u.as_str().to_string(), json!({ "state": s.state.as_str(), "action": acted }))
            })
            .collect();
        json!({ "vehicles": vehicles })
    }

    /// Inbox alerts for what the navigation monitor did: a vehicle whose navigation is untrusted
    /// and for which it proposed a safe order. Derived from the monitor's own record, so it
    /// clears when the vehicle's navigation recovers.
    pub fn nav_alerts(&self) -> Vec<super::alerts::Alert> {
        let rt = self.rt();
        let fleet = self.fleet();
        rt.nav_proposed
            .iter()
            .filter(|(u, _)| rt.nav.get(*u).is_some_and(|s| s.state == NavTrust::Untrusted))
            .filter_map(|(u, a)| {
                let uid = bridge_uid(u)?;
                Some(nav_alert(uid, fleet.get(&uid).and_then(|c| super::alerts::callsign(c)), a))
            })
            .collect()
    }

    /// Whether the dispatcher is about to try a vehicle's order again (a retry is scheduled, or a
    /// newer order than `intent` is out): an alert about that order's op failing is held back
    /// while automation handles it, and the person is told if the retries run out.
    pub fn dispatcher_retrying(&self, uid: crate::vehicle::Uid, intent: Option<u64>) -> bool {
        // only the dispatcher's own orders: a person's or another agent's failure is always shown
        let Some(i) = intent else { return false };
        let ours = {
            let core = self.lock();
            core.engine.intent(IntentId(i)).is_some_and(|x| matches!(&x.spec().source, fleetmesh_decide::Source::Agent { id } if id.as_str() == DISPATCHER))
        };
        if !ours {
            return false;
        }
        let rt = self.rt();
        rt.jobs.get(&DUid::new(uid.to_string())).is_some_and(|j| match j.intent {
            None => true,
            Some(i) => intent.is_some_and(|x| i.0 > x),
        })
    }

    /// Inbox alerts for the issues automation raised ([`Issue`]): what happened, what it did and
    /// what the person might do. Never an approval: the person may act (approve a new spot, order
    /// the parcel elsewhere) or not.
    pub fn issue_alerts(&self) -> Vec<super::alerts::Alert> {
        let now = self.clock.now();
        let fleet = self.fleet();
        let mut rt = self.rt();
        rt.issues.retain(|_, i| i.until_ms.is_none_or(|u| now.0 < u));
        rt.issues
            .iter()
            .map(|(id, i)| {
                let uid = i.uid.as_ref().and_then(bridge_uid);
                let callsign = match uid {
                    Some(u) => fleet.get(&u).and_then(|c| super::alerts::callsign(c)),
                    // an issue about a place, or another aircraft: it names the card
                    None => id
                        .strip_prefix("zone:")
                        .or_else(|| id.strip_prefix("traffic_disagree:"))
                        .map(str::to_string)
                        .or_else(|| (id == "traffic_picture").then(|| "traffic picture".to_string())),
                };
                let severity = match i.kind {
                    "traffic_conflict" | "traffic_noncooperative_near" | "traffic_broadcast_disagrees" => super::alerts::Severity::High,
                    _ => super::alerts::Severity::Medium,
                };
                super::alerts::Alert {
                    id: id.clone(),
                    kind: i.kind,
                    severity,
                    uid: uid.unwrap_or(0),
                    callsign,
                    detail: format!("{}; automation: {}; you: {}", i.problem, i.did, i.recommend),
                    since_t: i.at_wall_s,
                    op_id: None,
                    intent_id: i.intent.map(|x| x.0),
                    issue: Some(super::alerts::IssueText { problem: i.problem.clone(), did: i.did.clone(), recommend: i.recommend.clone() }),
                    track_id: i.track_ids.first().cloned(),
                    track_ids: if i.track_ids.len() > 1 { i.track_ids.clone() } else { Vec::new() },
                }
            })
            .collect()
    }

    /// `GET /api/hubs` and MCP `get_hubs`: each hub with its packs now and who is at it.
    pub fn hubs_json(&self) -> Value {
        let feed = self.feed();
        let hubs: Vec<Value> = self
            .places
            .hubs
            .values()
            .map(|h| {
                json!({
                    "id": h.id, "name": h.name, "at": [h.lon, h.lat], "radius_m": h.radius_m, "pads": h.pads,
                    "packs": feed.hub_packs.get(&h.id).copied().unwrap_or(h.packs), "open": h.open,
                })
            })
            .collect();
        json!({ "hubs": hubs })
    }
}

/// The inbox alert for an action of the navigation monitor.
pub fn nav_alert(uid: crate::vehicle::Uid, callsign: Option<String>, a: &NavAction) -> super::alerts::Alert {
    let did = if a.task == "land_now" { "to put it down" } else { "to hold it where it is" };
    super::alerts::Alert {
        id: format!("nav_monitor:{uid}"),
        kind: "nav_monitor",
        severity: super::alerts::Severity::High,
        uid,
        callsign,
        detail: format!(
            "its navigation is untrusted ({}): the navigation monitor proposed {} {did}; it follows the order's policy and may wait for you",
            a.why,
            a.task.replace('_', " "),
        ),
        since_t: a.at_wall_s,
        op_id: None,
        intent_id: Some(a.intent.0),
        issue: Some(super::alerts::IssueText {
            problem: format!("its position fix is untrusted ({}) while an order runs", a.why),
            did: format!("the navigation monitor proposed {} {did} (i{}); a {} over people or away from a hub waits for you in the inbox", a.task.replace('_', " "), a.intent.0, a.task.replace('_', " ")),
            recommend: "approve the safe order if it waits for you (or order a hold or land elsewhere); watch the vehicle until its navigation recovers".into(),
        }),
        track_id: None,
        track_ids: Vec::new(),
    }
}

/// A vehicle whose navigation fell to a worse state, with its causes.
type Fall = (DUid, NavTrust, Vec<(&'static str, String)>);

struct TwSample {
    wind: Option<[f64; 2]>,
    current: Option<[f64; 2]>,
    level: Option<f64>,
    depth: Option<f64>,
    ground: Option<f64>,
}

fn pair(s: &Value, e: &str, n: &str) -> Option<[f64; 2]> {
    Some([s[e].as_f64()?, s[n].as_f64()?])
}

/// The traffic tracks a check's words name (`trk-` and its number), in order, once each.
fn tracks_named(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in text.match_indices("trk-") {
        let id: String = text[i..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        if id.len() > 4 && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The rest of a route `pts` from `pos`: from where the vehicle is to the far end of the leg it
/// is nearest, then on.
pub fn remaining_route(pos: LonLat, pts: &[LonLat]) -> Vec<LonLat> {
    if pts.len() < 2 {
        return std::iter::once(pos).chain(pts.iter().copied()).collect();
    }
    let mut best = (f64::INFINITY, 0usize);
    for (i, w) in pts.windows(2).enumerate() {
        let (ax, ay) = fleetmesh_decide::geo::enu(pos, w[0]);
        let (bx, by) = fleetmesh_decide::geo::enu(pos, w[1]);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let f = if len2 > 0.0 { (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let d = (ax + f * dx).hypot(ay + f * dy);
        if d < best.0 {
            best = (d, i);
        }
    }
    std::iter::once(pos).chain(pts[best.1 + 1..].iter().copied()).collect()
}

/// How far from a hub's point a vehicle still counts as at it (its pads, its dock).
pub fn hub_catchment(radius_m: f64) -> f64 {
    (radius_m * 3.0).max(150.0)
}

/// The scenario's clock `el` seconds into the run, unix seconds: `FLEETMESH_AT` plus the time since
/// the run began times `FLEETMESH_RATE`, else the bridge's own.
fn scenario_unix(now: Time, el: f64) -> f64 {
    fleetmesh_space::scenario_clock::from_env()
        .ok()
        .and_then(|s| Some(s.at_unix()? + el * s.rate))
        .unwrap_or(now.0 as f64 / 1000.0)
}

#[cfg(test)]
mod nav_alert_tests {
    use super::*;

    #[test]
    fn the_monitors_action_is_an_alert_naming_the_order_and_why() {
        let a = NavAction { intent: IntentId(41), task: "land_now", at_wall_s: 100.0, why: "NAV_NO_FIX: no fix".into() };
        let al = nav_alert(7, Some("DRONE 7".into()), &a);
        assert_eq!(al.kind, "nav_monitor");
        assert_eq!(al.id, "nav_monitor:7");
        assert_eq!(al.intent_id, Some(41));
        assert_eq!(al.since_t, 100.0);
        assert!(al.detail.contains("land now") && al.detail.contains("put it down") && al.detail.contains("NAV_NO_FIX"), "{}", al.detail);
        let h = nav_alert(8, None, &NavAction { task: "hold", ..a });
        assert!(h.detail.contains("hold it where it is"), "{}", h.detail);
        // an issue: what happened, what automation did, what the person might do
        let i = al.issue.as_ref().expect("issue text");
        assert!(i.problem.contains("untrusted") && i.did.contains("land now") && i.recommend.contains("approve the safe order"), "{i:?}");
    }
}
