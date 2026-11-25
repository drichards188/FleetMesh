//! Vessels: deep-draft ships through the traffic separation scheme, the bar channel and the
//! Golden Gate to their berths (with the pilot launch and escort tugs each needs), ferries on
//! their schedules, ships moored or at anchor, a pilot station boat, and small craft wandering
//! in their areas (sailboats, kayaks, pleasure boats, fishing boats).

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::ident;
use super::{Class, Ctx, Gen, Mover, Poisson, pick_class, pick_weighted, uniform};
use crate::config::{ApproachCfg, StreamCfg};
use crate::geo::{self, KT, LatLon};
use crate::path::{Leg, Path, Plan, Turn, Wp, fillet, status};
use crate::rng;

fn wp(p: &[f64], kt: f64) -> Wp {
    let k = p.get(2).copied().unwrap_or(kt);
    Wp::new(p[0], p[1], 0.0, k * KT)
}

fn wp2(p: [f64; 2], kt: f64) -> Wp {
    Wp::new(p[0], p[1], 0.0, kt * KT)
}

fn status_of(s: Option<&str>) -> u8 {
    match s {
        Some("moored") => status::MOORED,
        Some("anchored") => status::AT_ANCHOR,
        Some("fishing") => status::FISHING,
        Some("sailing") => status::SAILING,
        Some("restricted") => status::RESTRICTED,
        Some("undefined") => status::UNDEFINED,
        _ => status::UNDER_WAY,
    }
}

/// When the ship is nearest to `p` along a plan, sampled every 10 s.
pub fn time_near(plan: &Plan, p: LatLon) -> f64 {
    let mut best = (f64::INFINITY, plan.t0);
    let mut t = plan.t0;
    while t <= plan.t1 {
        if let Some(k) = plan.state(t) {
            let d = geo::haversine_m(k.p, p);
            if d < best.0 {
                best = (d, t);
            }
        }
        t += 10.0;
    }
    best.1
}

/// Deep-draft ships to and from one destination (Oakland, Richmond, the north bay).
pub struct Ships {
    s: StreamCfg,
    rng: ChaCha8Rng,
    inbound: Poisson,
    outbound: Poisson,
    n: u64,
    berth_i: usize,
}

impl Ships {
    pub fn new(s: &StreamCfg, ctx: &Ctx) -> Result<Ships, String> {
        if ctx.cfg.ships.approach.is_empty() || ctx.cfg.ships.pilot_station.is_none() {
            return Err(format!("stream {}: [ships] needs a pilot_station and approaches", s.name));
        }
        let m = ctx.scn.rate(&s.name);
        Ok(Ships {
            s: s.clone(),
            rng: rng::chacha(ctx.seed, &[rng::tag(&s.name)]),
            inbound: Poisson::new(s.per_hour * m, None, ctx.t_begin - 3.0 * 3600.0),
            outbound: Poisson::new(s.per_hour_out * m, None, ctx.t_begin),
            n: 0,
            berth_i: 0,
        })
    }

    fn next_berth(&mut self) -> Option<(LatLon, f64)> {
        if self.s.berths.is_empty() {
            return None;
        }
        let b = &self.s.berths[self.berth_i % self.s.berths.len()];
        self.berth_i += 1;
        // ships at one berth lie along it
        let along = (self.berth_i / self.s.berths.len()) as f64 * 320.0;
        let h = b.get(2).copied().unwrap_or(0.0);
        Some((geo::destination(LatLon::new(b[0], b[1]), h, along), h))
    }
}

fn approach<'a>(rng: &mut ChaCha8Rng, ctx: &'a Ctx) -> Option<&'a ApproachCfg> {
    pick_weighted(rng, &ctx.cfg.ships.approach, |a| a.weight)
}

