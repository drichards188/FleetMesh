//! Rules 3 ([`crate::rules::TRAFFIC_SEPARATION_SINCE`]): separation from the traffic the fleet does
//! not control (docs/design/traffic.md section 6, delivery-rules.md section 8.1).
//!
//! Pure functions of a [`TrafficPicture`] (what the company's tracker made of what it received,
//! logged by value with each decision) and the figures of `[rules.delivery.separation.traffic]`:
//!
//! - [`trim`]: the part of the picture a decision reads (the engine keeps nothing else, so the
//!   logged inputs stay small and the rule, which reads the same part, decides the same on them);
//! - [`assess`]: where one of our vehicles, along a route, comes within a minimum of a track along
//!   the tracker's prediction widened by its uncertainty: well clear of a manned aircraft, clear
//!   of another drone and of an air track that broadcasts nothing (a bearing-only track as the line
//!   it is), a ship under way's closest point of approach and its course ahead, other vessels;
//! - [`coverage_gap`]: a route the picture's receivers no longer cover;
//! - [`detour`]: a route around a predicted conflict, for the bridge's automation to propose (it
//!   goes through the engine like any order, so the rule judges it again).
//!
//! Altitudes are above mean sea level on both sides: a track's as the bridge gives it (geometric,
//! or a pressure altitude brought to sea level with the QNH, its conversion in the vertical
//! sigma), ours the cruise height above the vehicle's home plus that home's height above sea
//! level. Cars have no traffic model (they keep to the roads, which is not modelled).

use crate::catalog::Domain;
use crate::delivery::{TrackFact, TrafficKnobs, TrafficPicture};
use crate::geo::{LonLat, enu, haversine_m, offset};
use crate::intent::{IntentSpec, Task, TaskKind};
use crate::rules::{RulesConfig, TRAFFIC_SEPARATION_SINCE};
use crate::time::Time;
use crate::world::WorldSnapshot;

/// What a track is to the separation rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// A manned aircraft (it broadcasts ADS-B, or our sensors see one whose broadcast disagrees):
    /// well clear.
    Manned,
    /// Another operator's drone (Remote ID).
    Drone,
    /// An air track that broadcasts nothing (our own sensors only): what it is is not known.
    Noncoop,
    /// A ship under way.
    Ship,
    /// Any other vessel (or a ship not under way).
    Vessel,
}

impl TrackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TrackKind::Manned => "manned",
            TrackKind::Drone => "drone",
            TrackKind::Noncoop => "noncoop",
            TrackKind::Ship => "ship",
            TrackKind::Vessel => "vessel",
        }
    }
}

/// The track's kind, or None for one the rule does not consider (an aircraft on the ground).
pub fn kind_of(t: &TrackFact, k: &TrafficKnobs) -> Option<TrackKind> {
    match t.domain {
        Domain::Air => {
            if t.has_flag("on_ground") {
                return None;
            }
            if t.class == "drone" {
                Some(TrackKind::Drone)
            } else if !t.cooperative && !t.has_flag("sensor_disagree") {
                Some(TrackKind::Noncoop)
            } else {
                Some(TrackKind::Manned)
            }
        }
        Domain::Surface => {
            let speed = t.vel_mps.map_or(0.0, |[e, n]| e.hypot(n));
            let underway = !t.has_flag("moored") && !t.has_flag("at_anchor") && speed >= k.underway_mps;
            if underway && k.ship_classes.iter().any(|c| c == &t.class) {
                Some(TrackKind::Ship)
            } else {
                Some(TrackKind::Vessel)
            }
        }
        Domain::Ground => None,
    }
}

/// The minimum (horizontal, vertical) for a track kind, m, and how far ahead it is checked, s.
fn minimum(kind: TrackKind, k: &TrafficKnobs, surface_surface_m: f64) -> (f64, Option<f64>, f64) {
    match kind {
        TrackKind::Manned => (k.manned_h_m, Some(k.manned_v_m), k.horizon_s),
        TrackKind::Drone => (k.drone_h_m, Some(k.drone_v_m), k.horizon_s),
        TrackKind::Noncoop => (k.noncoop_h_m, Some(k.noncoop_v_m), k.horizon_s),
        TrackKind::Ship => (k.ship_cpa_m, None, k.surface_horizon_s),
        TrackKind::Vessel => (surface_surface_m, None, k.horizon_s),
    }
}

/// One of our vehicles at a time ahead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// seconds from the decision's time
    pub dt_s: f64,
    pub at: LonLat,
    /// above mean sea level, m (none on the surface or when not known)
    pub alt_m: Option<f64>,
}

