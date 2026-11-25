//! Decoders against published real-world examples with known decoded values:
//! gpsd's AIVDM samples (tests/data/gpsd-sample.aivdm, BSD-2-Clause, notice inside) and the
//! SBS-1 examples of node-sbs1 (tests/data/sbs-examples.txt, X11), plus the JSON lines of
//! docs/design/traffic.md section 5 and malformed input.

use fleetmesh_tracker::ais::{self, AisMessage};
use fleetmesh_tracker::nmea::{self, Assembler, NmeaError};
use fleetmesh_tracker::sbs::{self, SbsKind};
use fleetmesh_tracker::wire::{self, JsonLine, SensorKind};
use fleetmesh_tracker::{Config, Flag, Outcome, Tracker};

const GPSD: &str = include_str!("data/gpsd-sample.aivdm");
const SBS: &str = include_str!("data/sbs-examples.txt");

fn close(a: Option<f64>, b: f64, tol: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() <= tol)
}

/// Every message the gpsd excerpt decodes to, in order.
fn gpsd_messages() -> Vec<AisMessage> {
    let mut asm = Assembler::new(10_000);
    let mut out = Vec::new();
    for line in GPSD.lines().filter(|l| l.starts_with('!')) {
        let (tag, s) = nmea::split_line(line).expect("framing");
        let s = nmea::parse_vdm(s, true).expect("checksum");
        if let Some(a) = asm.push(tag.as_ref(), s, "test", 0).expect("fragments") {
            out.push(ais::decode(&a.payload, a.fill).expect("payload"));
        }
    }
    out
}

#[test]
fn gpsd_class_a_positions() {
    let m = gpsd_messages();
    let AisMessage::Position(p) = &m[0] else {
        panic!("{:?}", m[0])
    };
    assert_eq!(
        (p.msg_type, p.mmsi, p.nav_status, p.rot_raw),
        (1, 371_798_000, Some(0), Some(-127))
    );
    assert!(close(p.sog_kn, 12.3, 1e-9) && p.accuracy);
    assert!(close(p.lon, -123.395_383_3, 1e-6) && close(p.lat, 48.381_633_3, 1e-6));
    assert!(close(p.cog_deg, 224.0, 1e-9));
    assert_eq!((p.heading_deg, p.second, p.raim), (Some(215), 33, false));

    let AisMessage::Position(p) = &m[1] else {
        panic!()
    };
    assert_eq!(
        (p.mmsi, p.rot_raw, p.heading_deg, p.second),
        (440_348_000, None, None, 13)
    );
    assert!(close(p.sog_kn, 0.0, 1e-9) && !p.accuracy);
    assert!(
        close(p.lon, -70.7582, 1e-6)
            && close(p.lat, 43.08015, 1e-6)
            && close(p.cog_deg, 93.4, 1e-9)
    );

    let AisMessage::Position(p) = &m[2] else {
        panic!()
    };
    assert_eq!(
        (p.msg_type, p.mmsi, p.rot_raw, p.heading_deg, p.second),
        (2, 356_302_000, Some(127), Some(91), 41)
    );
    assert!(close(p.sog_kn, 13.9, 1e-9) && close(p.cog_deg, 87.7, 1e-9));
    assert!(close(p.lon, -71.626_143_3, 1e-6) && close(p.lat, 40.392_358_3, 1e-6));

    let AisMessage::Position(p) = &m[3] else {
        panic!()
    };
    assert_eq!(
        (p.msg_type, p.mmsi, p.nav_status, p.rot_raw),
        (3, 563_808_000, Some(5), Some(0))
    );
    assert!(close(p.lon, -76.327_533_3, 1e-6) && close(p.lat, 36.91, 1e-6));
    assert!(close(p.cog_deg, 252.0, 1e-9) && p.accuracy);
    assert_eq!((p.heading_deg, p.second), (Some(352), 35));
    assert_eq!(ais::nav_status_text(5), "moored");
}

#[test]
fn gpsd_type5_static_and_voyage() {
    let m = gpsd_messages();
    let AisMessage::StaticVoyage(s) = &m[4] else {
        panic!("{:?}", m[4])
    };
    assert_eq!(s.mmsi, 351_759_000);
    assert_eq!(s.imo, 9_134_270);
    assert_eq!(s.callsign, "3FOF8");
    assert_eq!(s.name, "EVER DIADEM");
    assert_eq!(s.ship_type, 70);
    assert_eq!(
        (s.to_bow, s.to_stern, s.to_port, s.to_starboard),
        (225, 70, 1, 31)
    );
    assert_eq!(s.epfd, 1);
    // gpsd: "eta":"05-15T14:00Z".
    assert_eq!(s.eta, (5, 15, 14, 0));
    assert!((s.draught_m - 12.2).abs() < 1e-9);
    assert_eq!(s.destination, "NEW YORK");
    assert!(!s.dte);
}

