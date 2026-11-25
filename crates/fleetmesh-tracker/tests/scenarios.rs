//! Tracker behaviour on synthetic traffic encoded in the real wire formats.

mod common;

use common::*;
use fleetmesh_tracker::{
    Config, Domain, Flag, OwnFleet, OwnVehicle, SensorSite, SiteKind, Track, TrackState, Tracker,
};
use serde_json::json;

const FT: f64 = 0.3048;
const KT: f64 = 1852.0 / 3600.0;

fn site(
    id: &str,
    kind: SiteKind,
    lat: f64,
    lon: f64,
    alt: f64,
    range: f64,
    scan: i64,
) -> SensorSite {
    SensorSite {
        id: id.into(),
        sbs_session: None,
        kind,
        lat: Some(lat),
        lon: Some(lon),
        alt_m: Some(alt),
        range_m: Some(range),
        scan_ms: Some(scan),
        domain: Some(Domain::Air),
        owner: None,
    }
}

fn by_icao<'a>(tr: &'a Tracker, hex: &str) -> Option<&'a Track> {
    tr.tracks()
        .iter()
        .find(|t| t.ids.icao24.as_deref() == Some(hex))
}

fn by_mmsi(tr: &Tracker, mmsi: u32) -> Option<&Track> {
    tr.tracks().iter().find(|t| t.ids.mmsi == Some(mmsi))
}

fn err_m(t: &Track, lat: f64, lon: f64) -> f64 {
    distance(t.lat, t.lon, lat, lon)
}

fn radar_det(
    tr: &mut Tracker,
    noise: &mut Noise,
    platform: &str,
    t: i64,
    s: (f64, f64, f64),
    target: (f64, f64, f64),
) {
    let l = look(s, target);
    let d = det(
        "ground_radar",
        platform,
        t,
        s,
        l.bearing + 0.5 * noise.normal(),
        None,
        Some(l.range + 15.0 * noise.normal()),
        0.5,
    );
    tr.ingest_json(&d.replace("\"sigma_range_m\":20.0", "\"sigma_range_m\":15.0"));
}

const RADAR: (f64, f64, f64) = (37.78, -122.40, 30.0);

// -------------------------------------------------------------------- two aircraft crossing

#[test]
fn two_aircraft_crossing_with_radar() {
    let mut tr = Tracker::new(Config::default());
    tr.set_sensors(vec![site(
        "radar-1",
        SiteKind::GroundRadar,
        RADAR.0,
        RADAR.1,
        RADAR.2,
        8_000.0,
        1_000,
    )]);
    let p = offset(RADAR.0, RADAR.1, 0.0, 3_000.0);
    let mut noise = Noise::new(1);
    let alt = 450.0;
    let truth = |t: f64| {
        let a = offset(p.0, p.1, -1_800.0 + 60.0 * t, 0.0);
        let b = offset(p.0, p.1, 0.0, -1_920.0 + 60.0 * t);
        (a, b)
    };
    for k in 0..=60 {
        let t = T0 + k * 1_000;
        let (a, b) = truth(k as f64);
        for l in adsb_pair("1", "A0A001", t, a.0, a.1, alt / FT, 60.0 / KT, 90.0) {
            tr.ingest_sbs(&l, t + 50);
        }
        for l in adsb_pair("1", "A0A002", t, b.0, b.1, alt / FT, 60.0 / KT, 0.0) {
            tr.ingest_sbs(&l, t + 50);
        }
        radar_det(&mut tr, &mut noise, "radar-1", t, RADAR, (a.0, a.1, alt));
        radar_det(&mut tr, &mut noise, "radar-1", t, RADAR, (b.0, b.1, alt));
        tr.update(t + 500);
        // Two tracks; a detection that falls outside its gate (0.1% by design) may start a
        // tentative track that never confirms.
        let firm = tr
            .tracks()
            .iter()
            .filter(|t| t.state != TrackState::Tentative)
            .count();
        assert_eq!(
            firm,
            if k == 0 { 0 } else { 2 },
            "t={k}: {:#?}",
            tr.tracks()
                .iter()
                .map(|t| (&t.id, t.state))
                .collect::<Vec<_>>()
        );
    }
    assert!(tr.tracks().iter().all(|t| t.cooperative));
    let (a, b) = truth(60.5);
    let ta = by_icao(&tr, "a0a001").expect("A");
    let tb = by_icao(&tr, "a0a002").expect("B");
    assert!(err_m(ta, a.0, a.1) < 40.0, "{}", err_m(ta, a.0, a.1));
    assert!(err_m(tb, b.0, b.1) < 40.0, "{}", err_m(tb, b.0, b.1));
    for t in [ta, tb] {
        assert_eq!(t.state, TrackState::Confirmed);
        assert!(
            t.sources.iter().any(|s| s.rx == "radar-1"),
            "{:?}",
            t.sources
        );
        assert!(!t.has_flag(Flag::SensorDisagree));
    }
    assert!(tr.disagreements().is_empty());
    assert!(tr.stats().det_corroborated > 100, "{:?}", tr.stats());
}

