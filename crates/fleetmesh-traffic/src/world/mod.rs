//! The world: every object FleetMesh does not control, as [`Mover`]s whose [`Plan`]s are
//! generated per stream from seeded schedules. Generators run ahead of the clock (a lookahead),
//! each in its own seeded sequence, and name their objects `<stream>-<n>`, so the world is the
//! same however the caller steps it.

pub mod air;
pub mod ident;
pub mod issues;
pub mod sea;

use std::collections::BTreeMap;

use rand::Rng;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;

use crate::config::{EquipCfg, RouteCfg, ScenarioCfg, StreamCfg, WorldConfig};
use crate::geo::{self, FT, KT, LatLon};
use crate::path::{Kin, Plan, Wp};
use crate::rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Airliner,
    Bizjet,
    Ga,
    Helicopter,
    Seaplane,
    Drone,
    Cargo,
    Tanker,
    Passenger,
    Ferry,
    Tug,
    Pilot,
    Fishing,
    Sailing,
    Pleasure,
    SmallCraft,
}

impl Class {
    pub const ALL: [Class; 16] = [
        Class::Airliner,
        Class::Bizjet,
        Class::Ga,
        Class::Helicopter,
        Class::Seaplane,
        Class::Drone,
        Class::Cargo,
        Class::Tanker,
        Class::Passenger,
        Class::Ferry,
        Class::Tug,
        Class::Pilot,
        Class::Fishing,
        Class::Sailing,
        Class::Pleasure,
        Class::SmallCraft,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Class::Airliner => "airliner",
            Class::Bizjet => "bizjet",
            Class::Ga => "ga",
            Class::Helicopter => "helicopter",
            Class::Seaplane => "seaplane",
            Class::Drone => "drone",
            Class::Cargo => "cargo",
            Class::Tanker => "tanker",
            Class::Passenger => "passenger",
            Class::Ferry => "ferry",
            Class::Tug => "tug",
            Class::Pilot => "pilot",
            Class::Fishing => "fishing",
            Class::Sailing => "sailing",
            Class::Pleasure => "pleasure",
            Class::SmallCraft => "small_craft",
        }
    }

    pub fn parse(s: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.as_str() == s)
    }

    pub fn air(self) -> bool {
        matches!(self, Class::Airliner | Class::Bizjet | Class::Ga | Class::Helicopter | Class::Seaplane | Class::Drone)
    }

    pub fn domain(self) -> &'static str {
        if self.air() { "air" } else { "surface" }
    }

    /// What a sensor's size estimate would say.
    pub fn size(self) -> &'static str {
        match self {
            Class::Drone | Class::SmallCraft | Class::Sailing | Class::Pleasure => "small",
            Class::Ga | Class::Helicopter | Class::Seaplane | Class::Bizjet | Class::Tug | Class::Pilot | Class::Fishing | Class::Ferry => {
                "medium"
            }
            Class::Airliner | Class::Cargo | Class::Tanker | Class::Passenger => "large",
        }
    }

    /// A VHF antenna's height above the waterline (vessels), metres.
    pub fn antenna_m(self) -> f64 {
        match self {
            Class::Cargo => 40.0,
            Class::Tanker => 35.0,
            Class::Passenger => 50.0,
            Class::Ferry => 10.0,
            Class::Tug => 10.0,
            Class::Pilot => 6.0,
            Class::Fishing => 6.0,
            Class::Pleasure => 4.0,
            Class::Sailing => 12.0,
            Class::SmallCraft => 1.0,
            _ => 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Adsb {
    #[serde(rename = "1090")]
    Es1090,
    Uat,
    /// a Mode S transponder without ADS-B Out: radar replies (address, altitude, squawk) only
    ModeS,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Ais {
    A,
    B,
    #[serde(rename = "none")]
    None,
}

/// A Remote ID broadcast transport (ASTM F3411).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RidLink {
    Bt4,
    Bt5,
    Wifi,
}

impl RidLink {
    /// Nominal reception range from a good receiver in the open.
    pub fn range_m(self) -> f64 {
        match self {
            RidLink::Bt4 => 350.0,
            RidLink::Bt5 => 1_200.0,
            RidLink::Wifi => 900.0,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Equip {
    pub adsb: Adsb,
    pub ais: Ais,
    pub rid: Option<RidLink>,
    /// a privacy ICAO address (PIA)
    pub pia: bool,
    /// blocked from distribution (LADD)
    pub ladd: bool,
    /// UAT anonymous mode: a self-assigned address and no callsign
    pub anonymous: bool,
    /// AIS fitted but switched off
    pub ais_off: bool,
    /// Class B that also sends message 19
    pub type19: bool,
    #[serde(skip)]
    pub nav_sigma_m: f64,
    /// AIS: a rate-of-turn indicator and a gyro heading
    #[serde(skip)]
    pub rot_indicator: bool,
    #[serde(skip)]
    pub gyro: bool,
    /// AIS position accuracy flag (DGNSS)
    #[serde(skip)]
    pub dgnss: bool,
    /// AIS message type for position reports (1, 2 or 3)
    #[serde(skip)]
    pub pos_type: u8,
}

/// Identities the object broadcasts.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Ident {
    #[serde(serialize_with = "hex_opt")]
    pub icao24: Option<u32>,
    pub callsign: Option<String>,
    #[serde(skip)]
    pub squawk: Option<u16>,
    pub mmsi: Option<u32>,
    pub imo: Option<u32>,
    pub name: Option<String>,
    pub ship_callsign: Option<String>,
    #[serde(skip)]
    pub ship_type: u8,
    #[serde(skip)]
    pub dims: [u16; 4],
    #[serde(skip)]
    pub draught_dm: u8,
    pub destination: Option<String>,
    #[serde(skip)]
    pub eta: Option<[u8; 4]>,
    pub uas_id: Option<String>,
    #[serde(skip)]
    pub ua_type: &'static str,
    #[serde(skip)]
    pub vendor: Option<(String, u8, u32)>,
    /// a drone's operator (or take-off) position and ground height
    #[serde(skip)]
    pub op: Option<LatLon>,
    #[serde(skip)]
    pub ground_m: f64,
}

fn hex_opt<S: serde::Serializer>(v: &Option<u32>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => s.serialize_str(&format!("{x:06x}")),
        None => s.serialize_none(),
    }
}

/// What goes wrong with an object's broadcasts (never with its truth).
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fault {
    /// broadcast positions are off by this much (a faulty or spoofed navigation source)
    PositionOffset { t0: f64, t1: f64, north_m: f64, east_m: f64 },
    /// the transmitter is off
    Silent { t0: f64, t1: f64 },
}

#[derive(Clone, Debug)]
pub struct Mover {
    pub id: String,
    pub key: u64,
    pub stream: String,
    pub class: Class,
    pub plan: Plan,
    pub ident: Ident,
    pub equip: Equip,
    pub faults: Vec<Fault>,
    /// lateral offset of a smooth wander, metres (sailboats, kayaks); 0 for none
    pub wander_m: f64,
    pub(crate) tx: crate::sensors::radio::TxState,
}

impl Mover {
    pub fn new(stream: &str, n: u64, class: Class, plan: Plan, ident: Ident, equip: Equip) -> Mover {
        let id = format!("{stream}-{n:04}");
        let key = rng::tag(&id);
        Mover {
            id,
            key,
            stream: stream.to_string(),
            class,
            plan,
            ident,
            equip,
            faults: Vec::new(),
            wander_m: 0.0,
            tx: Default::default(),
        }
    }

    /// Truth at `t` (scenario seconds), with any wander.
    pub fn state(&self, t: f64) -> Option<Kin> {
        let mut k = self.plan.state(t)?;
        if self.wander_m > 0.0 && !k.held {
            let s = rng::Smooth::new(self.key ^ 0x3A11, 240.0).at(t) * self.wander_m;
            let side = geo::wrap360(k.track_deg + 90.0);
            k.p = geo::destination(k.p, side, s);
        }
        Some(k)
    }

    pub fn silent(&self, t: f64) -> bool {
        self.faults.iter().any(|f| matches!(f, Fault::Silent { t0, t1 } if t >= *t0 && t < *t1))
    }

    pub fn offset(&self, t: f64) -> Option<(f64, f64)> {
        self.faults.iter().find_map(|f| match f {
            Fault::PositionOffset { t0, t1, north_m, east_m } if t >= *t0 && t < *t1 => Some((*north_m, *east_m)),
            _ => None,
        })
    }
}

/// What every generator may read.
pub struct Ctx<'a> {
    pub cfg: &'a WorldConfig,
    pub scn: &'a ScenarioCfg,
    pub seed: u64,
    pub start_ms: i64,
    /// the schedules' window, scenario seconds
    pub t_begin: f64,
    pub t_end: f64,
}

impl Ctx<'_> {
    /// Local hour of day at scenario time `t`.
    pub fn local_h(&self, t: f64) -> f64 {
        let h = (self.start_ms as f64 / 1000.0 + t) / 3600.0 + self.cfg.world.utc_offset_h;
        h.rem_euclid(24.0)
    }

    pub fn equip_cfg(&self, class: Class) -> EquipCfg {
        self.cfg.equipage.get(class.as_str()).cloned().unwrap_or_default()
    }

    pub fn in_region(&self, p: LatLon) -> bool {
        let w = &self.cfg.world;
        p.lat >= w.lat_min && p.lat <= w.lat_max && p.lon >= w.lon_min && p.lon <= w.lon_max
    }

    pub fn utc_parts(&self, t: f64) -> (u8, u8, u8, u8) {
        let ms = self.start_ms + (t * 1000.0) as i64;
        let d = chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default();
        use chrono::{Datelike, Timelike};
        (d.month() as u8, d.day() as u8, d.hour() as u8, d.minute() as u8)
    }
}

pub trait Gen: Send {
    fn name(&self) -> &str;
    /// Create every object whose plan starts by `until`.
    fn fill(&mut self, until: f64, ctx: &Ctx, out: &mut Vec<Mover>);
}

/// A Poisson process of events at `per_hour`, only within the stream's local hours.
pub struct Poisson {
    pub per_s: f64,
    pub hours: Option<[f64; 2]>,
    pub next: f64,
}

impl Poisson {
    pub fn new(per_hour: f64, hours: Option<[f64; 2]>, t0: f64) -> Poisson {
        Poisson { per_s: per_hour.max(0.0) / 3600.0, hours, next: t0 }
    }

    fn in_hours(&self, ctx: &Ctx, t: f64) -> bool {
        match self.hours {
            None => true,
            Some([a, b]) => {
                let h = ctx.local_h(t);
                if a <= b { h >= a && h < b } else { h >= a || h < b }
            }
        }
    }

    /// Advance to the next event; infinity when the rate is zero.
    pub fn advance(&mut self, rng: &mut ChaCha8Rng, ctx: &Ctx) -> f64 {
        if self.per_s <= 0.0 {
            self.next = f64::INFINITY;
            return self.next;
        }
        let mut t = self.next;
        for _ in 0..10_000 {
            let u: f64 = rng.random::<f64>().max(1e-12);
            t += -u.ln() / self.per_s;
            if self.in_hours(ctx, t) {
                break;
            }
            // skip to the hour the stream opens
            let mut probe = t;
            for _ in 0..48 * 4 {
                probe += 900.0;
                if self.in_hours(ctx, probe) {
                    break;
                }
            }
            t = probe - 900.0;
        }
        self.next = t;
        t
    }
}

pub fn uniform(rng: &mut ChaCha8Rng, r: Option<[f64; 2]>, d: [f64; 2]) -> f64 {
    let [a, b] = r.unwrap_or(d);
    if b > a { rng.random_range(a..b) } else { a }
}

pub fn pick_weighted<'a, T>(rng: &mut ChaCha8Rng, items: &'a [T], w: impl Fn(&T) -> f64) -> Option<&'a T> {
    let total: f64 = items.iter().map(&w).sum();
    if items.is_empty() || total <= 0.0 {
        return items.first();
    }
    let mut x = rng.random::<f64>() * total;
    for it in items {
        x -= w(it);
        if x < 0.0 {
            return Some(it);
        }
    }
    items.last()
}

