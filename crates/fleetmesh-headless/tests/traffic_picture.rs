//! The company's picture against the simulated world, with no bridge: fleetmesh-traffic's lines fed
//! to fleetmesh-tracker as the bridge feeds them, an own boat and drones moving among the traffic.

use std::collections::BTreeMap;
use std::path::PathBuf;

use fleetmesh_tracker::{Config, SensorSite, Tracker};
use fleetmesh_traffic::{Options, OwnKind, OwnVehicle, Stream, Traffic};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dl, dn) = ((b.0 - a.0) * 111_320.0 * a.1.to_radians().cos(), (b.1 - a.1) * 111_320.0);
    dl.hypot(dn)
}

struct Run {
    traffic: Traffic,
    tracker: Tracker,
    start: i64,
}

fn run(scenario: &str) -> Run {
    let r = root();
    let traffic = Traffic::from_files(&r.join("sim/traffic.toml"), Some(&r.join(format!("sim/scenarios/{scenario}/traffic.toml"))), Options::default()).expect("traffic");
    let start = traffic.start_ms();
    let mut cfg = Config { geoid_n_m: -32.0, ..Config::default() };
    cfg.anonymous_icao_ranges = vec![("ADF800".into(), "ADFBFF".into())];
    let mut tracker = Tracker::new(cfg);
    let sites: Vec<SensorSite> = traffic
        .sensors()
        .iter()
        .filter_map(|s| serde_json::to_value(s).ok())
        .filter(|v| !matches!(v["kind"].as_str(), Some("daa" | "marine_radar")))
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    tracker.set_sensors(sites);
    Run { traffic, tracker, start }
}

/// Our boat going west along the Cityfront and back at 5 m/s, and two drones out over the
/// waterfront at 90 m, `s` seconds in.
fn own_at(s: f64) -> Vec<OwnVehicle> {
    let leg = 3500.0;
    let x = (s * 5.0) % (2.0 * leg);
    let f = if x < leg { x / leg } else { 2.0 - x / leg };
    let (lon0, lon1) = (-122.4100, -122.4500);
    let boat = OwnVehicle { uid: "14".into(), kind: OwnKind::Boat, lat: 37.8112, lon: lon0 + (lon1 - lon0) * f, alt_msl_m: 0.0, alt_rel_m: Some(0.0), heading_deg: if x < leg { 270.0 } else { 90.0 }, speed_mps: 5.0 };
    let d = |uid: &str, phase: f64, lat: f64| {
        let a = (s / 300.0 + phase) * std::f64::consts::TAU;
        OwnVehicle { uid: uid.into(), kind: OwnKind::Drone, lat: lat + 0.006 * a.sin(), lon: -122.44 + 0.012 * a.cos(), alt_msl_m: 93.0, alt_rel_m: Some(90.0), heading_deg: 0.0, speed_mps: 12.0 }
    };
    vec![boat, d("1", 0.0, 37.806), d("2", 0.5, 37.809)]
}

fn to_tracker(v: &[OwnVehicle], t: i64) -> Vec<fleetmesh_tracker::OwnVehicle> {
    v.iter().map(|o| fleetmesh_tracker::OwnVehicle { uid: o.uid.clone(), t, lat: o.lat, lon: o.lon, alt_m: o.alt_msl_m }).collect()
}

/// Every second for `secs`: the disagreements the tracker reports, by the cooperative track's
/// identity, with whether the sensed track is really the same object (truth), and the domain.
fn disagreements(scenario: &str, secs: i64) -> BTreeMap<String, (String, bool, f64, String)> {
    let mut r = run(scenario);
    let mut out = BTreeMap::new();
    for s in 1..=secs {
        let t = r.start + s * 1000;
        let own = own_at(s as f64);
        r.traffic.set_own_vehicles(own.clone());
        for e in r.traffic.step_to(t) {
            match e.stream {
                Stream::Sbs => {
                    r.tracker.ingest_sbs(e.line.trim_end(), e.t_ms);
                }
                Stream::Ais => {
                    r.tracker.ingest_nmea(e.line.trim_end(), e.t_ms);
                }
                Stream::Json => {
                    r.tracker.ingest_json(e.line.trim_end());
                }
            }
        }
        r.tracker.set_own_vehicles(&to_tracker(&own, t));
        r.tracker.update(t);
        if s < 120 {
            continue;
        }
        let truth = r.traffic.truth();
        for d in r.tracker.disagreements() {
            let tracks = r.tracker.tracks();
            let (Some(c), Some(n)) = (tracks.iter().find(|x| x.id == d.track), tracks.iter().find(|x| x.id == d.sensed)) else { continue };
            let who = c.ids.mmsi.map(|m| m.to_string()).or(c.ids.icao24.clone()).unwrap_or_default();
            let real = truth.iter().find(|o| o.ids.mmsi.is_some_and(|m| Some(m) == c.ids.mmsi) || o.ids.icao24.is_some_and(|a| c.ids.icao24.as_deref().is_some_and(|h| u32::from_str_radix(h, 16).ok() == Some(a))));
            let same = real.is_some_and(|o| dist((n.lon, n.lat), (o.lon, o.lat)) < 150.0);
            let faulty = real.is_some_and(|o| !o.faults.is_empty());
            out.entry(who).or_insert((format!("{:?} {:?} {}", c.domain, c.class, if faulty { "FAULTY" } else { "" }), same, d.offset_m, d.sensor.clone()));
        }
    }
    out
}

#[test]
fn harbour_city_flags_the_broadcast_fault_and_no_ordinary_traffic() {
    // the scripted fault: a tour helicopter's ADS-B 700 m off from 1500 s for ten minutes; the
    // Pier hub's radar sees it where it is once it reaches the waterfront
    let found = disagreements("harbour-city", 2100);
    assert!(found.values().any(|(what, same, offset, sensor)| what.contains("FAULTY") && *same && (*offset - 700.0).abs() < 200.0 && sensor.starts_with("ground_radar")), "{found:?}");
    assert!(found.values().all(|(what, ..)| what.contains("FAULTY")), "an ordinary vessel or aircraft flagged: {found:?}");
}

#[test]
fn an_hour_of_boats_among_ships_and_sailors_flags_nothing() {
    // coastal-boat has no broadcast fault: our boat's radar against two hours of AIS vessels
    let found = disagreements("coastal-boat", 2400);
    assert!(found.is_empty(), "{found:?}");
}
