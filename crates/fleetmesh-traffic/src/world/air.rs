//! Aircraft: SFO and OAK arrivals and departures on their runways, overflights, route streams
//! (helicopter and fixed-wing tours, seaplanes, the Coast Guard, medical flights, coastal VFR
//! transits) and other operators' drones.

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::ident;
use super::{Class, Ctx, Gen, Ident, Mover, Poisson, pick_class, pick_route, pick_weighted, route_points, uniform};
use crate::config::{AirportCfg, RunwayCfg, StreamCfg};
use crate::geo::{self, FT, KT, LatLon, NM};
use crate::path::{Leg, Path, Plan, Turn, Wp, fillet, status};
use crate::rng;

/// Speeds on final and the take-off roll, scaled by class (an airliner's 140 kt threshold
/// speed, a light aircraft's 70).
fn speed_scale(class: Class) -> f64 {
    match class {
        Class::Airliner => 1.0,
        Class::Bizjet => 0.92,
        _ => 0.52,
    }
}

fn turn_for(class: Class) -> Turn {
    match class {
        Class::Airliner | Class::Bizjet => Turn::AIRLINER,
        Class::Cargo | Class::Tanker | Class::Passenger => Turn::SHIP,
        Class::Ferry | Class::Tug | Class::Pilot | Class::Fishing => Turn::BOAT,
        Class::Sailing | Class::Pleasure | Class::SmallCraft => Turn::SMALL,
        _ => Turn::LIGHT,
    }
}

/// An aircraft's identity: callsign by the stream's style, an ICAO address, a squawk.
pub fn aircraft_ident(rng: &mut ChaCha8Rng, class: Class, style: &str, airport: Option<&AirportCfg>, squawk: Option<u16>, mil: Option<[u32; 2]>) -> Ident {
    let (callsign, icao) = match style {
        "none" => (None, ident::icao_us(rng)),
        "airline" if class == Class::Airliner => {
            let prefix = airport
                .and_then(|a| pick_weighted(rng, &a.airlines, |(_, w)| *w).map(|(p, _)| p.clone()))
                .unwrap_or_else(|| "UAL".to_string());
            let icao = ident::icao_for_airline(rng, &prefix);
            (Some(ident::flight_number(rng, &prefix)), icao)
        }
        "airline" if class == Class::Bizjet && rng.random_bool(0.3) => {
            let p = ["EJA", "LXJ", "XOJ", "JTZ"][rng.random_range(0..4)];
            (Some(ident::flight_number(rng, p)), ident::icao_us(rng))
        }
        s if s.starts_with("prefix:") => {
            let p = &s["prefix:".len()..];
            let n: u32 = rng.random_range(0..100);
            let icao = match mil {
                Some(b) => ident::icao_in(rng, b),
                None => ident::icao_us(rng),
            };
            (Some(format!("{p}{n:02}")), icao)
        }
        _ => (Some(ident::n_number(rng)), ident::icao_us(rng)),
    };
    Ident {
        icao24: Some(icao),
        callsign,
        squawk: Some(squawk.unwrap_or_else(|| ident::discrete_squawk(rng))),
        ..Default::default()
    }
}

/// Arrivals or departures at one airport, on its runways in use.
pub struct AirportFlow {
    s: StreamCfg,
    airport: AirportCfg,
    runways: Vec<RunwayCfg>,
    rng: ChaCha8Rng,
    next: f64,
    last_on: Vec<f64>,
    n: u64,
    mult: f64,
}