/// A class from a stream's `class` or `mix`.
pub fn pick_class(rng: &mut ChaCha8Rng, s: &StreamCfg, default: Class) -> Class {
    if !s.mix.is_empty() {
        let items: Vec<(&String, &f64)> = s.mix.iter().collect();
        if let Some((name, _)) = pick_weighted(rng, &items, |(_, w)| **w) {
            return Class::parse(name).unwrap_or(default);
        }
    }
    s.class.as_deref().and_then(Class::parse).unwrap_or(default)
}

/// Route points as waypoints: `[lat, lon]` (at `alt_m` and `kt`), `[lat, lon, alt_ft, kt]`,
/// `[lat, lon, alt_ft, kt, hold_s]`; a negative altitude is on the surface at that height.
/// Each point is moved up to `jitter_m` at random. Returns the waypoints and the holds
/// (index of the waypoint, seconds).
pub fn route_points(pts: &[Vec<f64>], jitter_m: f64, rng: &mut ChaCha8Rng, alt_m: f64, kt: f64) -> (Vec<Wp>, Vec<(usize, f64)>) {
    let mut out = Vec::new();
    let mut holds = Vec::new();
    for (i, p) in pts.iter().enumerate() {
        if p.len() < 2 {
            continue;
        }
        let mut ll = LatLon::new(p[0], p[1]);
        if jitter_m > 0.0 && i > 0 && i + 1 < pts.len() {
            let b = rng.random_range(0.0..360.0);
            let d = rng.random_range(0.0..jitter_m);
            ll = geo::destination(ll, b, d);
        }
        let (alt, ground) = match p.get(2) {
            Some(a) if *a < 0.0 => (-a * FT, true),
            Some(a) => (a * FT, false),
            None => (alt_m, false),
        };
        let spd = p.get(3).map_or(kt * KT, |k| k * KT);
        let mut w = Wp::new(ll.lat, ll.lon, alt, spd);
        w.ground = ground;
        out.push(w);
        if let Some(h) = p.get(4).filter(|h| **h > 0.0) {
            holds.push((out.len() - 1, *h));
        }
    }
    (out, holds)
}