#[test]
fn gpsd_class_b() {
    let m = gpsd_messages();
    let AisMessage::Position(p) = &m[5] else {
        panic!("{:?}", m[5])
    };
    assert_eq!(
        (p.msg_type, p.mmsi, p.heading_deg, p.second),
        (18, 338_087_471, None, 49)
    );
    assert!(close(p.sog_kn, 0.1, 1e-9) && !p.accuracy && p.raim && p.class_b_cs == Some(true));
    assert!(
        close(p.lon, -74.072_131_7, 1e-6)
            && close(p.lat, 40.68454, 1e-6)
            && close(p.cog_deg, 79.6, 1e-9)
    );

    let AisMessage::Position(p) = &m[6] else {
        panic!()
    };
    assert_eq!((p.mmsi, p.second), (338_088_483, 20));
    assert!(
        close(p.lon, -70.811_196_7, 1e-6)
            && close(p.lat, 43.115_558_3, 1e-6)
            && close(p.cog_deg, 171.6, 1e-9)
    );

    let AisMessage::Position(p) = &m[7] else {
        panic!()
    };
    assert_eq!((p.mmsi, p.second), (368_161_000, 17));
    assert!(close(p.sog_kn, 5.1, 1e-9) && p.accuracy && close(p.cog_deg, 34.9, 1e-9));
    assert!(close(p.lon, -72.233_848_3, 1e-6) && close(p.lat, 39.480_925, 1e-6));

    let AisMessage::ClassBExtended(e) = &m[8] else {
        panic!("{:?}", m[8])
    };
    assert_eq!(e.position.mmsi, 367_059_850);
    assert!(close(e.position.sog_kn, 8.7, 1e-9) && close(e.position.cog_deg, 335.9, 1e-9));
    assert!(close(e.position.lon, -88.810_391_7, 1e-6) && close(e.position.lat, 29.543_695, 1e-6));
    assert_eq!((e.position.heading_deg, e.position.second), (None, 46));
    assert_eq!(e.name, "CAPT.J.RIMES");
    assert_eq!(e.ship_type, 70);
    assert_eq!(
        (e.to_bow, e.to_stern, e.to_port, e.to_starboard, e.epfd),
        (5, 21, 4, 4, 1)
    );
}

#[test]
fn gpsd_type24_parts_and_collisions() {
    let m = gpsd_messages();
    assert!(
        matches!(&m[9], AisMessage::StaticA { mmsi: 271_041_815, name, .. } if name == "PROGUY")
    );
    let AisMessage::StaticB(b) = &m[10] else {
        panic!("{:?}", m[10])
    };
    assert_eq!((b.mmsi, b.ship_type), (271_041_815, 60));
    assert_eq!(b.callsign, "TC6163");
    assert!(b.vendor_id.starts_with("1D0"));
    assert_eq!(
        (b.to_bow, b.to_stern, b.to_port, b.to_starboard),
        (0, 15, 0, 5)
    );
    // The collision section: parts arrive alone and out of step; each still decodes.
    assert!(
        matches!(&m[13], AisMessage::StaticA { mmsi: 271_040_660, name, .. } if name == "GOZDEM-1")
    );
    let AisMessage::StaticB(b) = &m[15] else {
        panic!("{:?}", m[15])
    };
    assert_eq!(
        (b.mmsi, b.ship_type, b.callsign.as_str()),
        (271_040_660, 37, "YM5504")
    );
    assert_eq!(
        (b.to_bow, b.to_stern, b.to_port, b.to_starboard),
        (0, 24, 0, 6)
    );
}

