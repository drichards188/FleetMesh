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
}

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
}

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
                    rt.parcels.insert(
                        p.id.clone(),
                        ParcelRec { spec: p, state: PState::Waiting, vehicle: None, assigned_ms: None, loaded_ms: None, delivered_ms: None, tries: 0 },
                    );
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

    fn intent_state(&self, id: IntentId) -> Option<(State, Vec<String>)> {
        let core = self.lock();
        core.engine.intent(id).map(|i| (i.state(), i.checks().iter().flat_map(|c| c.codes.clone()).collect()))
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
            // a held deliver for an unconfirmed zone: the observation goes to a person as a
            // confirm_drop_zone, once
            if job.stage == Stage::Deliver
                && state == State::PendingApproval
                && job.confirm.is_none()
                && codes.iter().any(|c| c == "DZ_UNCONFIRMED")
                && let Some(p) = job.parcel.clone()
            {
                self.propose_confirmation(&uid, &p, now).await;
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
            self.job_ended(&uid, &job, state, now).await;
        }
    }

    async fn job_ended(&self, uid: &DUid, job: &Job, state: State, now: Time) {
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
                    self.propose_return(uid, now).await;
                } else {
                    // not released: try again twice, then bring it home
                    let tries = {
                        let mut rt = self.rt();
                        let r = rt.parcels.get_mut(&p);
                        r.map_or(9, |r| {
                            r.tries += 1;
                            r.tries
                        })
                    };
                    self.log_delivery("parcel", Some(uid), Some(&p), &format!("delivery of {p} by {uid} ended {state:?} before the parcel was released (try {tries})"), json!({ "state": "loaded", "ended": format!("{state:?}") }));
                    if tries < 3 && state != State::Rejected {
                        let mut rt = self.rt();
                        if let Some(j) = rt.jobs.get_mut(uid) {
                            j.intent = None;
                            j.not_before_ms = now.0 + 30_000;
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
                        self.propose_return(uid, now).await;
                    }
                }
            }
            Stage::Return | Stage::Swap => {
                self.rt().jobs.remove(uid);
            }
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
                .filter(|p| p.state == PState::Ready && p.vehicle.is_none())
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
        // a swap for a vehicle that came home low, before the next parcel
        let swap_below = 45.0;
        self.propose_swaps(&world, &busy, swap_below, now).await;
        if open.is_empty() {
            return;
        }
        let vehicles = self.free_vehicles(&world, &busy, swap_below);
        let weather = self.feed().weather.clone();
        let input = AssignInput {
            t: now,
            params: params.clone(),
            energy: self.rules.delivery_rules().energy.clone(),
            places: {
                let mut p = self.places.clone();
                for (id, n) in &self.feed().hub_packs {
                    if let Some(h) = p.hubs.get_mut(id) {
                        h.packs = *n;
                    }
                }
                p
            },
            weather,
            parcels: open,
            vehicles,
        };
        let plan = fleetmesh_decide::dispatch::assign(&input);
        if plan.assignments.is_empty() && plan.unassigned.is_empty() {
            return;
        }
        // a plan with nothing new (the same parcels left over for the same reasons) is not logged again
        let sig: BTreeSet<String> = plan.unassigned.iter().map(|(p, r)| format!("{p}:{}", r.join(","))).collect();
        let tick = {
            let mut rt = self.rt();
            rt.plan_tick += 1;
            let fresh = !plan.assignments.is_empty() || sig != rt.last_unassigned;
            rt.last_unassigned = sig;
            fresh.then_some(rt.plan_tick)
        };
        let Some(tick) = tick else { return };
        {
            let mut core = self.lock();
            if let Err(e) = core.engine.record_dispatch_plan(tick, input.clone(), plan.clone()) {
                eprintln!("dispatch: log plan: {e}");
            }
        }
        for a in &plan.assignments {
            {
                let mut rt = self.rt();
                if !self.proposal_allowed(&mut rt, now, params.max_proposals_per_min) {
                    break;
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
        let id = self.propose(DISPATCHER, req, "pickup").await;
        let mut rt = self.rt();
        match id {
            Some(id) => {
                rt.jobs.insert(uid.clone(), Job { parcel: Some(parcel.into()), stage: Stage::Pickup, intent: Some(id), confirm: None, proposed_ms: now.0, not_before_ms: 0 });
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
        let id = self.propose(DISPATCHER, req, "deliver").await;
        let mut rt = self.rt();
        let job = rt.jobs.entry(uid.clone()).or_insert(Job { parcel: Some(parcel.into()), stage: Stage::Deliver, intent: None, confirm: None, proposed_ms: now.0, not_before_ms: 0 });
        job.stage = Stage::Deliver;
        job.intent = id;
        job.confirm = None;
        job.proposed_ms = now.0;
        if id.is_none() {
            job.not_before_ms = now.0 + 30_000;
        }
    }

    async fn propose_confirmation(&self, uid: &DUid, parcel: &str, now: Time) {
        let Some(b) = bridge_uid(uid) else { return };
        let zone = match self.rt().parcels.get(parcel) {
            Some(p) => p.spec.to.clone(),
            None => return,
        };
        let obs = self.feed().observations.get(&zone).cloned();
        let seen = match &obs {
            Some(o) => format!("{:?} ({} seen) at {}", o.state, o.count, rfc3339(o.t.0)),
            None => "no look yet".into(),
        };
        let params = TaskParams {
            parcel: Some(parcel.into()),
            drop_zone: Some(zone.clone()),
            evidence: Some(obs.iter().map(|o| format!("obs:{zone}:{}", o.t.0)).collect()),
            ..TaskParams::default()
        };
        let req = IntentRequest {
            task: "confirm_drop_zone".into(),
            uids: vec![b],
            params,
            constraints: Default::default(),
            rationale: Some(format!("dispatcher: {zone} has not been confirmed for {parcel}; the latest look: {seen}")),
            group: None,
        };
        let id = self.propose(DISPATCHER, req, "confirm_drop_zone").await;
        if let Some(j) = self.rt().jobs.get_mut(uid) {
            j.confirm = Some(id.unwrap_or(IntentId(0)));
        }
        let _ = now;
    }

    async fn propose_return(&self, uid: &DUid, now: Time) {
        let Some(b) = bridge_uid(uid) else { return };
        let Some(pos) = self.world.vehicles().into_iter().find(|c| c.uid == b).and_then(|c| c.read(|v| v.pos)) else { return };
        let Some((hub, _)) = self.places.nearest_hub(LonLat::new(pos.1, pos.0)) else { return };
        let params = TaskParams { hub: Some(hub.id.clone()), ..TaskParams::default() };
        let req = IntentRequest {
            task: "return_to_hub".into(),
            uids: vec![b],
            params,
            constraints: Default::default(),
            rationale: Some(format!("dispatcher: {uid} is done: back to {}", hub.id)),
            group: None,
        };
        let id = self.propose(DISPATCHER, req, "return_to_hub").await;
        if let Some(id) = id {
            self.rt().jobs.insert(uid.clone(), Job { parcel: None, stage: Stage::Return, intent: Some(id), confirm: None, proposed_ms: now.0, not_before_ms: 0 });
        }
    }

    /// A vehicle at a hub with its battery under `below` gets a swap, if the hub has a pack.
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
            if let Some(id) = id {
                self.rt().jobs.insert(uid.clone(), Job { parcel: None, stage: Stage::Swap, intent: Some(id), confirm: None, proposed_ms: now.0, not_before_ms: 0 });
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
    }
}