#[test]
fn two_noncooperative_aircraft_crossing_keep_their_tracks() {
    let mut tr = Tracker::new(Config::default());
    tr.set_sensors(vec![site(
        "radar-1",
        SiteKind::GroundRadar,
        RADAR.0,
        RADAR.1,
        RADAR.2,
        8_000.0,
        1_000,
    )]);
    let p = offset(RADAR.0, RADAR.1, 0.0, 3_000.0);
    let mut noise = Noise::new(2);
    let truth = |t: f64| {
        let a = offset(p.0, p.1, -1_800.0 + 60.0 * t, 0.0);
        let b = offset(p.0, p.1, 0.0, -1_920.0 + 60.0 * t);
        (a, b)
    };
    let nearest = |tr: &Tracker, q: (f64, f64)| -> String {
        let t = tr
            .tracks()
            .iter()
            .min_by(|x, y| err_m(x, q.0, q.1).total_cmp(&err_m(y, q.0, q.1)))
            .expect("a track");
        t.id.clone()
    };
    let mut id_a = String::new();
    let mut id_b = String::new();
    for k in 0..=60 {
        let t = T0 + k * 1_000;
        let (a, b) = truth(k as f64);
        radar_det(&mut tr, &mut noise, "radar-1", t, RADAR, (a.0, a.1, 450.0));
        radar_det(&mut tr, &mut noise, "radar-1", t, RADAR, (b.0, b.1, 450.0));
        tr.update(t + 500);
        if k == 15 {
            id_a = nearest(&tr, a);
            id_b = nearest(&tr, b);
            assert_ne!(id_a, id_b);
        }
    }
    let (a, b) = truth(60.5);
    let confirmed: Vec<&Track> = tr
        .tracks()
        .iter()
        .filter(|t| t.state == TrackState::Confirmed)
        .collect();
    assert_eq!(confirmed.len(), 2, "{:#?}", tr.tracks());
    assert_eq!(nearest(&tr, a), id_a, "track of A swapped");
    assert_eq!(nearest(&tr, b), id_b, "track of B swapped");
    for t in confirmed {
        assert!(!t.cooperative);
        assert_eq!(t.class, fleetmesh_tracker::Class::UnknownAir);
        let e = err_m(t, a.0, a.1).min(err_m(t, b.0, b.1));
        assert!(
            e < (3.5 * t.h_sigma_m).max(60.0),
            "{e} m, sigma {}",
            t.h_sigma_m
        );
    }
}

// -------------------------------------------------------------------- de-duplication

#[test]
fn three_receivers_hear_one_aircraft() {
    let mut tr = Tracker::new(Config::default());
    let rx = |id: &str, n: &str| SensorSite {
        sbs_session: Some(n.into()),
        ..site(id, SiteKind::Adsb1090, 37.8, -122.4, 10.0, 100_000.0, 1_000)
    };
    tr.set_sensors(vec![
        rx("hub-pier", "1"),
        rx("hub-sfo", "2"),
        rx("hub-oak", "3"),
    ]);
    let start = (37.70, -122.30);
    for k in 0..20 {
        let t = T0 + k * 1_000;
        let q = offset(start.0, start.1, -100.0 * k as f64, -80.0 * k as f64);
        for (i, s) in ["1", "2", "3"].iter().enumerate() {
            // The same transmission, generated at t, logged and received a little later by each.
            for l in adsb_pair(s, "ABCDEF", t, q.0, q.1, 3_000.0, 248.0, 231.3) {
                tr.ingest_sbs(&l, t + 30 + 15 * i as i64);
            }
        }
    }
    tr.update(T0 + 19_500);
    assert_eq!(tr.tracks().len(), 1);
    let trk = &tr.tracks()[0];
    let rxs: Vec<&str> = trk.sources.iter().map(|s| s.rx.as_str()).collect();
    assert_eq!(rxs, ["hub-oak", "hub-pier", "hub-sfo"]);
    assert_eq!(tr.stats().duplicates, 2 * 2 * 20);
    let q = offset(start.0, start.1, -100.0 * 19.5, -80.0 * 19.5);
    assert!(err_m(trk, q.0, q.1) < 30.0, "{}", err_m(trk, q.0, q.1));
    let v = trk.vel_mps.expect("velocity");
    assert!(
        (v[0] + 100.0).abs() < 3.0 && (v[1] + 80.0).abs() < 3.0,
        "{v:?}"
    );
}

#[test]
fn sensors_listing_deserialises_and_names_sbs_receivers() {
    // The shape of an entry of the simulator's `/sensors` (docs/design/traffic.md section 5).
    let listing = r#"[
        {"id":"hub-pier","kind":"adsb_1090","num":4,"owner":"company","platform":null,"lat":37.8,"lon":-122.4,"alt_m":12.0,"nominal_range_m":150000.0,"feeders":null,"enabled":true,"up":true},
        {"id":"feed","kind":"adsb_1090","num":9,"owner":"network","platform":null,"lat":null,"lon":null,"alt_m":null,"nominal_range_m":400000.0,"feeders":40,"enabled":true,"up":true},
        {"id":"hub-rid","kind":"rid","num":null,"owner":"company","lat":37.8,"lon":-122.4,"alt_m":12.0,"nominal_range_m":800.0},
        {"id":"radar-1","kind":"ground_radar","num":null,"owner":"company","lat":37.78,"lon":-122.40,"alt_m":30.0,"nominal_range_m":8000.0}
    ]"#;
    let sites: Vec<SensorSite> = serde_json::from_str(listing).expect("sensors");
    assert_eq!(sites[0].sbs_session.as_deref(), Some("4"));
    assert_eq!(sites[1].owner.as_deref(), Some("network"));
    assert_eq!(sites[2].kind, SiteKind::RemoteId);
    assert_eq!(sites[3].range_m, Some(8_000.0));
    let mut tr = Tracker::new(Config::default());
    tr.set_sensors(sites);
    for k in 0..2 {
        let t = T0 + k * 1_000;
        for l in adsb_pair(
            "4",
            "A4A4A4",
            t,
            37.7,
            -122.3 + 0.001 * k as f64,
            2_000.0,
            200.0,
            90.0,
        ) {
            tr.ingest_sbs(&l, t + 30);
        }
        for l in adsb_pair(
            "9",
            "A4A4A4",
            t,
            37.7,
            -122.3 + 0.001 * k as f64,
            2_000.0,
            200.0,
            90.0,
        ) {
            tr.ingest_sbs(&l, t + 3_000);
        }
    }
    tr.update(T0 + 1_500);
    let rxs: Vec<&str> = tr.tracks()[0]
        .sources
        .iter()
        .map(|s| s.rx.as_str())
        .collect();
    assert_eq!(rxs, ["feed", "hub-pier"]);
}

// -------------------------------------------------------------------- AIS rate change

fn ais_a(tr: &mut Tracker, rx: &str, t: i64, a: &ClassA) {
    let (p, f) = ais_type1(a);
    for l in aivdm(&p, f, 'A', 0, rx, t) {
        tr.ingest_nmea(&l, t + 300);
    }
}