/// An inbound ship from sea to its berth, starting at `t0`; returns the ship and the time it
/// passes the pilot station.
#[allow(clippy::too_many_arguments)]
pub fn inbound_ship(s: &StreamCfg, rng: &mut ChaCha8Rng, ctx: &Ctx, t0: f64, n: u64, berth: Option<(LatLon, f64)>, class: Class, name: Option<&str>) -> Option<(Mover, f64)> {
    let ships = &ctx.cfg.ships;
    let station = ships.pilot_station?;
    let sea_kt = ships.sea_kt.unwrap_or(14.0) * rng.random_range(0.9..1.1);
    let a = approach(rng, ctx)?;
    let mut sea: Vec<Wp> = a.inbound.iter().map(|p| wp2(*p, sea_kt)).collect();
    sea.push(wp2(station, 8.0));
    let leg1 = Path::new(fillet(&sea, Turn::SHIP), status::UNDER_WAY);
    let mut bay: Vec<Wp> = vec![wp2(station, 8.0)];
    bay.extend(ships.bar_inbound.iter().map(|p| wp(p, 12.0)));
    bay.extend(s.inbound.iter().map(|p| wp(p, 10.0)));
    let (berth_p, berth_h) = berth.unwrap_or((bay.last()?.p, 0.0));
    bay.push(Wp::new(berth_p.lat, berth_p.lon, 0.0, 0.5 * KT));
    let leg2 = Path::new(fillet(&bay, Turn::SHIP), status::UNDER_WAY);
    let t_station = t0 + leg1.duration();
    let mut plan = Plan::new(t0);
    plan.push(Leg::Move(leg1));
    plan.push(Leg::Move(leg2));
    let dwell = uniform(rng, s.dwell_h, [14.0, 30.0]) * 3600.0;
    plan.hold(dwell, status::MOORED, Some((berth_p, 0.0, berth_h, false)));
    let us = rng.random_bool(0.08);
    let mut id = ident::vessel(rng, class, us, s.length_m, name);
    let (mo, d, h, mi) = ctx.utc_parts(t_station + 2.5 * 3600.0);
    id.eta = Some([mo, d, h, mi]);
    id.destination = s.destination.clone();
    let eq = ident::equip(rng, class, &ctx.equip_cfg(class));
    Some((Mover::new(&s.name, n, class, plan, id, eq), t_station))
}

/// A pilot launch from the pilot base to the station, timed to meet a ship there at `t_meet`,
/// and back.
pub fn pilot_launch(rng: &mut ChaCha8Rng, ctx: &Ctx, stream: &str, n: u64, t_meet: f64) -> Option<Mover> {
    let ships = &ctx.cfg.ships;
    let (base, station) = (ships.pilot_base?, ships.pilot_station?);
    let kt = 20.0;
    let mut out: Vec<Wp> = vec![wp2(base, 8.0)];
    out.extend(ships.launch_route.iter().map(|p| wp2(*p, kt)));
    out.push(wp2(station, 6.0));
    let go = Path::new(fillet(&out, Turn::BOAT), status::UNDER_WAY);
    let mut back_pts: Vec<Wp> = out.iter().rev().copied().collect();
    back_pts.iter_mut().for_each(|w| w.speed_mps = kt * KT);
    if let Some(last) = back_pts.last_mut() {
        last.speed_mps = 3.0 * KT;
    }
    let back = Path::new(fillet(&back_pts, Turn::BOAT), status::UNDER_WAY);
    let mut plan = Plan::new(t_meet - go.duration() - 300.0);
    plan.push(Leg::Move(go));
    plan.hold(600.0 + rng.random_range(0.0..300.0), status::UNDER_WAY, None);
    plan.push(Leg::Move(back));
    let id = ident::vessel(rng, Class::Pilot, true, None, None);
    let eq = ident::equip(rng, Class::Pilot, &ctx.equip_cfg(Class::Pilot));
    Some(Mover::new(stream, n, Class::Pilot, plan, id, eq))
}

