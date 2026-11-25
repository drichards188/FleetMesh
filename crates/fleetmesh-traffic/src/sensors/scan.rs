//! The company's own sensors that look rather than listen: detect-and-avoid on own aircraft
//! (vision: bearing and elevation, no range; about 1 to 2 km for a light aircraft, a few
//! hundred metres for a drone), marine radar on own boats (range and bearing, weak on small
//! craft, the radar horizon), and an optional ground radar (range, bearing, elevation and
//! Doppler). Each scan sees what is in its field of view and line of sight with a detection
//! probability `1 / (1 + (R / r50)^slope)`, adds measurement noise, and a few false detections
//! (birds, sea clutter). The company's other own vehicles are seen like anything else (a
//! camera does not know whose drone it is); the tracker drops them against the fleet's own
//! positions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::Kind;
use super::radio::{Body, Out};
use crate::config::ScanCfg;
use crate::encode::json::{self, DetMsg};
use crate::geo::{self, LatLon};
use crate::rng;
use crate::terrain::Terrain;
use crate::world::{Class, Mover};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnKind {
    Drone,
    Plane,
    Boat,
    Ground,
}

/// An own vehicle as the caller (the bridge) knows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OwnVehicle {
    pub uid: String,
    pub kind: OwnKind,
    pub lat: f64,
    pub lon: f64,
    pub alt_msl_m: f64,
    #[serde(default)]
    pub alt_rel_m: Option<f64>,
    #[serde(default)]
    pub heading_deg: f64,
    #[serde(default)]
    pub speed_mps: f64,
}

impl OwnVehicle {
    pub fn airborne(&self) -> bool {
        matches!(self.kind, OwnKind::Drone | OwnKind::Plane) && self.alt_rel_m.map_or(self.speed_mps > 2.0, |a| a > 5.0)
    }
}

/// A sensor's scan schedule.
#[derive(Clone, Debug)]
pub struct ScanState {
    pub next_ms: i64,
    pub k: u64,
}