impl AirportFlow {
    pub fn new(s: &StreamCfg, ctx: &Ctx) -> Result<AirportFlow, String> {
        let id = s.airport.clone().unwrap_or_default();
        let airport = ctx
            .cfg
            .airport
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| format!("stream {}: no airport {id:?}", s.name))?;
        let runways: Vec<RunwayCfg> = s
            .runways
            .iter()
            .map(|r| {
                airport.runway.iter().find(|x| &x.id == r).cloned().ok_or_else(|| format!("stream {}: no runway {r} at {id}", s.name))
            })
            .collect::<Result<_, _>>()?;
        if runways.is_empty() {
            return Err(format!("stream {}: no runways", s.name));
        }
        let n_rwy = runways.len();
        Ok(AirportFlow {
            s: s.clone(),
            airport,
            runways,
            rng: rng::chacha(ctx.seed, &[rng::tag(&s.name)]),
            next: ctx.t_begin,
            last_on: vec![f64::NEG_INFINITY; n_rwy],
            n: 0,
            mult: ctx.scn.rate(&s.name),
        })
    }

    fn arrival(&mut self, t_thr: f64, class: Class, rwy: &RunwayCfg) -> Option<Plan> {
        let route = pick_route(&mut self.rng, &self.s.routes)?.clone();
        let (mut wps, _) = route_points(&route.points, self.s.jitter_m, &mut self.rng, 3000.0, 250.0);
        let thr = LatLon::new(rwy.lat, rwy.lon);
        let back = geo::wrap360(rwy.heading_deg + 180.0);
        let elev = self.airport.elevation_m;
        let k = speed_scale(class);
        let final_nm = if self.s.final_nm > 0.0 { self.s.final_nm } else { 10.0 };
        let fin = |d_nm: f64, kt: f64| {
            let p = geo::destination(thr, back, d_nm * NM);
            Wp::new(p.lat, p.lon, elev + 15.0 + d_nm * NM * 3f64.to_radians().tan(), kt * KT * k)
        };
        if class != Class::Airliner && class != Class::Bizjet {
            // light aircraft join a shorter final
            wps.push(fin(final_nm.min(4.0), 95.0));
        } else {
            wps.push(fin(final_nm, 180.0));
            wps.push(fin(final_nm * 0.5, 165.0));
        }
        wps.push(fin(2.0, 145.0));
        wps.push(fin(0.0, 140.0));
        // scheduled by threshold time: the approach starts its own duration earlier
        let approach = Path::new(fillet(&wps, turn_for(class)), status::UNDER_WAY);
        let mut plan = Plan::new(t_thr - approach.duration());
        plan.push(Leg::Move(approach));
        let td = geo::destination(thr, rwy.heading_deg, 350.0);
        let slow = geo::destination(thr, rwy.heading_deg, rwy.length_m * 0.55);
        let exit = geo::destination(thr, rwy.heading_deg, rwy.length_m * 0.62);
        plan.path(
            vec![
                Wp::new(thr.lat, thr.lon, elev + 15.0, 140.0 * KT * k),
                Wp::new(td.lat, td.lon, elev, 135.0 * KT * k).ground(),
                Wp::new(slow.lat, slow.lon, elev, 30.0 * KT).ground(),
                Wp::new(exit.lat, exit.lon, elev, 15.0 * KT).ground(),
            ],
            None,
            status::UNDER_WAY,
        );
        Some(plan)
    }

    fn departure(&mut self, t_roll: f64, class: Class, rwy: &RunwayCfg) -> Option<Plan> {
        let route = pick_route(&mut self.rng, &self.s.routes)?.clone();
        let (route_wps, _) = route_points(&route.points, self.s.jitter_m, &mut self.rng, 5000.0, 250.0);
        let start = LatLon::new(rwy.lat, rwy.lon);
        let elev = self.airport.elevation_m;
        let k = speed_scale(class);
        let (roll_m, climb) = if k >= 0.9 { (1_900.0, 0.10) } else { (450.0, 0.09) };
        let lift = geo::destination(start, rwy.heading_deg, roll_m);
        let out = if self.s.final_nm > 0.0 { self.s.final_nm } else { 3.0 };
        let p2 = geo::destination(lift, rwy.heading_deg, out * NM);
        let mut wps = vec![
            Wp::new(start.lat, start.lon, elev, 0.0).ground(),
            Wp::new(lift.lat, lift.lon, elev, 150.0 * KT * k).ground(),
            Wp::new(lift.lat, lift.lon, elev + 1.0, 155.0 * KT * k),
            Wp::new(p2.lat, p2.lon, elev + out * NM * climb, 200.0 * KT * k),
        ];
        wps.extend(route_wps);
        let mut plan = Plan::new(t_roll);
        let (ground, air) = wps.split_at(2);
        plan.path(ground.to_vec(), None, status::UNDER_WAY);
        plan.path(air.to_vec(), Some(turn_for(class)), status::UNDER_WAY);
        Some(plan)
    }
}

