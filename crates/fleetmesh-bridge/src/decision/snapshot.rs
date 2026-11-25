//! The world model as the decision layer sees it: a [`WorldSnapshot`] keyed by uid, each
//! fact stamped with when the message carrying it arrived (on the bridge's clock, which the
//! engine reads too), unknown left `None`.

use fleetmesh_decide::{
    AreaSet, AutopilotKind, Catalog, Circle, CircleSource, Fact, GpsFixFrom, LonLat, LostLinkPlan,
    LostLinkSource, Time, Uid as DUid, VehicleFacts, WorldSnapshot,
};

use crate::clock;
use crate::mav::Kind;
use crate::vehicle::{Uid, Vehicle};
use crate::world::World;

/// The decision layer's name for a vehicle: its fleet uid in decimal.
pub fn duid(uid: Uid) -> DUid {
    DUid::new(uid.to_string())
}

pub fn bridge_uid(uid: &DUid) -> Option<Uid> {
    uid.as_str().parse().ok()
}

/// The bridge's clock's seconds (`clock::wall`, the engine's clock) to the engine's integer
/// milliseconds.
pub fn ms(wall_s: f64) -> Time {
    Time((wall_s * 1000.0).round() as i64)
}

fn autopilot(kind: Kind) -> Option<AutopilotKind> {
    Some(match kind {
        Kind::Plane => AutopilotKind::Plane,
        Kind::Vtol => AutopilotKind::Vtol,
        Kind::Copter => AutopilotKind::Copter,
        Kind::Blimp => AutopilotKind::Blimp,
        Kind::Boat => AutopilotKind::Boat,
        Kind::Ugv => AutopilotKind::Ugv,
        Kind::Unknown => return None,
    })
}

/// One vehicle's facts. The autopilot kind comes from its heartbeat; a drifting buoy
/// reports itself as a boat (its stand-in imitates ArduRover), so when the catalog says the
/// platform is a drifter and the heartbeat says boat, the fact is "drifter" (as Q_ENABLE
/// turns a plane heartbeat into a VTOL).
///
/// `orbit` is the circle an orbit task flies on station (from the task layer); a fixed wing
/// loitering or circling home on its own reports its circle too ([`Vehicle::loiter_circle`]).
pub fn vehicle_facts(
    v: &Vehicle,
    catalog: &Catalog,
    now_mono: f64,
    report_interval_ms: u32,
    orbit: Option<([f64; 2], f64)>,
) -> VehicleFacts {
    let platform = v
        .identity
        .as_ref()
        .and_then(|i| i.str("platform"))
        .and_then(|p| catalog.get(p))
        .cloned();
    fn fact<T>(value: T, t: Option<f64>) -> Option<Fact<T>> {
        t.map(|t| Fact::new(value, ms(t)))
    }
    VehicleFacts {
        autopilot: v
            .kind
            .and_then(autopilot)
            .and_then(|k| fact(k, v.seen.heartbeat)),
        nav_ready: v
            .nav_ready_fact(now_mono)
            .map(|(ready, t)| Fact::new(ready, ms(t))),
        position: v
            .pos
            .and_then(|(lat, lon)| fact(LonLat::new(lon, lat), v.seen.position)),
        home: v
            .home
            .and_then(|(lat, lon)| fact(LonLat::new(lon, lat), v.seen.home)),
        battery_pct: v
            .battery_pct
            .and_then(|b| fact(f64::from(b), v.seen.battery)),
        nav_source: v
            .nav_source_fact(now_mono)
            .map(|(source, t)| Fact::new(source, ms(t))),
        gps_fix: v.gps_fix_fact().map(|(f, t)| Fact::new(f, ms(t))),
        gps_glitch: v.gps_glitch_fact().map(|(g, t)| Fact::new(g, ms(t))),
        gps_fix_from: (v.gps_fix_hl2 && v.seen.gps.is_some()).then_some(GpsFixFrom::HighLatency2),
        // the circle it flies, stamped with the older of the position and its center
        circling: v.seen.position.and_then(|pos_t| {
            let (center, radius_m, source, center_t) = match orbit {
                Some((c, r)) => (LonLat::new(c[0], c[1]), r, CircleSource::OrbitTask, None),
                None => {
                    let l = v.loiter_circle()?;
                    let source = if l.rtl { CircleSource::Rtl } else { CircleSource::Loiter };
                    (LonLat::new(l.center.1, l.center.0), l.radius_m, source, Some(l.center_t))
                }
            };
            let circle = Circle {
                center,
                radius_m,
                source: Some(source),
                center_t: center_t.map(ms),
            };
            Some(Fact::new(circle, ms(center_t.map_or(pos_t, |c| c.min(pos_t)))))
        }),
        report_interval_ms: Some(report_interval_ms),
        // velocity over ground, observed with the position
        velocity_mps: v.vel_en.and_then(|vel| fact(vel, v.seen.position)),
        // altitude above mean sea level, with the position (rules 6: keep-outs in 3D)
        alt_msl_m: v.alt_msl.and_then(|a| fact(a, v.seen.position)),
        // when it booted (rules 5): a queued order approved before a reboot is looked at again
        booted_t: v.booted_wall.map(ms),
        // rules 25: the damage it reported of itself, stamped with its latest report
        damage: v.damage.map(|d| {
            Fact::new(
                fleetmesh_decide::VehicleDamage { mobility: d.mobility, payload_loss: d.payload_loss },
                ms(d.last),
            )
        }),
        // rules 30: its own lost-link failsafe, read or configured: [`lost_link_fact`], filled
        // by [`build`] for a run under rules 30
        lost_link: None,
        // rules 32: its sensors' reach now: not filled here
        reach_now: None,
        platform,
    }
}

