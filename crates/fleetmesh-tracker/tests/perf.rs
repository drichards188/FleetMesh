//! About 500 tracks and about 2,000 reports a second, timed. Run in release for the numbers:
//! `cargo test -p fleetmesh-tracker --release --test perf -- --nocapture`.
//!
//! The traffic: 300 aircraft (ADS-B position and velocity each second, heard by two receivers,
//! a third of them by a third), 120 vessels (AIS Class A every 10 s, two receivers), 50 drones
//! (Remote ID each second, two receivers), and 30 non-cooperative targets. Three ground radars
//! each see 120 aircraft and 7 non-cooperative aircraft every second; two boats' marine radars
//! each see 60 vessels and 5 kayaks every 2.5 s. The lines are encoded before the clock starts,
//! so only the tracker is timed.

mod common;

use common::*;
use fleetmesh_tracker::{Config, Domain, SensorSite, SiteKind, TrackState, Tracker};
use std::time::Instant;

const KT: f64 = 1852.0 / 3600.0;
const FT: f64 = 0.3048;
const C: (f64, f64) = (37.75, -122.50);

struct Mover {
    lat: f64,
    lon: f64,
    ve: f64,
    vn: f64,
    alt: f64,
}

impl Mover {
    fn at(&self, s: f64) -> (f64, f64) {
        offset(self.lat, self.lon, self.ve * s, self.vn * s)
    }
    fn speed_course(&self) -> (f64, f64) {
        (
            (self.ve * self.ve + self.vn * self.vn).sqrt(),
            (self.ve.atan2(self.vn).to_degrees() + 360.0) % 360.0,
        )
    }
}

fn movers(
    n: usize,
    noise: &mut Noise,
    spread: f64,
    vmin: f64,
    vmax: f64,
    alt: (f64, f64),
) -> Vec<Mover> {
    (0..n)
        .map(|_| {
            let p = offset(
                C.0,
                C.1,
                spread * (2.0 * noise.uniform() - 1.0),
                spread * (2.0 * noise.uniform() - 1.0),
            );
            let v = vmin + (vmax - vmin) * noise.uniform();
            let h = std::f64::consts::TAU * noise.uniform();
            Mover {
                lat: p.0,
                lon: p.1,
                ve: v * h.sin(),
                vn: v * h.cos(),
                alt: alt.0 + (alt.1 - alt.0) * noise.uniform(),
            }
        })
        .collect()
}

enum Line {
    Sbs(String, i64),
    Nmea(String, i64),
    Json(String),
    Update(i64),
}

#[allow(clippy::too_many_arguments)]
fn radar(
    platform: &str,
    kind: &str,
    t: i64,
    s: (f64, f64, f64),
    q: (f64, f64, f64),
    sb: f64,
    sr: f64,
    n: &mut Noise,
) -> String {
    let l = look(s, q);
    serde_json::json!({"type":"det","sensor":kind,"platform":platform,"t":t,"sensor_lat":s.0,"sensor_lon":s.1,
        "sensor_alt_m":s.2,"bearing_deg":l.bearing + sb * n.normal(),"sigma_bearing_deg":sb,"range_m":l.range + sr * n.normal(),
        "sigma_range_m":sr,"size":"medium"})
    .to_string()
}