impl Gen for AirportFlow {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        let per_hour = self.s.per_hour * self.mult;
        if per_hour <= 0.0 {
            return;
        }
        let mean = 3600.0 / per_hour;
        let gap = self.s.min_gap_s.max(0.0).min(mean * 0.9);
        let arrivals = self.s.kind == "arrivals";
        // arrivals are scheduled by threshold time: look an hour further for their approach
        let horizon = if arrivals { until + 3600.0 } else { until };
        while self.next <= horizon && self.next <= ctx.t_end + 3600.0 {
            let u: f64 = self.rng.random::<f64>().max(1e-12);
            let t = self.next + gap + -(u.ln()) * (mean - gap);
            self.next = t;
            // the runway free longest, staggering pairs
            let ri = (0..self.runways.len())
                .min_by(|a, b| self.last_on[*a].total_cmp(&self.last_on[*b]))
                .unwrap_or(0);
            self.last_on[ri] = t;
            let rwy = self.runways[ri].clone();
            let class = pick_class(&mut self.rng, &self.s, Class::Airliner);
            let plan = if arrivals { self.arrival(t, class, &rwy) } else { self.departure(t, class, &rwy) };
            let Some(plan) = plan else { continue };
            let style = self.s.callsign.clone().unwrap_or_else(|| "airline".into());
            let style = if class == Class::Ga { "n_number".to_string() } else { style };
            let mut id = aircraft_ident(&mut self.rng, class, &style, Some(&self.airport), self.s.squawk, None);
            let eq = ident::equip(&mut self.rng, class, &ctx.equip_cfg(class));
            ident::apply_privacy(&mut self.rng, &ctx.cfg.addresses, &mut id, &eq);
            self.n += 1;
            out.push(Mover::new(&self.s.name, self.n, class, plan, id, eq));
        }
    }
}

/// High-level traffic crossing the region.
pub struct Overflights {
    s: StreamCfg,
    rng: ChaCha8Rng,
    p: Poisson,
    n: u64,
}

impl Overflights {
    pub fn new(s: &StreamCfg, ctx: &Ctx) -> Overflights {
        Overflights {
            s: s.clone(),
            rng: rng::chacha(ctx.seed, &[rng::tag(&s.name)]),
            p: Poisson::new(s.per_hour * ctx.scn.rate(&s.name), s.active_local_h, ctx.t_begin),
            n: 0,
        }
    }

    fn edge_point(rng: &mut ChaCha8Rng, ctx: &Ctx, side: usize) -> LatLon {
        let w = &ctx.cfg.world;
        let f: f64 = rng.random();
        match side % 4 {
            0 => LatLon::new(w.lat_max, w.lon_min + f * (w.lon_max - w.lon_min)),
            1 => LatLon::new(w.lat_min + f * (w.lat_max - w.lat_min), w.lon_max),
            2 => LatLon::new(w.lat_min, w.lon_min + f * (w.lon_max - w.lon_min)),
            _ => LatLon::new(w.lat_min + f * (w.lat_max - w.lat_min), w.lon_min),
        }
    }
}

impl Gen for Overflights {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        while self.p.next <= until {
            let t = self.p.advance(&mut self.rng, ctx);
            if !t.is_finite() || t > ctx.t_end {
                break;
            }
            let side = self.rng.random_range(0..4usize);
            let a = Self::edge_point(&mut self.rng, ctx, side);
            let other = side + self.rng.random_range(1..4usize);
            let b = Self::edge_point(&mut self.rng, ctx, other);
            let alt = (uniform(&mut self.rng, self.s.alt_ft, [25_000.0, 39_000.0]) / 1000.0).round() * 1000.0 * FT;
            let v = uniform(&mut self.rng, self.s.speed_kt, [420.0, 490.0]) * KT;
            let class = pick_class(&mut self.rng, &self.s, Class::Airliner);
            let mut plan = Plan::new(t);
            plan.path(vec![Wp::new(a.lat, a.lon, alt, v), Wp::new(b.lat, b.lon, alt, v)], None, status::UNDER_WAY);
            let mut id = aircraft_ident(&mut self.rng, class, "airline", ctx.cfg.airport.first(), None, None);
            let eq = ident::equip(&mut self.rng, class, &ctx.equip_cfg(class));
            ident::apply_privacy(&mut self.rng, &ctx.cfg.addresses, &mut id, &eq);
            self.n += 1;
            out.push(Mover::new(&self.s.name, self.n, class, plan, id, eq));
        }
    }
}

