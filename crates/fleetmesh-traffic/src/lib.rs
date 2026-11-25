//! fleetmesh-traffic: the traffic FleetMesh does not control, and what the company's receivers
//! and sensors make of it, in real wire formats. See the README and docs/design/traffic.md.
//!
//! As a library (the bridge, the headless simulator):
//!
//! ```no_run
//! use fleetmesh_traffic::{Options, Traffic};
//! let mut t = Traffic::from_files("sim/traffic.toml".as_ref(), Some("sim/scenarios/harbour-city/traffic.toml".as_ref()), Options::default())?;
//! let start = t.start_ms();
//! for s in 1..=60 {
//!     // t.set_own_vehicles(...) whenever the fleet's positions change
//!     for e in t.step_to(start + s * 1000) {
//!         // e.stream is Sbs, Ais or Json; e.line is the exact bytes, with its line ending
//!         let _ = e;
//!     }
//! }
//! # Ok::<(), String>(())
//! ```
//!
//! The same seed and start give the same truth and the same bytes, however it is stepped.

pub mod config;
pub mod encode;
pub mod geo;
pub mod path;
pub mod rng;
pub mod sensors;
pub mod terrain;
pub mod world;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;

use serde::Serialize;

use config::{ScenarioCfg, WorldConfig};
use sensors::radio::{self, Body, Env, OwnRx, Out, RadioStats};
use sensors::scan::{self, ScanState, ScanStats};
use sensors::{Kind, Receiver, SensorInfo};
use terrain::{Grid, Terrain};
use world::{Ais, Ctx, World};

pub use sensors::radio::Stream;
pub use sensors::scan::{OwnKind, OwnVehicle};

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// overrides the scenario's seed
    pub seed: Option<u64>,
    /// the start, unix ms; overrides the scenario's `at`
    pub start_ms: Option<i64>,
    /// how long schedules run; the scenario's, else the world's horizon
    pub duration_s: Option<f64>,
}

/// One line on one stream, exactly as it goes out.
#[derive(Clone, Debug, PartialEq)]
pub struct Emission {
    pub stream: Stream,
    /// delivery time, unix ms
    pub t_ms: i64,
    /// the receiver or sensor (`hub-pier`, `net-ais`, an own vehicle's uid)
    pub src: String,
    pub kind: &'static str,
    pub line: String,
}

/// An object as it really is (evaluation and the debug overlay only).
#[derive(Clone, Debug, Serialize)]
pub struct TruthObject {
    pub id: String,
    pub stream: String,
    pub domain: &'static str,
    pub class: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f64,
    pub track_deg: f64,
    pub speed_mps: f64,
    pub vs_mps: f64,
    pub on_ground: bool,
    /// the AIS navigational status of a vessel
    pub nav_status: Option<u8>,
    pub ids: world::Ident,
    pub equipage: world::Equip,
    /// faults active now
    pub faults: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub t_ms: i64,
    pub start_ms: i64,
    pub seed: u64,
    pub objects: BTreeMap<&'static str, usize>,
    pub objects_total: usize,
    /// lines delivered, by stream
    pub lines: BTreeMap<Stream, u64>,
    /// messages delivered by `<source>/<kind>`
    pub messages: BTreeMap<String, u64>,
    /// messages a second over the last minute, by `<source>/<kind>`
    pub rates_per_s: BTreeMap<String, f64>,
    pub radio: RadioStats,
    pub scans: ScanStats,
    pub own_vehicles: usize,
    pub terrain: String,
    pub generators: Vec<String>,
}

pub struct Traffic {
    cfg: WorldConfig,
    scn: ScenarioCfg,
    seed: u64,
    start_ms: i64,
    t_begin: f64,
    t_end: f64,
    world: World,
    receivers: Vec<Receiver>,
    terrain: Terrain,
    terrain_note: String,
    los: HashMap<(u32, u32, u8, i32, i32, i32), bool>,
    ais_load: HashMap<(usize, i64), f64>,
    load_bucket: i64,
    own: BTreeMap<String, OwnVehicle>,
    scans: BTreeMap<(String, Kind), ScanState>,
    pending: Vec<Out>,
    /// scenario ms stepped to
    now_rel: i64,
    ais_seq: HashMap<String, u8>,
    radio_stats: RadioStats,
    scan_stats: ScanStats,
    lines: BTreeMap<Stream, u64>,
    messages: BTreeMap<String, u64>,
    history: VecDeque<(i64, BTreeMap<String, u64>)>,
}