fn build(seconds: i64) -> (Vec<Line>, usize) {
    let mut n = Noise::new(42);
    let air = movers(300, &mut n, 25_000.0, 60.0, 200.0, (300.0, 6_000.0));
    let ships = movers(120, &mut n, 15_000.0, 1.0, 9.0, (0.0, 0.0));
    let drones = movers(50, &mut n, 8_000.0, 3.0, 15.0, (40.0, 120.0));
    let ncair = movers(21, &mut n, 10_000.0, 30.0, 70.0, (150.0, 600.0));
    let kayaks = movers(10, &mut n, 8_000.0, 0.5, 2.0, (0.0, 0.0));
    let radars = [
        (37.62, -122.38, 20.0),
        (37.72, -122.22, 20.0),
        (37.86, -122.48, 250.0),
    ];
    let boats = [(37.80, -122.43, 4.0), (37.70, -122.35, 4.0)];
    let mut out = Vec::new();
    let mut reports = 0;
    for k in 0..seconds * 10 {
        let tms = T0 + k * 100;
        let s = k as f64 / 10.0;
        // Spread each source's reports over the second by index.
        for (i, a) in air.iter().enumerate() {
            if (i as i64) % 10 != k % 10 {
                continue;
            }
            let p = a.at(s);
            let (sp, co) = a.speed_course();
            let hex = format!("{:06X}", 0xA00000 + i);
            for rx in ["1", "2", "3"] {
                if rx == "3" && i % 3 != 0 {
                    continue;
                }
                for l in adsb_pair(rx, &hex, tms, p.0, p.1, a.alt / FT, sp / KT, co) {
                    out.push(Line::Sbs(l, tms + 30));
                    reports += 1;
                }
            }
        }
        for (i, v) in ships.iter().enumerate() {
            if (i as i64 * 3) % 100 != k % 100 {
                continue;
            }
            let p = v.at(s);
            let (sp, co) = v.speed_course();
            let a = ClassA {
                mmsi: 366_000_000 + i as u32 * 7,
                nav: 0,
                sog_kn: sp / KT,
                lat: p.0,
                lon: p.1,
                cog: co,
                heading: co as u16,
                second: ((tms / 1000) % 60) as u8,
            };
            let (pl, f) = ais_type1(&a);
            for rx in ["ais-pier", "ais-oak"] {
                for l in aivdm(&pl, f, 'A', 0, rx, tms) {
                    out.push(Line::Nmea(l, tms + 200));
                    reports += 1;
                }
            }
        }
        for (i, d) in drones.iter().enumerate() {
            if (i as i64) % 10 != k % 10 {
                continue;
            }
            let p = d.at(s);
            let (sp, co) = d.speed_course();
            for rx in ["hub-pier", "hub-oak"] {
                out.push(Line::Json(rid(
                    rx,
                    tms,
                    &format!("UAS-{i:04}"),
                    "serial",
                    p.0,
                    p.1,
                    d.alt,
                    sp,
                    co,
                )));
                reports += 1;
            }
        }
        if k % 10 == 5 {
            for (r, site) in radars.iter().enumerate() {
                let name = format!("radar-{r}");
                for a in air.iter().skip(r * 100).take(120) {
                    let p = a.at(s);
                    out.push(Line::Json(radar(
                        &name,
                        "ground_radar",
                        tms,
                        *site,
                        (p.0, p.1, a.alt),
                        0.5,
                        15.0,
                        &mut n,
                    )));
                    reports += 1;
                }
                for a in ncair.iter().skip(r * 7).take(7) {
                    let p = a.at(s);
                    out.push(Line::Json(radar(
                        &name,
                        "ground_radar",
                        tms,
                        *site,
                        (p.0, p.1, a.alt),
                        0.5,
                        15.0,
                        &mut n,
                    )));
                    reports += 1;
                }
            }
        }
        if k % 25 == 12 {
            for (b, boat) in boats.iter().enumerate() {
                let name = format!("boat-{b}");
                for v in ships.iter().skip(b * 60).take(60) {
                    let p = v.at(s);
                    out.push(Line::Json(radar(
                        &name,
                        "marine_radar",
                        tms,
                        *boat,
                        (p.0, p.1, 0.0),
                        1.0,
                        25.0,
                        &mut n,
                    )));
                    reports += 1;
                }
                for v in kayaks.iter().skip(b * 5).take(5) {
                    let p = v.at(s);
                    out.push(Line::Json(radar(
                        &name,
                        "marine_radar",
                        tms,
                        *boat,
                        (p.0, p.1, 0.0),
                        1.0,
                        25.0,
                        &mut n,
                    )));
                    reports += 1;
                }
            }
        }
        if k % 10 == 9 {
            out.push(Line::Update(tms + 50));
        }
    }
    (out, reports)
}