impl Ships {
    fn tug(&mut self, ctx: &Ctx, n: u64, ship: &Plan, side: f64, inbound: bool) -> Option<Mover> {
        let base = self.s.tug_base?;
        let rng = &mut self.rng;
        let path_pts: Vec<Vec<f64>> = if inbound { self.s.inbound.clone() } else { self.s.outbound.clone() };
        let meet_i = self.s.tug_meet.min(path_pts.len().saturating_sub(1));
        let offset = |p: &[f64], h: f64| {
            let q = geo::destination(LatLon::new(p[0], p[1]), geo::wrap360(h + 90.0 * side), 45.0);
            [q.lat, q.lon]
        };
        let mut plan;
        let retrace: Vec<Wp>;
        if inbound {
            let meet = path_pts.get(meet_i)?;
            let t_meet = time_near(ship, LatLon::new(meet[0], meet[1]));
            let to = Path::new(fillet(&[wp2(base, 4.0), wp(meet, 9.0)], Turn::BOAT), status::UNDER_WAY);
            plan = Plan::new(t_meet - to.duration() - 120.0);
            plan.push(Leg::Move(to));
            plan.hold(120.0, status::UNDER_WAY, None);
            let mut follow: Vec<Wp> = Vec::new();
            for (i, p) in path_pts.iter().enumerate().skip(meet_i) {
                let h = if i + 1 < path_pts.len() {
                    geo::bearing_deg(LatLon::new(p[0], p[1]), LatLon::new(path_pts[i + 1][0], path_pts[i + 1][1]))
                } else {
                    0.0
                };
                follow.push(wp(&offset(p, h), p.get(2).copied().unwrap_or(10.0)));
            }
            if let Some(end) = ship.end() {
                let q = geo::destination(end.p, geo::wrap360(end.track_deg + 90.0 * side), 40.0);
                follow.push(Wp::new(q.lat, q.lon, 0.0, 1.0 * KT));
            }
            let home: Vec<Wp> = follow.iter().rev().map(|w| Wp { speed_mps: 8.0 * KT, ..*w }).collect();
            plan.path(follow, Some(Turn::BOAT), status::RESTRICTED);
            plan.hold(600.0 + rng.random_range(0.0..600.0), status::RESTRICTED, None);
            retrace = home;
        } else {
            let start = ship.state(ship.t0)?;
            let t_dep = ship_departure_time(ship);
            let at = geo::destination(start.p, geo::wrap360(start.track_deg + 90.0 * side), 40.0);
            let to = Path::new(fillet(&[wp2(base, 4.0), Wp::new(at.lat, at.lon, 0.0, 2.0 * KT)], Turn::BOAT), status::UNDER_WAY);
            plan = Plan::new(t_dep - to.duration() - 600.0);
            plan.push(Leg::Move(to));
            plan.hold(600.0, status::RESTRICTED, None);
            let follow: Vec<Wp> = path_pts.iter().take(meet_i.max(1) + 1).map(|p| wp(&offset(p, 0.0), p.get(2).copied().unwrap_or(6.0))).collect();
            let mut pts = vec![Wp::new(at.lat, at.lon, 0.0, 2.0 * KT)];
            pts.extend(follow);
            retrace = pts.iter().rev().map(|w| Wp { speed_mps: 8.0 * KT, ..*w }).collect();
            plan.path(pts, Some(Turn::BOAT), status::RESTRICTED);
        }
        // home the way it came, so a tug never cuts across land
        let here = plan.end()?.p;
        let mut home = vec![Wp::new(here.lat, here.lon, 0.0, 8.0 * KT)];
        home.extend(retrace);
        home.push(wp2(base, 2.0));
        plan.path(home, Some(Turn::BOAT), status::UNDER_WAY);
        plan.hold(3600.0, status::MOORED, None);
        let id = ident::vessel(rng, Class::Tug, true, None, None);
        let eq = ident::equip(rng, Class::Tug, &ctx.equip_cfg(Class::Tug));
        Some(Mover::new(&self.s.name, n, Class::Tug, plan, id, eq))
    }
}

