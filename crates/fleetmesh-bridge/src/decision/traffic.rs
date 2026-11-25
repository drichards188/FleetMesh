//! The company's picture of the traffic the fleet does not control (docs/design/traffic.md,
//! phase 3): the bridge's side of `fleetmesh-tracker`.
//!
//! - **Ingest.** A live run (`FLEETMESH_TRAFFIC_URL`, the simulator's HTTP) connects to the
//!   simulator's SBS, AIS and JSON TCP ports as a client, as it would to a real receiver's feed
//!   (dump1090's port 30003, an AIS receiver's NMEA), and reconnects when one drops; it reads
//!   `/sensors` at start for the receivers' sites (the fixed ones: onboard sensors' positions come
//!   in each detection) and `/health` now and then for the scenario clock. The headless simulator
//!   steps the simulator in process and hands its lines over with no sockets ([`Picture::ingest`]).
//!   Every line is given its receipt time on the scenario clock.
//! - **The picture.** Once a second ([`Decision::traffic_tick`]) the tracker is told the fleet's own
//!   vehicles (never traffic), the QNH from the weather (standard pressure, flagged, when the
//!   weather has none), and brought to now; the tracks, the receivers heard and the feeds make the
//!   picture the rules read (`DeliveryInputs::picture`, rules 3), the API serves and the console's
//!   WebSocket gets. A feed silent or a receiver not heard degrades it, as data.
//! - **Truth** (`/api/traffic/truth`) is the simulator's, for evaluation and a debug overlay in a
//!   dev run only: no decision and no tracker ever reads it.
//!
//! Altitudes: the tracker keeps an ADS-B track in pressure altitude (`baro`) and a Remote ID one
//! in geometric height above mean sea level (`geo`, converted from the ellipsoid with the Bay's
//! geoid height, [`BAY_GEOID_N_M`]). The rules compare everything above mean sea level, so a
//! pressure altitude is brought to sea level here with the QNH, and the conversion's own
//! uncertainty (the tracker's figure, wide when the QNH is not known) goes into its vertical sigma.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use fleetmesh_decide::delivery::{
    BearingFact, PathPoint, PictureState, ReceiverFact, TrackFact, TrafficKnobs, TrafficPicture,
};
use fleetmesh_decide::{Clock as _, Domain, LonLat, Time};
use fleetmesh_tracker::{AltRef, Config, OwnFleet, OwnVehicle, SensorSite, Track, TrackState, Tracker};
use serde_json::{Value, json};
use tokio::sync::watch;

/// The geoid height on San Francisco Bay (EGM96), m: Remote ID's ellipsoid heights to mean sea
/// level. The simulator's world says the same (`[world] geoid_n_m` in sim/traffic.toml; a test
/// holds the two together).
pub const BAY_GEOID_N_M: f64 = -32.0;

/// Standard pressure, hPa.
const STANDARD_HPA: f64 = 1013.25;

/// One of the three feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feed {
    Sbs,
    Ais,
    Json,
}

impl Feed {
    pub const ALL: [Feed; 3] = [Feed::Sbs, Feed::Ais, Feed::Json];

    pub fn as_str(self) -> &'static str {
        match self {
            Feed::Sbs => "sbs",
            Feed::Ais => "ais",
            Feed::Json => "json",
        }
    }
}

/// Where the picture comes from and how it is made (the tracker's configuration is logged).
#[derive(Clone, Debug)]
pub struct Settings {
    /// The simulator's HTTP (`FLEETMESH_TRAFFIC_URL`): a live run's source. None: no live source.
    pub url: Option<String>,
    pub host: String,
    pub sbs_port: u16,
    pub ais_port: u16,
    pub json_port: u16,
    /// `FLEETMESH_TRAFFIC=0` turns the picture off whatever else is set.
    pub off: bool,
    pub tracker: Config,
}

impl Settings {
    /// From the environment, with the Privacy ICAO Address block of `root/sim/traffic.toml`
    /// (`[addresses] pia_block`: the pool a company that knows it marks anonymous).
    pub fn from_env(root: &std::path::Path) -> Settings {
        let var = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
        let port = |k: &str, d: u16| var(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d);
        let off = matches!(var("FLEETMESH_TRAFFIC").as_deref().map(str::trim), Some("0" | "off" | "false" | "no"));
        let url = var("FLEETMESH_TRAFFIC_URL").map(|u| u.trim().trim_end_matches('/').to_string());
        let host = url
            .as_deref()
            .and_then(|u| u.strip_prefix("http://"))
            .and_then(|r| r.split(['/', ':']).next())
            .unwrap_or("127.0.0.1")
            .to_string();
        Settings {
            url,
            host,
            sbs_port: port("FLEETMESH_TRAFFIC_SBS_PORT", 8768),
            ais_port: port("FLEETMESH_TRAFFIC_AIS_PORT", 8769),
            json_port: port("FLEETMESH_TRAFFIC_JSON_PORT", 8771),
            off,
            tracker: tracker_config(root),
        }
    }
}

/// The tracker's configuration: its defaults (tuned for the Bay), the Bay's geoid height and the
/// Privacy ICAO Address block from the world's addresses.
pub fn tracker_config(root: &std::path::Path) -> Config {
    let mut c = Config { geoid_n_m: BAY_GEOID_N_M, ..Config::default() };
    c.anonymous_icao_ranges = pia_block(root).into_iter().collect();
    c
}

/// `[addresses] pia_block` of sim/traffic.toml as hex strings, if it says.
pub fn pia_block(root: &std::path::Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(root.join("sim/traffic.toml")).ok()?;
    let doc: toml::Table = toml::from_str(&text).ok()?;
    let b = doc.get("addresses")?.get("pia_block")?.as_array()?;
    let lo = b.first()?.as_integer()?;
    let hi = b.get(1)?.as_integer()?;
    Some((format!("{lo:06X}"), format!("{hi:06X}")))
}