#[test]
fn class_a_vessel_slowing_to_anchor_keeps_its_track() {
    let mut tr = Tracker::new(Config::default());
    let mmsi = 366_999_712;
    let other = 366_999_713;
    let start = (37.76, -122.36);
    // Position along a northbound path: 12 kn for 300 s, slowing to 0 over 120 s, then still.
    let v0 = 12.0 * KT;
    let north = |s: f64| -> f64 {
        if s <= 300.0 {
            v0 * s
        } else if s <= 420.0 {
            let u = s - 300.0;
            v0 * 300.0 + v0 * u - 0.5 * (v0 / 120.0) * u * u
        } else {
            v0 * 300.0 + 0.5 * v0 * 120.0
        }
    };
    let speed_kn = |s: f64| {
        if s <= 300.0 {
            12.0
        } else if s <= 420.0 {
            12.0 * (420.0 - s) / 120.0
        } else {
            0.0
        }
    };
    let mut next_report = 0.0;
    let mut anchored_seen = false;
    for k in 0..=(2_400 / 5) {
        let s = k as f64 * 5.0;
        let t = T0 + (s * 1000.0) as i64;
        if s >= next_report {
            let q = offset(start.0, start.1, 0.0, north(s));
            let nav = if s >= 420.0 { 1 } else { 0 };
            let a = ClassA {
                mmsi,
                nav,
                sog_kn: speed_kn(s),
                lat: q.0,
                lon: q.1,
                cog: 0.0,
                heading: 0,
                second: ((t / 1000) % 60) as u8,
            };
            ais_a(&mut tr, "ais-pier", t, &a);
            if (s as i64) % 360 == 0 {
                let (p, f) = ais_type5(
                    mmsi,
                    "WDA1234",
                    "BAY TRADER",
                    70,
                    [200, 30, 16, 16],
                    9.4,
                    "OAKLAND",
                );
                for l in aivdm(&p, f, 'B', 3, "ais-pier", t) {
                    tr.ingest_nmea(&l, t + 300);
                }
            }
            next_report = if s >= 420.0 { s + 180.0 } else { s + 10.0 };
        }
        // A second vessel under way that goes silent at 100 s.
        if s <= 100.0 && (s as i64) % 10 == 0 {
            let q = offset(start.0, start.1, 2_000.0, v0 * s);
            let a = ClassA {
                mmsi: other,
                nav: 0,
                sog_kn: 12.0,
                lat: q.0,
                lon: q.1,
                cog: 0.0,
                heading: 0,
                second: ((t / 1000) % 60) as u8,
            };
            ais_a(&mut tr, "ais-pier", t, &a);
        }
        tr.update(t + 1_000);
        let trk = by_mmsi(&tr, mmsi).unwrap_or_else(|| panic!("vessel lost at {s} s"));
        assert_ne!(trk.state, TrackState::Coasting, "coasting at {s} s");
        assert!(!trk.has_flag(Flag::Stale), "stale at {s} s");
        if s > 425.0 {
            assert!(trk.has_flag(Flag::AtAnchor), "{:?}", trk.flags);
            anchored_seen = true;
            let q = offset(start.0, start.1, 0.0, north(500.0));
            assert!(
                err_m(trk, q.0, q.1) < 40.0,
                "{} at {s}",
                err_m(trk, q.0, q.1)
            );
        }
        let o = by_mmsi(&tr, other);
        match s {
            s if s <= 125.0 => {
                assert_eq!(o.map(|o| o.state), Some(TrackState::Confirmed), "at {s}")
            }
            s if (140.0..=455.0).contains(&s) => {
                assert_eq!(o.map(|o| o.state), Some(TrackState::Coasting), "at {s}")
            }
            s if s >= 465.0 => assert!(o.is_none(), "other vessel still there at {s}"),
            _ => {}
        }
    }
    assert!(anchored_seen);
    let trk = by_mmsi(&tr, mmsi).expect("vessel");
    assert_eq!(trk.class, fleetmesh_tracker::Class::Cargo);
    assert_eq!(trk.domain, Domain::Surface);
    assert_eq!(trk.alt_m, None);
    let st = serde_json::to_value(&trk.static_).expect("json");
    assert_eq!(st["name"], "BAY TRADER");
    assert_eq!(st["callsign"], "WDA1234");
    assert_eq!(st["length_m"], 230.0);
    assert_eq!(st["beam_m"], 32.0);
    assert_eq!(st["draught_m"], 9.4);
    assert_eq!(st["destination"], "OAKLAND");
    assert_eq!(st["eta"], "06-14T18:30Z");
    assert_eq!(st["nav_status_text"], "at anchor");
}

// -------------------------------------------------------------------- coasting and dropping

#[test]
fn coasting_then_dropped() {
    let mut tr = Tracker::new(Config::default());
    let start = (37.62, -122.38);
    let speed = 100.0;
    for k in 0..=20 {
        let t = T0 + k * 1_000;
        let q = offset(start.0, start.1, speed * k as f64, 0.0);
        for l in adsb_pair("1", "A12345", t, q.0, q.1, 5_000.0, speed / KT, 90.0) {
            tr.ingest_sbs(&l, t + 40);
        }
    }
    let last = offset(start.0, start.1, speed * 20.0, 0.0);
    let at = |tr: &mut Tracker, s: f64| -> Option<Track> {
        tr.update(T0 + (s * 1000.0) as i64);
        by_icao(tr, "a12345").cloned()
    };
    assert_eq!(tr.picture_time(), None);
    let t1 = at(&mut tr, 21.5).expect("present");
    assert_eq!(tr.picture_time(), Some(T0 + 21_500));
    assert_eq!(t1.state, TrackState::Confirmed);
    assert!(!t1.has_flag(Flag::Stale));
    let t2 = at(&mut tr, 24.0).expect("present");
    assert_eq!(t2.state, TrackState::Confirmed);
    assert!(t2.has_flag(Flag::Stale));
    let t3 = at(&mut tr, 26.0).expect("present");
    assert_eq!(t3.state, TrackState::Coasting);
    // Extrapolated along its velocity, with a wider sigma.
    let d = err_m(&t3, last.0, last.1);
    assert!((d - 600.0).abs() < 40.0, "{d}");
    assert!(
        t3.h_sigma_m > t1.h_sigma_m * 2.0,
        "{} vs {}",
        t3.h_sigma_m,
        t1.h_sigma_m
    );
    let p10 = tr.predict(&t3, 10.0);
    let p60 = tr.predict(&t3, 60.0);
    assert!((distance(t3.lat, t3.lon, p10.lat, p10.lon) - 1_000.0).abs() < 30.0);
    assert!((distance(t3.lat, t3.lon, p60.lat, p60.lon) - 6_000.0).abs() < 150.0);
    assert!(
        p60.h_sigma_m > p10.h_sigma_m * 3.0,
        "{} {}",
        p60.h_sigma_m,
        p10.h_sigma_m
    );
    assert!(p60.v_sigma_m.expect("alt sigma") > p10.v_sigma_m.expect("alt sigma"));
    assert_eq!(tr.predict_path(&t3, 120.0, 30.0).len(), 4);
    assert!(at(&mut tr, 49.0).is_some());
    assert!(at(&mut tr, 51.0).is_none());
    assert_eq!(tr.stats().tracks_dropped, 1);
}

