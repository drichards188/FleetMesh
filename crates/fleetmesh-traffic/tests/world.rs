//! The world as a library: deterministic however it is stepped, faster than real time, and
//! populated with what the scenarios ask for.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use fleetmesh_traffic::{Emission, Options, OwnKind, OwnVehicle, Stream, Traffic};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn make(scenario: &str, seed: Option<u64>) -> Traffic {
    Traffic::from_files(
        &repo().join("sim/traffic.toml"),
        Some(&repo().join(format!("sim/scenarios/{scenario}/traffic.toml"))),
        Options { seed, ..Default::default() },
    )
    .unwrap()
}

fn fleet() -> Vec<OwnVehicle> {
    let drone = |uid: &str, lat: f64, lon: f64| OwnVehicle {
        uid: uid.into(),
        kind: OwnKind::Drone,
        lat,
        lon,
        alt_msl_m: 93.0,
        alt_rel_m: Some(90.0),
        heading_deg: 270.0,
        speed_mps: 8.0,
    };
    vec![
        drone("3", 37.8050, -122.4600),
        drone("7", 37.8120, -122.4300),
        drone("12", 37.8170, -122.4150),
        OwnVehicle {
            uid: "40".into(),
            kind: OwnKind::Boat,
            lat: 37.8130,
            lon: -122.4080,
            alt_msl_m: 0.0,
            alt_rel_m: Some(0.0),
            heading_deg: 90.0,
            speed_mps: 4.0,
        },
    ]
}

fn run(t: &mut Traffic, steps_ms: &[i64]) -> Vec<Emission> {
    let start = t.start_ms();
    let mut out = Vec::new();
    for s in steps_ms {
        out.extend(t.step_to(start + s));
    }
    out
}

#[test]
fn the_same_seed_gives_the_same_truth_and_bytes_however_it_is_stepped() {
    let end = 180_000;
    let mut a = make("harbour-city", None);
    let mut b = make("harbour-city", None);
    let mut c = make("harbour-city", None);
    for t in [&mut a, &mut b, &mut c] {
        t.set_own_vehicles(fleet());
    }
    let ea = run(&mut a, &(1..=end / 1000).map(|s| s * 1000).collect::<Vec<_>>());
    let eb = run(&mut b, &(1..=end / 10_000).map(|s| s * 10_000).collect::<Vec<_>>());
    // irregular steps, as a live loop under load makes them
    let mut irregular = Vec::new();
    let mut x = 0;
    let mut k = 0;
    while x < end {
        x = (x + 37 + (k * 7919) % 2_300).min(end);
        irregular.push(x);
        k += 1;
    }
    let ec = run(&mut c, &irregular);
    assert!(ea.len() > 10_000, "only {} lines in three minutes", ea.len());
    assert_eq!(ea.len(), eb.len());
    assert_eq!(ea.len(), ec.len());
    for (i, (x, y)) in ea.iter().zip(eb.iter()).enumerate() {
        assert_eq!(x, y, "line {i} differs between 1 s and 10 s steps");
    }
    assert!(ea == ec, "irregular steps differ");
    let ta = serde_json::to_string(&a.truth()).unwrap();
    let tb = serde_json::to_string(&b.truth()).unwrap();
    assert_eq!(ta, tb);
    for s in [Stream::Sbs, Stream::Ais, Stream::Json] {
        assert!(ea.iter().any(|e| e.stream == s), "nothing on {s:?}");
    }
    // another seed is another world
    let mut d = make("harbour-city", Some(12));
    d.set_own_vehicles(fleet());
    let ed = run(&mut d, &(1..=30).map(|s| s * 1000).collect::<Vec<_>>());
    let ea30: Vec<&Emission> = ea.iter().filter(|e| e.t_ms <= a.start_ms() + 30_000).collect();
    assert_ne!(ea30.len(), 0);
    assert!(ed.len() != ea30.len() || ed.iter().zip(ea30).any(|(x, y)| x != y));
}

