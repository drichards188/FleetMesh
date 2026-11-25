//! Scripted issue events from a scenario's traffic.toml: the objects they add to the world (a
//! drone without Remote ID near a hub, a ship arriving at a given time, an aircraft whose
//! ADS-B position is wrong, an aircraft of a stream's kind flying a given route at a given time,
//! a vessel that switches its AIS off). Receiver outages are the
//! sensors' (crate::sensors).

use rand::Rng;

use super::{Adsb, Ais, Class, Ctx, Fault, Gen, Mover, air, sea};
use crate::geo::LatLon;
use crate::rng;

pub struct Issues {
    done: bool,
}

impl Issues {
    pub fn new(ctx: &Ctx) -> Result<Issues, String> {
        for (i, is) in ctx.scn.issue.iter().enumerate() {
            match is.kind.as_str() {
                "drone_no_rid" | "receiver_down" => {}
                "ship_arrival" | "adsb_bad_position" | "ais_off" | "flight" => {
                    if ctx.cfg.stream(&is.stream).is_none() {
                        return Err(format!("issue {i} ({}): no stream {:?}", is.kind, is.stream));
                    }
                    if is.kind == "flight" && is.route.len() < 2 {
                        return Err(format!("issue {i} (flight): a route of at least two points"));
                    }
                }
                other => return Err(format!("issue {i}: unknown kind {other:?}")),
            }
        }
        Ok(Issues { done: false })
    }
}

impl Gen for Issues {
    fn name(&self) -> &str {
        "issue"
    }

    fn fill(&mut self, _until: f64, ctx: &Ctx, out: &mut Vec<Mover>) {
        if self.done {
            return;
        }
        self.done = true;
        for (i, is) in ctx.scn.issue.iter().enumerate() {
            let n = i as u64 + 1;
            let mut rng = rng::chacha(ctx.seed, &[rng::tag("issue"), n]);
            let stream = format!("issue-{}", is.kind.replace('_', "-"));
            match is.kind.as_str() {
                "drone_no_rid" => {
                    let minutes = if is.duration_s > 0.0 { is.duration_s / 60.0 } else { 12.0 };
                    let agl = if is.agl_m > 0.0 { is.agl_m } else { 60.0 };
                    let m = air::drone_object(&mut rng, ctx, &stream, n, is.at_s, LatLon::new(is.lat, is.lon), is.ground_m, agl, minutes, 6.0, 0.0);
                    out.push(m);
                }
                "ship_arrival" => {
                    let Some(s) = ctx.cfg.stream(&is.stream) else { continue };
                    let class = Class::parse(&is.class).unwrap_or(Class::Cargo);
                    let name = (!is.vessel_name.is_empty()).then_some(is.vessel_name.as_str());
                    let berth = s.berths.first().map(|b| (LatLon::new(b[0], b[1]), b.get(2).copied().unwrap_or(0.0)));
                    // `at_s` is when the ship passes under the Golden Gate Bridge (the end of
                    // the bar channel): time a probe built from the same draws
                    let Some((probe, _)) = sea::inbound_ship(s, &mut rng.clone(), ctx, 0.0, n, berth, class, name) else { continue };
                    let gate = ctx.cfg.ships.bar_inbound.last().map_or(LatLon::new(37.8130, -122.4790), |p| LatLon::new(p[0], p[1]));
                    let t_gate = sea::time_near(&probe.plan, gate);
                    let Some((m, t_station)) = sea::inbound_ship(s, &mut rng, ctx, is.at_s - t_gate, n, berth, class, name) else { continue };
                    if let Some(l) = sea::pilot_launch(&mut rng, ctx, &stream, n + 1000, t_station) {
                        out.push(l);
                    }
                    let mut m = m;
                    m.id = format!("{stream}-{n:04}");
                    m.key = rng::tag(&m.id);
                    m.stream = stream.clone();
                    out.push(m);
                }
                "adsb_bad_position" => {
                    let Some(s) = ctx.cfg.stream(&is.stream) else { continue };
                    let class = Class::parse(&is.class).unwrap_or(Class::Helicopter);
                    // airborne a few minutes before the fault starts
                    let Some(mut m) = air::route_object(s, &mut rng, ctx, is.at_s - 240.0, n, class) else { continue };
                    m.equip.adsb = Adsb::Es1090;
                    let b = is.bearing_deg.to_radians();
                    let d = if is.offset_m > 0.0 { is.offset_m } else { 800.0 };
                    let dur = if is.duration_s > 0.0 { is.duration_s } else { 600.0 };
                    m.faults.push(Fault::PositionOffset { t0: is.at_s, t1: is.at_s + dur, north_m: d * b.cos(), east_m: d * b.sin() });
                    m.id = format!("{stream}-{n:04}");
                    m.key = rng::tag(&m.id);
                    m.stream = stream.clone();
                    out.push(m);
                }
                "flight" => {
                    // one flight of the stream's kind (its class, callsign style, equipage) along
                    // the issue's own route, from `at_s`, broadcasting ADS-B
                    let Some(s) = ctx.cfg.stream(&is.stream) else { continue };
                    let mut s = s.clone();
                    s.routes = vec![crate::config::RouteCfg { name: is.name.clone(), weight: 1.0, points: is.route.clone() }];
                    s.jitter_m = 0.0;
                    s.bidirectional = false;
                    let class = Class::parse(&is.class).unwrap_or(Class::Helicopter);
                    let Some(mut m) = air::route_object(&s, &mut rng, ctx, is.at_s, n, class) else { continue };
                    m.equip.adsb = Adsb::Es1090;
                    m.id = format!("{stream}-{n:04}");
                    m.key = rng::tag(&m.id);
                    m.stream = stream.clone();
                    out.push(m);
                }
                "ais_off" => {
                    let Some(s) = ctx.cfg.stream(&is.stream) else { continue };
                    let class = Class::parse(&is.class).unwrap_or(Class::Fishing);
                    let t0 = is.at_s - rng.random_range(600.0..1200.0);
                    let made = match s.kind.as_str() {
                        "wander" => sea::wander_object(s, &mut rng, ctx, t0, n, class),
                        _ => air::route_object(s, &mut rng, ctx, t0, n, class),
                    };
                    let Some(mut m) = made else { continue };
                    m.equip.ais = Ais::A;
                    m.equip.ais_off = false;
                    let dur = if is.duration_s > 0.0 { is.duration_s } else { 1800.0 };
                    m.faults.push(Fault::Silent { t0: is.at_s, t1: is.at_s + dur });
                    m.id = format!("{stream}-{n:04}");
                    m.key = rng::tag(&m.id);
                    m.stream = stream.clone();
                    out.push(m);
                }
                _ => {}
            }
        }
    }
}