// -------------------------------------------------------------------- non-cooperative from radar

fn marine_det(
    platform: &str,
    t: i64,
    s: (f64, f64, f64),
    bearing: f64,
    range: f64,
    sb: f64,
    sr: f64,
) -> String {
    json!({"type":"det","sensor":"marine_radar","platform":platform,"t":t,"sensor_lat":s.0,"sensor_lon":s.1,
        "sensor_alt_m":s.2,"bearing_deg":bearing,"sigma_bearing_deg":sb,"elev_deg":null,"sigma_elev_deg":null,
        "range_m":range,"sigma_range_m":sr,"radial_speed_mps":null,"size":"small"})
    .to_string()
}

#[test]
fn noncooperative_track_from_marine_radar() {
    let mut tr = Tracker::new(Config::default());
    let mut noise = Noise::new(5);
    let boat0 = (37.81, -122.43);
    let kayak0 = offset(boat0.0, boat0.1, 600.0, 1_040.0);
    let (kve, kvn) = (
        1.5 * 200f64.to_radians().sin(),
        1.5 * 200f64.to_radians().cos(),
    );
    let mut confirmed_at = None;
    for k in 0..30 {
        let s = k as f64 * 2.5;
        let t = T0 + (s * 1000.0) as i64;
        let boat = offset(boat0.0, boat0.1, 5.0 * s, 0.0);
        let kayak = offset(kayak0.0, kayak0.1, kve * s, kvn * s);
        let l = look((boat.0, boat.1, 4.0), (kayak.0, kayak.1, 0.0));
        let d = marine_det(
            "boat-3",
            t,
            (boat.0, boat.1, 4.0),
            l.bearing + noise.normal(),
            l.range + 25.0 * noise.normal(),
            1.0,
            25.0,
        );
        tr.ingest_json(&d);
        // Clutter: one false detection a scan anywhere out to 6 km.
        let cb = 360.0 * noise.uniform();
        let cr = 500.0 + 5_500.0 * noise.uniform();
        tr.ingest_json(&marine_det(
            "boat-3",
            t,
            (boat.0, boat.1, 4.0),
            cb,
            cr,
            1.0,
            25.0,
        ));
        tr.update(t + 100);
        let near = |x: &&Track| err_m(x, kayak.0, kayak.1) < 150.0;
        let firm = tr
            .tracks()
            .iter()
            .filter(near)
            .filter(|x| x.state != TrackState::Tentative)
            .count();
        match k {
            0 | 1 => {
                assert_eq!(firm, 0, "scan {k}");
                assert!(
                    tr.tracks()
                        .iter()
                        .filter(near)
                        .any(|x| x.state == TrackState::Tentative),
                    "scan {k}"
                );
            }
            _ => {
                assert_eq!(firm, 1, "scan {k}: {:#?}", tr.tracks());
                if confirmed_at.is_none() {
                    confirmed_at = Some(k);
                }
            }
        }
    }
    assert_eq!(confirmed_at, Some(2), "confirmed on the third look");
    let s = 29.0 * 2.5;
    let kayak = offset(kayak0.0, kayak0.1, kve * s, kvn * s);
    let confirmed: Vec<&Track> = tr
        .tracks()
        .iter()
        .filter(|t| t.state != TrackState::Tentative)
        .collect();
    assert_eq!(confirmed.len(), 1, "clutter confirmed: {confirmed:#?}");
    let k = confirmed[0];
    assert!(!k.cooperative);
    assert_eq!(k.domain, Domain::Surface);
    assert_eq!(k.class, fleetmesh_tracker::Class::SmallCraft);
    assert!(
        err_m(k, kayak.0, kayak.1) < 60.0,
        "{}",
        err_m(k, kayak.0, kayak.1)
    );
    assert!(k.sources.iter().all(|s| s.rx == "boat-3"));
    // Clutter never piles up: tentative tracks die after missing looks.
    assert!(tr.tracks().len() <= 4, "{}", tr.tracks().len());
}

// -------------------------------------------------------------------- bearing-only

const DRONE: (f64, f64, f64) = (37.80, -122.45, 120.0);

fn daa(
    tr: &mut Tracker,
    noise: &mut Noise,
    platform: &str,
    t: i64,
    s: (f64, f64, f64),
    target: (f64, f64, f64),
) {
    let l = look(s, target);
    tr.ingest_json(&det(
        "daa",
        platform,
        t,
        s,
        l.bearing + 2.0 * noise.normal(),
        Some(l.elev + noise.normal()),
        None,
        2.0,
    ));
}