/// When a ship moored at the start of its plan casts off.
fn ship_departure_time(plan: &Plan) -> f64 {
    let mut t = plan.t0;
    while t < plan.t1 {
        if plan.state(t).is_some_and(|k| !k.held) {
            return t;
        }
        t += 30.0;
    }
    plan.t0
}

impl Gen for Ships {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        while self.inbound.next <= until {
            let t = self.inbound.advance(&mut self.rng, ctx);
            if !t.is_finite() || t > ctx.t_end {
                break;
            }
            let class = pick_class(&mut self.rng, &self.s, Class::Cargo);
            let berth = self.next_berth();
            self.n += 1;
            let n = self.n;
            let Some((ship, t_station)) = inbound_ship(&self.s, &mut self.rng, ctx, t, n, berth, class, None) else { continue };
            self.n += 1;
            if let Some(l) = pilot_launch(&mut self.rng, ctx, &self.s.name, self.n, t_station) {
                out.push(l);
            }
            for k in 0..self.s.tugs {
                self.n += 1;
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                if let Some(tug) = self.tug(ctx, self.n, &ship.plan, side, true) {
                    out.push(tug);
                }
            }
            out.push(ship);
        }
        while self.outbound.next <= until {
            let t = self.outbound.advance(&mut self.rng, ctx);
            if !t.is_finite() || t > ctx.t_end {
                break;
            }
            let class = pick_class(&mut self.rng, &self.s, Class::Cargo);
            let Some((berth_p, berth_h)) = self.next_berth() else { continue };
            let Some(station) = ctx.cfg.ships.pilot_station else { break };
            let Some(a) = approach(&mut self.rng, ctx).cloned() else { break };
            let dwell = uniform(&mut self.rng, self.s.dwell_h, [8.0, 24.0]) * 3600.0;
            let t0 = (t - dwell).max(ctx.t_begin);
            let mut plan = Plan::new(t0);
            plan.hold(t - t0, status::MOORED, Some((berth_p, 0.0, berth_h, false)));
            let mut bay = vec![Wp::new(berth_p.lat, berth_p.lon, 0.0, 0.5 * KT)];
            bay.extend(self.s.outbound.iter().map(|p| wp(p, 10.0)));
            bay.extend(ctx.cfg.ships.bar_outbound.iter().map(|p| wp(p, 12.0)));
            bay.push(wp2(station, 8.0));
            let leg = Path::new(fillet(&bay, Turn::SHIP), status::UNDER_WAY);
            let t_station = t + leg.duration();
            plan.push(Leg::Move(leg));
            let sea_kt = ctx.cfg.ships.sea_kt.unwrap_or(14.0);
            let mut sea = vec![wp2(station, 8.0)];
            sea.extend(a.outbound.iter().map(|p| wp2(*p, sea_kt)));
            plan.path(sea, Some(Turn::SHIP), status::UNDER_WAY);
            self.n += 1;
            let n = self.n;
            let us = self.rng.random_bool(0.08);
            let mut id = ident::vessel(&mut self.rng, class, us, self.s.length_m, None);
            if class == Class::Tanker {
                // out in ballast
                id.draught_dm = (f64::from(id.draught_dm) * 0.6) as u8;
            }
            id.destination = Some(["USLGB", "CNSHA", "KRPUS", "JPTYO", "USSEA", "CAVAN", "USHNL", "SGSIN", "TWKHH"][self.rng.random_range(0..9)].into());
            let (mo, d, h, mi) = ctx.utc_parts(t + 9.0 * 86_400.0);
            id.eta = Some([mo, d, h, mi]);
            let eq = ident::equip(&mut self.rng, class, &ctx.equip_cfg(class));
            let ship = Mover::new(&self.s.name, n, class, plan, id, eq);
            self.n += 1;
            if let Some(l) = pilot_launch(&mut self.rng, ctx, &self.s.name, self.n, t_station) {
                out.push(l);
            }
            for k in 0..self.s.tugs {
                self.n += 1;
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                if let Some(tug) = self.tug(ctx, self.n, &ship.plan, side, false) {
                    out.push(tug);
                }
            }
            out.push(ship);
        }
    }
}