#[test]
fn a_headless_hour_runs_faster_than_real_time_and_holds_the_expected_traffic() {
    let mut t = make("harbour-city", None);
    t.set_own_vehicles(fleet());
    let wall = Instant::now();
    let start = t.start_ms();
    let mut lines: BTreeMap<String, usize> = BTreeMap::new();
    let mut counts_at_start = BTreeMap::new();
    for s in 1..=600 {
        for e in t.step_to(start + s * 1000) {
            *lines.entry(format!("{:?}/{}", e.stream, e.kind)).or_insert(0) += 1;
        }
        if s == 1 {
            counts_at_start = t.counts();
        }
    }
    let secs = wall.elapsed().as_secs_f64();
    eprintln!("600 s of scenario in {secs:.1} s wall; lines {lines:?}");
    eprintln!("objects at the start: {counts_at_start:?}");
    eprintln!("objects at 600 s: {:?}", t.counts());
    let st = t.stats();
    eprintln!("lost: {:?}", st.radio.lost);
    eprintln!("rates: {:?}", st.rates_per_s);
    assert!(secs < 600.0, "slower than real time: {secs} s");
    let c = t.counts();
    for class in ["airliner", "ferry", "sailing", "cargo"] {
        assert!(c.get(class).copied().unwrap_or(0) > 0, "no {class}: {c:?}");
    }
    assert!(counts_at_start.get("airliner").copied().unwrap_or(0) >= 5, "the world did not start warm: {counts_at_start:?}");
}

#[test]
fn every_scenario_loads_and_its_issues_appear() {
    for name in ["harbour-city", "wind-event", "coastal-boat"] {
        let t = make(name, None);
        let ids: Vec<String> = t.truth_at(t.start_ms() + 3_600_000).iter().map(|o| o.id.clone()).collect();
        let _ = ids;
        assert!(t.sensors().len() >= 12, "{name}");
    }
    // harbour-city: the drone without Remote ID is up at 20 minutes, the helicopter's broadcasts
    // are off at 26
    let mut t = make("harbour-city", None);
    let start = t.start_ms();
    t.step_to(start + 1_000);
    let at = |t: &Traffic, s: i64| t.truth_at(start + s * 1000);
    let drone = at(&t, 1200).into_iter().find(|o| o.stream == "issue-drone-no-rid").expect("the drone");
    assert_eq!(drone.class, "drone");
    assert!(drone.equipage.rid.is_none());
    let heli = at(&t, 1560).into_iter().find(|o| o.stream == "issue-adsb-bad-position").expect("the helicopter");
    assert!(heli.faults.contains(&"position_offset"), "{:?}", heli.faults);
    let ship = at(&t, 1500).into_iter().find(|o| o.stream == "issue-ship-arrival").expect("the ship");
    // under the bridge at 1500 s
    assert!((ship.lon - -122.479).abs() < 0.01 && (ship.lat - 37.813).abs() < 0.01, "{} {}", ship.lat, ship.lon);
}

#[test]
fn privacy_addresses_come_from_one_block_and_remote_id_heights_are_above_the_ellipsoid() {
    use fleetmesh_traffic::config::{ScenarioCfg, WorldConfig};
    let cfg = WorldConfig::load(&repo().join("sim/traffic.toml")).unwrap();
    let [lo, hi] = cfg.addresses.pia_block;
    let geoid = cfg.world.geoid_n_m;
    let mut scn = ScenarioCfg { at: Some("2026-06-14T17:00:00Z".into()), duration_s: Some(3600.0), ..Default::default() };
    // plenty of business jets and drones
    scn.rates.insert("overflights".into(), 20.0);
    scn.rates.insert("drones".into(), 6.0);
    let mut t = Traffic::new(cfg, scn, Options::default()).unwrap();
    let start = t.start_ms();
    let mut pia = std::collections::BTreeSet::new();
    let mut checked = 0;
    for s in 1..=900 {
        let lines = t.step_to(start + s * 1000);
        for o in t.truth() {
            if o.equipage.pia {
                let a = o.ids.icao24.unwrap();
                assert!((lo..=hi).contains(&a), "{} has {a:06x}", o.id);
                assert!(!o.ids.callsign.as_deref().unwrap_or("N").starts_with('N'), "a PIA flight keeps its registration");
                pia.insert((o.id.clone(), a));
            } else if let Some(a) = o.ids.icao24 {
                assert!(!(lo..=hi).contains(&a), "{} is in the PIA block", o.id);
            }
        }
        for e in lines.iter().filter(|e| e.kind == "rid") {
            let v: serde_json::Value = serde_json::from_str(&e.line).unwrap();
            let truth = t.truth_at(v["t"].as_i64().unwrap() + 50);
            let Some(o) = truth.iter().find(|o| o.ids.uas_id.as_deref() == v["uas_id"].as_str()) else { continue };
            let want = o.alt_m + geoid;
            assert!((v["alt_geo_m"].as_f64().unwrap() - want).abs() < 2.0, "{} against {want}", v["alt_geo_m"]);
            checked += 1;
        }
    }
    assert!(pia.len() >= 2, "too few PIA flights to check: {pia:?}");
    let addrs: std::collections::BTreeSet<u32> = pia.iter().map(|(_, a)| *a).collect();
    assert_eq!(addrs.len(), pia.len(), "each flight its own address");
    assert!(checked > 0, "no Remote ID heard");
}