#[test]
fn bearing_only_from_one_hovering_drone_claims_no_position() {
    let mut tr = Tracker::new(Config::default());
    let mut noise = Noise::new(6);
    let c = offset(DRONE.0, DRONE.1, -250.0, 1_000.0);
    let mut target = c;
    for k in 0..=12 {
        let t = T0 + k * 1_000;
        target = offset(c.0, c.1, 50.0 * k as f64, 0.0);
        daa(
            &mut tr,
            &mut noise,
            "12",
            t,
            DRONE,
            (target.0, target.1, 300.0),
        );
        tr.update(t + 200);
    }
    assert_eq!(tr.tracks().len(), 1, "{:#?}", tr.tracks());
    let trk = &tr.tracks()[0];
    assert!(trk.has_flag(Flag::BearingOnly), "{:?}", trk.flags);
    assert!(!trk.has_flag(Flag::Triangulated));
    assert_eq!(trk.state, TrackState::Confirmed);
    assert!(!trk.cooperative);
    assert!(trk.h_sigma_m >= 500.0, "{}", trk.h_sigma_m);
    assert_eq!(trk.vel_mps, None);
    let line = trk.bearing_only.as_ref().expect("the line");
    let truth = bearing(DRONE.0, DRONE.1, target.0, target.1);
    assert!(
        (line.bearing_deg - truth).abs() < 6.0,
        "{} vs {truth}",
        line.bearing_deg
    );
    assert_eq!(line.range_m[1], 3_000.0);
    assert!((line.sensor_lat - DRONE.0).abs() < 1e-6);
    // Its prediction stays on the line and only widens.
    let p = tr.predict(trk, 30.0);
    assert!(p.h_sigma_m > trk.h_sigma_m);
}

#[test]
fn bearing_only_from_two_drones_is_triangulated() {
    let mut tr = Tracker::new(Config::default());
    let mut noise = Noise::new(7);
    let d2 = offset(DRONE.0, DRONE.1, 1_200.0, 0.0);
    let d2 = (d2.0, d2.1, 100.0);
    let c = offset(DRONE.0, DRONE.1, 300.0, 1_000.0);
    for k in 0..=10 {
        let t = T0 + k * 1_000;
        let target = offset(c.0, c.1, 50.0 * k as f64, 0.0);
        daa(
            &mut tr,
            &mut noise,
            "12",
            t,
            DRONE,
            (target.0, target.1, 300.0),
        );
        daa(
            &mut tr,
            &mut noise,
            "14",
            t + 30,
            d2,
            (target.0, target.1, 300.0),
        );
        tr.update(t + 200);
    }
    let firm: Vec<&Track> = tr
        .tracks()
        .iter()
        .filter(|t| t.state != TrackState::Tentative)
        .collect();
    assert_eq!(firm.len(), 1, "{:#?}", tr.tracks());
    let trk = firm[0];
    assert!(trk.has_flag(Flag::Triangulated), "{:?}", trk.flags);
    assert!(!trk.has_flag(Flag::BearingOnly));
    assert!(trk.bearing_only.is_none());
    let target = offset(c.0, c.1, 50.0 * 10.2, 0.0);
    let e = err_m(trk, target.0, target.1);
    assert!(e < 120.0, "error {e} m, sigma {}", trk.h_sigma_m);
    assert!(trk.h_sigma_m < 150.0, "{}", trk.h_sigma_m);
    let alt = trk.alt_m.expect("altitude from the elevations");
    assert!((alt - 300.0).abs() < 80.0, "{alt}");
    assert!(tr.stats().triangulated >= 1);
}

#[test]
fn bearing_only_from_a_moving_drone_is_triangulated_once_it_turns() {
    let mut tr = Tracker::new(Config::default());
    let mut noise = Noise::new(8);
    // Another operator's drone hovering without Remote ID, 700 m north-east.
    let target = offset(DRONE.0, DRONE.1, 495.0, 495.0);
    let target = (target.0, target.1, 100.0);
    let mut first_fix = None;
    for k in 0..=40 {
        let t = T0 + k * 1_000;
        let s = k as f64;
        // North at 15 m/s for 15 s, then east.
        let (e, n) = if s <= 15.0 {
            (0.0, 15.0 * s)
        } else {
            (15.0 * (s - 15.0), 225.0)
        };
        let me = offset(DRONE.0, DRONE.1, e, n);
        daa(&mut tr, &mut noise, "20", t, (me.0, me.1, 120.0), target);
        tr.update(t + 200);
        if first_fix.is_none() && tr.tracks().iter().any(|x| x.has_flag(Flag::Triangulated)) {
            first_fix = Some(k);
        }
        if k <= 15 {
            assert!(
                tr.tracks().iter().all(|x| !x.has_flag(Flag::Triangulated)),
                "fix too soon at {k}"
            );
        }
    }
    // No fix while the drone flies straight (bearings alone cannot range a target then), one
    // once its turn has given enough parallax.
    let k = first_fix.expect("triangulated");
    assert!(k > 15, "{k}");
    let firm: Vec<&Track> = tr
        .tracks()
        .iter()
        .filter(|t| t.state != TrackState::Tentative)
        .collect();
    assert_eq!(firm.len(), 1, "{:#?}", tr.tracks());
    let trk = firm[0];
    let e = err_m(trk, target.0, target.1);
    assert!(
        e < 120.0 && e < 3.5 * trk.h_sigma_m,
        "{e} m, sigma {} (fix at {k})",
        trk.h_sigma_m
    );
}

// -------------------------------------------------------------------- association