/// A ferry route's vessels shuttling between their stops all day.
pub struct Ferry {
    s: StreamCfg,
    done: bool,
}

impl Ferry {
    pub fn new(s: &StreamCfg, _ctx: &Ctx) -> Ferry {
        Ferry { s: s.clone(), done: false }
    }

    /// One way through the stops, in order or reversed: the paths and dwells.
    fn trip(&self, reverse: bool, kt: f64) -> Vec<(Path, f64)> {
        let n = self.s.stops.len();
        let mut out = Vec::new();
        for k in 0..n.saturating_sub(1) {
            let (i, j) = if reverse { (n - 1 - k, n - 2 - k) } else { (k, k + 1) };
            let (a, b) = (&self.s.stops[i], &self.s.stops[j]);
            let mut mid: Vec<[f64; 2]> = self.s.legs.get(i.min(j)).cloned().unwrap_or_default();
            if reverse {
                mid.reverse();
            }
            let pa = LatLon::new(a.lat, a.lon);
            let pb = LatLon::new(b.lat, b.lon);
            let first = mid.first().map_or(pb, |p| LatLon::new(p[0], p[1]));
            let last = mid.last().map_or(pa, |p| LatLon::new(p[0], p[1]));
            let out_p = geo::destination(pa, geo::bearing_deg(pa, first), 250.0);
            let in_p = geo::destination(pb, geo::bearing_deg(pb, last), 250.0);
            let mut w = vec![Wp::new(pa.lat, pa.lon, 0.0, 2.0 * KT), Wp::new(out_p.lat, out_p.lon, 0.0, 9.0 * KT)];
            w.extend(mid.iter().map(|p| wp2(*p, kt)));
            w.push(Wp::new(in_p.lat, in_p.lon, 0.0, 9.0 * KT));
            w.push(Wp::new(pb.lat, pb.lon, 0.0, 1.0 * KT));
            let dwell = b.dwell_min * 60.0;
            out.push((Path::new(fillet(&w, Turn::BOAT), status::UNDER_WAY), dwell));
        }
        out
    }
}

impl Gen for Ferry {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, _until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        if self.done || self.s.stops.len() < 2 {
            return;
        }
        self.done = true;
        let mut rng = rng::chacha(ctx.seed, &[rng::tag(&self.s.name)]);
        let kt = uniform(&mut rng, self.s.speed_kt, [20.0, 20.0]);
        let headway = (self.s.headway_min.max(5.0) / ctx.scn.rate(&self.s.name).max(0.1)) * 60.0;
        let [h0, h1] = self.s.active_local_h.unwrap_or([7.0, 21.0]);
        let fwd = self.trip(false, kt);
        let back = self.trip(true, kt);
        let cycle: f64 = fwd.iter().chain(back.iter()).map(|(p, d)| p.duration() + d).sum::<f64>() + 300.0;
        let vessels = if self.s.vessels > 0 { self.s.vessels } else { (cycle / headway).ceil().max(1.0) as usize };
        // departures from the first stop: every headway within the local hours, every day
        let day0 = (ctx.t_begin - ctx.local_h(ctx.t_begin) * 3600.0).floor();
        let mut deps = Vec::new();
        let mut day = day0;
        while day < ctx.t_end + 86_400.0 {
            let mut t = day + h0 * 3600.0;
            while t < day + h1 * 3600.0 {
                if t >= ctx.t_begin && t <= ctx.t_end {
                    deps.push(t);
                }
                t += headway;
            }
            day += 86_400.0;
        }
        let home = &self.s.stops[0];
        let home_p = LatLon::new(home.lat, home.lon);
        for j in 0..vessels {
            let mut plan = Plan::new(ctx.t_begin);
            let mut here_t = ctx.t_begin;
            let heading = fwd.first().map_or(0.0, |(p, _)| p.points().get(1).map_or(0.0, |w| geo::bearing_deg(home_p, w.p)));
            for (i, d) in deps.iter().enumerate() {
                if i % vessels != j || *d < here_t {
                    continue;
                }
                let idle = *d - here_t;
                let st = if idle > 3.0 * 3600.0 { status::MOORED } else { status::UNDER_WAY };
                plan.hold(idle, st, Some((home_p, 0.0, geo::wrap360(heading + 180.0), false)));
                for (p, dwell) in fwd.iter().chain(back.iter()) {
                    plan.push(Leg::Move(p.clone()));
                    plan.hold(*dwell, status::UNDER_WAY, None);
                }
                here_t = plan.t1;
            }
            plan.hold((ctx.t_end - plan.t1).max(0.0) + 600.0, status::MOORED, Some((home_p, 0.0, geo::wrap360(heading + 180.0), false)));
            let name = self.s.names.get(j).map(String::as_str);
            let mut id = ident::vessel(&mut rng, Class::Ferry, true, self.s.length_m, name);
            id.destination = self.s.destination.clone();
            let eq = ident::equip(&mut rng, Class::Ferry, &ctx.equip_cfg(Class::Ferry));
            out.push(Mover::new(&self.s.name, j as u64 + 1, Class::Ferry, plan, id, eq));
        }
    }
}

