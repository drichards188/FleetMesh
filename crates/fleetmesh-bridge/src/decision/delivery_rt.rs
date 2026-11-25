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
//! 3. runs the greedy assigner ([`fleetmesh_decide::dispatch::assign`]) every `tick_s` and proposes
//!    the first order of each assignment as the agent `dispatcher`, through the same port as any
//!    agent's proposal: policy routes it, a person decides what policy holds;
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
    #[serde(default = "default_mass")]
    mass_kg: [f64; 2],
    #[serde(default = "default_mix")]
    priority_mix: [f64; 4],
    #[serde(default = "default_due")]
    due_s: f64,
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
];

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
                let zone = g.zones[(rng.next() % g.zones.len() as u64) as usize].clone();
                let mass = g.mass_kg[0] + rng.unit() * (g.mass_kg[1] - g.mass_kg[0]);
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
                    due_s: t.round() + g.due_s,
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
}

impl ParcelRec {
    fn new(spec: ParcelSpec) -> ParcelRec {
        ParcelRec { spec, state: PState::Waiting, vehicle: None, assigned_ms: None, loaded_ms: None, delivered_ms: None, tries: 0, returns: 0, not_before_ms: 0 }
    }
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
}

impl Job {
    fn new(parcel: Option<String>, stage: Stage, intent: Option<IntentId>, now_ms: i64) -> Job {
        Job { parcel, stage, intent, confirm: None, proposed_ms: now_ms, not_before_ms: 0, waiting_since_ms: None, waiting_on: Vec::new(), escalated: false }
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
}

/// A return to a hub that fails (a mission upload with no answer, a link that drops mid-exchange)
/// is proposed again this many times before the person is told.
const RETURN_TRIES: u32 = 3;

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
        self.refresh_feed(now).await;
        self.follow_jobs(now).await;
        self.watch_navigation(now).await;
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
            // trusted navigation, not flying away from a hub
            let trust = self.rt().nav.get(uid).map(|n| n.state);
            if matches!(trust, Some(NavTrust::Untrusted | NavTrust::Degraded)) {
                continue;
            }
            let Some((hub, d)) = self.places.nearest_hub(pos) else { continue };
            if d > hub_catchment(hub.radius_m) {
                continue;
            }
            out.push(Candidate { uid: uid.clone(), hub: hub.id.clone(), platform, kind: ap, battery_pct: batt.value });
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
                    {
                        let mut rt = self.rt();
                        if let Some(r) = rt.parcels.get_mut(&p) {
                            r.state = PState::Ready;
                            r.vehicle = None;
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
                        },
                    );
                    return;
                }
                // not released: try again twice, then bring it home. A refusal is final, except
                // one for a predicted conflict with moving traffic, whose fix is a short delay
                // (delivery-rules.md section 8: the dispatcher re-proposes with the fix)
                let tries = {
                    let mut rt = self.rt();
                    let r = rt.parcels.get_mut(&p);
                    r.map_or(9, |r| {
                        r.tries += 1;
                        r.tries
                    })
                };
                let codes = self.intent_state(id).map(|(_, c)| c).unwrap_or_default();
                let sequencing = state == State::Rejected
                    && !codes.is_empty()
                    && codes.iter().all(|c| c == "SEP_ROUTE_CONFLICT" || WAIT_OUT.contains(&c.as_str()));
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
                            r.state = PState::Failed;
                        }
                        rt.jobs.remove(uid);
                    }
                    self.log_delivery("parcel", Some(uid), Some(&p), &format!("parcel {p} failed: not delivered, {uid} is sent home"), json!({ "state": "failed" }));
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
                        },
                    );
                }
            }
            Stage::Return => {
                self.rt().jobs.remove(uid);
                // a return that failed on its way out (an upload with no answer, a link that
                // dropped mid-exchange) is tried again: going home is the safe default
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
                r.state = PState::Failed;
                true
            } else {
                r.state = PState::Ready;
                r.not_before_ms = now.0 + (a.retry_after_s * 1000.0) as i64;
                false
            }
        };
        let at = hub.map_or(String::new(), |h| format!(" at {h}"));
        if failed {
            self.log_delivery("parcel", None, Some(p), &format!("parcel {p} {how}{at}: tried {} times, not sent again", a.max_tries), json!({ "state": "failed" }));
        } else {
            self.log_delivery("parcel", None, Some(p), &format!("parcel {p} {how}{at}: tried again in {:.0} s", a.retry_after_s), json!({ "state": "ready", "retry_s": a.retry_after_s }));
        }
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
        let p = self.agent_dry_run(&AgentId::new(DISPATCHER), req).await.ok()?;
        if p.would_reach != State::PendingApproval {
            return None;
        }
        let held: Vec<String> = p
            .checks
            .iter()
            .filter(|c| matches!(c.outcome, fleetmesh_decide::Outcome::Hold | fleetmesh_decide::Outcome::Unverified))
            .flat_map(|c| if c.codes.is_empty() { vec![format!("{}:{:?}", c.rule.as_str(), c.outcome)] } else { c.codes.clone() })
            .collect();
        (!held.is_empty() && held.iter().all(|c| WAIT_OUT.contains(&c.as_str()))).then_some(held)
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
        let swap_below = 45.0;
        self.propose_swaps(&world, &busy, swap_below, now).await;
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
        // each parcel is offered to the vehicles that carry it inside the payload rule's hold band:
        // a parcel near a drone's limit goes by a bigger vehicle (re-planned, not held for a
        // person); one no free vehicle carries so is offered to all (a person judges the margin).
        // The heaviest go first, to the few that can take them; each group is its own plan
        let hold_frac = self.rules.delivery_rules().payload.hold_frac;
        let roomy = |v: &Candidate, mass: f64| v.platform.payload_kg.is_none_or(|max| mass <= max * hold_frac);
        let mut groups: Vec<(Vec<DUid>, Vec<OpenParcel>)> = Vec::new();
        for p in open {
            let mut key: Vec<DUid> = vehicles.iter().filter(|v| roomy(v, p.mass_kg)).map(|v| v.uid.clone()).collect();
            if key.is_empty() {
                key = vehicles.iter().map(|v| v.uid.clone()).collect();
            }
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, ps)) => ps.push(p),
                None => groups.push((key, vec![p])),
            }
        }
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
            let plan = fleetmesh_decide::dispatch::assign(&input);
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
        let params = TaskParams { parcel: Some(parcel.into()), site: Some(SiteParam::Id(hub.into())), ..TaskParams::default() };
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
        if let Err(why) = self.zone_look(&zone, now, false) {
            let since = since.unwrap_or(now.0);
            if (now.0 - since) as f64 / 1000.0 >= a.depart_wait_s {
                self.rt().jobs.remove(uid);
                self.rt().waits_said.remove(uid);
                self.log_delivery("look", Some(uid), Some(parcel), &format!("{zone} has not looked clear for {:.0} s ({why}): {parcel} stays at the hub, {uid} is free", a.depart_wait_s), json!({ "zone": zone, "where": "before departure", "kept": true }));
                let hub = self.vehicle_hub(uid);
                let again = self.rt().parcels.get(parcel).is_some_and(|r| r.returns + 1 < a.max_tries);
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
        let params = TaskParams {
            parcel: Some(parcel.into()),
            drop_zone: Some(zone.clone()),
            release: Some(release.into()),
            window_end: Some(rfc3339(due_ms + 3_600_000)),
            ..TaskParams::default()
        };
        let req = IntentRequest {
            task: "deliver".into(),
            uids: vec![b],
            params,
            constraints: Default::default(),
            rationale: Some(format!("dispatcher: {parcel} is aboard {uid}: take it to {zone}")),
            group: None,
        };
        // a hold automation waits out: the vehicle waits at the hub with the parcel and tries
        // again; after hold_retry_max_s the order goes as it is and a person decides
        self.publish_parcels();
        if !escalated && let Some(codes) = self.wait_out(&req).await {
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
        let job = rt.jobs.entry(uid.clone()).or_insert_with(|| Job::new(Some(parcel.into()), Stage::Deliver, None, now.0));
        job.stage = Stage::Deliver;
        job.intent = id;
        job.confirm = None;
        job.proposed_ms = now.0;
        job.waiting_since_ms = None;
        job.waiting_on.clear();
        if id.is_none() {
            job.not_before_ms = now.0 + 30_000;
        }
    }

    /// Send a vehicle to its nearest hub; with `parcel`, one it is bringing back undelivered.
    /// Returns the hub. A hold automation waits out holds it where it is (a safety order is
    /// never delayed for long: after hold_retry_max_s it goes as it is).
    async fn propose_return(&self, uid: &DUid, now: Time, parcel: Option<&str>) -> Option<String> {
        let b = bridge_uid(uid)?;
        let pos = self.world.vehicles().into_iter().find(|c| c.uid == b).and_then(|c| c.read(|v| v.pos))?;
        let (hub, _) = self.places.nearest_hub(LonLat::new(pos.1, pos.0))?;
        let hub = hub.id.clone();
        let params = TaskParams { hub: Some(hub.clone()), ..TaskParams::default() };
        let why = match parcel {
            Some(p) => format!("dispatcher: {uid} could not deliver {p}: back to {hub} with it"),
            None => format!("dispatcher: {uid} is done: back to {hub}"),
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
        if !job.as_ref().is_some_and(|j| j.escalated)
            && let Some(codes) = self.wait_out(&req).await
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
        match id {
            Some(id) => {
                self.rt().jobs.insert(uid.clone(), Job::new(parcel.map(str::to_string), Stage::Return, Some(id), now.0));
            }
            None => {
                // refused: the parcel, if it had one, is off the job's books where it is
                if let Some(p) = parcel {
                    self.parcel_back(p, None, now, "could not be brought back by order");
                }
                self.rt().jobs.remove(uid);
            }
        }
        Some(hub)
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

    // ---- reads ------------------------------------------------------------------------------

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
                    // an issue about a place: the place names the card
                    None => id.strip_prefix("zone:").map(str::to_string),
                };
                super::alerts::Alert {
                    id: id.clone(),
                    kind: i.kind,
                    severity: super::alerts::Severity::Medium,
                    uid: uid.unwrap_or(0),
                    callsign,
                    detail: format!("{}; automation: {}; you: {}", i.problem, i.did, i.recommend),
                    since_t: i.at_wall_s,
                    op_id: None,
                    intent_id: i.intent.map(|x| x.0),
                    issue: Some(super::alerts::IssueText { problem: i.problem.clone(), did: i.did.clone(), recommend: i.recommend.clone() }),
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
