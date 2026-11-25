//! Trajectories as pure functions of time. A [`Plan`] is a start time and a sequence of legs:
//! a [`Path`] flown or sailed (waypoints with altitude and speed, corners rounded to a turn
//! radius) or a hold (moored, anchored, at a gate, hovering). Its state at any instant is computed
//! directly, so a world built from plans can be evaluated at any rate and started warm: a ship
//! that left the pilot station two hours before the scenario starts is simply part way along.

use crate::geo::{self, LatLon};

/// AIS navigational status codes the world uses (ITU-R M.1371 message 1 to 3).
pub mod status {
    pub const UNDER_WAY: u8 = 0;
    pub const AT_ANCHOR: u8 = 1;
    pub const RESTRICTED: u8 = 3;
    pub const MOORED: u8 = 5;
    pub const FISHING: u8 = 7;
    pub const SAILING: u8 = 8;
    pub const UNDEFINED: u8 = 15;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wp {
    pub p: LatLon,
    /// metres above mean sea level
    pub alt_m: f64,
    pub speed_mps: f64,
    /// on the ground or the water surface (an aircraft's take-off roll, a seaplane taxiing)
    pub ground: bool,
}

impl Wp {
    pub fn new(lat: f64, lon: f64, alt_m: f64, speed_mps: f64) -> Wp {
        Wp {
            p: LatLon::new(lat, lon),
            alt_m,
            speed_mps,
            ground: false,
        }
    }

    pub fn ground(mut self) -> Wp {
        self.ground = true;
        self
    }
}

/// How corners are rounded: a coordinated turn at `bank_deg` (aircraft), or a fixed radius
/// (vessels), clamped to `[min_radius_m, max_radius_m]`.
#[derive(Clone, Copy, Debug)]
pub struct Turn {
    pub bank_deg: f64,
    pub min_radius_m: f64,
    pub max_radius_m: f64,
}

impl Turn {
    pub const AIRLINER: Turn = Turn { bank_deg: 25.0, min_radius_m: 300.0, max_radius_m: 8_000.0 };
    pub const LIGHT: Turn = Turn { bank_deg: 30.0, min_radius_m: 80.0, max_radius_m: 2_000.0 };
    pub const SHIP: Turn = Turn { bank_deg: 0.0, min_radius_m: 600.0, max_radius_m: 1_500.0 };
    pub const BOAT: Turn = Turn { bank_deg: 0.0, min_radius_m: 60.0, max_radius_m: 300.0 };
    pub const SMALL: Turn = Turn { bank_deg: 0.0, min_radius_m: 8.0, max_radius_m: 40.0 };

    fn radius(&self, v: f64) -> f64 {
        let r = if self.bank_deg > 0.0 {
            v * v / (geo::G * self.bank_deg.to_radians().tan())
        } else {
            self.min_radius_m
        };
        r.clamp(self.min_radius_m, self.max_radius_m)
    }
}

/// Round each interior corner with a circular arc tangent to both legs.
pub fn fillet(wps: &[Wp], turn: Turn) -> Vec<Wp> {
    if wps.len() < 3 {
        return wps.to_vec();
    }
    let mut out = vec![wps[0]];
    for i in 1..wps.len() - 1 {
        let (prev, v, next) = (wps[i - 1], wps[i], wps[i + 1]);
        let o = v.p;
        let (ax, ay) = geo::enu(o, prev.p);
        let (cx, cy) = geo::enu(o, next.p);
        let (la, lc) = ((ax * ax + ay * ay).sqrt(), (cx * cx + cy * cy).sqrt());
        if la < 1.0 || lc < 1.0 || v.ground != next.ground {
            out.push(v);
            continue;
        }
        let u1 = (-ax / la, -ay / la);
        let u2 = (cx / lc, cy / lc);
        let dot = (u1.0 * u2.0 + u1.1 * u2.1).clamp(-1.0, 1.0);
        let theta = dot.acos();
        if theta < 3f64.to_radians() || theta > 175f64.to_radians() {
            out.push(v);
            continue;
        }
        let r0 = turn.radius(v.speed_mps.max(0.5));
        let mut d = r0 * (theta / 2.0).tan();
        d = d.min(0.45 * la.min(lc));
        let r = d / (theta / 2.0).tan();
        let left = u1.0 * u2.1 - u1.1 * u2.0 > 0.0;
        let sign = if left { 1.0 } else { -1.0 };
        let s = (-u1.0 * d, -u1.1 * d);
        let e = (u2.0 * d, u2.1 * d);
        let n_left = (-u1.1, u1.0);
        let c = (s.0 + sign * r * n_left.0, s.1 + sign * r * n_left.1);
        let phs = (s.1 - c.1).atan2(s.0 - c.0);
        let sweep = sign * theta;
        let n = ((theta.to_degrees() / 8.0).ceil() as usize).max(2);
        let fs = (la - d) / la;
        let fe = d / lc;
        let alt_s = prev.alt_m + (v.alt_m - prev.alt_m) * fs;
        let alt_e = v.alt_m + (next.alt_m - v.alt_m) * fe;
        let spd_s = prev.speed_mps + (v.speed_mps - prev.speed_mps) * fs;
        let spd_e = v.speed_mps + (next.speed_mps - v.speed_mps) * fe;
        for j in 0..=n {
            let f = j as f64 / n as f64;
            let ph = phs + sweep * f;
            let (x, y) = if j == 0 {
                s
            } else if j == n {
                e
            } else {
                (c.0 + r * ph.cos(), c.1 + r * ph.sin())
            };
            out.push(Wp {
                p: geo::from_enu(o, x, y),
                alt_m: alt_s + (alt_e - alt_s) * f,
                speed_mps: spd_s + (spd_e - spd_s) * f,
                ground: if j == 0 { prev.ground && v.ground } else { v.ground && next.ground },
            });
        }
    }
    out.push(wps[wps.len() - 1]);
    out
}

/// Kinematic state at an instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kin {
    pub p: LatLon,
    pub alt_m: f64,
    pub track_deg: f64,
    pub gs_mps: f64,
    pub vs_mps: f64,
    pub on_ground: bool,
    /// AIS navigational status of the current leg
    pub status: u8,
    /// in a hold (moored, anchored, hovering, at a gate)
    pub held: bool,
}