/// Where a vehicle moving along `pts` at `speed_mps` is every `step_s` from now to `horizon_s`,
/// at `alt_m` (it stays at the last point once there).
pub fn trajectory(pts: &[LonLat], speed_mps: f64, alt_m: Option<f64>, step_s: f64, horizon_s: f64) -> Vec<Sample> {
    let step = step_s.max(0.5);
    let n = (horizon_s / step).floor() as usize;
    (0..=n)
        .map(|i| {
            let dt = i as f64 * step;
            Sample { dt_s: dt, at: along(pts, speed_mps, dt), alt_m }
        })
        .collect()
}

fn along(pts: &[LonLat], speed: f64, t: f64) -> LonLat {
    let mut left = speed.max(0.0) * t;
    for w in pts.windows(2) {
        let len = haversine_m(w[0], w[1]);
        if left <= len || len == 0.0 {
            let f = if len == 0.0 { 0.0 } else { left / len };
            return LonLat::new(w[0].lon + (w[1].lon - w[0].lon) * f, w[0].lat + (w[1].lat - w[0].lat) * f);
        }
        left -= len;
    }
    pts.last().copied().unwrap_or(LonLat::new(0.0, 0.0))
}

/// A track's predicted position `dt_s` after the picture's time: where, at what altitude, with
/// what horizontal and vertical 1-sigma. None past the end of the tracker's prediction.
fn track_at(t: &TrackFact, dt_s: f64, k: &TrafficKnobs) -> Option<(LonLat, Option<f64>, f64, Option<f64>)> {
    let cap_v = |sv: Option<f64>| -> Option<f64> {
        // a level aircraft's altitude does not random-walk: the growth is capped at vrate
        let cap = t.v_sigma_m.map(|v0| v0 + k.vrate_mps * dt_s.max(0.0));
        match (sv, cap) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    };
    if t.path.is_empty() {
        // no kinematics the tracker would project: where it was, widening at a target's speed
        let alt = t.alt_m.map(|a| a + t.vs_mps.unwrap_or(0.0) * dt_s);
        let h = t.h_sigma_m + k.noncoop_speed_mps * dt_s.max(0.0);
        return Some((t.at, alt, h, cap_v(t.v_sigma_m.map(|v| v + k.vrate_mps * dt_s.max(0.0)))));
    }
    if dt_s <= 0.0 {
        return Some((t.at, t.alt_m, t.h_sigma_m, t.v_sigma_m));
    }
    let mut prev = (0.0, t.at, t.alt_m, t.h_sigma_m, t.v_sigma_m);
    for p in &t.path {
        if dt_s <= p.dt_s {
            let span = (p.dt_s - prev.0).max(1e-9);
            let f = (dt_s - prev.0) / span;
            let lerp = |a: f64, b: f64| a + (b - a) * f;
            let at = LonLat::new(lerp(prev.1.lon, p.at.lon), lerp(prev.1.lat, p.at.lat));
            let alt = match (prev.2, p.alt_m) {
                (Some(a), Some(b)) => Some(lerp(a, b)),
                (a, b) => a.or(b),
            };
            let sv = match (prev.4, p.v_sigma_m) {
                (Some(a), Some(b)) => Some(lerp(a, b)),
                (a, b) => a.or(b),
            };
            return Some((at, alt, lerp(prev.3, p.h_sigma_m), cap_v(sv)));
        }
        prev = (p.dt_s, p.at, p.alt_m, p.h_sigma_m, p.v_sigma_m);
    }
    None
}

/// A predicted conflict between one of our vehicles and a track.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub track: String,
    pub label: String,
    pub class: String,
    pub kind: TrackKind,
    /// `SEP_TRAFFIC_CONFLICT`, `SEP_TRAFFIC_NONCOOP`, `SEP_SHIP_CPA` or `SEP_SHIP_AHEAD`
    pub code: &'static str,
    /// When, seconds from the decision's time.
    pub dt_s: f64,
    /// Horizontal distance over the widened minimum (below 1: inside it).
    pub ratio: f64,
    pub h_m: f64,
    pub band_h_m: f64,
    pub v_m: Option<f64>,
    pub band_v_m: Option<f64>,
    /// The minimum itself (before the uncertainty).
    pub min_h_m: f64,
    pub min_v_m: Option<f64>,
    /// Where we would be then, and where the track is predicted.
    pub at: LonLat,
    pub traffic_at: LonLat,
}

impl Conflict {
    /// The track, in words: its class and its label.
    pub fn who(&self) -> String {
        let class = self.class.replace('_', " ");
        if self.label.is_empty() { format!("{class} {}", self.track) } else { format!("{class} {} ({})", self.label, self.track) }
    }