/// Rules 30: a vehicle's own lost-link failsafe as the fact `lost_link`, for its autopilot
/// `autopilot` (the fact, a drifting buoy told from a boat): read, once the vehicle has
/// reported every GCS failsafe parameter of its firmware (in any boot: a reboot asks again,
/// and the last reports stand meanwhile), stamped with the oldest of those reports (or its
/// last heartbeat, if older: the action follows the mode and armed state it was last heard
/// in); else configured, where the link could not carry a read.
pub fn lost_link_fact(v: &Vehicle, autopilot: Option<AutopilotKind>) -> Option<Fact<LostLinkPlan>> {
    let autopilot = autopilot?;
    let heard = v.seen.heartbeat?;
    let p = &v.lost_link;
    let names = super::lost_link_read::lost_link_names(autopilot);
    let read = |name: &str| p.reported.get(name).map(|r| r.value);
    // the plan a vehicle will have once it flies: a failsafe acts on an armed vehicle only, but
    // a delivery order arms its vehicle, so the rules judge the plan it will fly under
    let (plan, source, t) = match super::lost_link_read::lost_link_plan(autopilot, v.mode.as_deref(), true, read) {
        Some(plan) => {
            let t = names.iter().filter_map(|n| p.reported.get(n)).map(|r| r.t).fold(heard, f64::min);
            (plan, LostLinkSource::Read, t)
        }
        // every parameter reported, and still no plan: not known (no fallback over a read)
        None if names.iter().all(|n| p.reported.contains_key(n)) => return None,
        None => {
            let (params, t) = p.configured.as_ref()?;
            let plan = super::lost_link_read::lost_link_plan(autopilot, v.mode.as_deref(), true, |n| params.get(n).copied())?;
            (plan, LostLinkSource::Configured, t.min(heard))
        }
    };
    let (action, timer_s) = plan;
    // a fixed wing's own circle, where it holds and arrives home (RTL_RADIUS, else WP_LOITER_RAD,
    // as ArduPlane flies it), once read: a silent one's region circles at it (fleetmesh_decide::silent)
    let circle_m = autopilot
        .is_fixed_wing()
        .then(|| match (v.rtl_radius_m, v.loiter_radius_m) {
            (Some(r), _) if r != 0.0 => Some(r.abs()),
            (_, Some(l)) => Some(l.abs()),
            _ => None,
        })
        .flatten()
        .filter(|r| *r > 0.0);
    Some(Fact::new(LostLinkPlan { action, timer_s, source, circle_m }, ms(t)))
}