/// One object along a stream's route (tours, seaplanes, patrols, transits, fishing boats
/// coming home), with its holds.
pub fn route_object(s: &StreamCfg, rng: &mut ChaCha8Rng, ctx: &Ctx, t0: f64, n: u64, class: Class) -> Option<Mover> {
    let route = pick_route(rng, &s.routes)?.clone();
    let mut pts = route.points.clone();
    if s.bidirectional && rng.random_bool(0.5) {
        pts.reverse();
    }
    let kt = uniform(rng, s.speed_kt, [100.0, 100.0]);
    let alt = if class.air() { 300.0 } else { 0.0 };
    let (wps, holds) = route_points(&pts, s.jitter_m, rng, alt, kt);
    if wps.len() < 2 {
        return None;
    }
    let mut plan = Plan::new(t0);
    let move_status = status::UNDER_WAY;
    let mut from = 0;
    for (i, hold) in holds.iter().chain(std::iter::once(&(wps.len() - 1, 0.0))) {
        let i = *i;
        if i > from {
            plan.path(wps[from..=i].to_vec(), Some(turn_for(class)), move_status);
        }
        if *hold > 0.0 {
            let w = wps[i];
            plan.hold(*hold, if class.air() { status::UNDER_WAY } else { status::MOORED }, Some((w.p, w.alt_m, plan_heading(&plan), w.ground)));
        }
        from = i;
    }
    let ident = if class.air() {
        let style = s.callsign.clone().unwrap_or_else(|| "n_number".into());
        let mil = style.starts_with("prefix:C").then_some(ctx.cfg.addresses.military_block);
        aircraft_ident(rng, class, &style, None, s.squawk, mil)
    } else {
        let mut id = ident::vessel(rng, class, true, s.length_m, None);
        if let Some(d) = &s.destination {
            id.destination = Some(d.clone());
        }
        id
    };
    let eq = ident::equip(rng, class, &ctx.equip_cfg(class));
    let mut ident = ident;
    if class.air() {
        ident::apply_privacy(rng, &ctx.cfg.addresses, &mut ident, &eq);
    }
    Some(Mover::new(&s.name, n, class, plan, ident, eq))
}

fn plan_heading(plan: &Plan) -> f64 {
    plan.end().map_or(0.0, |k| k.track_deg)
}

pub struct RouteStream {
    s: StreamCfg,
    rng: ChaCha8Rng,
    p: Poisson,
    n: u64,
}

impl RouteStream {
    pub fn new(s: &StreamCfg, ctx: &Ctx) -> RouteStream {
        RouteStream {
            s: s.clone(),
            rng: rng::chacha(ctx.seed, &[rng::tag(&s.name)]),
            p: Poisson::new(s.per_hour * ctx.scn.rate(&s.name), s.active_local_h, ctx.t_begin),
            n: 0,
        }
    }
}

impl Gen for RouteStream {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        while self.p.next <= until {
            let t = self.p.advance(&mut self.rng, ctx);
            if !t.is_finite() || t > ctx.t_end {
                break;
            }
            let class = pick_class(&mut self.rng, &self.s, Class::Ga);
            self.n += 1;
            if let Some(m) = route_object(&self.s, &mut self.rng, ctx, t, self.n, class) {
                out.push(m);
            }
        }
    }
}