/// Where a scan is taken from.
#[derive(Clone, Debug)]
pub struct Pose {
    pub id: String,
    pub kind: Kind,
    pub p: LatLon,
    pub alt_m: f64,
    pub vel_en: (f64, f64),
    pub order: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScanStats {
    pub scans: BTreeMap<&'static str, u64>,
    pub detections: BTreeMap<&'static str, u64>,
    pub false_detections: BTreeMap<&'static str, u64>,
}

fn neighbour_size(s: &'static str, up: bool) -> &'static str {
    match (s, up) {
        ("small", true) => "medium",
        ("medium", true) => "large",
        ("medium", false) => "small",
        ("large", false) => "medium",
        _ => s,
    }
}

fn poisson(h: u64, mean: f64) -> u32 {
    if mean <= 0.0 {
        return 0;
    }
    let u = rng::unit(h);
    let (mut k, mut p) = (0u32, (-mean).exp());
    let mut cdf = p;
    while u > cdf && k < 20 {
        k += 1;
        p *= mean / f64::from(k);
        cdf += p;
    }
    k
}

/// One scan at `t_ms` (scenario ms) from `pose`.
#[allow(clippy::too_many_arguments)]
pub fn scan(
    pose: &Pose,
    cfg: &ScanCfg,
    k: u64,
    t_ms: i64,
    start_ms: i64,
    seed: u64,
    movers: &[Mover],
    own: &[OwnVehicle],
    terrain: &Terrain,
    stats: &mut ScanStats,
    out: &mut Vec<Out>,
) {
    let t_s = t_ms as f64 / 1000.0;
    let kind_s = pose.kind.as_str();
    *stats.scans.entry(kind_s).or_insert(0) += 1;
    let max_r50 = cfg.r50_m.values().copied().fold(0.0, f64::max);
    let max_r = max_r50 * 2.5;
    let abs = start_ms + t_ms;
    let base = |bearing: f64, elev: Option<f64>, range: Option<f64>, radial: Option<f64>, size: &'static str| DetMsg {
        kind: "det",
        sensor: kind_s,
        platform: pose.id.clone(),
        t: abs,
        sensor_lat: json::round(pose.p.lat, 6),
        sensor_lon: json::round(pose.p.lon, 6),
        sensor_alt_m: json::round(pose.alt_m, 1),
        bearing_deg: json::round(geo::wrap360(bearing), 1),
        sigma_bearing_deg: cfg.sigma_bearing_deg,
        elev_deg: elev.map(|e| json::round(e, 1)),
        sigma_elev_deg: elev.and(cfg.sigma_elev_deg),
        range_m: range.map(|r| json::round(r.max(1.0), 0)),
        sigma_range_m: range.and(cfg.sigma_range_m),
        radial_speed_mps: radial.map(|v| json::round(v, 1)),
        size,
    };
    let mut seq: u8 = 0;
    let push = |out: &mut Vec<Out>, m_key: u64, seq: u8, d: DetMsg| {
        out.push(Out {
            deliver_ms: abs + cfg.latency_ms,
            order: (pose.order, t_ms, m_key, 100, k, seq),
            src: pose.id.clone(),
            kind: pose.kind,
            body: Body::Json(json::line(&d)),
        });
    };
    for m in movers {
        let Some(r50) = cfg.r50_m.get(m.class.as_str()).copied() else { continue };
        let Some(kin) = m.state(t_s) else { continue };
        let surface_target = !m.class.air() || kin.alt_m < 15.0;
        let wants_surface = pose.kind == Kind::MarineRadar;
        if wants_surface != surface_target && pose.kind != Kind::GroundRadar {
            continue;
        }
        if m.class.air() && kin.on_ground && pose.kind == Kind::Daa {
            continue;
        }
        let g = geo::haversine_m(pose.p, kin.p);
        if g > max_r {
            continue;
        }
        let tgt_alt = if m.class.air() { kin.alt_m } else { kin.alt_m + m.class.antenna_m() * 0.5 };
        let dz = tgt_alt - pose.alt_m;
        let r = (g * g + dz * dz).sqrt();
        if r < cfg.min_range_m.max(1.0) {
            continue;
        }
        let elev = dz.atan2(g).to_degrees();
        if elev > cfg.fov_up_deg || elev < -cfg.fov_down_deg {
            continue;
        }
        if terrain.line_of_sight(pose.p, pose.alt_m, kin.p, tgt_alt.max(0.5)).is_err() {
            continue;
        }
        let mut pd = 1.0 / (1.0 + (r / r50).powf(cfg.slope));
        if pose.kind == Kind::Daa && elev < -2.0 {
            // against the ground or the water: clutter
            pd *= 0.7;
        }
        let h = rng::hash(seed, &[pose.order, m.key, k, 0xDE7]);
        if rng::unit(h) >= pd {
            continue;
        }
        let bearing = geo::bearing_deg(pose.p, kin.p) + cfg.sigma_bearing_deg * rng::normal(rng::hash(h, &[1]));
        let e = cfg.sigma_elev_deg.map(|s| elev + s * rng::normal(rng::hash(h, &[2])));
        let range = cfg
            .sigma_range_m
            .map(|s| r + (s + cfg.sigma_range_frac * r) * rng::normal(rng::hash(h, &[3])));
        let radial = cfg.doppler.then(|| {
            let b = geo::bearing_deg(pose.p, kin.p).to_radians();
            let (ue, un) = (b.sin(), b.cos());
            let tv = (kin.gs_mps * kin.track_deg.to_radians().sin(), kin.gs_mps * kin.track_deg.to_radians().cos());
            let rel = (tv.0 - pose.vel_en.0, tv.1 - pose.vel_en.1);
            rel.0 * ue + rel.1 * un + cfg.sigma_speed_mps * rng::normal(rng::hash(h, &[4]))
        });
        let u = rng::unit(rng::hash(h, &[5]));
        let size = if u < 0.08 {
            neighbour_size(m.class.size(), true)
        } else if u < 0.16 {
            neighbour_size(m.class.size(), false)
        } else {
            m.class.size()
        };
        push(out, m.key, seq, base(bearing, e, range, radial, size));
        seq = seq.wrapping_add(1);
        *stats.detections.entry(kind_s).or_insert(0) += 1;
    }
    // the company's other vehicles: small drones and small boats to these sensors
    for v in own {
        if v.uid == pose.id {
            continue;
        }
        let (class, surface) = match v.kind {
            OwnKind::Drone | OwnKind::Plane if v.airborne() => ("drone", false),
            OwnKind::Boat => ("pleasure", true),
            _ => continue,
        };
        if (pose.kind == Kind::MarineRadar) != surface && pose.kind != Kind::GroundRadar {
            continue;
        }
        let Some(r50) = cfg.r50_m.get(class).copied() else { continue };
        let p = LatLon::new(v.lat, v.lon);
        let g = geo::haversine_m(pose.p, p);
        let tgt_alt = if surface { 1.5 } else { v.alt_msl_m };
        let dz = tgt_alt - pose.alt_m;
        let r = (g * g + dz * dz).sqrt();
        if g > max_r || r < cfg.min_range_m.max(1.0) {
            continue;
        }
        let elev = dz.atan2(g).to_degrees();
        if elev > cfg.fov_up_deg || elev < -cfg.fov_down_deg || terrain.line_of_sight(pose.p, pose.alt_m, p, tgt_alt).is_err() {
            continue;
        }
        let key = rng::tag(&v.uid) | (1 << 63);
        let h = rng::hash(seed, &[pose.order, key, k, 0xDE7]);
        if rng::unit(h) >= 1.0 / (1.0 + (r / r50).powf(cfg.slope)) {
            continue;
        }
        let bearing = geo::bearing_deg(pose.p, p) + cfg.sigma_bearing_deg * rng::normal(rng::hash(h, &[1]));
        let e = cfg.sigma_elev_deg.map(|s| elev + s * rng::normal(rng::hash(h, &[2])));
        let range = cfg.sigma_range_m.map(|s| r + (s + cfg.sigma_range_frac * r) * rng::normal(rng::hash(h, &[3])));
        let radial = cfg.doppler.then(|| {
            let b = geo::bearing_deg(pose.p, p).to_radians();
            let hd = v.heading_deg.to_radians();
            let rel = (v.speed_mps * hd.sin() - pose.vel_en.0, v.speed_mps * hd.cos() - pose.vel_en.1);
            rel.0 * b.sin() + rel.1 * b.cos() + cfg.sigma_speed_mps * rng::normal(rng::hash(h, &[4]))
        });
        push(out, key, seq, base(bearing, e, range, radial, "small"));
        seq = seq.wrapping_add(1);
        *stats.detections.entry(kind_s).or_insert(0) += 1;
    }
    let n_false = poisson(rng::hash(seed, &[pose.order, k, 0xFA15]), cfg.false_per_scan);
    for i in 0..n_false {
        let h = rng::hash(seed, &[pose.order, k, 0xFA16, u64::from(i)]);
        let bearing = rng::range(h, 0.0, 360.0);
        let (e, range) = match pose.kind {
            Kind::MarineRadar => (None, Some(rng::range(rng::hash(h, &[1]), 40.0, 1_500.0))),
            Kind::GroundRadar => (Some(rng::range(rng::hash(h, &[1]), 0.0, 10.0)), Some(rng::range(rng::hash(h, &[2]), 100.0, 3_000.0))),
            _ => (Some(3.0 * rng::normal(rng::hash(h, &[1]))), None),
        };
        let radial = cfg.doppler.then(|| 2.0 * rng::normal(rng::hash(h, &[3])));
        let e = e.filter(|_| cfg.sigma_elev_deg.is_some());
        let range = range.filter(|_| cfg.sigma_range_m.is_some());
        push(out, 0, seq, base(bearing, e, range, radial, "small"));
        seq = seq.wrapping_add(1);
        *stats.false_detections.entry(kind_s).or_insert(0) += 1;
    }
}

/// Which class of own vehicle carries which sensor.
pub fn onboard_kind(v: &OwnVehicle) -> Option<Kind> {
    match v.kind {
        OwnKind::Drone | OwnKind::Plane => Some(Kind::Daa),
        OwnKind::Boat => Some(Kind::MarineRadar),
        OwnKind::Ground => None,
    }
}

/// Whether the sensor runs now (detect-and-avoid in the air only).
pub fn running(v: &OwnVehicle, kind: Kind) -> bool {
    match kind {
        Kind::Daa => v.airborne(),
        _ => true,
    }
}

pub fn pose_of(v: &OwnVehicle, kind: Kind, mount_m: f64) -> Pose {
    let h = v.heading_deg.to_radians();
    Pose {
        id: v.uid.clone(),
        kind,
        p: LatLon::new(v.lat, v.lon),
        alt_m: v.alt_msl_m + mount_m,
        vel_en: (v.speed_mps * h.sin(), v.speed_mps * h.cos()),
        order: (1u64 << 40) | (rng::tag(&v.uid) & 0xFF_FFFF_FFFF),
    }
}

/// The class names a scan config may give r50 for.
pub fn r50_classes() -> Vec<&'static str> {
    Class::ALL.iter().map(|c| c.as_str()).collect()
}