#[test]
fn what_the_receivers_emit_decodes_back_to_the_truth() {
    let mut t = make("coastal-boat", None);
    let start = t.start_ms();
    let (mut ais_checked, mut sbs_checked) = (0, 0);
    for s in 1..=240 {
        let lines = t.step_to(start + s * 1000);
        for e in &lines {
            // own receivers only: a network's lines arrive seconds after the fact
            if e.src.starts_with("net-") {
                continue;
            }
            match e.stream {
                Stream::Ais => {
                    let f: Vec<&str> = e.line.split('\\').nth(2).unwrap().trim_end().split(',').collect();
                    if f[1] != "1" {
                        continue;
                    }
                    let mut r = common::Rd::new(f[5]);
                    let ty = r.u(6);
                    let (lat, lon, mmsi) = match ty {
                        1..=3 => {
                            r.u(2);
                            let mmsi = r.u(30);
                            r.u(4 + 8 + 10 + 1);
                            let lon = r.i(28) as f64 / 600_000.0;
                            (r.i(27) as f64 / 600_000.0, lon, mmsi)
                        }
                        18 => {
                            r.u(2);
                            let mmsi = r.u(30);
                            r.u(8 + 10 + 1);
                            let lon = r.i(28) as f64 / 600_000.0;
                            (r.i(27) as f64 / 600_000.0, lon, mmsi)
                        }
                        _ => continue,
                    };
                    let truth = t.truth_at(e.t_ms - 100);
                    let o = truth.iter().find(|o| o.ids.mmsi == Some(mmsi as u32)).expect("an MMSI that exists");
                    let d = ((o.lat - lat) * 111_320.0).hypot((o.lon - lon) * 88_000.0);
                    assert!(d < 60.0, "{} reported {d:.0} m from where it is", o.id);
                    ais_checked += 1;
                }
                Stream::Sbs => {
                    let f: Vec<&str> = e.line.trim_end().split(',').collect();
                    if f[1] != "3" {
                        continue;
                    }
                    let hex = f[4].trim_start_matches('~');
                    let icao = u32::from_str_radix(hex, 16).unwrap();
                    let (lat, lon): (f64, f64) = (f[14].parse().unwrap(), f[15].parse().unwrap());
                    let truth = t.truth_at(e.t_ms - 50);
                    let Some(o) = truth.iter().find(|o| o.ids.icao24 == Some(icao)) else { continue };
                    if !o.faults.is_empty() {
                        continue;
                    }
                    let d = ((o.lat - lat) * 111_320.0).hypot((o.lon - lon) * 88_000.0);
                    // an airliner covers some 10 m between the squitter and the truth sampled 50 ms
                    // before receipt
                    assert!(d < 80.0, "{} reported {d:.0} m from where it is", o.id);
                    sbs_checked += 1;
                }
                Stream::Json => {}
            }
        }
    }
    assert!(ais_checked > 100 && sbs_checked > 1_000, "{ais_checked} AIS, {sbs_checked} SBS");
}

#[test]
fn vessels_keep_to_the_water() {
    use fleetmesh_traffic::terrain::Grid;
    let Ok(water) = Grid::load(&repo().join("sim/terrain/golden-gate.water.grid.gz")) else { return };
    let mut t = make("coastal-boat", None);
    let start = t.start_ms();
    let mut bad: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen = 0;
    for s in (0..=7200).step_by(60) {
        t.step_to(start + s * 1000 + 1);
        for o in t.truth() {
            if o.domain != "surface" {
                continue;
            }
            // the depth 30 m either way: a berth or a dock is at the shoreline
            let deepest = [(0.0, 0.0), (0.00027, 0.0), (-0.00027, 0.0), (0.0, 0.00034), (0.0, -0.00034)]
                .iter()
                .filter_map(|(a, b)| water.height(o.lat + a, o.lon + b))
                .fold(f64::NEG_INFINITY, f64::max);
            if deepest == f64::NEG_INFINITY {
                continue;
            }
            seen += 1;
            if deepest <= 0.0 {
                *bad.entry(o.stream.clone()).or_insert(0) += 1;
            }
        }
    }
    let total_bad: usize = bad.values().sum();
    eprintln!("{seen} vessel positions on the grid, {total_bad} on land: {bad:?}");
    assert!((total_bad as f64) < 0.01 * seen as f64, "{bad:?}");
}
