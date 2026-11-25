//! The traffic simulator in process (docs/design/traffic.md, phase 3): `fleetmesh-traffic` stepped on
//! the episode's clock, its lines handed to the bridge's tracker with no sockets, as a live run's
//! feeds would deliver them, each received at its own delivery time. Own-vehicle positions go to
//! its onboard sensors from the bridge's world (read-only, as the live binary polls `/api/state`).
//!
//! The scenario's traffic file is `FLEETMESH_TRAFFIC_SCENARIO`, else the `traffic` key of the
//! scenario's `scenario.toml` (`FLEETMESH_SCENARIO_DIR`); `FLEETMESH_TRAFFIC=0` turns it off. The
//! simulator's truth is kept for the evaluation and the dev-run overlay only.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fleetmesh_traffic::{Options, OwnKind, OwnVehicle, Stream, Traffic};
use serde_json::{Value, json};

use crate::decision::Decision;
use crate::decision::traffic::Feed;
use crate::world::World;

/// The scenario's traffic file, if this episode has one.
pub fn scenario_file(root: &Path) -> Option<PathBuf> {
    let var = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
    if matches!(var("FLEETMESH_TRAFFIC").as_deref().map(str::trim), Some("0" | "off" | "false" | "no")) {
        return None;
    }
    let abs = |p: PathBuf| if p.is_absolute() { p } else { root.join(p) };
    if let Some(f) = var("FLEETMESH_TRAFFIC_SCENARIO") {
        return Some(abs(PathBuf::from(f)));
    }
    let dir = abs(PathBuf::from(var("FLEETMESH_SCENARIO_DIR")?));
    let sc: toml::Table = toml::from_str(&std::fs::read_to_string(dir.join("scenario.toml")).ok()?).ok()?;
    let f = dir.join(sc.get("traffic")?.as_str()?);
    f.exists().then_some(f)
}

/// The simulator, started at the episode's epoch (the scenario's start).
pub fn load(root: &Path, scenario: &Path, epoch_s: f64) -> Result<Traffic, String> {
    Traffic::from_files(
        &root.join("sim/traffic.toml"),
        Some(scenario),
        Options { seed: None, start_ms: Some((epoch_s * 1000.0).round() as i64), duration_s: None },
    )
}

/// The fleet's vehicles as the simulator's onboard sensors need them.
fn own_vehicles(world: &World) -> Vec<OwnVehicle> {
    world
        .vehicles()
        .iter()
        .filter_map(|c| {
            c.read(|v| {
                let (lat, lon) = v.pos?;
                let kind = match v.kind {
                    Some(crate::mav::Kind::Copter | crate::mav::Kind::Vtol | crate::mav::Kind::Blimp) => OwnKind::Drone,
                    Some(crate::mav::Kind::Plane) => OwnKind::Plane,
                    Some(crate::mav::Kind::Boat) => OwnKind::Boat,
                    _ => OwnKind::Ground,
                };
                Some(OwnVehicle {
                    uid: c.uid.to_string(),
                    kind,
                    lat,
                    lon,
                    alt_msl_m: v.alt_msl.or(v.alt_rel).unwrap_or(0.0),
                    alt_rel_m: v.alt_rel,
                    heading_deg: v.heading.unwrap_or(0.0),
                    speed_mps: v.groundspeed.unwrap_or(0.0),
                })
            })
        })
        .collect()
}

fn feed(s: Stream) -> Feed {
    match s {
        Stream::Sbs => Feed::Sbs,
        Stream::Ais => Feed::Ais,
        Stream::Json => Feed::Json,
    }
}

/// Attach the simulator to the bridge's picture and step it once a second of the episode's clock
/// from `from`: the fleet's positions in, every line delivered by then out to the tracker, the
/// picture's tick.
pub fn spawn(mut traffic: Traffic, world: Arc<World>, decision: Arc<Decision>, from: tokio::time::Instant) {
    let pic = decision.traffic();
    pic.attach("in_process");
    let own: std::collections::BTreeSet<String> = world.vehicles().iter().map(|c| c.uid.to_string()).collect();
    let sensors: Vec<Value> = traffic.sensors().iter().filter_map(|s| serde_json::to_value(s).ok()).collect();
    pic.set_sensors(&sensors, &own);
    tokio::spawn(crate::sim::perf::timed("traffic", async move {
        tokio::time::sleep_until(from).await;
        let mut every = tokio::time::interval(Duration::from_secs(1));
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut k: u64 = 0;
        loop {
            every.tick().await;
            let now_ms = (crate::clock::wall() * 1000.0).round() as i64;
            if now_ms <= traffic.now_ms() {
                continue;
            }
            traffic.set_own_vehicles(own_vehicles(&world));
            for e in traffic.step_to(now_ms) {
                decision.traffic().ingest(feed(e.stream), &e.line, e.t_ms);
            }
            if k.is_multiple_of(5) {
                decision.traffic().set_truth(json!({ "t": now_ms, "objects": traffic.truth() }));
            }
            k += 1;
            decision.traffic_tick();
        }
    }));
}