/// Waypoints with speeds: along each segment the speed changes linearly in time from the speed
/// at its start to the speed at its end.
#[derive(Clone, Debug)]
pub struct Path {
    pts: Vec<Wp>,
    cum_t: Vec<f64>,
    len3: Vec<f64>,
    len_h: Vec<f64>,
    brg: Vec<f64>,
    pub status: u8,
}

impl Path {
    pub fn new(mut pts: Vec<Wp>, status: u8) -> Path {
        pts.dedup_by(|b, a| geo::haversine_m(a.p, b.p) < 0.01 && (a.alt_m - b.alt_m).abs() < 0.01);
        if pts.len() == 1 {
            let mut q = pts[0];
            q.p = geo::destination(q.p, 0.0, 0.1);
            pts.push(q);
        }
        let mut cum_t = vec![0.0];
        let mut len3 = Vec::new();
        let mut len_h = Vec::new();
        let mut brg: Vec<f64> = Vec::new();
        for w in pts.windows(2) {
            let h = geo::haversine_m(w[0].p, w[1].p);
            let dz = w[1].alt_m - w[0].alt_m;
            let l = (h * h + dz * dz).sqrt().max(0.01);
            let v = (w[0].speed_mps + w[1].speed_mps).max(0.2);
            cum_t.push(cum_t[cum_t.len() - 1] + 2.0 * l / v);
            len3.push(l);
            len_h.push(h);
            let b = if h > 0.5 { geo::bearing_deg(w[0].p, w[1].p) } else { f64::NAN };
            brg.push(b);
        }
        // a vertical segment keeps the track of its neighbour
        for i in 0..brg.len() {
            if brg[i].is_nan() {
                brg[i] = (0..i).rev().map(|j| brg[j]).find(|b| !b.is_nan()).unwrap_or(f64::NAN);
            }
        }
        for i in (0..brg.len()).rev() {
            if brg[i].is_nan() {
                brg[i] = (i + 1..brg.len()).map(|j| brg[j]).find(|b| !b.is_nan()).unwrap_or(0.0);
            }
        }
        Path { pts, cum_t, len3, len_h, brg, status }
    }

    pub fn duration(&self) -> f64 {
        self.cum_t[self.cum_t.len() - 1]
    }

    pub fn first(&self) -> Wp {
        self.pts[0]
    }

    pub fn last(&self) -> Wp {
        self.pts[self.pts.len() - 1]
    }

    pub fn last_track(&self) -> f64 {
        self.brg[self.brg.len() - 1]
    }

    pub fn points(&self) -> &[Wp] {
        &self.pts
    }