impl Traffic {
    pub fn from_files(world: &Path, scenario: Option<&Path>, opts: Options) -> Result<Traffic, String> {
        let cfg = WorldConfig::load(world)?;
        let scn = match scenario {
            Some(p) => ScenarioCfg::load(p)?,
            None => ScenarioCfg::default(),
        };
        Traffic::new(cfg, scn, opts)
    }

    pub fn new(cfg: WorldConfig, scn: ScenarioCfg, opts: Options) -> Result<Traffic, String> {
        let seed = opts.seed.or(scn.seed).unwrap_or(1);
        let start_ms = match (opts.start_ms, scn.at.as_deref()) {
            (Some(ms), _) => ms,
            (None, Some(at)) => fleetmesh_space::scenario_clock::parse_start(at)?.timestamp_millis(),
            (None, None) => return Err("no start: give Options::start_ms or the scenario's `at`".into()),
        };
        let duration = opts.duration_s.or(scn.duration_s).unwrap_or(cfg.world.horizon_s);
        let (grid, terrain_note) = match cfg.terrain.grid.as_deref() {
            Some(g) => match Grid::load(&cfg.resolve(g)) {
                Ok(grid) => {
                    let note = format!("grid {} and {} hills", grid.source, cfg.terrain.hill.len());
                    (Some(grid), note)
                }
                Err(e) => (None, format!("no grid ({e}); {} hills and the radio horizon", cfg.terrain.hill.len())),
            },
            None => (None, format!("{} hills and the radio horizon", cfg.terrain.hill.len())),
        };
        let terrain = Terrain::new(grid, cfg.terrain.hill.clone(), cfg.terrain.k, cfg.terrain.sample_m);
        let receivers = sensors::receivers(&cfg, &scn.issue)?;
        let classes = scan::r50_classes();
        for (name, sc) in [("daa", cfg.onboard.daa.as_ref()), ("marine_radar", cfg.onboard.marine_radar.as_ref())]
            .into_iter()
            .filter_map(|(n, s)| s.map(|s| (n, s)))
            .chain(cfg.ground_radar.iter().map(|g| (g.id.as_str(), &g.scan)))
        {
            if let Some(bad) = sc.r50_m.keys().find(|k| !classes.contains(&k.as_str())) {
                return Err(format!("{name}: r50_m has an unknown class {bad:?}"));
            }
        }
        let t_begin = -cfg.world.warmup_s;
        let t_end = duration + 600.0;
        let mut t = Traffic {
            world: {
                let ctx = Ctx { cfg: &cfg, scn: &scn, seed, start_ms, t_begin, t_end };
                World::new(&ctx)?
            },
            cfg,
            scn,
            seed,
            start_ms,
            t_begin,
            t_end,
            receivers,
            terrain,
            terrain_note,
            los: HashMap::new(),
            ais_load: HashMap::new(),
            load_bucket: -1,
            own: BTreeMap::new(),
            scans: BTreeMap::new(),
            pending: Vec::new(),
            now_rel: 0,
            ais_seq: HashMap::new(),
            radio_stats: RadioStats::default(),
            scan_stats: ScanStats::default(),
            lines: BTreeMap::new(),
            messages: BTreeMap::new(),
            history: VecDeque::new(),
        };
        let ctx = Ctx { cfg: &t.cfg, scn: &t.scn, seed, start_ms, t_begin: t.t_begin, t_end: t.t_end };
        t.world.fill(0.0, &ctx);
        Ok(t)
    }

    pub fn start_ms(&self) -> i64 {
        self.start_ms
    }