/// One drone flight from a spot: take off, a hover, an orbit or a survey pattern, back, land.
#[allow(clippy::too_many_arguments)]
pub fn drone_object(rng: &mut ChaCha8Rng, ctx: &Ctx, stream: &str, n: u64, t0: f64, spot: LatLon, ground_m: f64, agl: f64, minutes: f64, speed: f64, rid_share: f64) -> Mover {
    let alt = ground_m + agl;
    let climb = 3.0;
    let mut plan = Plan::new(t0);
    let base = Wp::new(spot.lat, spot.lon, ground_m, 0.0).ground();
    plan.hold(rng.random_range(10.0..40.0), status::UNDER_WAY, Some((spot, ground_m, 0.0, true)));
    plan.path(vec![Wp { speed_mps: climb, ..base }, Wp::new(spot.lat, spot.lon, alt, climb)], None, status::UNDER_WAY);
    let end_t = t0 + minutes * 60.0;
    let mut here = spot;
    let mut guard = 0;
    while plan.t1 < end_t - 90.0 && guard < 40 {
        guard += 1;
        let d = rng.random_range(40.0..450.0);
        let b = rng.random_range(0.0..360.0);
        let to = geo::destination(spot, b, d);
        let leg_alt = alt + rng.random_range(-10.0..10.0);
        plan.path(vec![Wp::new(here.lat, here.lon, plan.end().map_or(alt, |k| k.alt_m), speed), Wp::new(to.lat, to.lon, leg_alt, speed)], None, status::UNDER_WAY);
        here = to;
        match rng.random_range(0..3) {
            0 => {
                plan.hold(rng.random_range(30.0..180.0), status::UNDER_WAY, None);
            }
            1 => {
                // an orbit around a point of interest
                let r = rng.random_range(25.0..80.0);
                let c = geo::destination(here, b, r);
                let mut ring = Vec::new();
                let laps = rng.random_range(1..3);
                for j in 0..=(12 * laps) {
                    let a = geo::wrap360(b + 180.0 + j as f64 * 30.0);
                    let p = geo::destination(c, a, r);
                    ring.push(Wp::new(p.lat, p.lon, leg_alt, speed * 0.6));
                }
                plan.path(ring, None, status::UNDER_WAY);
                here = plan.end().map_or(here, |k| k.p);
            }
            _ => {}
        }
    }
    let cur_alt = plan.end().map_or(alt, |k| k.alt_m);
    plan.path(vec![Wp::new(here.lat, here.lon, cur_alt, speed), Wp::new(spot.lat, spot.lon, alt, speed)], None, status::UNDER_WAY);
    plan.path(vec![Wp::new(spot.lat, spot.lon, alt, 2.0), Wp { speed_mps: 2.0, ..base }], None, status::UNDER_WAY);
    plan.hold(rng.random_range(10.0..30.0), status::UNDER_WAY, Some((spot, ground_m, 0.0, true)));
    let op = geo::destination(spot, rng.random_range(0.0..360.0), rng.random_range(3.0..25.0));
    let id = Ident {
        uas_id: Some(ident::uas_serial(rng)),
        ua_type: if rng.random_bool(0.95) { "multirotor" } else { "vtol" },
        op: Some(op),
        ground_m,
        ..Default::default()
    };
    let mut eq = ident::equip(rng, Class::Drone, &ctx.equip_cfg(Class::Drone));
    if rid_share >= 0.0 {
        eq.rid = (rng.random::<f64>() < rid_share).then_some(eq.rid.unwrap_or(super::RidLink::Bt5));
    }
    Mover::new(stream, n, Class::Drone, plan, id, eq)
}

/// Other operators' drones: hobby flights, photography, inspection.
pub struct Drones {
    s: StreamCfg,
    rng: ChaCha8Rng,
    p: Poisson,
    n: u64,
}

impl Drones {
    pub fn new(s: &StreamCfg, ctx: &Ctx) -> Drones {
        Drones {
            s: s.clone(),
            rng: rng::chacha(ctx.seed, &[rng::tag(&s.name)]),
            p: Poisson::new(s.per_hour * ctx.scn.rate(&s.name), s.active_local_h, ctx.t_begin),
            n: 0,
        }
    }
}

impl Gen for Drones {
    fn name(&self) -> &str {
        &self.s.name
    }

    fn fill(&mut self, until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        while self.p.next <= until {
            let t = self.p.advance(&mut self.rng, ctx);
            if !t.is_finite() || t > ctx.t_end {
                break;
            }
            let Some(spot) = pick_weighted(&mut self.rng, &self.s.spots, |s| s.weight).cloned() else { break };
            let agl = uniform(&mut self.rng, self.s.agl_m, [30.0, 120.0]);
            let minutes = uniform(&mut self.rng, self.s.session_min, [6.0, 20.0]);
            let speed = uniform(&mut self.rng, self.s.speed_kt.map(|[a, b]| [a * KT, b * KT]), [3.0, 12.0]);
            let p = geo::destination(LatLon::new(spot.lat, spot.lon), self.rng.random_range(0.0..360.0), self.rng.random_range(0.0..120.0));
            self.n += 1;
            out.push(drone_object(&mut self.rng, ctx, &self.s.name, self.n, t, p, spot.ground_m, agl, minutes, speed, -1.0));
        }
    }
}
