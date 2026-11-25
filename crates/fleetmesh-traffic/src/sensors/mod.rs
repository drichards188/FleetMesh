//! The company's receivers and sensors, and the commercial feeds it buys.
//!
//! - Receivers at each hub (sim/hubs.toml): ADS-B 1090, UAT 978, AIS and Remote ID, passive.
//! - Network feeds: an aggregator's many feeders on hills around the Bay, one stream, a few
//!   seconds late (LADD honoured).
//! - Onboard: detect-and-avoid on own aircraft, marine radar on own boats, a Remote ID receiver
//!   on own aircraft (radio::OwnRx, scan.rs), from own-vehicle positions the caller gives.
//! - An optional ground radar at a site, off by default.
//!
//! The fleet only listens on other traffic's frequencies; nothing here transmits.

pub mod radio;
pub mod scan;

use serde::{Deserialize, Serialize};

use crate::config::{IssueCfg, RxKindCfg, WorldConfig};
use crate::geo::LatLon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[serde(rename = "adsb_1090")]
    Adsb1090,
    AdsbUat,
    Ais,
    Rid,
    Daa,
    MarineRadar,
    GroundRadar,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Adsb1090 => "adsb_1090",
            Kind::AdsbUat => "adsb_uat",
            Kind::Ais => "ais",
            Kind::Rid => "rid",
            Kind::Daa => "daa",
            Kind::MarineRadar => "marine_radar",
            Kind::GroundRadar => "ground_radar",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        [Kind::Adsb1090, Kind::AdsbUat, Kind::Ais, Kind::Rid, Kind::Daa, Kind::MarineRadar, Kind::GroundRadar]
            .into_iter()
            .find(|k| k.as_str() == s)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub p: LatLon,
    /// antenna, metres above sea level
    pub alt_m: f64,
}

/// A fixed receiver (a hub's) or a network feed (many feeder sites).
#[derive(Clone, Debug)]
pub struct Receiver {
    pub index: usize,
    pub id: String,
    pub kind: Kind,
    /// the SBS session id, and the order of `/sensors`
    pub num: u32,
    pub network: bool,
    pub sites: Vec<Site>,
    pub range_m: f64,
    pub base_loss: f64,
    pub latency_ms: [i64; 2],
    pub delay_ms: [i64; 2],
    pub class_b_range_m: f64,
    pub slot_factor: f64,
    pub ladd: bool,
    /// scenario seconds it is down, [from, to)
    pub down: Vec<(f64, f64)>,
}

impl Receiver {
    pub fn up(&self, t: f64) -> bool {
        !self.down.iter().any(|(a, b)| t >= *a && t < *b)
    }
}

#[derive(Deserialize)]
struct HubsFile {
    #[serde(default)]
    hub: Vec<HubRow>,
}

#[derive(Deserialize)]
struct HubRow {
    id: String,
    lon: f64,
    lat: f64,
}

/// The receivers, in `/sensors` order: each hub's kinds, then the networks.
pub fn receivers(cfg: &WorldConfig, issues: &[IssueCfg]) -> Result<Vec<Receiver>, String> {
    let path = cfg.resolve(&cfg.receivers.hubs);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let hubs: HubsFile = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let r = &cfg.receivers;
    let mut out: Vec<Receiver> = Vec::new();
    for h in &hubs.hub {
        for k in &r.kinds {
            let kind = Kind::parse(k).ok_or_else(|| format!("receivers.kinds: unknown {k:?}"))?;
            let kc: &RxKindCfg = match kind {
                Kind::Adsb1090 => &r.adsb_1090,
                Kind::AdsbUat => &r.adsb_uat,
                Kind::Ais => &r.ais,
                Kind::Rid => &r.rid,
                _ => return Err(format!("receivers.kinds: {k} is not a receiver")),
            };
            out.push(Receiver {
                index: out.len(),
                id: h.id.clone(),
                kind,
                num: out.len() as u32 + 1,
                network: false,
                sites: vec![Site { p: LatLon::new(h.lat, h.lon), alt_m: r.ground_m + r.mast_m }],
                range_m: kc.range_m,
                base_loss: kc.base_loss,
                latency_ms: kc.latency_ms,
                delay_ms: [0, 0],
                class_b_range_m: kc.class_b_range_m.unwrap_or(kc.range_m),
                slot_factor: kc.slot_factor,
                ladd: false,
                down: Vec::new(),
            });
        }
    }
    for n in cfg.network.iter().filter(|n| n.enabled) {
        let kind = Kind::parse(&n.kind).ok_or_else(|| format!("network {}: unknown kind {:?}", n.id, n.kind))?;
        let class_b = if kind == Kind::Ais { r.ais.class_b_range_m.unwrap_or(n.range_m) } else { n.range_m };
        out.push(Receiver {
            index: out.len(),
            id: n.id.clone(),
            kind,
            num: out.len() as u32 + 1,
            network: true,
            sites: n.feeders.iter().map(|f| Site { p: LatLon::new(f[0], f[1]), alt_m: f[2] }).collect(),
            range_m: n.range_m,
            base_loss: n.base_loss,
            latency_ms: [0, 0],
            delay_ms: [(n.delay_s[0] * 1000.0) as i64, (n.delay_s[1] * 1000.0) as i64],
            class_b_range_m: class_b,
            slot_factor: if kind == Kind::Ais { r.ais.slot_factor } else { 0.0 },
            ladd: n.ladd,
            down: Vec::new(),
        });
    }
    for (i, is) in issues.iter().enumerate().filter(|(_, is)| is.kind == "receiver_down") {
        let sensor = (!is.sensor.is_empty()).then(|| Kind::parse(&is.sensor)).flatten();
        let mut hit = false;
        for rx in out.iter_mut().filter(|rx| rx.id == is.receiver && sensor.is_none_or(|k| k == rx.kind)) {
            let dur = if is.duration_s > 0.0 { is.duration_s } else { 600.0 };
            rx.down.push((is.at_s, is.at_s + dur));
            hit = true;
        }
        if !hit {
            return Err(format!("issue {i} (receiver_down): no receiver {:?} {:?}", is.receiver, is.sensor));
        }
    }
    Ok(out)
}

/// One entry of `/sensors`.
#[derive(Clone, Debug, Serialize)]
pub struct SensorInfo {
    pub id: String,
    pub kind: &'static str,
    pub num: Option<u32>,
    pub owner: &'static str,
    pub platform: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt_m: Option<f64>,
    pub nominal_range_m: f64,
    pub feeders: Option<usize>,
    pub enabled: bool,
    pub up: bool,
}

impl Receiver {
    pub fn info(&self, t: f64) -> SensorInfo {
        let one = (self.sites.len() == 1 && !self.network).then(|| self.sites[0]);
        SensorInfo {
            id: self.id.clone(),
            kind: self.kind.as_str(),
            num: Some(self.num),
            owner: if self.network { "network" } else { "company" },
            platform: None,
            lat: one.map(|s| s.p.lat),
            lon: one.map(|s| s.p.lon),
            alt_m: one.map(|s| s.alt_m),
            nominal_range_m: self.range_m,
            feeders: self.network.then_some(self.sites.len()),
            enabled: true,
            up: self.up(t),
        }
    }
}