/// Vessels that stay put: the cruise ship at Pier 27, ships at anchor, idle tugs.
pub struct Moored {
    s: StreamCfg,
    done: bool,
}

impl Moored {
    pub fn new(s: &StreamCfg, _ctx: &Ctx) -> Moored {
        Moored { s: s.clone(), done: false }
    }
}

impl Gen for Moored {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, _until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        if self.done {
            return;
        }
        self.done = true;
        let mut rng = rng::chacha(ctx.seed, &[rng::tag(&self.s.name)]);
        let st = status_of(self.s.status.as_deref());
        let n = if self.s.count > 0 { ((self.s.count as f64) * ctx.scn.rate(&self.s.name)).round() as usize } else { self.s.positions.len() };
        for i in 0..n {
            let (p, h) = match self.s.positions.get(i) {
                Some(v) if v.len() >= 2 => (LatLon::new(v[0], v[1]), v.get(2).copied().unwrap_or(0.0)),
                _ => match random_in(&mut rng, &self.s.area) {
                    Some(p) => (p, rng.random_range(0.0..360.0)),
                    None => continue,
                },
            };
            let class = pick_class(&mut rng, &self.s, Class::Cargo);
            let mut plan = Plan::new(ctx.t_begin);
            plan.hold(ctx.t_end - ctx.t_begin + 3600.0, st, Some((p, 0.0, h, false)));
            let name = self.s.names.get(i).map(String::as_str);
            let mut id = ident::vessel(&mut rng, class, matches!(class, Class::Tug | Class::Ferry), self.s.length_m, name);
            id.destination = self.s.destination.clone();
            let eq = ident::equip(&mut rng, class, &ctx.equip_cfg(class));
            out.push(Mover::new(&self.s.name, i as u64 + 1, class, plan, id, eq));
        }
    }
}

/// A point inside a polygon, by rejection.
pub fn random_in(rng: &mut ChaCha8Rng, area: &[[f64; 2]]) -> Option<LatLon> {
    if area.len() < 3 {
        return None;
    }
    let poly: Vec<LatLon> = area.iter().map(|p| LatLon::new(p[0], p[1])).collect();
    let (la0, la1) = poly.iter().fold((90.0f64, -90.0f64), |(a, b), p| (a.min(p.lat), b.max(p.lat)));
    let (lo0, lo1) = poly.iter().fold((180.0f64, -180.0f64), |(a, b), p| (a.min(p.lon), b.max(p.lon)));
    for _ in 0..200 {
        let p = LatLon::new(rng.random_range(la0..la1), rng.random_range(lo0..lo1));
        if geo::in_polygon(p, &poly) {
            return Some(p);
        }
    }
    None
}