#[test]
fn gpsd_channel_multiplexing() {
    // Two type 5 messages interleaved on channels A and B, sequence ids 6 and 2.
    let m = gpsd_messages();
    assert_eq!(m.len(), 18);
    for s in &m[16..18] {
        let AisMessage::StaticVoyage(s) = s else {
            panic!("{s:?}")
        };
        assert_eq!(s.mmsi, 271_010_059);
        assert_eq!(s.name, "HEALTH CONTROL 13");
        assert_eq!(s.callsign, "TCA2350");
        assert_eq!(s.ship_type, 55);
        assert_eq!(
            (s.to_bow, s.to_stern, s.to_port, s.to_starboard),
            (6, 10, 2, 2)
        );
        assert!((s.draught_m - 2.0).abs() < 1e-9);
        // gpsd: "eta":"00-00T24:60Z", not available.
        assert_eq!(s.eta, (0, 0, 24, 60));
        assert_eq!(s.destination, "");
    }
}

#[test]
fn tag_block_from_the_aivdm_document() {
    // The tag block example in gpsd's AIVDM.adoc ("NMEA Tag Blocks"; same project and licence).
    let line = r"\g:1-2-73874,n:157036,s:r003669945,c:1241544035*4A\!AIVDM,1,1,,B,15N4cJ`005Jrek0H@9n`DW5608EP,0*13";
    let (tag, s) = nmea::split_line(line).expect("framing");
    let tag = tag.expect("tag block");
    assert_eq!(tag.source.as_deref(), Some("r003669945"));
    assert_eq!(tag.time_ms, Some(1_241_544_035_000));
    assert_eq!(tag.group, Some((1, 2, 73874)));
    assert_eq!(tag.line, Some(157_036));
    let s = nmea::parse_vdm(s, true).expect("checksum");
    let msg = ais::decode(&s.payload, s.fill).expect("payload");
    assert!(matches!(msg, AisMessage::Position(ref p) if p.msg_type == 1));
    // A corrupted tag block is refused.
    let bad = line.replace("c:1241544035", "c:1241544036");
    assert_eq!(
        nmea::split_line(&bad).map(|_| ()),
        Err(NmeaError::BadTagChecksum)
    );
    // Milliseconds in c: are accepted too.
    let tb = nmea::parse_tag_block(&format!(
        "s:x,c:1781456400123*{:02X}",
        nmea::checksum("s:x,c:1781456400123")
    ))
    .expect("tag");
    assert_eq!(tb.time_ms, Some(1_781_456_400_123));
}

#[test]
fn malformed_and_partial_nmea() {
    let mut tr = Tracker::new(Config::default());
    let ok = "!AIVDM,1,1,,A,15RTgt0PAso;90TKcjM8h6g208CQ,0*4A";
    assert!(matches!(tr.ingest_nmea("", 0), Outcome::Error(_)));
    assert!(matches!(
        tr.ingest_nmea("garbage without a sentence", 0),
        Outcome::Error(_)
    ));
    assert!(matches!(
        tr.ingest_nmea("!AIVDM,1,1,,A,15RTgt0PAso;90TKcjM8h6g208CQ,0*4B", 0),
        Outcome::Error(_)
    ));
    assert!(matches!(
        tr.ingest_nmea("!AIVDM,1,1,,A,15RTgt0PAso;90TK", 0),
        Outcome::Error(_)
    ));
    assert!(matches!(
        tr.ingest_nmea("!AIVDM,1,1,,A", 0),
        Outcome::Error(_)
    ));
    // Two bad or missing checksums, and one line with too few fields.
    assert_eq!(tr.stats().nmea_checksum_errors, 3);
    // Leading noise and a trailing CR are tolerated.
    assert_eq!(
        tr.ingest_nmea(&format!("12:00:01 {ok}\r\n"), 1_000),
        Outcome::Fused
    );
    // A second fragment with no first, and a first whose second never comes.
    let f1 = "!AIVDM,2,1,1,A,55?MbV02;H;s<HtKR20EHE:0@T4@Dn2222222216L961O5Gf0NSQEp6ClRp8,0*1C";
    let f2 = "!AIVDM,2,2,1,A,88888888880,2*25";
    assert!(matches!(tr.ingest_nmea(f2, 2_000), Outcome::Error(_)));
    assert_eq!(tr.ingest_nmea(f1, 3_000), Outcome::Pending);
    assert_eq!(tr.ingest_nmea(f1, 3_100), Outcome::Pending);
    assert_eq!(tr.ingest_nmea(f2, 3_200), Outcome::Fused);
    assert!(tr.stats().nmea_fragments_dropped >= 2);
    // A fragment too late is refused.
    assert_eq!(tr.ingest_nmea(f1, 10_000), Outcome::Pending);
    assert!(matches!(tr.ingest_nmea(f2, 20_000), Outcome::Error(_)));
    // A bad armour character.
    let body = "AIVDM,1,1,,A,15RTgt0P{so;90TKcjM8h6g208CQ,0";
    let line = format!("!{body}*{:02X}", nmea::checksum(body));
    assert!(matches!(tr.ingest_nmea(&line, 30_000), Outcome::Error(_)));
    // VDO is the receiver's own vessel, not traffic.
    let body = "AIVDO,1,1,,A,15RTgt0PAso;90TKcjM8h6g208CQ,0";
    let line = format!("!{body}*{:02X}", nmea::checksum(body));
    assert!(matches!(tr.ingest_nmea(&line, 31_000), Outcome::Ignored(_)));
    // A short payload of a known type is an error, not a panic.
    let body = "AIVDM,1,1,,A,15RTgt0P,0";
    let line = format!("!{body}*{:02X}", nmea::checksum(body));
    assert!(matches!(tr.ingest_nmea(&line, 32_000), Outcome::Error(_)));
}