    pub fn at(&self, dt: f64) -> Kin {
        let n = self.len3.len();
        let i = match self.cum_t.binary_search_by(|t| t.total_cmp(&dt)) {
            Ok(i) => i.min(n - 1),
            Err(i) => i.saturating_sub(1).min(n - 1),
        };
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        let seg_t = (self.cum_t[i + 1] - self.cum_t[i]).max(1e-9);
        let tau = (dt - self.cum_t[i]).clamp(0.0, seg_t);
        let acc = (b.speed_mps - a.speed_mps) / seg_t;
        let s = (a.speed_mps * tau + 0.5 * acc * tau * tau).clamp(0.0, self.len3[i]);
        let f = s / self.len3[i];
        let v = (a.speed_mps + acc * tau).max(0.0);
        let (l, h) = (self.len3[i], self.len_h[i]);
        Kin {
            p: geo::lerp(a.p, b.p, f),
            alt_m: a.alt_m + (b.alt_m - a.alt_m) * f,
            track_deg: self.brg[i],
            gs_mps: v * h / l,
            vs_mps: v * (b.alt_m - a.alt_m) / l,
            on_ground: a.ground && b.ground,
            status: self.status,
            held: false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Leg {
    Move(Path),
    Hold {
        p: LatLon,
        alt_m: f64,
        heading_deg: f64,
        dur: f64,
        status: u8,
        ground: bool,
    },
}

impl Leg {
    fn duration(&self) -> f64 {
        match self {
            Leg::Move(p) => p.duration(),
            Leg::Hold { dur, .. } => *dur,
        }
    }
}

/// A start time (scenario seconds) and legs one after another.
#[derive(Clone, Debug)]
pub struct Plan {
    pub t0: f64,
    starts: Vec<f64>,
    legs: Vec<Leg>,
    pub t1: f64,
}

impl Plan {
    pub fn new(t0: f64) -> Plan {
        Plan { t0, starts: Vec::new(), legs: Vec::new(), t1: t0 }
    }

    pub fn push(&mut self, leg: Leg) -> &mut Plan {
        let d = leg.duration().max(0.0);
        self.starts.push(self.t1);
        self.legs.push(leg);
        self.t1 += d;
        self
    }

    pub fn path(&mut self, wps: Vec<Wp>, turn: Option<Turn>, status: u8) -> &mut Plan {
        let pts = match turn {
            Some(t) => fillet(&wps, t),
            None => wps,
        };
        self.push(Leg::Move(Path::new(pts, status)))
    }

    /// Hold where the last leg ended (or at `p` if there is none).
    pub fn hold(&mut self, dur: f64, status: u8, at: Option<(LatLon, f64, f64, bool)>) -> &mut Plan {
        let (p, alt_m, heading_deg, ground) = match (at, self.legs.last()) {
            (Some(a), _) => a,
            (None, Some(Leg::Move(path))) => {
                let w = path.last();
                (w.p, w.alt_m, path.last_track(), w.ground)
            }
            (None, Some(Leg::Hold { p, alt_m, heading_deg, ground, .. })) => (*p, *alt_m, *heading_deg, *ground),
            (None, None) => (LatLon::new(0.0, 0.0), 0.0, 0.0, true),
        };
        self.push(Leg::Hold { p, alt_m, heading_deg, dur, status, ground })
    }

    /// Where the plan ends.
    pub fn end(&self) -> Option<Kin> {
        self.state(self.t1)
    }

    pub fn active(&self, t: f64) -> bool {
        t >= self.t0 && t <= self.t1
    }

    pub fn state(&self, t: f64) -> Option<Kin> {
        if !self.active(t) || self.legs.is_empty() {
            return None;
        }
        let i = match self.starts.binary_search_by(|s| s.total_cmp(&t)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let i = i.min(self.legs.len() - 1);
        Some(match &self.legs[i] {
            Leg::Move(p) => p.at(t - self.starts[i]),
            Leg::Hold { p, alt_m, heading_deg, status, ground, .. } => Kin {
                p: *p,
                alt_m: *alt_m,
                track_deg: *heading_deg,
                gs_mps: 0.0,
                vs_mps: 0.0,
                on_ground: *ground,
                status: *status,
                held: true,
            },
        })
    }

    /// Rate of turn, degrees per second (positive to starboard), by central difference.
    pub fn turn_dps(&self, t: f64) -> f64 {
        match (self.state(t - 1.0), self.state(t + 1.0)) {
            (Some(a), Some(b)) if a.gs_mps > 0.3 && b.gs_mps > 0.3 => geo::angle_diff(a.track_deg, b.track_deg) / 2.0,
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_takes_the_time_its_speeds_say_and_rounds_corners() {
        let a = Wp::new(37.80, -122.45, 0.0, 5.0);
        let b = Wp::new(37.81, -122.45, 0.0, 5.0);
        let c = Wp::new(37.81, -122.43, 0.0, 5.0);
        let mut plan = Plan::new(100.0);
        plan.path(vec![a, b, c], Some(Turn::BOAT), status::UNDER_WAY).hold(60.0, status::MOORED, None);
        let d = geo::haversine_m(a.p, b.p) + geo::haversine_m(b.p, c.p);
        // the fillet cuts the corner: a little shorter than the two legs
        let path_t = plan.t1 - 60.0 - 100.0;
        assert!(path_t < d / 5.0 && path_t > d / 5.0 - 30.0, "{path_t} vs {}", d / 5.0);
        let k = plan.state(100.0 + path_t / 2.0).expect("active");
        assert!((k.gs_mps - 5.0).abs() < 1e-6);
        assert!(plan.state(99.0).is_none());
        let end = plan.state(plan.t1 - 1.0).expect("held");
        assert!(end.held && end.status == status::MOORED);
        assert!(geo::haversine_m(end.p, c.p) < 0.5);
        // accelerating from rest along a runway
        let mut roll = Plan::new(0.0);
        roll.path(vec![Wp::new(37.0, -122.0, 0.0, 0.0).ground(), Wp::new(37.0, -121.98, 0.0, 80.0).ground()], None, 0);
        let len = geo::haversine_m(LatLon::new(37.0, -122.0), LatLon::new(37.0, -121.98));
        assert!((roll.t1 - 2.0 * len / 80.0).abs() < 1e-6);
        assert!(roll.state(1.0).expect("rolling").on_ground);
    }
}