/// Every vehicle the bridge knows, at engine time `t`. A vehicle the fleet manifest names that
/// has never been heard from is in it too, with its platform and no facts. With `node_loss`
/// (rules 30, the decision loop's `RulesConfig::node_loss`) each vehicle's facts carry its own
/// lost-link failsafe ([`lost_link_fact`]); without it that fact stays out, as before.
pub fn build(
    world: &World,
    catalog: &Catalog,
    areas: &AreaSet,
    t: Time,
    node_loss: bool,
) -> WorldSnapshot {
    let now = clock::mono();
    let mut snap = WorldSnapshot::new(t);
    for cell in world.vehicles() {
        let interval = cell.link_snapshot(now).expected_interval_ms;
        let orbit = world.tasks.orbit_of(&cell);
        let facts = cell.read(|v| {
            let mut f = vehicle_facts(v, catalog, now, interval, orbit);
            if node_loss {
                f.lost_link = lost_link_fact(v, f.autopilot.as_ref().map(|a| a.value));
            }
            f
        });
        snap.vehicles.insert(duid(cell.uid), facts);
    }
    let manifest = world.manifest();
    for (uid, ident) in &manifest.identities {
        snap.vehicles.entry(duid(*uid)).or_insert_with(|| VehicleFacts {
            platform: ident.str("platform").and_then(|p| catalog.get(p)).cloned(),
            ..VehicleFacts::default()
        });
    }
    drop(manifest);
    snap.areas = Some(areas.clone());
    snap
}

/// The bridge's own reading for the delivery rules: each vehicle's navigation fix (the GNSS state
/// it reports: satellites, HDOP, horizontal accuracy, and the jump of its reported position
/// against where its velocity put it) and, for the separation rule, where every vehicle is now.
/// Wrong-surface and range checks, a vehicle's route and the hysteresis of its trust state are
/// not tracked here yet: those inputs stay absent, and the rules say so.
pub fn delivery_vehicle_inputs(world: &World, catalog: &Catalog, di: &mut fleetmesh_decide::delivery::DeliveryInputs) {
    use fleetmesh_decide::delivery::{NavFacts, TrafficFact};
    let now = clock::mono();
    for cell in world.vehicles() {
        let uid = duid(cell.uid);
        let gps_t = cell.read(|v| v.seen.gps);
        let nav = cell.read(|v| NavFacts {
            sats: gps_t.map(|_| v.satellites),
            hdop: v.gps_hdop,
            h_acc_m: v.gps_h_acc_m,
            jump_m: v.nav_jump_m,
            speed_mps: v.groundspeed,
            ..NavFacts::default()
        });
        di.nav.insert(uid.clone(), nav);
        let interval = cell.link_snapshot(now).expected_interval_ms;
        let facts = cell.read(|v| vehicle_facts(v, catalog, now, interval, None));
        if let Some(pos) = facts.position {
            let domain = facts.platform.as_ref().map(|p| p.domain);
            if let Some(domain) = domain {
                di.traffic.insert(
                    uid,
                    TrafficFact {
                        position: pos,
                        alt_m: cell.read(|v| v.alt_rel),
                        velocity_mps: facts.velocity_mps.map(|f| f.value),
                        route: Vec::new(),
                        speed_mps: cell.read(|v| v.groundspeed),
                        domain,
                    },
                );
            }
        }
    }
}