#[test]
fn mmsi_validity() {
    let mut tr = Tracker::new(Config::default());
    tr.ingest_nmea("!AIVDM,1,1,,A,15RTgt0PAso;90TKcjM8h6g208CQ,0*4A", 1_000);
    tr.update(2_000);
    assert!(!tr.tracks()[0].has_flag(Flag::InvalidMmsi));
    assert!(ais::mmsi_valid(371_798_000));
    assert!(!ais::mmsi_valid(123_456_789));
}

#[test]
fn sbs_examples() {
    let lines: Vec<&str> = SBS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 17);
    let p = |i: usize| sbs::parse(lines[i], 0).expect("parses");

    let sel = p(0);
    assert_eq!(sel.kind, SbsKind::Sel);
    assert_eq!(sel.transmission, None);
    assert_eq!(sel.session_id.as_deref(), Some("496"));
    assert_eq!(sel.aircraft_id.as_deref(), Some("2286"));
    assert_eq!(sel.icao, Some(0x4CA4E5));
    assert_eq!(sel.flight_id.as_deref(), Some("27215"));
    assert_eq!(sel.generated_ms, Some(1_266_602_767_710));
    assert_eq!(sel.logged_ms, Some(1_266_602_767_710));
    assert_eq!(sel.callsign.as_deref(), Some("RYR1427"));
    assert_eq!(sel.altitude_ft, None);

    assert_eq!(p(1).kind, SbsKind::Id);
    assert_eq!(p(1).callsign.as_deref(), Some("EZY691A"));
    assert_eq!(p(2).kind, SbsKind::Air);
    assert_eq!(
        (p(3).kind, p(3).callsign.as_deref()),
        (SbsKind::Sta, Some("RM"))
    );
    assert_eq!((p(4).kind, p(4).icao), (SbsKind::Clk, None));

    let m1 = p(5);
    assert_eq!(
        (m1.transmission, m1.callsign.as_deref()),
        (Some(1), Some("RJA1118"))
    );
    // The MSG,2 example has one field too many, which puts 258.3 in the latitude: no position.
    let m2 = p(6);
    assert_eq!(m2.transmission, Some(2));
    assert_eq!((m2.lat, m2.lon), (None, None));
    let m3 = p(7);
    assert_eq!((m3.transmission, m3.icao), (Some(3), Some(0x4CA2D6)));
    assert_eq!(m3.altitude_ft, Some(37_000.0));
    assert_eq!((m3.lat, m3.lon), (Some(51.45735), Some(-1.02826)));
    assert_eq!(m3.generated_ms, Some(1_227_884_030_594));
    assert_eq!(
        (m3.alert, m3.emergency, m3.spi, m3.on_ground),
        (Some(false), Some(false), Some(false), Some(false))
    );
    let m4 = p(8);
    assert_eq!(
        (m4.ground_speed_kt, m4.track_deg, m4.vertical_rate_fpm),
        (Some(288.6), Some(103.2), Some(-832.0))
    );
    let m5 = p(9);
    assert_eq!(
        (m5.altitude_ft, m5.alert, m5.emergency, m5.on_ground),
        (Some(10_000.0), Some(false), None, Some(false))
    );
    let m6 = p(10);
    assert_eq!(
        (m6.altitude_ft, m6.squawk.as_deref()),
        (Some(33_325.0), Some("0271"))
    );
    assert_eq!(
        (p(11).transmission, p(11).altitude_ft),
        (Some(7), Some(3_775.0))
    );
    assert_eq!(
        (p(12).transmission, p(12).on_ground),
        (Some(8), Some(false))
    );

    // node-sbs1's own parse tests.
    let a = p(13);
    assert_eq!(
        (a.icao, a.session_id.clone(), a.callsign.as_deref()),
        (Some(0xAD18DA), None, Some("258"))
    );
    assert_eq!(
        (a.alert, a.emergency, a.spi, a.on_ground),
        (Some(false), Some(false), Some(false), Some(false))
    );
    let b = p(14);
    assert_eq!(
        (b.altitude_ft, b.lat, b.generated_ms),
        (Some(5_950.0), None, None)
    );
    let c = p(15);
    assert_eq!(
        (c.altitude_ft, c.lat, c.lon),
        (Some(5_925.0), Some(34.01839), Some(-118.35263))
    );
    let d = p(16);
    assert_eq!(
        (d.alert, d.emergency, d.spi, d.on_ground),
        (Some(true), None, Some(true), Some(true))
    );
}