pub fn pick_route<'a>(rng: &mut ChaCha8Rng, routes: &'a [RouteCfg]) -> Option<&'a RouteCfg> {
    pick_weighted(rng, routes, |r| r.weight)
}

/// The world's objects and the generators that make them.
pub struct World {
    pub movers: Vec<Mover>,
    gens: Vec<Box<dyn Gen>>,
    pub filled: f64,
    /// how far ahead of the clock objects are made
    pub lookahead_s: f64,
}

impl World {
    pub fn new(ctx: &Ctx) -> Result<World, String> {
        let mut gens: Vec<Box<dyn Gen>> = Vec::new();
        for s in &ctx.cfg.stream {
            if !s.on() || ctx.scn.disable.iter().any(|d| d == &s.name) {
                continue;
            }
            let g: Box<dyn Gen> = match s.kind.as_str() {
                "arrivals" | "departures" => Box::new(air::AirportFlow::new(s, ctx)?),
                "overflights" => Box::new(air::Overflights::new(s, ctx)),
                "route" => Box::new(air::RouteStream::new(s, ctx)),
                "drones" => Box::new(air::Drones::new(s, ctx)),
                "ships" => Box::new(sea::Ships::new(s, ctx)?),
                "ferry" => Box::new(sea::Ferry::new(s, ctx)),
                "moored" => Box::new(sea::Moored::new(s, ctx)),
                "wander" => Box::new(sea::Wander::new(s, ctx)),
                "pilot_station" => Box::new(sea::PilotStation::new(s, ctx)),
                other => return Err(format!("stream {}: unknown kind {other:?}", s.name)),
            };
            gens.push(g);
        }
        gens.push(Box::new(issues::Issues::new(ctx)?));
        Ok(World { movers: Vec::new(), gens, filled: f64::NEG_INFINITY, lookahead_s: 1_800.0 })
    }

    /// Make every object due by `t + lookahead`.
    pub fn fill(&mut self, t: f64, ctx: &Ctx) {
        let until = (t + self.lookahead_s).min(ctx.t_end);
        if until <= self.filled {
            return;
        }
        for g in &mut self.gens {
            g.fill(until, ctx, &mut self.movers);
        }
        self.filled = until;
    }

    /// Forget objects whose plans ended well before `t`.
    pub fn prune(&mut self, t: f64) {
        self.movers.retain(|m| m.plan.t1 >= t - 120.0);
    }

    pub fn generators(&self) -> Vec<String> {
        self.gens.iter().map(|g| g.name().to_string()).collect()
    }
}

/// Counts by class of what is in the region at `t`.
pub fn counts(movers: &[Mover], t: f64, ctx: &Ctx) -> BTreeMap<&'static str, usize> {
    let mut c = BTreeMap::new();
    for m in movers {
        if let Some(k) = m.state(t)
            && ctx.in_region(k.p)
        {
            *c.entry(m.class.as_str()).or_insert(0) += 1;
        }
    }
    c
}