    /// What it is, in one sentence.
    pub fn words(&self) -> String {
        let who = self.who();
        match self.code {
            "SEP_SHIP_AHEAD" => format!(
                "{who} under way: the route crosses its course {:.0} s ahead of it (a boat keeps out of a ship's way)",
                self.dt_s
            ),
            "SEP_SHIP_CPA" => format!(
                "{who} under way: closest approach {:.0} m at +{:.0} s, under {:.0} m ({:.0} m and its uncertainty)",
                self.h_m, self.dt_s, self.band_h_m, self.min_h_m
            ),
            _ => {
                let what = match self.kind {
                    TrackKind::Manned => format!("its well clear ({:.0} m, {:.0} m)", self.min_h_m, self.min_v_m.unwrap_or(0.0)),
                    TrackKind::Noncoop => format!("the {:.0} m kept from an aircraft that broadcasts nothing", self.min_h_m),
                    _ => format!("its {:.0} m minimum", self.min_h_m),
                };
                let v = match (self.v_m, self.band_v_m) {
                    (Some(v), Some(b)) => format!(", {v:.0} m apart vertically against {b:.0} m"),
                    _ if self.min_v_m.is_some() => ", its altitude not known".into(),
                    _ => String::new(),
                };
                format!(
                    "{who}: predicted inside {what} at +{:.0} s, {:.0} m apart against {:.0} m with its uncertainty{v}",
                    self.dt_s, self.h_m, self.band_h_m
                )
            }
        }
    }
}

/// What [`assess`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assessment {
    /// The worst conflict with each track (the earliest inside, else the closest).
    pub conflicts: Vec<Conflict>,
    /// The closest approach among the tracks that do not conflict (for the words).
    pub closest: Option<Conflict>,
    /// How many tracks were checked.
    pub checked: usize,
}

/// Every track of `picture` that `relevant` keeps and that is `our_domain`'s traffic, against our
/// vehicle at `samples` (its route `route` at `speed_mps`, for a ship's course ahead), at the
/// decision's time `now`, with the minimum by kind.
#[allow(clippy::too_many_arguments)]
pub fn assess(
    samples: &[Sample],
    route: &[LonLat],
    speed_mps: f64,
    our_domain: Domain,
    under_way: bool,
    picture: &TrafficPicture,
    now: Time,
    k: &TrafficKnobs,
    surface_surface_m: f64,
    relevant: impl Fn(&TrackFact) -> bool,
) -> Assessment {
    let mut out = Assessment::default();
    if our_domain == Domain::Ground {
        return out;
    }
    let age = (now.ms_since(picture.t) as f64 / 1000.0).max(0.0);
    let sf = k.sigma_factor;
    for t in &picture.tracks {
        if t.domain != our_domain || !relevant(t) {
            continue;
        }
        let Some(kind) = kind_of(t, k) else { continue };
        out.checked += 1;
        let (min_h, min_v, horizon) = minimum(kind, k, surface_surface_m);
        let code = match kind {
            TrackKind::Noncoop => "SEP_TRAFFIC_NONCOOP",
            TrackKind::Ship => "SEP_SHIP_CPA",
            _ => "SEP_TRAFFIC_CONFLICT",
        };
        let mk = |s: &Sample, h: f64, band_h: f64, v: Option<f64>, band_v: Option<f64>, at: LonLat| Conflict {
            track: t.id.clone(),
            label: t.label.clone(),
            class: t.class.clone(),
            kind,
            code,
            dt_s: s.dt_s,
            ratio: h / band_h.max(1e-6),
            h_m: h,
            band_h_m: band_h,
            v_m: v,
            band_v_m: band_v,
            min_h_m: min_h,
            min_v_m: min_v,
            at: s.at,
            traffic_at: at,
        };
        let mut worst: Option<Conflict> = None;
        let mut inside: Option<Conflict> = None;
        // each sample as (conflict, inside its minimum); a vehicle under way that is inside one
        // now and opening the distance is leaving it: only a closing (or held) one is a conflict
        let mut seen: Vec<(Conflict, bool)> = Vec::new();
        if let Some(line) = &t.line {
            // a bearing-only track: somewhere on its line, and moving at a target's speed since
            let (sb, cb) = line.bearing_deg.to_radians().sin_cos();
            let sig_b = (sf * line.sigma_deg).to_radians();
            for s in samples.iter().filter(|s| s.dt_s <= k.bearing_horizon_s) {
                let (e, n) = enu(line.from, s.at);
                let al = e * sb + n * cb;
                let cross = (e * cb - n * sb).abs();
                let r = al.clamp(line.range_m[0], line.range_m[1].max(line.range_m[0]));
                let h = (al - r).hypot(cross);
                let grow = k.noncoop_speed_mps * (s.dt_s + age);
                let band_h = min_h + r * sig_b.tan().abs() + grow;
                let (v, band_v) = match (line.elev_deg, s.alt_m) {
                    (Some(el), Some(a)) => {
                        let la = line.from_alt_m + r * el.to_radians().tan();
                        let se = (sf * line.sigma_elev_deg.unwrap_or(2.0)).to_radians();
                        (Some((a - la).abs()), Some(min_v.unwrap_or(0.0) + r * se.tan().abs() + k.vrate_mps * (s.dt_s + age)))
                    }
                    _ => (None, None),
                };
                let vert = match (v, band_v) {
                    (Some(v), Some(b)) => v < b,
                    _ => true,
                };
                if !vert {
                    continue;
                }
                let at = offset(line.from, r * sb, r * cb);
                let c = mk(s, h, band_h, v, band_v, at);
                seen.push((c, h < band_h));
            }
        } else {
            for s in samples.iter().filter(|s| s.dt_s <= horizon) {
                let Some((at, alt, sh, sv)) = track_at(t, s.dt_s + age, k) else { break };
                let h = haversine_m(s.at, at);
                let band_h = min_h + sf * sh;
                let (v, band_v) = match (min_v, s.alt_m, alt) {
                    (Some(mv), Some(a), Some(b)) => (Some((a - b).abs()), Some(mv + sf * sv.unwrap_or(0.0))),
                    _ => (None, None),
                };
                let vert = match (v, band_v) {
                    (Some(v), Some(b)) => v < b,
                    _ => true,
                };
                if !vert {
                    continue;
                }
                let c = mk(s, h, band_h, v, band_v, at);
                seen.push((c, h < band_h));
            }
        }
        for i in 0..seen.len() {
            let (c, inside_now) = &seen[i];
            let opening = seen.get(i + 1).is_some_and(|(n, _)| n.dt_s - c.dt_s <= 2.0 * k.horizon_s && n.h_m > c.h_m + 0.5);
            if *inside_now && inside.is_none() && !(under_way && opening) {
                inside = Some(c.clone());
            }
            if worst.as_ref().is_none_or(|w| c.ratio < w.ratio) {
                worst = Some(c.clone());
            }
        }
        if t.line.is_none() {
            // a ship under way: the boat stays out of its way ahead of it
            if kind == TrackKind::Ship
                && inside.is_none()
                && let Some(c) = crosses_ahead(t, route, speed_mps, age, k)
            {
                inside = Some(Conflict { code: "SEP_SHIP_AHEAD", ..c });
            }
        }
        match inside {
            Some(c) => out.conflicts.push(c),
            None => {
                if let Some(w) = worst
                    && out.closest.as_ref().is_none_or(|c| w.ratio < c.ratio)
                {
                    out.closest = Some(w);
                }
            }
        }
    }
    out.conflicts.sort_by(|a, b| a.dt_s.total_cmp(&b.dt_s).then_with(|| a.track.cmp(&b.track)));
    out
}