    pub fn now_ms(&self) -> i64 {
        self.start_ms + self.now_rel
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn config(&self) -> &WorldConfig {
        &self.cfg
    }

    /// The own fleet now (replacing what was given before). The onboard sensors scan from these
    /// positions until the next call.
    pub fn set_own_vehicles(&mut self, vehicles: Vec<OwnVehicle>) {
        self.own = vehicles.into_iter().map(|v| (v.uid.clone(), v)).collect();
    }

    pub fn own_vehicles(&self) -> usize {
        self.own.len()
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx { cfg: &self.cfg, scn: &self.scn, seed: self.seed, start_ms: self.start_ms, t_begin: self.t_begin, t_end: self.t_end }
    }

    /// AIS channel load around each AIS receiver, per ten-second bucket up to `t_rel`.
    fn update_loads(&mut self, t_rel: i64) {
        let last = t_rel.div_euclid(radio::LOAD_BUCKET_MS);
        let first = (self.load_bucket + 1).max(last - 6);
        for b in first..=last {
            let t = (b * radio::LOAD_BUCKET_MS) as f64 / 1000.0;
            let states: Vec<(geo::LatLon, f64, bool)> = self
                .world
                .movers
                .iter()
                .filter(|m| m.equip.ais != Ais::None && !m.equip.ais_off && !m.silent(t))
                .filter_map(|m| {
                    m.state(t).map(|k| {
                        let a = m.equip.ais == Ais::A;
                        let per_min = match (a, matches!(k.status, 1 | 5) && k.gs_mps < 1.5, k.gs_mps > 1.0) {
                            (true, true, _) => 1.0 / 3.0,
                            (true, false, _) => 6.0,
                            (false, _, true) => 2.0,
                            (false, _, false) => 1.0 / 3.0,
                        };
                        (k.p, per_min, a)
                    })
                })
                .collect();
            for rx in self.receivers.iter().filter(|r| r.kind == Kind::Ais) {
                let mut sum = 0.0;
                for (p, per_min, a) in &states {
                    let range = if *a { rx.range_m } else { rx.class_b_range_m };
                    if rx.sites.iter().any(|s| geo::haversine_m(s.p, *p) <= range) {
                        sum += per_min;
                    }
                }
                self.ais_load.insert((rx.index, b), sum / (2.0 * 2250.0));
            }
        }
        self.load_bucket = last;
        self.ais_load.retain(|(_, b), _| *b >= last - 12);
    }

    fn advance_scans(&mut self, t_rel: i64) {
        let daa = self.cfg.onboard.daa.clone();
        let radar = self.cfg.onboard.marine_radar.clone();
        // onboard: one schedule per own vehicle and sensor
        let mut wanted: Vec<(String, Kind)> = Vec::new();
        for v in self.own.values() {
            if let Some(k) = scan::onboard_kind(v) {
                let has = match k {
                    Kind::Daa => daa.is_some(),
                    Kind::MarineRadar => radar.is_some(),
                    _ => false,
                };
                if has {
                    wanted.push((v.uid.clone(), k));
                }
            }
        }
        for g in self.cfg.ground_radar.iter().filter(|g| g.enabled) {
            wanted.push((g.id.clone(), Kind::GroundRadar));
        }
        self.scans.retain(|key, _| wanted.contains(key));
        let own: Vec<OwnVehicle> = self.own.values().cloned().collect();
        for (key, k) in wanted {
            let cfg = match k {
                Kind::Daa => daa.clone(),
                Kind::MarineRadar => radar.clone(),
                _ => self.cfg.ground_radar.iter().find(|g| g.id == key).map(|g| g.scan.clone()),
            };
            let Some(cfg) = cfg else { continue };
            let period = ((cfg.period_s * 1000.0) as i64).max(100);
            let phase = (rng::hash(self.seed, &[rng::tag(&key), k as u64]) % period as u64) as i64;
            let now = self.now_rel;
            let st = self.scans.entry((key.clone(), k)).or_insert_with(|| {
                let n = (now - phase).div_euclid(period) + 1;
                ScanState { next_ms: phase + n * period, k: n.max(0) as u64 }
            });
            while st.next_ms <= t_rel {
                let pose = if k == Kind::GroundRadar {
                    self.cfg.ground_radar.iter().find(|g| g.id == key).map(|g| scan::Pose {
                        id: g.id.clone(),
                        kind: k,
                        p: geo::LatLon::new(g.lat, g.lon),
                        alt_m: g.alt_m,
                        vel_en: (0.0, 0.0),
                        order: (2u64 << 40) | (rng::tag(&g.id) & 0xFF_FFFF_FFFF),
                    })
                } else {
                    self.own.get(&key).filter(|v| scan::running(v, k)).map(|v| scan::pose_of(v, k, cfg.mount_m))
                };
                if let Some(pose) = pose {
                    scan::scan(&pose, &cfg, st.k, st.next_ms, self.start_ms, self.seed, &self.world.movers, &own, &self.terrain, &mut self.scan_stats, &mut self.pending);
                }
                st.next_ms += period;
                st.k += 1;
            }
        }
    }

    /// Run the world to `t_ms` (unix ms) and return every line delivered by then, in order.
    pub fn step_to(&mut self, t_ms: i64) -> Vec<Emission> {
        let t_rel = t_ms - self.start_ms;
        if t_rel <= self.now_rel {
            return Vec::new();
        }
        let t_s = t_rel as f64 / 1000.0;
        {
            let ctx = Ctx { cfg: &self.cfg, scn: &self.scn, seed: self.seed, start_ms: self.start_ms, t_begin: self.t_begin, t_end: self.t_end };
            self.world.fill(t_s, &ctx);
        }
        self.update_loads(t_rel);
        let own_rid: Vec<OwnRx> = if self.cfg.onboard.rid {
            self.own
                .values()
                .filter(|v| v.airborne())
                .map(|v| OwnRx {
                    id: v.uid.clone(),
                    p: geo::LatLon::new(v.lat, v.lon),
                    alt_m: v.alt_msl_m,
                    order: (1u64 << 40) | (rng::tag(&v.uid) & 0xFF_FFFF_FFFF) | (1 << 48),
                })
                .collect()
        } else {
            Vec::new()
        };
        let w = &self.cfg.world;
        let mut env = Env {
            seed: self.seed,
            start_ms: self.start_ms,
            qnh_off_m: (w.qnh_hpa - 1013.25) * 8.43,
            geoid_n_m: w.geoid_n_m,
            region: [w.lat_min, w.lat_max, w.lon_min, w.lon_max],
            rx: &self.receivers,
            own_rid: &own_rid,
            own_rid_range_m: self.cfg.onboard.rid_range_m,
            terrain: &self.terrain,
            los: &mut self.los,
            ais_load: &self.ais_load,
            stats: &mut self.radio_stats,
        };
        for m in self.world.movers.iter_mut() {
            radio::advance(m, t_rel, &mut env, &mut self.pending);
        }
        self.advance_scans(t_rel);
        self.world.prune(t_s);
        self.now_rel = t_rel;
        let (mut ready, rest): (Vec<Out>, Vec<Out>) = std::mem::take(&mut self.pending).into_iter().partition(|o| o.deliver_ms <= t_ms);
        self.pending = rest;
        ready.sort_by_key(|o| (o.deliver_ms, o.order));
        let mut out = Vec::with_capacity(ready.len());
        for o in ready {
            let stream = o.stream();
            let kind = o.kind.as_str();
            *self.messages.entry(format!("{}/{kind}", o.src)).or_insert(0) += 1;
            match o.body {
                Body::Sbs(m) => out.push(Emission { stream, t_ms: o.deliver_ms, src: o.src, kind, line: m.line() }),
                Body::Json(s) => out.push(Emission { stream, t_ms: o.deliver_ms, src: o.src, kind, line: s }),
                Body::Ais { bits, channel, c_s } => {
                    let multi = bits.len() > 60 * 6;
                    let seq = if multi {
                        let s = self.ais_seq.entry(o.src.clone()).or_insert(0);
                        let v = *s;
                        *s = (*s + 1) % 10;
                        v
                    } else {
                        0
                    };
                    for line in encode::ais::sentences(&bits, channel, seq, Some((&o.src, c_s)), 60) {
                        out.push(Emission { stream, t_ms: o.deliver_ms, src: o.src.clone(), kind, line });
                    }
                }
            }
        }
        for e in &out {
            *self.lines.entry(e.stream).or_insert(0) += 1;
        }
        if self.history.back().is_none_or(|(t, _)| t_ms - *t >= 5_000) {
            self.history.push_back((t_ms, self.messages.clone()));
            while self.history.len() > 2 && self.history.front().is_some_and(|(t, _)| t_ms - *t > 65_000) {
                self.history.pop_front();
            }
        }
        out
    }

    /// Truth now.
    pub fn truth(&self) -> Vec<TruthObject> {
        self.truth_at(self.now_ms())
    }

    /// Truth at `t_ms` (unix ms) for the objects the world holds.
    pub fn truth_at(&self, t_ms: i64) -> Vec<TruthObject> {
        let ctx = self.ctx();
        let t = (t_ms - self.start_ms) as f64 / 1000.0;
        let mut v: Vec<TruthObject> = self
            .world
            .movers
            .iter()
            .filter_map(|m| {
                let k = m.state(t)?;
                if !ctx.in_region(k.p) {
                    return None;
                }
                let mut faults = Vec::new();
                if m.offset(t).is_some() {
                    faults.push("position_offset");
                }
                if m.silent(t) {
                    faults.push("transmitter_off");
                }
                if m.equip.ais_off {
                    faults.push("ais_off");
                }
                Some(TruthObject {
                    id: m.id.clone(),
                    stream: m.stream.clone(),
                    domain: m.class.domain(),
                    class: m.class.as_str(),
                    lat: encode::json::round(k.p.lat, 6),
                    lon: encode::json::round(k.p.lon, 6),
                    alt_m: encode::json::round(k.alt_m, 1),
                    track_deg: encode::json::round(k.track_deg, 1),
                    speed_mps: encode::json::round(k.gs_mps, 2),
                    vs_mps: encode::json::round(k.vs_mps, 2),
                    on_ground: k.on_ground,
                    nav_status: (!m.class.air()).then_some(k.status),
                    ids: m.ident.clone(),
                    equipage: m.equip.clone(),
                    faults,
                })
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        world::counts(&self.world.movers, self.now_rel as f64 / 1000.0, &self.ctx())
    }

    /// Every receiver and sensor: `/sensors`.
    pub fn sensors(&self) -> Vec<SensorInfo> {
        let t = self.now_rel as f64 / 1000.0;
        let mut v: Vec<SensorInfo> = self.receivers.iter().map(|r| r.info(t)).collect();
        for g in &self.cfg.ground_radar {
            v.push(SensorInfo {
                id: g.id.clone(),
                kind: Kind::GroundRadar.as_str(),
                num: None,
                owner: "company",
                platform: Some(g.id.clone()),
                lat: Some(g.lat),
                lon: Some(g.lon),
                alt_m: Some(g.alt_m),
                nominal_range_m: g.range_m,
                feeders: None,
                enabled: g.enabled,
                up: g.enabled,
            });
        }
        for o in self.own.values() {
            let mut kinds: Vec<(Kind, f64, bool)> = Vec::new();
            if let Some(k) = scan::onboard_kind(o) {
                let c = match k {
                    Kind::Daa => self.cfg.onboard.daa.as_ref(),
                    _ => self.cfg.onboard.marine_radar.as_ref(),
                };
                if let Some(c) = c {
                    // detect-and-avoid: its reach for a light aircraft; a radar: its longest
                    let r = match k {
                        Kind::Daa => c.r50_m.get("ga").copied(),
                        _ => None,
                    }
                    .unwrap_or_else(|| c.r50_m.values().copied().fold(0.0, f64::max));
                    kinds.push((k, r, scan::running(o, k)));
                }
            }
            if self.cfg.onboard.rid && matches!(o.kind, OwnKind::Drone | OwnKind::Plane) {
                kinds.push((Kind::Rid, self.cfg.onboard.rid_range_m, o.airborne()));
            }
            for (k, r, up) in kinds {
                v.push(SensorInfo {
                    id: o.uid.clone(),
                    kind: k.as_str(),
                    num: None,
                    owner: "company",
                    platform: Some(o.uid.clone()),
                    lat: Some(o.lat),
                    lon: Some(o.lon),
                    alt_m: Some(o.alt_msl_m),
                    nominal_range_m: r,
                    feeders: None,
                    enabled: true,
                    up,
                });
            }
        }
        v
    }

    pub fn stats(&self) -> Stats {
        let mut rates = BTreeMap::new();
        if let (Some((t0, old)), Some((t1, _))) = (self.history.front(), self.history.back()) {
            let dt = (self.now_ms().max(*t1) - t0) as f64 / 1000.0;
            if dt > 0.5 {
                for (k, v) in &self.messages {
                    let before = old.get(k).copied().unwrap_or(0);
                    rates.insert(k.clone(), encode::json::round((v - before) as f64 / dt, 2));
                }
            }
        }
        let objects = self.counts();
        Stats {
            t_ms: self.now_ms(),
            start_ms: self.start_ms,
            seed: self.seed,
            objects_total: objects.values().sum(),
            objects,
            lines: self.lines.clone(),
            messages: self.messages.clone(),
            rates_per_s: rates,
            radio: self.radio_stats.clone(),
            scans: self.scan_stats.clone(),
            own_vehicles: self.own.len(),
            terrain: self.terrain_note.clone(),
            generators: self.world.generators(),
        }
    }
}