/// Whether the segment `a`-`b` stays out of every avoided circle.
fn clear_of(a: LatLon, b: LatLon, avoid: &[[f64; 3]]) -> bool {
    avoid.iter().all(|c| {
        let o = LatLon::new(c[0], c[1]);
        let (ax, ay) = geo::enu(o, a);
        let (bx, by) = geo::enu(o, b);
        let (dx, dy) = (bx - ax, by - ay);
        let l2 = dx * dx + dy * dy;
        let f = if l2 > 0.0 { (-(ax * dx + ay * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let (px, py) = (ax + f * dx, ay + f * dy);
        (px * px + py * py).sqrt() > c[2]
    })
}

/// Points that take a straight leg `a`-`b` round the avoided circles it would cross: for each,
/// the leg's nearest approach pushed out to 1.4 times the radius.
fn detour(a: LatLon, b: LatLon, avoid: &[[f64; 3]]) -> Vec<LatLon> {
    let mut out = Vec::new();
    for c in avoid {
        if clear_of(a, b, std::slice::from_ref(c)) {
            continue;
        }
        let o = LatLon::new(c[0], c[1]);
        let (ax, ay) = geo::enu(o, a);
        let (bx, by) = geo::enu(o, b);
        let (dx, dy) = (bx - ax, by - ay);
        let l2 = (dx * dx + dy * dy).max(1e-9);
        let f = (-(ax * dx + ay * dy) / l2).clamp(0.0, 1.0);
        let (mut px, mut py) = (ax + f * dx, ay + f * dy);
        let n = (px * px + py * py).sqrt();
        if n < 1.0 {
            // straight through the middle: go round to the left of the leg
            let l = l2.sqrt();
            (px, py) = (-dy / l, dx / l);
        } else {
            (px, py) = (px / n, py / n);
        }
        out.push(geo::from_enu(o, px * c[2] * 1.4, py * c[2] * 1.4));
    }
    out
}

/// A point in the stream's area reachable from `here` in a straight line clear of its islands.
fn clear_point(rng: &mut ChaCha8Rng, s: &StreamCfg, here: LatLon) -> Option<LatLon> {
    for _ in 0..30 {
        let p = random_in(rng, &s.area)?;
        if clear_of(here, p, &s.avoid) {
            return Some(p);
        }
    }
    None
}

/// One small-craft outing: from a spot, legs between random points in the area, back.
pub fn wander_object(s: &StreamCfg, rng: &mut ChaCha8Rng, ctx: &Ctx, t0: f64, n: u64, class: Class) -> Option<Mover> {
    let spot = pick_weighted(rng, &s.spots, |p| p.weight)?.clone();
    let start = LatLon::new(spot.lat, spot.lon);
    let via: Vec<LatLon> = spot.via.iter().map(|p| LatLon::new(p[0], p[1])).collect();
    let minutes = uniform(rng, s.session_min, [60.0, 180.0]);
    let st = status_of(s.status.as_deref());
    let mut plan = Plan::new(t0);
    let mut here = start;
    let end = t0 + minutes * 60.0;
    let mut first = true;
    let mut guard = 0;
    while (plan.t1 < end || first) && guard < 60 {
        guard += 1;
        let from = if first { via.last().copied().unwrap_or(here) } else { here };
        let Some(to) = clear_point(rng, s, from) else { break };
        let kt = uniform(rng, s.speed_kt, [4.0, 6.0]);
        let from_kt = if first { 2.0 } else { kt };
        let mut leg = vec![Wp::new(here.lat, here.lon, 0.0, from_kt * KT)];
        if first {
            leg.extend(via.iter().map(|p| Wp::new(p.lat, p.lon, 0.0, kt * KT)));
        }
        leg.push(Wp::new(to.lat, to.lon, 0.0, kt * KT));
        plan.path(leg, None, st);
        here = to;
        first = false;
        if class == Class::Fishing && rng.random_bool(0.5) {
            // drifting on a spot
            plan.hold(rng.random_range(300.0..1500.0), status::FISHING, None);
        } else if class == Class::SmallCraft && rng.random_bool(0.3) {
            plan.hold(rng.random_range(60.0..600.0), status::UNDEFINED, None);
        }
    }
    let kt = uniform(rng, s.speed_kt, [4.0, 6.0]);
    // home: round any island in the way, then in through the marina's channel
    let entry = via.last().copied().unwrap_or(start);
    let mut leg = vec![Wp::new(here.lat, here.lon, 0.0, kt * KT)];
    leg.extend(detour(here, entry, &s.avoid).into_iter().map(|p| Wp::new(p.lat, p.lon, 0.0, kt * KT)));
    leg.extend(via.iter().rev().map(|p| Wp::new(p.lat, p.lon, 0.0, kt * KT)));
    leg.push(Wp::new(start.lat, start.lon, 0.0, 1.0 * KT));
    plan.path(leg, None, st);
    let mut id = ident::vessel(rng, class, true, s.length_m, None);
    id.destination = s.destination.clone();
    let eq = ident::equip(rng, class, &ctx.equip_cfg(class));
    let mut m = Mover::new(&s.name, n, class, plan, id, eq);
    m.wander_m = s.wander_m;
    Some(m)
}

pub struct Wander {
    s: StreamCfg,
    rng: ChaCha8Rng,
    p: Poisson,
    n: u64,
}

impl Wander {
    pub fn new(s: &StreamCfg, ctx: &Ctx) -> Wander {
        Wander {
            s: s.clone(),
            rng: rng::chacha(ctx.seed, &[rng::tag(&s.name)]),
            p: Poisson::new(s.per_hour * ctx.scn.rate(&s.name), s.active_local_h, ctx.t_begin),
            n: 0,
        }
    }
}

impl Gen for Wander {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        while self.p.next <= until {
            let t = self.p.advance(&mut self.rng, ctx);
            if !t.is_finite() || t > ctx.t_end {
                break;
            }
            let class = pick_class(&mut self.rng, &self.s, Class::Sailing);
            self.n += 1;
            if let Some(m) = wander_object(&self.s, &mut self.rng, ctx, t, self.n, class) {
                out.push(m);
            }
        }
    }
}

/// The pilot station boat, holding near the boarding area all day.
pub struct PilotStation {
    s: StreamCfg,
    done: bool,
}

impl PilotStation {
    pub fn new(s: &StreamCfg, _ctx: &Ctx) -> PilotStation {
        PilotStation { s: s.clone(), done: false }
    }
}

impl Gen for PilotStation {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, _until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        if self.done {
            return;
        }
        self.done = true;
        let Some(station) = ctx.cfg.ships.pilot_station else { return };
        let mut rng = rng::chacha(ctx.seed, &[rng::tag(&self.s.name)]);
        let c = LatLon::new(station[0], station[1]);
        let r = if self.s.wander_m > 0.0 { self.s.wander_m } else { 1_500.0 };
        let mut plan = Plan::new(ctx.t_begin);
        let mut here = c;
        while plan.t1 < ctx.t_end + 600.0 {
            let to = geo::destination(c, rng.random_range(0.0..360.0), rng.random_range(0.0..r));
            let kt = uniform(&mut rng, self.s.speed_kt, [2.0, 4.0]);
            plan.path(vec![Wp::new(here.lat, here.lon, 0.0, kt * KT), Wp::new(to.lat, to.lon, 0.0, kt * KT)], None, status::UNDER_WAY);
            here = to;
        }
        let name = self.s.names.first().map(String::as_str);
        let id = ident::vessel(&mut rng, Class::Pilot, true, Some([50.0, 60.0]), name);
        let eq = ident::equip(&mut rng, Class::Pilot, &ctx.equip_cfg(Class::Pilot));
        out.push(Mover::new(&self.s.name, 1, Class::Pilot, plan, id, eq));
    }
}