#[test]
fn sbs_malformed_and_non_icao() {
    assert!(sbs::parse("", 0).is_err());
    assert!(sbs::parse("XYZ,1,2,3", 0).is_err());
    assert!(sbs::parse("MSG,3,1,1", 0).is_err());
    assert!(sbs::parse("MSG,3,1,1,ZZZZZZ,1,,,,,,1000,,,37.7,-122.4,,,0,0,0,0", 0).is_err());
    assert!(sbs::parse("MSG,3,1,1,A1B2C3,1,,,,,,abc,,,37.7,-122.4,,,0,0,0,0", 0).is_err());
    let m = sbs::parse("MSG,3,1,1,~A1B2C3,1,,,,,,1000,,,37.7,-122.4,,,0,0,0,0", 0).expect("parses");
    assert!(m.non_icao);
    assert_eq!(m.icao, Some(0xA1B2C3));
    // Local time with an offset: 17:00 at UTC-7 is 00:00 UTC the next day.
    assert_eq!(
        sbs::parse_datetime("2026/06/13", "17:00:00.000", -420),
        Some(1_781_395_200_000)
    );
}

#[test]
fn json_lines_of_section_5() {
    let rid = r#"{"type":"rid","rx":"hub-pier","t_rx":1781456400123,"t":1781456400050,"uas_id":"1581F5FKD229400B5E1J",
 "id_type":"serial","ua_type":"multirotor","status":"airborne","lat":37.8075,"lon":-122.4312,
 "alt_geo_m":96.0,"alt_baro_m":94.5,"height_agl_m":90.0,"speed_mps":6.2,"track_deg":271.0,
 "vspeed_mps":0.0,"h_acc_m":3.0,"v_acc_m":4.0,"op_lat":37.8069,"op_lon":-122.4288}"#
        .replace('\n', "");
    let JsonLine::Rid(r) = wire::parse(&rid).expect("rid") else {
        panic!()
    };
    assert_eq!(
        (r.rx.as_str(), r.t_rx, r.t),
        ("hub-pier", 1_781_456_400_123, Some(1_781_456_400_050))
    );
    assert_eq!(r.uas_id, "1581F5FKD229400B5E1J");
    assert_eq!(
        (r.lat, r.alt_geo_m, r.speed_mps, r.op_lon),
        (Some(37.8075), Some(96.0), Some(6.2), Some(-122.4288))
    );

    let det = r#"{"type":"det","sensor":"daa","platform":"12","t":1781456400200,"sensor_lat":37.80,"sensor_lon":-122.45,
 "sensor_alt_m":90.0,"bearing_deg":41.5,"sigma_bearing_deg":2.0,"elev_deg":3.1,"sigma_elev_deg":2.0,
 "range_m":null,"sigma_range_m":null,"radial_speed_mps":null,"size":"medium"}"#
        .replace('\n', "");
    let JsonLine::Det(d) = wire::parse(&det).expect("det") else {
        panic!()
    };
    assert_eq!(
        (d.sensor, d.platform.as_str(), d.t),
        (SensorKind::Daa, "12", 1_781_456_400_200)
    );
    assert_eq!(
        (d.bearing_deg, d.elev_deg, d.range_m),
        (Some(41.5), Some(3.1), None)
    );
    // A numeric platform id is accepted as text.
    let JsonLine::Det(d2) =
        wire::parse(&det.replace("\"platform\":\"12\"", "\"platform\":12")).expect("det")
    else {
        panic!()
    };
    assert_eq!(d2.platform, "12");
    assert!(wire::parse(r#"{"type":"det","sensor":"sonar","platform":"1","t":1}"#).is_err());
    assert!(wire::parse("{not json").is_err());

    let mut tr = Tracker::new(Config::default());
    assert_eq!(tr.ingest_json(&rid), Outcome::Fused);
    assert_eq!(tr.ingest_json(&det), Outcome::Pending);
    tr.update(1_781_456_400_300);
    let v = serde_json::to_value(tr.tracks()).expect("json");
    let drone = v
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|t| t["ids"]["uas_id"] == "1581F5FKD229400B5E1J")
        })
        .expect("rid track");
    assert_eq!(drone["class"], "drone");
    assert_eq!(drone["alt_ref"], "geo");
    assert_eq!(drone["static"]["ua_type"], "multirotor");
    assert_eq!(drone["static"]["op_lat"], 37.8069);
    let line = v
        .as_array()
        .and_then(|a| a.iter().find(|t| t["bearing_only"].is_object()))
        .expect("bearing-only track");
    assert_eq!(line["bearing_only"]["bearing_deg"], 41.5);
    assert_eq!(line["cooperative"], false);
}