/// Where our route crosses a ship's course line ahead of the ship: the line it covers in the next
/// `ship_cross_ahead_s` at its velocity, crossed before it gets there.
fn crosses_ahead(t: &TrackFact, route: &[LonLat], speed: f64, age: f64, k: &TrafficKnobs) -> Option<Conflict> {
    let [ve, vn] = t.vel_mps?;
    let v = ve.hypot(vn);
    if v < 1e-3 || k.ship_cross_ahead_s <= 0.0 || route.len() < 2 {
        return None;
    }
    let p0 = offset(t.at, ve * age, vn * age);
    let p1 = offset(p0, ve * k.ship_cross_ahead_s, vn * k.ship_cross_ahead_s);
    let (bx, by) = enu(p0, p1);
    let mut t_us = 0.0;
    for w in route.windows(2) {
        let (ax, ay) = enu(p0, w[0]);
        let (cx, cy) = enu(p0, w[1]);
        let seg = haversine_m(w[0], w[1]);
        // the segment a..c against 0..b
        let (dx, dy) = (cx - ax, cy - ay);
        let den = dx * by - dy * bx;
        if den.abs() > 1e-9 {
            let s = (bx * ay - by * ax) / den; // along our segment
            let u = (dx * ay - dy * ax) / den; // along the ship's line
            if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&u) {
                let t_cross = t_us + s * seg / speed.max(0.1);
                let t_ship = u * k.ship_cross_ahead_s;
                if t_cross < t_ship {
                    let at = LonLat::new(w[0].lon + (w[1].lon - w[0].lon) * s, w[0].lat + (w[1].lat - w[0].lat) * s);
                    return Some(Conflict {
                        track: t.id.clone(),
                        label: t.label.clone(),
                        class: t.class.clone(),
                        kind: TrackKind::Ship,
                        code: "SEP_SHIP_AHEAD",
                        dt_s: t_ship - t_cross,
                        ratio: 0.0,
                        h_m: 0.0,
                        band_h_m: k.ship_cpa_m,
                        v_m: None,
                        band_v_m: None,
                        min_h_m: k.ship_cpa_m,
                        min_v_m: None,
                        at,
                        traffic_at: at,
                    });
                }
            }
        }
        t_us += seg / speed.max(0.1);
    }
    None
}