#[derive(Default, Clone)]
struct FeedStat {
    connected: bool,
    last_t: Option<i64>,
    lines: u64,
    /// (scenario ms, lines then), the last ten seconds
    window: VecDeque<(i64, u64)>,
}

#[derive(Clone)]
struct Rx {
    id: String,
    kind: String,
    network: bool,
    at: Option<LonLat>,
    range_m: Option<f64>,
    last_t: Option<i64>,
    lines: u64,
}

/// How long a receiver of a kind may be quiet before it counts as not heard, s: ADS-B hears
/// something every second at a hub; a hub's AIS a few vessels' reports a minute.
fn silent_after_s(kind: &str, network: bool) -> f64 {
    match (kind, network) {
        ("adsb_1090", false) => 20.0,
        ("adsb_1090", true) => 30.0,
        ("ais", false) => 90.0,
        _ => 60.0,
    }
}

/// Before this much of the feeds' time has passed nothing counts as not heard yet, s.
const WARMUP_S: f64 = 45.0;

/// The picture's start: for this long after its source attached (the bridge may start before the
/// simulator's or a receiver's feeds are up), s, a picture not yet whole is `starting`, not a
/// fault: the rules read it as they read any degraded picture (a departure that needs it waits),
/// and no person is told. A picture still degraded after it is an issue as before.
pub const STARTUP_GRACE_S: f64 = 60.0;

/// One built picture: what the rules read, and what the API and the console get.
pub struct Built {
    /// The scenario clock's ms the picture is for (the tracks' own times).
    pub scen_t: i64,
    /// The rules' picture: every confirmed or coasting track within reach of the fleet, with the
    /// tracker's prediction.
    pub picture: TrafficPicture,
    /// Every track as the tracker serialises it (docs/design/traffic.md section 5), with where it is.
    pub tracks: Vec<(LonLat, Value)>,
    /// `/api/traffic`'s `picture`.
    pub summary: Value,
    /// The tracker's disagreements: (cooperative track, sensed track, offset m, sensor).
    pub disagreements: Vec<fleetmesh_tracker::Disagreement>,
    /// The tracker's tracks themselves (for the alerts).
    pub raw: Vec<Track>,
}

struct Inner {
    tracker: Tracker,
    source: Option<&'static str>,
    feeds: BTreeMap<Feed, FeedStat>,
    receivers: BTreeMap<(String, String), Rx>,
    /// SBS session number to (receiver id, kind)
    sessions: BTreeMap<String, (String, String)>,
    /// the fixed sites as `/sensors` listed them, the simulator's own up/enabled flags dropped
    sites: Vec<Value>,
    onboard: Vec<Value>,
    sensors_read: bool,
    first_t: Option<i64>,
    /// the scenario clock's ms at the first picture built (the start of [`STARTUP_GRACE_S`])
    started_ms: Option<i64>,
    /// scenario ms = local estimate + this (the simulator's clock, read from its `/health`)
    clock_offset_ms: i64,
    qnh: Option<f64>,
    truth: Option<Value>,
}

/// The bridge's traffic picture: the tracker and what it has been told.
pub struct Picture {
    pub settings: Settings,
    inner: Mutex<Inner>,
    built: watch::Sender<Option<Arc<Built>>>,
    config_json: Value,
    config_sha256: String,
}

fn r6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}
fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