#[test]
fn detection_with_two_candidates_goes_to_the_nearer() {
    let mut tr = Tracker::new(Config::default());
    let mut noise = Noise::new(9);
    let boat = (37.80, -122.42, 4.0);
    let a = offset(boat.0, boat.1, 260.0, 1_480.0);
    let b = offset(boat.0, boat.1, 280.0, 1_540.0);
    for k in 0..12 {
        let t = T0 + k * 2_500;
        for q in [a, b] {
            let l = look(boat, (q.0, q.1, 0.0));
            tr.ingest_json(&marine_det(
                "boat-1",
                t,
                boat,
                l.bearing + 0.2 * noise.normal(),
                l.range + 5.0 * noise.normal(),
                0.3,
                6.0,
            ));
        }
        tr.update(t + 100);
    }
    let near = |tr: &Tracker, q: (f64, f64)| -> String {
        tr.tracks()
            .iter()
            .min_by(|x, y| err_m(x, q.0, q.1).total_cmp(&err_m(y, q.0, q.1)))
            .map(|t| t.id.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        tr.tracks()
            .iter()
            .filter(|t| t.state == TrackState::Confirmed)
            .count(),
        2
    );
    let (ida, idb) = (near(&tr, a), near(&tr, b));
    assert_ne!(ida, idb);
    // One coarse detection inside both gates, 10 m from A and about 55 m from B.
    let t = T0 + 12 * 2_500;
    let q = offset(a.0, a.1, 0.0, 10.0);
    let l = look(boat, (q.0, q.1, 0.0));
    tr.ingest_json(&marine_det(
        "boat-1", t, boat, l.bearing, l.range, 2.0, 40.0,
    ));
    tr.update(t + 100);
    let last = |tr: &Tracker, id: &str| {
        tr.tracks()
            .iter()
            .find(|x| x.id == id)
            .map(|x| x.sources[0].last_t)
    };
    assert_eq!(last(&tr, &ida), Some(t), "A takes it");
    assert_eq!(last(&tr, &idb), Some(t - 2_500), "B does not");
    assert_eq!(tr.tracks().len(), 2, "no new track");
    // Two detections in one scan, each nearer one track: one each.
    let t = T0 + 13 * 2_500;
    for q in [offset(b.0, b.1, 0.0, -15.0), offset(a.0, a.1, 0.0, 15.0)] {
        let l = look(boat, (q.0, q.1, 0.0));
        tr.ingest_json(&marine_det(
            "boat-1", t, boat, l.bearing, l.range, 2.0, 40.0,
        ));
    }
    tr.update(t + 100);
    assert_eq!(last(&tr, &ida), Some(t));
    assert_eq!(last(&tr, &idb), Some(t));
    assert_eq!(tr.tracks().len(), 2);
}

// -------------------------------------------------------------------- integrity

#[test]
fn adsb_position_that_disagrees_with_radar_is_flagged() {
    let run = || {
        let mut tr = Tracker::new(Config::default());
        tr.set_sensors(vec![site(
            "radar-1",
            SiteKind::GroundRadar,
            RADAR.0,
            RADAR.1,
            RADAR.2,
            8_000.0,
            1_000,
        )]);
        let mut noise = Noise::new(10);
        let x0 = offset(RADAR.0, RADAR.1, -2_000.0, 2_500.0);
        let y0 = offset(RADAR.0, RADAR.1, -2_500.0, -2_000.0);
        let mut pictures = Vec::new();
        for k in 0..=45 {
            let t = T0 + k * 1_000;
            let x = offset(x0.0, x0.1, 70.0 * k as f64, 0.0);
            let y = offset(y0.0, y0.1, 0.0, 60.0 * k as f64);
            // X broadcasts where it is, and the radar sees it there; then, from 10 s, a position
            // 1.2 km north of where it is (a faulty navigation source).
            let xb = offset(x.0, x.1, 0.0, if k < 10 { 0.0 } else { 1_200.0 });
            for l in adsb_pair("1", "BADBAD", t, xb.0, xb.1, 300.0 / FT, 70.0 / KT, 90.0) {
                tr.ingest_sbs(&l, t + 40);
            }
            for l in adsb_pair("1", "600D00", t, y.0, y.1, 400.0 / FT, 60.0 / KT, 0.0) {
                tr.ingest_sbs(&l, t + 40);
            }
            radar_det(&mut tr, &mut noise, "radar-1", t, RADAR, (x.0, x.1, 300.0));
            radar_det(&mut tr, &mut noise, "radar-1", t, RADAR, (y.0, y.1, 400.0));
            tr.update(t + 500);
            pictures.push(serde_json::to_string(tr.tracks()).expect("json"));
        }
        (tr, pictures)
    };
    let (tr, pictures) = run();
    let x = by_icao(&tr, "badbad").expect("X");
    let y = by_icao(&tr, "600d00").expect("Y");
    assert!(x.has_flag(Flag::SensorDisagree), "{:?}", x.flags);
    assert!(!y.has_flag(Flag::SensorDisagree), "{:?}", y.flags);
    assert_eq!(tr.disagreements().len(), 1);
    let d = &tr.disagreements()[0];
    assert_eq!(d.track, x.id);
    assert!((d.offset_m - 1_200.0).abs() < 150.0, "{}", d.offset_m);
    assert_eq!(d.sensor, "ground_radar:radar-1");
    let sensed = tr
        .tracks()
        .iter()
        .find(|t| t.id == d.sensed)
        .expect("sensed track");
    assert!(!sensed.cooperative);
    assert!(sensed.has_flag(Flag::SensorDisagree));
    // Y is seen by the radar where it says it is: one track, corroborated.
    assert!(y.sources.iter().any(|s| s.rx == "radar-1"));
    let firm = tr
        .tracks()
        .iter()
        .filter(|t| t.state != TrackState::Tentative)
        .count();
    assert_eq!(
        firm,
        3,
        "{:#?}",
        tr.tracks()
            .iter()
            .map(|t| (&t.id, t.cooperative, t.state))
            .collect::<Vec<_>>()
    );
    // The flag needs persistence: not in the first seconds of the fault.
    assert!(!pictures[3].contains("sensor_disagree"));
    assert!(!pictures[13].contains("sensor_disagree"));
    // Deterministic: the same inputs give the same picture at every step.
    let (_, again) = run();
    assert_eq!(pictures, again);
}

#[test]
fn position_jump_is_flagged_and_a_lasting_shift_is_followed() {
    let mut tr = Tracker::new(Config::default());
    let start = (37.66, -122.30);
    for k in 0..=40 {
        let t = T0 + k * 1_000;
        let mut q = offset(start.0, start.1, 80.0 * k as f64, 0.0);
        if k == 10 || k >= 25 {
            q = offset(q.0, q.1, 0.0, 3_000.0);
        }
        for l in adsb_pair("1", "C0C0C0", t, q.0, q.1, 4_000.0, 80.0 / KT, 90.0) {
            tr.ingest_sbs(&l, t + 40);
        }
        tr.update(t + 500);
        let trk = by_icao(&tr, "c0c0c0").expect("track");
        let truth = offset(start.0, start.1, 80.0 * (k as f64 + 0.5), 0.0);
        if k == 10 {
            assert!(trk.has_flag(Flag::PositionJump));
            assert!(
                err_m(trk, truth.0, truth.1) < 60.0,
                "moved by one bad report"
            );
        }
        if (11..25).contains(&k) {
            assert!(err_m(trk, truth.0, truth.1) < 60.0);
        }
        if k >= 27 {
            let shifted = offset(truth.0, truth.1, 0.0, 3_000.0);
            assert!(
                err_m(trk, shifted.0, shifted.1) < 80.0,
                "k={k} {}",
                err_m(trk, shifted.0, shifted.1)
            );
            assert!(trk.has_flag(Flag::PositionJump));
        }
    }
    assert_eq!(tr.tracks().len(), 1);
}

// -------------------------------------------------------------------- identities

#[test]
fn rotating_anonymous_address_continues_the_track() {
    let mut tr = Tracker::new(Config::default());
    let start = (37.85, -122.55);
    let mut id_before = String::new();
    for k in 0..=60 {
        let t = T0 + k * 1_000;
        let q = offset(start.0, start.1, 55.0 * k as f64, 20.0 * k as f64);
        let hex = if k <= 30 { "~A00001" } else { "~B00002" };
        for l in adsb_pair("1", hex, t, q.0, q.1, 1_200.0, 58.5 / KT, 70.0) {
            tr.ingest_sbs(&l, t + 40);
        }
        tr.update(t + 500);
        if k == 20 {
            id_before = tr.tracks()[0].id.clone();
        }
        if k >= 32 {
            assert_eq!(tr.tracks().len(), 1, "k={k}: {:#?}", tr.tracks());
        }
    }
    let trk = &tr.tracks()[0];
    assert_eq!(trk.id, id_before);
    assert_eq!(trk.ids.icao24.as_deref(), Some("b00002"));
    assert!(trk.ids.anonymous);
    assert!(trk.has_flag(Flag::Anonymous) && trk.has_flag(Flag::IdChanged));
    assert_eq!(tr.stats().handovers, 1);

    // Two anonymous aircraft at once are two tracks, never merged.
    let mut tr = Tracker::new(Config::default());
    for k in 0..=20 {
        let t = T0 + k * 1_000;
        let q = offset(start.0, start.1, 55.0 * k as f64, 0.0);
        let r = offset(start.0, start.1, 55.0 * k as f64, 400.0);
        for l in adsb_pair("1", "~C00003", t, q.0, q.1, 1_200.0, 55.0 / KT, 90.0) {
            tr.ingest_sbs(&l, t + 40);
        }
        if k >= 5 {
            for l in adsb_pair("1", "~D00004", t, r.0, r.1, 1_200.0, 55.0 / KT, 90.0) {
                tr.ingest_sbs(&l, t + 40);
            }
        }
        tr.update(t + 500);
    }
    assert_eq!(tr.tracks().len(), 2);
    assert_eq!(tr.stats().handovers, 0);
}

#[test]
fn own_fleet_ladd_and_emergency() {
    let mut tr = Tracker::new(Config {
        ladd_icao: vec!["A0A0A0".into()],
        ..Default::default()
    });
    tr.set_own_fleet(&OwnFleet {
        icao: vec!["C0FFEE".into()],
        mmsi: vec![366_000_001],
        uas_ids: vec!["OWN-1".into()],
    });
    let q = (37.7, -122.4);
    let cs = SbsFields {
        callsign: Some("N123AB".into()),
        ..Default::default()
    };
    let sq = SbsFields {
        alt_ft: Some(2_000.0),
        squawk: Some("7700".into()),
        ..Default::default()
    };
    for k in 0..3 {
        let t = T0 + k * 1_000;
        for hex in ["C0FFEE", "A0A0A0", "E0E0E0"] {
            let p = offset(
                q.0,
                q.1,
                50.0 * k as f64,
                if hex == "E0E0E0" { 2_000.0 } else { 0.0 },
            );
            for l in adsb_pair("1", hex, t, p.0, p.1, 2_000.0, 97.0, 90.0) {
                tr.ingest_sbs(&l, t + 40);
            }
            tr.ingest_sbs(&sbs(1, "1", hex, t + 10, t + 20, &cs), t + 50);
        }
        tr.ingest_sbs(&sbs(6, "1", "E0E0E0", t + 15, t + 25, &sq), t + 60);
        tr.ingest_json(&rid(
            "hub-pier", t, "OWN-1", "serial", 37.71, -122.41, 60.0, 5.0, 90.0,
        ));
    }
    tr.update(T0 + 2_500);
    assert!(by_icao(&tr, "c0ffee").is_none());
    assert!(tr.tracks().iter().all(|t| t.ids.uas_id.is_none()));
    let l = by_icao(&tr, "a0a0a0").expect("LADD aircraft is tracked");
    assert!(l.has_flag(Flag::Ladd));
    assert_eq!(l.ids.callsign, None);
    let e = by_icao(&tr, "e0e0e0").expect("aircraft");
    assert!(e.has_flag(Flag::Emergency));
    assert_eq!(e.ids.callsign.as_deref(), Some("N123AB"));
    assert_eq!(
        serde_json::to_value(&e.static_).expect("json")["squawk"],
        "7700"
    );
    assert!(tr.stats().own_ignored >= 9);

    // A sensor detection of an own vehicle is not traffic.
    let boat = (37.80, -122.42, 4.0);
    let own = offset(boat.0, boat.1, 300.0, 400.0);
    tr.set_own_vehicles(&[OwnVehicle {
        uid: "boat-7".into(),
        t: T0 + 3_000,
        lat: own.0,
        lon: own.1,
        alt_m: 0.0,
    }]);
    let l = look(boat, (own.0, own.1, 0.0));
    tr.ingest_json(&marine_det(
        "boat-1",
        T0 + 3_000,
        boat,
        l.bearing,
        l.range,
        1.0,
        20.0,
    ));
    tr.update(T0 + 3_100);
    assert_eq!(tr.stats().own_detections, 1);
    assert!(tr.tracks().iter().all(|t| t.cooperative));
}

#[test]
fn class_b_static_parts_and_remote_id_session() {
    let mut tr = Tracker::new(Config::default());
    let mmsi = 338_123_456;
    let q = (37.82, -122.44);
    for k in 0..4 {
        let t = T0 + k * 30_000;
        let p = offset(q.0, q.1, 0.0, 77.0 * k as f64);
        let (pl, f) = ais_type18(mmsi, 5.0, p.0, p.1, 0.0, ((t / 1000) % 60) as u8);
        for l in aivdm(&pl, f, 'B', 0, "ais-pier", t) {
            tr.ingest_nmea(&l, t + 200);
        }
    }
    let (a, fa) = ais_type24a(mmsi, "SEA BREEZE");
    let (b, fb) = ais_type24b(mmsi, 36, "WXY9876", [8, 4, 2, 2]);
    for l in aivdm(&a, fa, 'A', 0, "ais-pier", T0 + 95_000)
        .into_iter()
        .chain(aivdm(&b, fb, 'A', 0, "ais-pier", T0 + 96_000))
    {
        tr.ingest_nmea(&l, T0 + 96_500);
    }
    // A Remote ID drone with a session id (anonymous) and an ellipsoid height.
    for k in 0..3 {
        let t = T0 + 90_000 + k * 1_000;
        let p = offset(37.79, -122.40, 6.0 * k as f64, 0.0);
        tr.ingest_json(&rid(
            "hub-pier",
            t,
            "SESSION-77",
            "session",
            p.0,
            p.1,
            96.0,
            6.0,
            90.0,
        ));
    }
    tr.update(T0 + 97_000);
    let v = by_mmsi(&tr, mmsi).expect("vessel");
    assert_eq!(v.class, fleetmesh_tracker::Class::Sailing);
    let st = serde_json::to_value(&v.static_).expect("json");
    assert_eq!(
        (
            st["name"].as_str(),
            st["callsign"].as_str(),
            st["length_m"].as_f64()
        ),
        (Some("SEA BREEZE"), Some("WXY9876"), Some(12.0))
    );
    assert_eq!(v.sources[0].kind, fleetmesh_tracker::SourceKind::AisB);
    assert_eq!(v.state, TrackState::Confirmed);
    let d = tr
        .tracks()
        .iter()
        .find(|t| t.ids.uas_id.as_deref() == Some("SESSION-77"))
        .expect("drone");
    assert!(d.has_flag(Flag::Anonymous));
    assert_eq!(d.class, fleetmesh_tracker::Class::Drone);
    // 96 m above the ellipsoid is 128 m above sea level where the geoid is 32 m below it.
    assert!((d.alt_m.expect("alt") - 128.0).abs() < 2.0, "{:?}", d.alt_m);
}

#[test]
fn delayed_feed_and_out_of_order_reports() {
    // A commercial feed relays the same transmissions seconds late: duplicates, not new data.
    let mut tr = Tracker::new(Config::default());
    let start = (37.70, -122.25);
    let mut late = Vec::new();
    for k in 0..15 {
        let t = T0 + k * 1_000;
        let q = offset(start.0, start.1, 90.0 * k as f64, 0.0);
        for l in adsb_pair("1", "FEED01", t, q.0, q.1, 6_000.0, 90.0 / KT, 90.0) {
            tr.ingest_sbs(&l, t + 30);
            late.push((l.replacen(",1,1,", ",7,1,", 1), t + 4_000));
        }
        while let Some((l, rt)) = late.first().cloned() {
            if rt > t + 30 {
                break;
            }
            tr.ingest_sbs(&l, rt);
            late.remove(0);
        }
        tr.update(t + 500);
    }
    let trk = by_icao(&tr, "feed01").expect("track");
    assert!(trk.sources.iter().any(|s| s.rx == "7"));
    // Every relayed position is a duplicate. A relayed steady velocity message matches a newer
    // one from the own receiver only by content, so it arrives as a late report and is not fused.
    assert!(tr.stats().duplicates >= 11, "{:?}", tr.stats());
    assert_eq!(
        tr.stats().duplicates + tr.stats().late_reports,
        22,
        "{:?}",
        tr.stats()
    );
    let q = offset(start.0, start.1, 90.0 * 14.5, 0.0);
    assert!(err_m(trk, q.0, q.1) < 30.0);
}

#[test]
fn a_vessel_moving_like_one_the_radar_never_saw_is_not_a_disagreement() {
    // A boat's radar sees a small boat with no AIS; 1 km away a sailboat with AIS moves just as it
    // does, too far for the radar to see a boat its size. The radar never saw the sailboat, so not
    // seeing it now contradicts nothing.
    let mut tr = Tracker::new(Config::default());
    tr.set_sensors(vec![site("radar-1", SiteKind::GroundRadar, RADAR.0, RADAR.1, RADAR.2, 8_000.0, 1_000)]);
    let mut noise = Noise::new(11);
    let a0 = offset(RADAR.0, RADAR.1, 1_500.0, 0.0);
    let b0 = offset(RADAR.0, RADAR.1, 1_500.0, 1_000.0);
    for k in 0..=90 {
        let t = T0 + k * 1_000;
        let a = offset(a0.0, a0.1, 4.0 * k as f64, 0.0);
        let b = offset(b0.0, b0.1, 4.0 * k as f64, 0.0);
        // the AIS boat: a ground radar watching the air here stands in for a radar that sees only
        // the other (an air target is what this tracker's ground radar associates)
        for l in adsb_pair("1", "5A1105", t, b.0, b.1, 300.0 / FT, 4.0 / KT, 90.0) {
            tr.ingest_sbs(&l, t + 40);
        }
        radar_det(&mut tr, &mut noise, "radar-1", t, RADAR, (a.0, a.1, 300.0));
        tr.update(t + 500);
    }
    assert!(tr.disagreements().is_empty(), "{:?}", tr.disagreements());
}