/// Why the picture does not cover a route of `domain` at `pts`: no ADS-B receiver (air) or AIS
/// receiver (surface) heard covers every point, and no commercial feed is heard. None: covered,
/// or a car (no traffic model).
pub fn coverage_gap(picture: &TrafficPicture, domain: Domain, pts: &[LonLat]) -> Option<String> {
    let (kind, what) = match domain {
        Domain::Air => ("adsb_1090", "ADS-B"),
        Domain::Surface => ("ais", "AIS"),
        Domain::Ground => return None,
    };
    let rx: Vec<_> = picture.receivers.iter().filter(|r| r.kind == kind).collect();
    let covers = |r: &&crate::delivery::ReceiverFact| {
        r.network
            || match (r.at, r.range_m) {
                (Some(at), Some(range)) => pts.iter().all(|p| haversine_m(*p, at) <= range),
                _ => false,
            }
    };
    if rx.iter().any(|r| r.up && covers(r)) {
        return None;
    }
    if rx.is_empty() {
        return Some(format!("the traffic picture lists no {what} receiver: other traffic over the route cannot be seen"));
    }
    let quiet: Vec<String> = rx.iter().filter(|r| !r.up).map(|r| r.id.clone()).collect();
    Some(format!(
        "no {what} receiver heard covers the route ({} not heard): traffic over it cannot be seen",
        if quiet.is_empty() { "none in range".to_string() } else { quiet.join(", ") }
    ))
}

/// The points a decision on `spec` reads traffic around: its vehicles' positions, the order's own
/// points, and the nearest hub for a return that names none.
pub fn reference_points(spec: &IntentSpec, world: &WorldSnapshot) -> Vec<LonLat> {
    let mut pts: Vec<LonLat> = spec
        .uids
        .iter()
        .filter_map(|u| world.vehicles.get(u).and_then(|v| v.position.as_ref()).map(|p| p.value))
        .collect();
    if let Some(p) = spec.task.path() {
        pts.extend(p.points);
    }
    if matches!(spec.task, Task::ReturnToHub { hub: None })
        && let Some(di) = &world.delivery
    {
        let near: Vec<LonLat> = pts.iter().filter_map(|p| di.places.nearest_hub(*p).map(|(h, _)| h.at())).collect();
        pts.extend(near);
    }
    pts
}

/// Whether a track is within `relevant_km` of any of `pts` (a bearing-only track by its sensor).
pub fn relevant(t: &TrackFact, pts: &[LonLat], relevant_km: f64) -> bool {
    let at = t.line.as_ref().map_or(t.at, |l| l.from);
    pts.iter().any(|p| haversine_m(*p, at) <= relevant_km * 1000.0)
}

/// The order kinds whose separation check reads the picture.
fn reads_picture(kind: TaskKind) -> bool {
    matches!(kind, TaskKind::Pickup | TaskKind::Deliver | TaskKind::Reroute | TaskKind::ReturnToHub)
}

/// Rules 3: cut a decision's picture to what its checks read: the tracks of its vehicles' domains
/// near its reference points ([`relevant`], the separation rule's own test) and the receivers of
/// those domains. An order whose checks do not read it (a hold, a swap, a car's) carries none.
pub fn trim(w: &mut WorldSnapshot, spec: &IntentSpec, cfg: &RulesConfig) {
    if !cfg.at_least(TRAFFIC_SEPARATION_SINCE) {
        return;
    }
    let domains: Vec<Domain> = spec
        .uids
        .iter()
        .filter_map(|u| w.vehicles.get(u).and_then(|v| v.platform.as_ref()).map(|p| p.domain))
        .filter(|d| *d != Domain::Ground)
        .collect();
    let keep = reads_picture(spec.kind()) && !domains.is_empty();
    let pts = if keep { reference_points(spec, w) } else { Vec::new() };
    let relevant_km = cfg.delivery_rules().separation.relevant_km;
    let Some(di) = w.delivery.as_mut() else { return };
    if !keep {
        di.picture = None;
        return;
    }
    let Some(p) = di.picture.as_mut() else { return };
    p.tracks.retain(|t| domains.contains(&t.domain) && relevant(t, &pts, relevant_km));
    p.receivers.retain(|r| {
        (r.kind == "adsb_1090" && domains.contains(&Domain::Air)) || (r.kind == "ais" && domains.contains(&Domain::Surface))
    });
}