#[test]
fn track_json_matches_section_5() {
    let mut tr = Tracker::new(Config::default());
    let l1 =
        "MSG,1,1,1,A1B2C3,1,2026/06/14,17:00:00.000,2026/06/14,17:00:00.050,UAL123,,,,,,,,0,0,0,0";
    let l2 = "MSG,3,1,1,A1B2C3,1,2026/06/14,17:00:00.100,2026/06/14,17:00:00.150,,2953,,,37.70000,-122.35000,,,0,0,0,0";
    let l3 = "MSG,4,1,1,A1B2C3,1,2026/06/14,17:00:00.200,2026/06/14,17:00:00.250,,,140.2,236.3,,,-787,,0,0,0,0";
    let l4 = "MSG,3,1,1,A1B2C3,1,2026/06/14,17:00:01.100,2026/06/14,17:00:01.150,,2940,,,37.69980,-122.35068,,,0,0,0,0";
    for (l, t) in [(l1, 50), (l2, 150), (l3, 250), (l4, 1150)] {
        tr.ingest_sbs(l, 1_781_456_400_000 + t);
    }
    tr.update(1_781_456_401_250);
    let s = serde_json::to_string(&tr.tracks()[0]).expect("json");
    let keys = [
        "\"id\"",
        "\"t\"",
        "\"domain\"",
        "\"class\"",
        "\"cooperative\"",
        "\"ids\"",
        "\"lat\"",
        "\"lon\"",
        "\"alt_m\"",
        "\"alt_ref\"",
        "\"h_sigma_m\"",
        "\"v_sigma_m\"",
        "\"vel_mps\"",
        "\"vs_mps\"",
        "\"heading_deg\"",
        "\"state\"",
        "\"sources\"",
        "\"flags\"",
        "\"static\"",
    ];
    let mut at = 0;
    for k in keys {
        let i = s[at..]
            .find(k)
            .unwrap_or_else(|| panic!("{k} missing or out of order in {s}"));
        at += i + k.len();
    }
    assert!(!s.contains("bearing_only"), "{s}");
    let v: serde_json::Value = serde_json::from_str(&s).expect("json");
    assert_eq!(v["id"], "trk-000001");
    assert_eq!(v["domain"], "air");
    assert_eq!(v["class"], "airliner");
    assert_eq!(
        v["ids"],
        serde_json::json!({"icao24":"a1b2c3","callsign":"UAL123","anonymous":false})
    );
    assert_eq!(v["alt_ref"], "baro");
    assert_eq!(v["state"], "confirmed");
    assert_eq!(
        v["sources"],
        serde_json::json!([{"kind":"adsb_1090","rx":"1","last_t":1_781_456_401_100i64}])
    );
    assert_eq!(v["flags"], serde_json::json!([]));
    assert_eq!(v["static"], serde_json::json!({}));
    let heading = v["heading_deg"].as_f64().expect("heading");
    assert!((heading - 236.3).abs() < 3.0, "{heading}");
    // And back.
    let back: fleetmesh_tracker::Track = serde_json::from_str(&s).expect("parse back");
    assert_eq!(back.id, "trk-000001");
}