impl Picture {
    pub fn new(settings: Settings) -> Picture {
        let config_json = serde_json::to_value(&settings.tracker).unwrap_or(Value::Null);
        let config_sha256 = fleetmesh_decide::hash::sha256_hex(config_json.to_string().as_bytes());
        let tracker = Tracker::new(settings.tracker.clone());
        Picture {
            settings,
            inner: Mutex::new(Inner {
                tracker,
                source: None,
                feeds: Feed::ALL.into_iter().map(|f| (f, FeedStat::default())).collect(),
                receivers: BTreeMap::new(),
                sessions: BTreeMap::new(),
                sites: Vec::new(),
                onboard: Vec::new(),
                sensors_read: false,
                first_t: None,
                started_ms: None,
                clock_offset_ms: 0,
                qnh: None,
                truth: None,
            }),
            built: watch::channel(None).0,
            config_json,
            config_sha256,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The tracker's configuration as logged, and its sha256.
    pub fn config(&self) -> (&Value, &str) {
        (&self.config_json, &self.config_sha256)
    }

    /// A source is attached: `live` (the simulator's sockets) or `in_process` (the headless one).
    pub fn attach(&self, source: &'static str) {
        self.lock().source = Some(source);
    }

    /// Whether the last picture built is still starting ([`STARTUP_GRACE_S`] of the scenario's
    /// clock since the first) and not yet whole.
    pub fn starting(&self) -> bool {
        self.built().is_some_and(|b| b.summary["starting"].as_bool() == Some(true))
    }

    /// Whether this run has a traffic source.
    pub fn active(&self) -> bool {
        !self.settings.off && self.lock().source.is_some()
    }

    pub fn set_feed_connected(&self, feed: Feed, connected: bool) {
        if let Some(f) = self.lock().feeds.get_mut(&feed) {
            f.connected = connected;
        }
    }

    /// The scenario clock offset from the simulator's own (`/health`'s `t` against ours).
    pub fn set_clock_offset(&self, ms: i64) {
        self.lock().clock_offset_ms = ms;
    }

    pub fn clock_offset(&self) -> i64 {
        self.lock().clock_offset_ms
    }

    /// The receivers and sensors (the simulator's `/sensors`). Onboard sensors (`daa`,
    /// `marine_radar`, and any entry on one of `own` vehicles) are not fixed sites: they are kept
    /// apart and never given to the tracker as one (a detection carries its sensor's position).
    pub fn set_sensors(&self, list: &[Value], own: &BTreeSet<String>) {
        let mut g = self.lock();
        let mut sites: Vec<SensorSite> = Vec::new();
        let mut shown = Vec::new();
        let mut onboard = Vec::new();
        // what each receiver was heard as stays across a re-read of the list (each ten minutes):
        // a re-read is not a silence
        let heard: BTreeMap<(String, String), (Option<i64>, u64)> = g.receivers.iter().map(|(k, r)| (k.clone(), (r.last_t, r.lines))).collect();
        g.receivers.clear();
        g.sessions.clear();
        for e in list {
            let kind = e["kind"].as_str().unwrap_or("");
            let id = e["id"].as_str().unwrap_or("").to_string();
            let platform = e["platform"].as_str().map(str::to_string);
            let on_vehicle = matches!(kind, "daa" | "marine_radar") || platform.as_ref().is_some_and(|p| own.contains(p)) || own.contains(&id);
            let mut v = e.clone();
            if let Some(o) = v.as_object_mut() {
                // the simulator's own view of whether it works is its truth, not ours
                o.remove("up");
                o.remove("enabled");
            }
            if on_vehicle {
                if let Some(o) = v.as_object_mut() {
                    o.remove("lat");
                    o.remove("lon");
                    o.remove("alt_m");
                }
                onboard.push(v);
                continue;
            }
            // a ground radar switched off in the simulator delivers nothing: it is listed all the same
            if let Ok(s) = serde_json::from_value::<SensorSite>(e.clone()) {
                sites.push(s);
            }
            let network = e["owner"].as_str() == Some("network");
            if let Some(num) = e.get("num").filter(|n| !n.is_null()) {
                let n = num.as_i64().map_or_else(|| num.as_str().unwrap_or("").to_string(), |n| n.to_string());
                g.sessions.insert(n, (id.clone(), kind.to_string()));
            }
            if matches!(kind, "adsb_1090" | "ais") {
                let at = match (e["lon"].as_f64(), e["lat"].as_f64()) {
                    (Some(lon), Some(lat)) => Some(LonLat::new(lon, lat)),
                    _ => None,
                };
                let key = (id.clone(), kind.to_string());
                let (last_t, lines) = heard.get(&key).copied().unwrap_or((None, 0));
                g.receivers.insert(key, Rx { id: id.clone(), kind: kind.into(), network, at, range_m: e["nominal_range_m"].as_f64(), last_t, lines });
            }
            shown.push(v);
        }
        g.tracker.set_sensors(sites);
        g.sites = shown;
        g.onboard = onboard;
        g.sensors_read = true;
    }

    /// One line from a feed, received at `receipt_ms` on the scenario clock.
    pub fn ingest(&self, feed: Feed, line: &str, receipt_ms: i64) {
        let mut g = self.lock();
        let g = &mut *g;
        g.first_t.get_or_insert(receipt_ms);
        if let Some(f) = g.feeds.get_mut(&feed) {
            f.lines += 1;
            f.last_t = Some(receipt_ms);
        }
        // which receiver heard it, for whether each is heard
        let rx: Option<(String, String)> = match feed {
            Feed::Sbs => line.split(',').nth(2).and_then(|s| g.sessions.get(s.trim()).cloned()),
            Feed::Ais => line
                .strip_prefix("\\s:")
                .and_then(|r| r.split([',', '*']).next())
                .map(|id| (id.to_string(), "ais".to_string())),
            Feed::Json => None,
        };
        if let Some(key) = rx
            && let Some(r) = g.receivers.get_mut(&key)
        {
            r.last_t = Some(receipt_ms);
            r.lines += 1;
        }
        match feed {
            Feed::Sbs => {
                g.tracker.ingest_sbs(line.trim_end(), receipt_ms);
            }
            Feed::Ais => {
                g.tracker.ingest_nmea(line.trim_end(), receipt_ms);
            }
            Feed::Json => {
                g.tracker.ingest_json(line.trim_end());
            }
        }
    }

    /// The simulator's truth now (the headless simulator sets it; evaluation only).
    #[cfg_attr(not(fleetmesh_headless), allow(dead_code))]
    pub fn set_truth(&self, truth: Value) {
        self.lock().truth = Some(truth);
    }

    pub fn truth(&self) -> Option<Value> {
        self.lock().truth.clone()
    }

    /// The last picture built.
    pub fn built(&self) -> Option<Arc<Built>> {
        self.built.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<Arc<Built>>> {
        self.built.subscribe()
    }

    /// Bring the picture to `scen_ms` (the engine's clock `engine_t` then): the fleet's own
    /// vehicles and identities, the QNH, the tracker's update, and the picture built from it. The
    /// rules' tracks are those within `reach_m` of `ops` (the fleet and its hubs).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        scen_ms: i64,
        engine_t: Time,
        own: &[OwnVehicle],
        own_fleet: &OwnFleet,
        qnh: Option<f64>,
        knobs: &TrafficKnobs,
        ops: &[LonLat],
        reach_m: f64,
    ) -> Arc<Built> {
        let mut g = self.lock();
        let g = &mut *g;
        g.tracker.set_own_fleet(own_fleet);
        g.tracker.set_own_vehicles(own);
        g.tracker.set_qnh(qnh);
        g.qnh = qnh;
        g.tracker.update(scen_ms);
        let cfg = g.tracker.config().clone();
        // each feed's rate over the last ten seconds
        for f in g.feeds.values_mut() {
            f.window.push_back((scen_ms, f.lines));
            while f.window.front().is_some_and(|(t, _)| scen_ms - t > 10_000) {
                f.window.pop_front();
            }
        }
        let warm = g.first_t.is_some_and(|t0| (scen_ms - t0) as f64 / 1000.0 >= WARMUP_S);
        let mut reasons: Vec<String> = Vec::new();
        // what is not heard, by a key that stays the same while it lasts (the words count seconds)
        let mut keys: Vec<String> = Vec::new();
        if !g.sensors_read {
            reasons.push("the receivers' list (the simulator's /sensors) has not been read: which receivers there are is not known".into());
            keys.push("sensors".into());
        }
        for (feed, f) in &g.feeds {
            if g.source == Some("live") && !f.connected {
                reasons.push(format!("the {} feed is not connected", feed.as_str()));
                keys.push(format!("feed:{}", feed.as_str()));
            } else if warm && f.last_t.is_none_or(|t| (scen_ms - t) as f64 / 1000.0 > 30.0) && *feed != Feed::Json {
                reasons.push(format!("the {} feed has delivered nothing for 30 s", feed.as_str()));
                keys.push(format!("feed:{}", feed.as_str()));
            }
        }
        let mut receivers = Vec::new();
        let mut rx_json = Vec::new();
        for r in g.receivers.values() {
            let quiet_s = r.last_t.map(|t| (scen_ms - t) as f64 / 1000.0);
            let limit = silent_after_s(&r.kind, r.network);
            let up = match quiet_s {
                Some(q) => q <= limit,
                None => !warm,
            };
            if !up {
                keys.push(format!("rx:{}:{}", r.id, r.kind));
                reasons.push(match quiet_s {
                    Some(q) => format!("{} {} receiver not heard for {:.0} s", r.id, r.kind, q),
                    None => format!("{} {} receiver not heard since the start", r.id, r.kind),
                });
            }
            receivers.push(ReceiverFact { id: r.id.clone(), kind: r.kind.clone(), network: r.network, at: r.at, range_m: r.range_m, up });
            rx_json.push(json!({
                "id": r.id, "kind": r.kind, "network": r.network, "up": up, "last_t": r.last_t,
                "silent_s": quiet_s.map(r1), "lines": r.lines,
            }));
        }
        let state = if reasons.is_empty() && g.source.is_some() { PictureState::Ok } else { PictureState::Degraded };
        let tracks_all: Vec<Track> = g.tracker.tracks().to_vec();
        let mut facts = Vec::new();
        let mut shown = Vec::with_capacity(tracks_all.len());
        let mut by_class: BTreeMap<String, u32> = BTreeMap::new();
        let mut by_source: BTreeMap<String, u32> = BTreeMap::new();
        for t in &tracks_all {
            let v = serde_json::to_value(t).unwrap_or(Value::Null);
            *by_class.entry(v["class"].as_str().unwrap_or("?").to_string()).or_insert(0) += 1;
            for s in &t.sources {
                *by_source.entry(serde_json::to_value(s.kind).ok().and_then(|k| k.as_str().map(str::to_string)).unwrap_or_default()).or_insert(0) += 1;
            }
            shown.push((LonLat::new(t.lon, t.lat), v));
            // a tentative track is a detection not confirmed yet (M of N looks): clutter and a
            // sensor's false alarms stay there, so the rules read confirmed and coasting tracks only
            if t.state == TrackState::Tentative {
                continue;
            }
            let at = t.bearing_only.as_ref().map_or(LonLat::new(t.lon, t.lat), |b| LonLat::new(b.sensor_lon, b.sensor_lat));
            if !ops.iter().any(|p| fleetmesh_decide::geo::haversine_m(*p, at) <= reach_m) {
                continue;
            }
            facts.push(track_fact(&g.tracker, t, &cfg, qnh, knobs));
        }
        let picture = TrafficPicture {
            t: engine_t,
            state,
            reasons: reasons.clone(),
            qnh_hpa: qnh,
            config_sha256: self.config_sha256.clone(),
            receivers,
            tracks: facts,
        };
        let feeds: serde_json::Map<String, Value> = g
            .feeds
            .iter()
            .map(|(k, f)| {
                let rate = match (f.window.front(), f.window.back()) {
                    (Some((t0, n0)), Some((t1, n1))) if t1 > t0 => (n1 - n0) as f64 / ((t1 - t0) as f64 / 1000.0),
                    _ => 0.0,
                };
                (
                    k.as_str().to_string(),
                    json!({ "connected": f.connected || g.source == Some("in_process"), "last_t": f.last_t, "lines_per_s": r1(rate), "lines": f.lines }),
                )
            })
            .collect();
        let started = *g.started_ms.get_or_insert(scen_ms);
        let starting = ((scen_ms - started) as f64) < STARTUP_GRACE_S * 1000.0;
        let summary = json!({
            "state": if state == PictureState::Ok { "ok" } else { "degraded" },
            // its first minute: not whole yet, and not a fault (no one is told)
            "starting": starting && state != PictureState::Ok,
            "reasons": reasons,
            "not_heard": keys,
            "feeds": feeds,
            "receivers": rx_json,
            "qnh": { "hpa": qnh, "source": if qnh.is_some() { "weather" } else { "standard" }, "standard_hpa": STANDARD_HPA },
            "source": g.source.unwrap_or("none"),
            "enabled": g.source.is_some(),
            "counts": { "tracks": tracks_all.len(), "by_class": by_class, "by_source": by_source, "in_rules": picture.tracks.len() },
            "tracker": { "config_sha256": self.config_sha256, "stats": serde_json::to_value(g.tracker.stats()).unwrap_or(Value::Null) },
            "clock": { "scenario_ms": scen_ms, "engine_ms": engine_t.0, "offset_ms": g.clock_offset_ms },
        });
        let built = Arc::new(Built {
            scen_t: scen_ms,
            picture,
            tracks: shown,
            summary,
            disagreements: g.tracker.disagreements().to_vec(),
            raw: tracks_all,
        });
        self.built.send_replace(Some(built.clone()));
        built
    }

    /// `GET /api/traffic` and the WebSocket's `traffic` message: every track (in `bounds`, w s e n,
    /// grown by a quarter on each side, when given) and the picture's state.
    pub fn api_json(&self, bounds: Option<[f64; 4]>) -> Value {
        let Some(b) = self.built() else {
            let reason = if self.settings.off {
                "the traffic picture is off in this run (FLEETMESH_TRAFFIC=0)"
            } else {
                "no traffic source in this run (no FLEETMESH_TRAFFIC_URL, or it has not delivered yet)"
            };
            return json!({
                "t": null,
                "tracks": [],
                "picture": { "state": "degraded", "reasons": [reason], "feeds": {}, "enabled": false, "source": "none" },
            });
        };
        let tracks: Vec<&Value> = b
            .tracks
            .iter()
            .filter(|(at, v)| match bounds {
                None => true,
                Some([w, s, e, n]) => {
                    let (mw, mh) = ((e - w).abs() * 0.25, (n - s).abs() * 0.25);
                    let at = v["bearing_only"]
                        .as_object()
                        .and_then(|o| Some(LonLat::new(o.get("sensor_lon")?.as_f64()?, o.get("sensor_lat")?.as_f64()?)))
                        .unwrap_or(*at);
                    at.lon >= w - mw && at.lon <= e + mw && at.lat >= s - mh && at.lat <= n + mh
                }
            })
            .map(|(_, v)| v)
            .collect();
        json!({ "t": b.scen_t, "tracks": tracks, "disagreements": b.disagreements, "picture": b.summary })
    }

    /// `GET /api/traffic/sensors`: the fixed receivers and sites as the simulator listed them
    /// (without its own view of whether each works), each with whether the bridge hears it, and
    /// the onboard sensors by vehicle.
    pub fn sensors_json(&self) -> Value {
        let g = self.lock();
        let heard: BTreeMap<String, Value> = self
            .built()
            .map(|b| {
                b.summary["receivers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| (format!("{}|{}", r["id"].as_str().unwrap_or(""), r["kind"].as_str().unwrap_or("")), r.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let sites: Vec<Value> = g
            .sites
            .iter()
            .map(|s| {
                let mut s = s.clone();
                let key = format!("{}|{}", s["id"].as_str().unwrap_or(""), s["kind"].as_str().unwrap_or(""));
                if let (Some(o), Some(h)) = (s.as_object_mut(), heard.get(&key)) {
                    o.insert("heard".into(), json!(if h["up"] == json!(true) { "up" } else { "silent" }));
                    o.insert("last_t".into(), h["last_t"].clone());
                }
                s
            })
            .collect();
        json!({ "read": g.sensors_read, "sensors": sites, "onboard": g.onboard })
    }
}

/// A tracker track as the rules read it: its altitude above mean sea level, its prediction.
fn track_fact(tr: &Tracker, t: &Track, cfg: &Config, qnh: Option<f64>, knobs: &TrafficKnobs) -> TrackFact {
    let conv_sigma = if qnh.is_some() { cfg.baro_geo_sigma_known_m } else { cfg.baro_geo_sigma_unknown_m };
    let offset = qnh.map_or(0.0, |q| 8.32 * (q - STANDARD_HPA));
    let msl = |alt: Option<f64>, sv: Option<f64>| -> (Option<f64>, Option<f64>) {
        match t.alt_ref {
            AltRef::Baro => (alt.map(|a| r1(a + offset)), sv.map(|s| r1(s.hypot(conv_sigma))).or(alt.map(|_| conv_sigma))),
            _ => (alt.map(r1), sv.map(r1)),
        }
    };
    let domain = match t.domain {
        fleetmesh_tracker::Domain::Air => Domain::Air,
        fleetmesh_tracker::Domain::Surface => Domain::Surface,
    };
    let class = serde_json::to_value(t.class).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let label = t
        .ids
        .callsign
        .clone()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .or_else(|| t.static_.name.clone())
        .or_else(|| t.ids.icao24.clone())
        .or_else(|| t.ids.mmsi.map(|m| m.to_string()))
        .or_else(|| t.ids.uas_id.clone())
        .unwrap_or_default();
    let flags: Vec<String> = t
        .flags
        .iter()
        .filter_map(|f| serde_json::to_value(f).ok().and_then(|v| v.as_str().map(str::to_string)))
        .collect();
    let (alt_m, v_sigma_m) = msl(t.alt_m, t.v_sigma_m);
    let line = t.bearing_only.as_ref().map(|b| BearingFact {
        from: LonLat::new(r6(b.sensor_lon), r6(b.sensor_lat)),
        from_alt_m: r1(b.sensor_alt_m),
        bearing_deg: r1(b.bearing_deg),
        sigma_deg: r1(b.sigma_bearing_deg),
        elev_deg: b.elev_deg.map(r1),
        sigma_elev_deg: b.elev_deg.map(|_| cfg.elev_sigma_deg),
        range_m: [b.range_m[0].round(), b.range_m[1].round()],
    });
    let path = if line.is_some() || !t.kin.valid {
        Vec::new()
    } else {
        let (horizon, step) = match domain {
            Domain::Surface => (knobs.surface_horizon_s.max(knobs.horizon_s), 30.0),
            _ => (knobs.horizon_s, 10.0),
        };
        tr.predict_path(t, horizon, step)
            .into_iter()
            .map(|p| {
                let (alt, sv) = msl(p.alt_m, p.v_sigma_m);
                PathPoint {
                    dt_s: ((p.t - t.t) as f64 / 1000.0).round(),
                    at: LonLat::new(r6(p.lon), r6(p.lat)),
                    alt_m: alt,
                    h_sigma_m: r1(p.h_sigma_m),
                    v_sigma_m: sv,
                }
            })
            .collect()
    };
    TrackFact {
        id: t.id.clone(),
        domain,
        class,
        cooperative: t.cooperative,
        label,
        at: LonLat::new(r6(t.lon), r6(t.lat)),
        alt_m,
        alt_ref: match t.alt_ref {
            AltRef::Baro => "baro".into(),
            AltRef::Geo => "geo".into(),
            AltRef::None => "none".into(),
        },
        h_sigma_m: r1(t.h_sigma_m),
        v_sigma_m,
        vel_mps: t.vel_mps.map(|[e, n]| [r1(e), r1(n)]),
        vs_mps: t.vs_mps.map(r1),
        flags,
        line,
        path,
    }
}

// ---- the decision layer's side ----------------------------------------------------------------

/// The bridge's start on its own clock, for the scenario clock's elapsed time.
static STARTED_WALL_S: std::sync::OnceLock<f64> = std::sync::OnceLock::new();

impl super::Decision {
    /// The picture of other traffic.
    pub fn traffic(&self) -> &Picture {
        &self.traffic
    }

    /// The scenario clock now, Unix ms: `FLEETMESH_AT` plus the time since the bridge started
    /// times `FLEETMESH_RATE`, set to the simulator's own clock (its `/health`), else the
    /// bridge's clock (a run with no scenario start, and the headless simulator, whose clock starts
    /// at the scenario's start).
    pub fn traffic_now_ms(&self) -> i64 {
        let wall = crate::clock::wall();
        let started = *STARTED_WALL_S.get_or_init(|| wall);
        let base = fleetmesh_space::scenario_clock::from_env()
            .ok()
            .and_then(|s| Some(s.at_unix()? + (wall - started) * s.rate))
            .unwrap_or(wall);
        (base * 1000.0).round() as i64 + self.traffic.clock_offset()
    }

    /// Bring the picture to now (once a second): the fleet's own vehicles (so the company's own are
    /// never traffic), the QNH from the weather, the tracker's update. The first picture logs the
    /// tracker's configuration; a change between ok and degraded is logged too.
    pub fn traffic_tick(&self) {
        if !self.traffic.active() {
            return;
        }
        let scen = self.traffic_now_ms();
        let engine_t = self.clock.now();
        let wall = crate::clock::wall();
        let mut own = Vec::new();
        let mut ops: Vec<LonLat> = self.places.hubs.values().map(|h| h.at()).collect();
        let mut fleet = OwnFleet::default();
        for cell in self.world.vehicles() {
            cell.read(|v| {
                if let Some((lat, lon)) = v.pos {
                    let seen = v.seen.position.unwrap_or(wall);
                    own.push(OwnVehicle {
                        uid: cell.uid.to_string(),
                        t: scen - ((wall - seen) * 1000.0) as i64,
                        lat,
                        lon,
                        alt_m: v.alt_msl.or(v.alt_rel).unwrap_or(0.0),
                    });
                    ops.push(LonLat::new(lon, lat));
                }
                // the fleet's own broadcast identities, where its manifest names them
                if let Some(i) = v.identity.as_ref() {
                    if let Some(x) = i.str("icao24") {
                        fleet.icao.push(x.to_string());
                    }
                    if let Some(m) = i.num("mmsi") {
                        fleet.mmsi.push(m as u32);
                    }
                    if let Some(u) = i.str("uas_id") {
                        fleet.uas_ids.push(u.to_string());
                    }
                }
            });
        }
        let qnh = self.qnh_now();
        let d = self.rules.delivery_rules();
        let knobs = d.separation.traffic_knobs();
        let before = self.traffic.built().map(|b| (b.summary["state"].clone(), b.summary["not_heard"].clone()));
        let b = self.traffic.update(scen, engine_t, &own, &fleet, qnh, &knobs, &ops, 2.0 * d.separation.relevant_km * 1000.0);
        let mut core = self.lock();
        match before {
            None => {
                let (cfg, sha) = self.traffic.config();
                let sensors = self.traffic.sensors_json();
                if let Err(e) = core.engine.record_delivery_event(
                    "traffic_config",
                    None,
                    None,
                    &format!("the traffic picture starts ({}): the tracker's configuration {sha}", b.summary["source"].as_str().unwrap_or("?")),
                    json!({ "config": cfg, "config_sha256": sha, "sensors": sensors, "qnh_hpa": qnh }),
                ) {
                    eprintln!("traffic: log the configuration: {e}");
                }
            }
            Some((state, keys)) if state != b.summary["state"] || keys != b.summary["not_heard"] => {
                let now = b.summary["state"].as_str().unwrap_or("?");
                let why = b.summary["reasons"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default();
                if let Err(e) = core.engine.record_delivery_event(
                    "traffic_picture",
                    None,
                    None,
                    &format!("the traffic picture is {now}{}", if why.is_empty() { String::new() } else { format!(": {why}") }),
                    json!({ "state": now, "reasons": b.summary["reasons"], "qnh_hpa": qnh }),
                ) {
                    eprintln!("traffic: log the picture's state: {e}");
                }
            }
            Some(_) => {}
        }
    }

    /// A live run's source: the simulator's three TCP feeds, read as a receiver's are, each
    /// reconnecting when it drops; its `/sensors` (until read) and `/health` (the clock, each
    /// minute); and the picture's tick, once a second.
    pub async fn run_traffic_live(self: Arc<Self>) {
        let s = self.traffic.settings.clone();
        let Some(url) = s.url.clone().filter(|_| !s.off) else { return };
        self.traffic.attach("live");
        println!("traffic: the picture from {url} (sbs :{}, ais :{}, json :{})", s.sbs_port, s.ais_port, s.json_port);
        for (feed, port) in [(Feed::Sbs, s.sbs_port), (Feed::Ais, s.ais_port), (Feed::Json, s.json_port)] {
            let me = self.clone();
            let addr = format!("{}:{port}", s.host);
            tokio::spawn(async move { me.read_feed(feed, addr).await });
        }
        {
            let me = self.clone();
            tokio::spawn(async move { me.watch_simulator(url).await });
        }
        let mut every = tokio::time::interval(std::time::Duration::from_secs(1));
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            crate::perf::time("traffic_tick", || self.traffic_tick());
        }
    }

    /// One feed: connect, read lines, reconnect after a pause when it drops.
    async fn read_feed(self: Arc<Self>, feed: Feed, addr: String) {
        use tokio::io::AsyncBufReadExt;
        let mut said = false;
        loop {
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(sock) => {
                    let _ = sock.set_nodelay(true);
                    println!("traffic: {} feed connected ({addr})", feed.as_str());
                    said = false;
                    self.traffic.set_feed_connected(feed, true);
                    let mut lines = tokio::io::BufReader::new(sock).lines();
                    loop {
                        match lines.next_line().await {
                            Ok(Some(line)) => {
                                let t = self.traffic_now_ms();
                                self.traffic.ingest(feed, &line, t);
                            }
                            Ok(None) => break,
                            Err(e) => {
                                eprintln!("traffic: {} feed: {e}", feed.as_str());
                                break;
                            }
                        }
                    }
                    self.traffic.set_feed_connected(feed, false);
                    eprintln!("traffic: {} feed lost ({addr}): reconnecting", feed.as_str());
                }
                Err(e) => {
                    if !said {
                        eprintln!("traffic: {} feed {addr}: {e} (trying again)", feed.as_str());
                        said = true;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    /// The simulator's `/sensors` until it is read (and again each ten minutes), and its
    /// `/health` each minute for the scenario clock.
    async fn watch_simulator(self: Arc<Self>, url: String) {
        let mut sensors_at: Option<std::time::Instant> = None;
        loop {
            let t0 = self.traffic_now_ms() - self.traffic.clock_offset();
            if let Ok(body) = http_get(&format!("{url}/health"), std::time::Duration::from_secs(3)).await
                && let Ok(v) = serde_json::from_str::<Value>(&body)
                && let Some(sim_t) = v["t"].as_i64()
            {
                let t1 = self.traffic_now_ms() - self.traffic.clock_offset();
                self.traffic.set_clock_offset(sim_t - (t0 + t1) / 2);
            }
            if sensors_at.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(600)) {
                match http_get(&format!("{url}/sensors"), std::time::Duration::from_secs(5)).await.and_then(|b| serde_json::from_str::<Vec<Value>>(&b).map_err(|e| e.to_string())) {
                    Ok(list) => {
                        let own: BTreeSet<String> = self.world.vehicles().iter().map(|c| c.uid.to_string()).collect();
                        self.traffic.set_sensors(&list, &own);
                        println!("traffic: {} receivers and sensors from {url}/sensors", list.len());
                        sensors_at = Some(std::time::Instant::now());
                    }
                    Err(e) => eprintln!("traffic: {url}/sensors: {e} (trying again)"),
                }
            }
            let pause = if sensors_at.is_some() { 60 } else { 5 };
            tokio::time::sleep(std::time::Duration::from_secs(pause)).await;
        }
    }

    /// `GET /api/traffic/truth`: the simulator's truth, labelled so (a dev run only: the route
    /// answers 404 in a credentials run). Never an input to the tracker or a decision.
    pub async fn traffic_truth_json(&self) -> Option<Value> {
        if self.auth != fleetmesh_decide::AuthMode::Dev {
            return None;
        }
        let truth = match (&self.traffic.settings.url, self.traffic.truth()) {
            (_, Some(t)) => t,
            (Some(url), None) => {
                let body = http_get(&format!("{url}/truth"), std::time::Duration::from_secs(5)).await.ok()?;
                serde_json::from_str::<Value>(&body).ok()?
            }
            _ => return None,
        };
        Some(json!({
            "simulator_truth": true,
            "note": "SIMULATOR TRUTH: where every aircraft and vessel really is. A real operator has nothing like it; it is here for evaluation and a debug overlay, and no decision or tracker reads it.",
            "t": truth["t"],
            "truth": truth["objects"],
        }))
    }
}

// ---- the live source ------------------------------------------------------------------------

/// A plain HTTP/1.0 GET of `url` (the simulator's small JSON answers).
pub async fn http_get(url: &str, timeout: std::time::Duration) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("{url}: only http:// is supported"))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let fut = async {
        let mut s = tokio::net::TcpStream::connect(hostport).await.map_err(|e| format!("connect {hostport}: {e}"))?;
        let req = format!("GET {path} HTTP/1.0\r\nHost: {hostport}\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        s.take(512 * 1024 * 1024).read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
        Ok::<Vec<u8>, String>(buf)
    };
    let buf = tokio::time::timeout(timeout, fut).await.map_err(|_| format!("{url}: timed out"))??;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| format!("{url}: no HTTP header"))?;
    let status = head.lines().next().unwrap_or("");
    if !status.split_whitespace().nth(1).is_some_and(|c| c == "200") {
        return Err(format!("{url}: {status}"));
    }
    Ok(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_geoid_and_the_anonymous_block_agree_with_the_simulated_world() {
        let text = std::fs::read_to_string(root().join("sim/traffic.toml")).expect("sim/traffic.toml");
        let doc: toml::Table = toml::from_str(&text).expect("toml");
        let n = doc["world"]["geoid_n_m"].as_float().expect("geoid_n_m");
        assert_eq!(n, BAY_GEOID_N_M, "the tracker converts Remote ID heights with the world's geoid");
        let c = tracker_config(&root());
        assert_eq!(c.geoid_n_m, BAY_GEOID_N_M);
        assert_eq!(c.anonymous_icao_ranges, vec![("ADF800".to_string(), "ADFBFF".to_string())]);
    }

    fn sensors() -> Vec<Value> {
        serde_json::from_str(
            r#"[
            {"id":"hub-pier","kind":"adsb_1090","num":1,"owner":"company","platform":null,"lat":37.817,"lon":-122.415,"alt_m":12.0,"nominal_range_m":150000.0,"feeders":null,"enabled":true,"up":true},
            {"id":"hub-pier","kind":"ais","num":null,"owner":"company","platform":null,"lat":37.817,"lon":-122.415,"alt_m":12.0,"nominal_range_m":40000.0,"feeders":null,"enabled":true,"up":true},
            {"id":"net-adsb","kind":"adsb_1090","num":9,"owner":"network","platform":null,"lat":null,"lon":null,"alt_m":null,"nominal_range_m":400000.0,"feeders":12,"enabled":true,"up":true},
            {"id":"12","kind":"daa","num":null,"owner":"company","platform":"12","lat":37.80,"lon":-122.45,"alt_m":90.0,"nominal_range_m":1800.0,"feeders":null,"enabled":true,"up":true},
            {"id":"12","kind":"rid","num":null,"owner":"company","platform":"12","lat":37.80,"lon":-122.45,"alt_m":90.0,"nominal_range_m":800.0,"feeders":null,"enabled":true,"up":true}
        ]"#,
        )
        .expect("listing")
    }

    fn sbs(session: u32, hex: &str, t_ms: i64, lat: f64, lon: f64, alt_ft: u32) -> String {
        let d = chrono::DateTime::from_timestamp_millis(t_ms).expect("time");
        let (date, time) = (d.format("%Y/%m/%d"), d.format("%H:%M:%S%.3f"));
        format!("MSG,3,{session},1,{hex},1,{date},{time},{date},{time},,{alt_ft},,,{lat:.5},{lon:.5},,,0,0,0,0\r\n")
    }

    const T0: i64 = 1_781_456_400_000;

    #[test]
    fn onboard_sensors_are_not_sites_and_a_silent_receiver_degrades_the_picture() {
        let p = Picture::new(Settings { url: None, host: "127.0.0.1".into(), sbs_port: 0, ais_port: 0, json_port: 0, off: false, tracker: tracker_config(&root()) });
        p.attach("in_process");
        let own: BTreeSet<String> = ["12".to_string()].into();
        p.set_sensors(&sensors(), &own);
        let s = p.sensors_json();
        assert_eq!(s["sensors"].as_array().map(Vec::len), Some(3), "{s}");
        assert_eq!(s["onboard"].as_array().map(Vec::len), Some(2), "{s}");
        assert!(s["onboard"][0].get("lat").is_none(), "an onboard sensor's last position is not a site's");
        assert!(s["sensors"][0].get("up").is_none(), "the simulator's own up flag is not passed on");
        let k = TrafficKnobs::default();
        let ops = [LonLat::new(-122.43, 37.80)];
        // a minute of an aircraft heard by both ADS-B receivers, and AIS from the hub
        for i in 0..60 {
            let t = T0 + i * 1000;
            for session in [1, 9] {
                p.ingest(Feed::Sbs, &sbs(session, "A1B2C3", t, 37.80 + i as f64 * 0.0005, -122.44, 1000), t + 50);
            }
            if i % 10 == 0 {
                p.ingest(Feed::Ais, "\\s:hub-pier,c:1781456400*00\\!AIVDM,1,1,,A,15M67FC000G?ufbE`FepT@3n00Sa,0*5C\r\n", t);
            }
            p.update(t + 100, Time(t + 100), &[], &OwnFleet::default(), Some(1014.5), &k, &ops, 10_000.0);
        }
        let b = p.built().expect("built");
        assert_eq!(b.summary["state"], json!("ok"), "{}", b.summary);
        assert_eq!(b.picture.tracks.len(), 1);
        let tf = &b.picture.tracks[0];
        assert_eq!(tf.alt_ref, "baro");
        // 1,000 ft of pressure altitude at QNH 1014.5 is about 315 m above sea level
        assert!((tf.alt_m.expect("alt") - (304.8 + 8.32 * 1.25)).abs() < 3.0, "{tf:?}");
        assert!(!tf.path.is_empty() && tf.path.len() <= 6, "{:?}", tf.path);
        // then hub-pier's ADS-B goes quiet: the network still hears the aircraft
        for i in 60..100 {
            let t = T0 + i * 1000;
            p.ingest(Feed::Sbs, &sbs(9, "A1B2C3", t, 37.80 + i as f64 * 0.0005, -122.44, 1000), t + 3000);
            p.update(t + 100, Time(t + 100), &[], &OwnFleet::default(), Some(1014.5), &k, &ops, 10_000.0);
        }
        let b = p.built().expect("built");
        assert_eq!(b.summary["state"], json!("degraded"), "{}", b.summary);
        assert!(b.summary["reasons"].as_array().is_some_and(|r| r.iter().any(|x| x.as_str().is_some_and(|s| s.starts_with("hub-pier adsb_1090")))), "{}", b.summary);
        let rx: Vec<_> = b.picture.receivers.iter().map(|r| (r.id.as_str(), r.kind.as_str(), r.up)).collect();
        assert!(rx.contains(&("hub-pier", "adsb_1090", false)) && rx.contains(&("net-adsb", "adsb_1090", true)), "{rx:?}");
        // the API's shape
        let api = p.api_json(None);
        assert!(api["t"].is_i64() && api["tracks"][0]["id"].is_string() && api["picture"]["feeds"]["sbs"]["lines_per_s"].is_number(), "{api}");
        let away = p.api_json(Some([-121.0, 36.0, -120.9, 36.1]));
        assert_eq!(away["tracks"].as_array().map(Vec::len), Some(0), "outside the view");
    }
}