#[test]
fn five_hundred_tracks_two_thousand_reports_a_second() {
    let release = !cfg!(debug_assertions);
    let seconds = if release { 300 } else { 20 };
    let (lines, reports) = build(seconds);
    let mut tr = Tracker::new(Config::default());
    tr.set_sensors(vec![
        SensorSite {
            id: "radar-0".into(),
            sbs_session: None,
            kind: SiteKind::GroundRadar,
            lat: Some(37.62),
            lon: Some(-122.38),
            alt_m: Some(20.0),
            range_m: Some(60_000.0),
            scan_ms: Some(1_000),
            domain: Some(Domain::Air),
            owner: None,
        },
        SensorSite {
            id: "radar-1".into(),
            sbs_session: None,
            kind: SiteKind::GroundRadar,
            lat: Some(37.72),
            lon: Some(-122.22),
            alt_m: Some(20.0),
            range_m: Some(60_000.0),
            scan_ms: Some(1_000),
            domain: Some(Domain::Air),
            owner: None,
        },
        SensorSite {
            id: "radar-2".into(),
            sbs_session: None,
            kind: SiteKind::GroundRadar,
            lat: Some(37.86),
            lon: Some(-122.48),
            alt_m: Some(250.0),
            range_m: Some(60_000.0),
            scan_ms: Some(1_000),
            domain: Some(Domain::Air),
            owner: None,
        },
    ]);
    let start = Instant::now();
    let mut worst_update = std::time::Duration::ZERO;
    for l in &lines {
        match l {
            Line::Sbs(s, t) => {
                tr.ingest_sbs(s, *t);
            }
            Line::Nmea(s, t) => {
                tr.ingest_nmea(s, *t);
            }
            Line::Json(s) => {
                tr.ingest_json(s);
            }
            Line::Update(t) => {
                let u = Instant::now();
                tr.update(*t);
                worst_update = worst_update.max(u.elapsed());
            }
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let st = tr.stats().clone();
    let rate = reports as f64 / seconds as f64;
    let confirmed = tr
        .tracks()
        .iter()
        .filter(|t| t.state == TrackState::Confirmed)
        .count();
    println!(
        "perf ({}): {seconds} s simulated, {reports} reports ({rate:.0}/s), {} tracks ({confirmed} confirmed, {} cooperative), \
         wall {wall:.3} s = {:.4} of real time, {:.2} us/report, worst update {:.2} ms, duplicates {}, corroborated {}",
        if release { "release" } else { "debug" },
        st.tracks_live,
        st.tracks_cooperative,
        wall / seconds as f64,
        wall * 1e6 / reports as f64,
        worst_update.as_secs_f64() * 1e3,
        st.duplicates,
        st.det_corroborated,
    );
    assert!((1_800.0..2_300.0).contains(&rate), "{rate}");
    assert!((470..=560).contains(&(st.tracks_live as usize)), "{:?}", st);
    assert!(confirmed >= 460, "{confirmed}");
    // Every non-cooperative target is tracked, no cooperative aircraft grows a radar twin, and
    // nothing honest is flagged as disagreeing.
    let noncoop = tr
        .tracks()
        .iter()
        .filter(|t| !t.cooperative && t.state == TrackState::Confirmed)
        .count();
    assert!((29..=34).contains(&noncoop), "{noncoop}");
    assert!(tr.disagreements().is_empty(), "{:?}", tr.disagreements());
    if release {
        assert!(
            wall < 0.2 * seconds as f64,
            "{wall} s for {seconds} s of traffic"
        );
        assert!(worst_update.as_secs_f64() < 0.2, "{worst_update:?}");
    }
}