/// A route from `from` to `to` around the first conflict a straight one meets: one waypoint
/// beside the conflict, on the side away from the track first, at 1 to 3 times the widened
/// minimum, the shortest that [`assess`] finds clear and that adds at most `max_added_frac` of the
/// straight length. None when the straight route is clear, when the conflict is now and the
/// vehicle is not under way (no route leaves it in time), or when nothing clears.
#[allow(clippy::too_many_arguments)]
pub fn detour(
    from: LonLat,
    to: LonLat,
    speed_mps: f64,
    alt_m: Option<f64>,
    our_domain: Domain,
    under_way: bool,
    picture: &TrafficPicture,
    now: Time,
    k: &TrafficKnobs,
    step_s: f64,
    surface_surface_m: f64,
    max_added_frac: f64,
    relevant: impl Fn(&TrackFact) -> bool + Copy,
) -> Option<Vec<LonLat>> {
    let horizon = if our_domain == Domain::Surface { k.surface_horizon_s.max(k.horizon_s) } else { k.horizon_s };
    let judge = |pts: &[LonLat]| {
        let s = trajectory(pts, speed_mps, alt_m, step_s, horizon);
        assess(&s, pts, speed_mps, our_domain, under_way, picture, now, k, surface_surface_m, relevant)
    };
    let straight = [from, to];
    let first = judge(&straight).conflicts.into_iter().next()?;
    // on the ground inside a minimum now, no route leaves it in time; under way one may
    if first.dt_s < step_s && !under_way {
        return None;
    }
    let base = haversine_m(from, to);
    if base < 1.0 {
        return None;
    }
    let (de, dn) = enu(from, to);
    let (ue, un) = (de / base, dn / base);
    // left of the direction of travel, and which side the track is on
    let (le, ln) = (-un, ue);
    let (te, tn) = enu(first.at, first.traffic_at);
    let away = if te * le + tn * ln > 0.0 { -1.0 } else { 1.0 };
    let mut best: Option<(f64, LonLat)> = None;
    for mult in [1.0, 1.5, 2.0, 3.0] {
        for side in [away, -away] {
            let off = first.band_h_m * mult + 50.0;
            let wp = offset(first.at, le * off * side, ln * off * side);
            let len = haversine_m(from, wp) + haversine_m(wp, to);
            if len > base * (1.0 + max_added_frac.max(0.0)) {
                continue;
            }
            if best.as_ref().is_some_and(|(l, _)| *l <= len) {
                continue;
            }
            if judge(&[from, wp, to]).conflicts.is_empty() {
                best = Some((len, wp));
            }
        }
    }
    best.map(|(_, wp)| vec![wp])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::{BearingFact, PathPoint, PictureState, ReceiverFact};

    fn pic(tracks: Vec<TrackFact>) -> TrafficPicture {
        TrafficPicture {
            t: Time(1_000_000),
            state: PictureState::Ok,
            reasons: Vec::new(),
            qnh_hpa: Some(1014.5),
            config_sha256: String::new(),
            receivers: vec![ReceiverFact { id: "net-adsb".into(), kind: "adsb_1090".into(), network: true, at: None, range_m: None, up: true }],
            tracks,
        }
    }

    /// A track at `at` moving at `vel`, with a straight prediction every 10 s and sigma growing.
    fn moving(id: &str, class: &str, coop: bool, at: LonLat, alt: Option<f64>, vel: [f64; 2]) -> TrackFact {
        let path = (1..=6)
            .map(|i| {
                let dt = i as f64 * 10.0;
                PathPoint { dt_s: dt, at: offset(at, vel[0] * dt, vel[1] * dt), alt_m: alt, h_sigma_m: 20.0 + dt * 2.0, v_sigma_m: alt.map(|_| 10.0 + dt) }
            })
            .collect();
        TrackFact {
            id: id.into(),
            domain: if class == "cargo" || class == "sailing" { Domain::Surface } else { Domain::Air },
            class: class.into(),
            cooperative: coop,
            label: String::new(),
            at,
            alt_m: alt,
            alt_ref: "geo".into(),
            h_sigma_m: 20.0,
            v_sigma_m: alt.map(|_| 10.0),
            vel_mps: Some(vel),
            vs_mps: Some(0.0),
            flags: Vec::new(),
            line: None,
            path,
        }
    }

    fn run(p: &TrafficPicture, pts: &[LonLat], alt: f64, domain: Domain) -> Assessment {
        let k = TrafficKnobs::default();
        let s = trajectory(pts, 10.0, Some(alt), 5.0, 300.0);
        assess(&s, pts, 10.0, domain, false, p, Time(1_000_000), &k, 30.0, |_| true)
    }

    const A: LonLat = LonLat { lon: -122.45, lat: 37.80 };

    #[test]
    fn a_helicopter_crossing_low_is_a_conflict_and_one_high_above_is_not() {
        let to = offset(A, 2000.0, 0.0);
        // a helicopter coming south over the route's middle at 150 m
        let heli = moving("t1", "helicopter", true, offset(A, 1000.0, 1200.0), Some(150.0), [0.0, -40.0]);
        let a = run(&pic(vec![heli.clone()]), &[A, to], 100.0, Domain::Air);
        assert_eq!(a.conflicts.len(), 1, "{a:?}");
        assert_eq!(a.conflicts[0].code, "SEP_TRAFFIC_CONFLICT");
        assert!(a.conflicts[0].words().contains("well clear"), "{}", a.conflicts[0].words());
        // the same at 1,500 ft (457 m): vertical separation holds
        let mut high = heli;
        high.alt_m = Some(457.0);
        for p in &mut high.path {
            p.alt_m = Some(457.0);
        }
        let a = run(&pic(vec![high]), &[A, to], 100.0, Domain::Air);
        assert!(a.conflicts.is_empty(), "{a:?}");
    }

    #[test]
    fn a_manned_aircraft_whose_altitude_is_unknown_is_taken_as_level_with_us() {
        let to = offset(A, 2000.0, 0.0);
        let mut ga = moving("t2", "unknown_air", true, offset(A, 1000.0, 1200.0), None, [0.0, -40.0]);
        ga.alt_ref = "none".into();
        let a = run(&pic(vec![ga]), &[A, to], 100.0, Domain::Air);
        assert_eq!(a.conflicts.len(), 1, "{a:?}");
    }

    #[test]
    fn another_drone_has_a_smaller_minimum_than_a_manned_aircraft() {
        let to = offset(A, 2000.0, 0.0);
        // hovering 300 m north of the route: outside a drone's 100 m, inside a manned 610 m
        let mut d = moving("t3", "drone", true, offset(A, 1000.0, 300.0), Some(100.0), [0.0, 0.0]);
        let a = run(&pic(vec![d.clone()]), &[A, to], 100.0, Domain::Air);
        assert!(a.conflicts.is_empty(), "{a:?}");
        d.class = "helicopter".into();
        let a = run(&pic(vec![d]), &[A, to], 100.0, Domain::Air);
        assert_eq!(a.conflicts.len(), 1);
    }

    #[test]
    fn a_bearing_only_line_across_the_route_holds_and_one_pointing_away_does_not() {
        let to = offset(A, 1000.0, 0.0);
        let line = |bearing: f64| TrackFact {
            id: "b1".into(),
            domain: Domain::Air,
            class: "unknown_air".into(),
            cooperative: false,
            label: String::new(),
            at: A,
            alt_m: None,
            alt_ref: "none".into(),
            h_sigma_m: 1000.0,
            v_sigma_m: None,
            vel_mps: None,
            vs_mps: None,
            flags: vec!["bearing_only".into()],
            line: Some(BearingFact { from: offset(A, 500.0, -1500.0), from_alt_m: 100.0, bearing_deg: bearing, sigma_deg: 2.0, elev_deg: None, sigma_elev_deg: None, range_m: [20.0, 2000.0] }),
            path: Vec::new(),
        };
        let a = run(&pic(vec![line(0.0)]), &[A, to], 100.0, Domain::Air);
        assert_eq!(a.conflicts.first().map(|c| c.code), Some("SEP_TRAFFIC_NONCOOP"), "{a:?}");
        let a = run(&pic(vec![line(180.0)]), &[A, to], 100.0, Domain::Air);
        assert!(a.conflicts.is_empty(), "{a:?}");
    }

    #[test]
    fn a_boat_keeps_its_cpa_from_a_ship_and_does_not_cross_ahead_of_it() {
        // our boat east 1 km at 8 m/s; a ship 1.5 km north coming south at 6 m/s: they would meet
        let to = offset(A, 1000.0, 0.0);
        let ship = moving("s1", "cargo", true, offset(A, 500.0, 1500.0), None, [0.0, -6.0]);
        let mut long = ship.clone();
        long.path = (1..=10)
            .map(|i| {
                let dt = i as f64 * 30.0;
                PathPoint { dt_s: dt, at: offset(ship.at, 0.0, -6.0 * dt), alt_m: None, h_sigma_m: 20.0 + dt, v_sigma_m: None }
            })
            .collect();
        let k = TrafficKnobs::default();
        let s = trajectory(&[A, to], 8.0, None, 5.0, 300.0);
        let a = assess(&s, &[A, to], 8.0, Domain::Surface, false, &pic(vec![long.clone()]), Time(1_000_000), &k, 30.0, |_| true);
        assert!(a.conflicts.iter().any(|c| matches!(c.code, "SEP_SHIP_CPA" | "SEP_SHIP_AHEAD")), "{a:?}");
        // the same ship moored: only the vessel minimum applies, and it is far
        let mut moored = long;
        moored.flags.push("moored".into());
        moored.vel_mps = Some([0.0, 0.0]);
        moored.path.iter_mut().for_each(|p| p.at = moored.at);
        let a = assess(&s, &[A, to], 8.0, Domain::Surface, false, &pic(vec![moored]), Time(1_000_000), &k, 30.0, |_| true);
        assert!(a.conflicts.is_empty(), "{a:?}");
    }

    #[test]
    fn crossing_well_behind_a_ship_is_clear_and_ahead_of_it_is_not() {
        let k = TrafficKnobs::default();
        // a ship 2 km west heading east at 6 m/s along y = 0; our boat crosses y = 0 going north
        let ship_at = offset(A, -2000.0, 0.0);
        let mut ship = moving("s2", "cargo", true, ship_at, None, [6.0, 0.0]);
        ship.path.clear();
        ship.h_sigma_m = 10.0;
        // crossing 1.2 km ahead of the bow, now: ahead
        let from = offset(A, -800.0, -100.0);
        let to = offset(A, -800.0, 100.0);
        assert!(crosses_ahead(&ship, &[from, to], 5.0, 0.0, &k).is_some());
        // crossing 500 m behind its stern (it has passed): clear
        let from = offset(A, -2500.0, -100.0);
        let to = offset(A, -2500.0, 100.0);
        assert!(crosses_ahead(&ship, &[from, to], 5.0, 0.0, &k).is_none());
    }

    #[test]
    fn under_way_inside_a_minimum_and_leaving_it_is_not_a_conflict_and_hovering_there_is() {
        let k = TrafficKnobs::default();
        // a hovering drone 50 m off; our drone heads away from it
        let d = moving("t5", "drone", true, offset(A, -50.0, 0.0), Some(100.0), [0.0, 0.0]);
        let p = pic(vec![d]);
        let away = [A, offset(A, 2000.0, 0.0)];
        let s = trajectory(&away, 10.0, Some(100.0), 5.0, 60.0);
        assert!(assess(&s, &away, 10.0, Domain::Air, true, &p, Time(1_000_000), &k, 30.0, |_| true).conflicts.is_empty());
        // on the ground (taking off) it is one: it does not leave until the drone is clear
        assert_eq!(assess(&s, &away, 10.0, Domain::Air, false, &p, Time(1_000_000), &k, 30.0, |_| true).conflicts.len(), 1);
        // and hovering where it is, under way, is one too
        let here = trajectory(&[A], 0.0, Some(100.0), 5.0, 60.0);
        assert_eq!(assess(&here, &[A], 0.0, Domain::Air, true, &p, Time(1_000_000), &k, 30.0, |_| true).conflicts.len(), 1);
    }

    #[test]
    fn a_route_no_receiver_covers_is_a_gap() {
        let mut p = pic(Vec::new());
        assert!(coverage_gap(&p, Domain::Air, &[A]).is_none());
        p.receivers[0].up = false;
        p.receivers.push(ReceiverFact { id: "hub-pier".into(), kind: "adsb_1090".into(), network: false, at: Some(offset(A, 0.0, 200_000.0)), range_m: Some(150_000.0), up: true });
        assert!(coverage_gap(&p, Domain::Air, &[A]).is_some_and(|g| g.contains("net-adsb")), "out of the hub's range, the network silent");
        p.receivers[1].at = Some(offset(A, 1000.0, 0.0));
        assert!(coverage_gap(&p, Domain::Air, &[A]).is_none());
        assert!(coverage_gap(&p, Domain::Surface, &[A]).is_some(), "no AIS receiver at all");
        assert!(coverage_gap(&p, Domain::Ground, &[A]).is_none(), "cars: no traffic model");
    }

    #[test]
    fn a_detour_goes_round_a_hovering_drone_and_none_is_needed_when_clear() {
        let k = TrafficKnobs::default();
        let to = offset(A, 3000.0, 0.0);
        // a hovering drone on the route, 400 m along it
        let d = moving("t4", "drone", true, offset(A, 400.0, 0.0), Some(100.0), [0.0, 0.0]);
        let p = pic(vec![d]);
        let w = detour(A, to, 10.0, Some(100.0), Domain::Air, false, &p, Time(1_000_000), &k, 5.0, 30.0, 0.25, |_| true).expect("a way round");
        assert_eq!(w.len(), 1);
        let s = trajectory(&[A, w[0], to], 10.0, Some(100.0), 5.0, 60.0);
        assert!(assess(&s, &[A, w[0], to], 10.0, Domain::Air, false, &p, Time(1_000_000), &k, 30.0, |_| true).conflicts.is_empty());
        let clear = pic(Vec::new());
        assert!(detour(A, to, 10.0, Some(100.0), Domain::Air, false, &clear, Time(1_000_000), &k, 5.0, 30.0, 0.25, |_| true).is_none());
    }
}
