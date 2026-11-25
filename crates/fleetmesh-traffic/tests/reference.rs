//! The encoders against published reference examples, not only against themselves.
//!
//! AIS: real-world sentences and their decoded field values from the test suite of pyais
//! (https://github.com/M0r13n/pyais, tests/test_decode.py, tests/test_encode.py and
//! tests/test_tag_block.py), which is under the MIT licence:
//!
//!   Copyright (c) 2019 M0r13n
//!
//!   Permission is hereby granted, free of charge, to any person obtaining a copy of this
//!   software and associated documentation files (the "Software"), to deal in the Software
//!   without restriction, including without limitation the rights to use, copy, modify, merge,
//!   publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
//!   to whom the Software is furnished to do so, subject to the following conditions:
//!
//!   The above copyright notice and this permission notice shall be included in all copies or
//!   substantial portions of the Software.
//!
//!   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
//!   INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
//!   PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
//!   FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
//!   OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//!   DEALINGS IN THE SOFTWARE.
//!
//! For each sentence: decode its payload with the small decoder below, check the decoded values
//! pyais publishes (which checks the bit layout and scaling independently of our encoder), then
//! encode those fields with this crate and require the very same sentence, checksum included.
//! The gpsd AIVDM document states no licence, so nothing is copied from it.
//!
//! SBS: dump1090-fa's and readsb's `modesSendSBSOutput` (net_io.c), the reference
//! implementations of port 30003, fix the field order and formats checked here.
#![allow(clippy::unwrap_used)]

mod common;
use common::Rd;

use fleetmesh_traffic::encode::ais::{self, ExtendedB, PositionA, PositionB, StaticA, StaticB, StaticVoyage};
use fleetmesh_traffic::encode::sbs::{SbsMsg, pad_callsign};

fn trimmed(s: &str) -> &str {
    s.trim_end_matches(['@', ' '])
}

fn payload(sentence: &str) -> &str {
    sentence.split(',').nth(5).unwrap()
}

fn one(bits: &ais::Bits, channel: char) -> String {
    let v = ais::sentences(bits, channel, 0, None, 60);
    assert_eq!(v.len(), 1);
    v[0].trim_end().to_string()
}

#[test]
fn type_1_matches_a_real_sentence_from_oakland() {
    let s = "!AIVDM,1,1,,B,15M67FC000G?ufbE`FepT@3n00Sa,0*5C";
    let mut r = Rd::new(payload(s));
    let (ty, rep, mmsi, st, rot, sog, acc) = (r.u(6), r.u(2), r.u(30), r.u(4), r.i(8), r.u(10), r.b());
    let (lon, lat, cog, hdg, sec, man, _spare, raim, radio) = (r.i(28), r.i(27), r.u(12), r.u(9), r.u(6), r.u(2), r.u(3), r.b(), r.u(19));
    // pyais: mmsi 366053209, restricted manoeuvrability, turn 0, speed 0.0, accuracy false,
    // lon -122.341618, lat 37.802118, course 219.3, heading 1, second 59, raim false, radio 2281
    assert_eq!((ty, rep, mmsi, st, rot, sog, acc), (1, 0, 366053209, 3, 0, 0, false));
    assert!((lon as f64 / 600_000.0 - -122.341618).abs() < 1e-6);
    assert!((lat as f64 / 600_000.0 - 37.802118).abs() < 1e-6);
    assert_eq!((cog, hdg, sec, man, raim, radio), (2193, 1, 59, 0, false, 2281));
    let m = PositionA {
        msg_type: 1,
        repeat: 0,
        mmsi: mmsi as u32,
        status: st as u8,
        rot,
        sog: ais::sog_field(Some(0.0)),
        accuracy: acc,
        lon: Some(lon as f64 / 600_000.0),
        lat: Some(lat as f64 / 600_000.0),
        cog: ais::cog_field(Some(219.3)),
        heading: 1,
        second: 59,
        maneuver: 0,
        raim,
        radio: radio as u32,
    };
    assert_eq!(one(&m.bits(), 'B'), s);
}

#[test]
fn types_2_and_3_match_real_sentences() {
    for (s, want_type) in [("!AIVDM,1,1,,A,23aFfl0P00PCR?0MEB@h0?w020S7,0*68", 2), ("!AIVDM,1,1,,A,35NSH95001G?wopE`beasVk@0E5:,0*6F", 3)] {
        let mut r = Rd::new(payload(s));
        let m = PositionA {
            msg_type: r.u(6) as u8,
            repeat: r.u(2) as u8,
            mmsi: r.u(30) as u32,
            status: r.u(4) as u8,
            rot: r.i(8),
            sog: r.u(10),
            accuracy: r.b(),
            lon: Some(r.i(28) as f64 / 600_000.0),
            lat: Some(r.i(27) as f64 / 600_000.0),
            cog: r.u(12),
            heading: r.u(9) as u16,
            second: r.u(6) as u8,
            maneuver: r.u(2) as u8,
            raim: {
                r.u(3);
                r.b()
            },
            radio: r.u(19) as u32,
        };
        assert_eq!(m.msg_type, want_type);
        if want_type == 3 {
            // pyais: mmsi 367581220, moored, turn 0, speed 0.1, accuracy 0, lat 37.8107,
            // lon -122.3343, course 254.2, heading 217, second 40, raim false
            assert_eq!((m.mmsi, m.status, m.rot, m.sog, m.accuracy), (367581220, 5, 0, 1, false));
            assert!((m.lat.unwrap() - 37.8107).abs() < 5e-5 && (m.lon.unwrap() - -122.3343).abs() < 5e-5);
            assert_eq!((m.cog, m.heading, m.second, m.raim), (2542, 217, 40, false));
        }
        assert_eq!(one(&m.bits(), 'A'), s);
    }
}

#[test]
fn type_5_matches_a_real_two_sentence_message() {
    let s1 = "!AIVDM,2,1,1,A,55?MbV02;H;s<HtKR20EHE:0@T4@Dn2222222216L961O5Gf0NSQEp6ClRp8,0*1C";
    let s2 = "!AIVDM,2,2,1,A,88888888880,2*25";
    let mut r = Rd::new(&format!("{}{}", payload(s1), payload(s2)));
    let (ty, rep, mmsi, ver, imo) = (r.u(6), r.u(2), r.u(30), r.u(2), r.u(30));
    let (callsign, name) = (r.text(7), r.text(20));
    let (ship_type, bow, stern, port, stbd, epfd) = (r.u(8), r.u(9), r.u(9), r.u(6), r.u(6), r.u(4));
    let (mo, d, h, mi, draught) = (r.u(4), r.u(5), r.u(5), r.u(6), r.u(8));
    let dest = r.text(20);
    let dte = r.b();
    // pyais: callsign 3FOF8, EVER DIADEM, cargo (70), 225/70/1/31, GPS, draught 12.2,
    // NEW YORK, dte 0
    assert_eq!((ty, trimmed(&callsign), trimmed(&name), ship_type), (5, "3FOF8", "EVER DIADEM", 70));
    assert_eq!((bow, stern, port, stbd, epfd, draught, trimmed(&dest), dte), (225, 70, 1, 31, 1, 122, "NEW YORK", false));
    let m = StaticVoyage {
        repeat: rep as u8,
        mmsi: mmsi as u32,
        ais_version: ver as u8,
        imo: imo as u32,
        callsign,
        name,
        ship_type: ship_type as u8,
        to_bow: bow as u16,
        to_stern: stern as u16,
        to_port: port as u8,
        to_starboard: stbd as u8,
        epfd: epfd as u8,
        eta_month: mo as u8,
        eta_day: d as u8,
        eta_hour: h as u8,
        eta_minute: mi as u8,
        draught_dm: draught as u8,
        destination: dest,
        dte,
    };
    let v = ais::sentences(&m.bits(), 'A', 1, None, 60);
    assert_eq!(v.iter().map(|l| l.trim_end()).collect::<Vec<_>>(), vec![s1, s2]);
}

#[test]
fn type_18_matches_a_real_class_b_report() {
    let s = "!AIVDM,1,1,,A,B5NJ;PP005l4ot5Isbl03wsUkP06,0*76";
    let mut r = Rd::new(payload(s));
    let (ty, rep, mmsi, _res) = (r.u(6), r.u(2), r.u(30), r.u(8));
    let (sog, acc, lon, lat, cog, hdg, sec, _res2) = (r.u(10), r.b(), r.i(28), r.i(27), r.u(12), r.u(9), r.u(6), r.u(2));
    let (cs, display, dsc, band, msg22, assigned, raim) = (r.b(), r.b(), r.b(), r.b(), r.b(), r.b(), r.b());
    let radio = r.u(20);
    // pyais: mmsi 367430530, speed 0.0, accuracy 0, lat 37.79, lon -122.27, course 0, heading
    // 511, second 55, cs 1, display 0, dsc 1, band 1, msg22 1, not assigned, raim false
    assert_eq!((ty, mmsi, sog, acc, cog, hdg, sec), (18, 367430530, 0, false, 0, 511, 55));
    assert!((lat as f64 / 600_000.0 - 37.79).abs() < 0.005 && (lon as f64 / 600_000.0 - -122.27).abs() < 0.005);
    assert_eq!((cs, display, dsc, band, msg22, assigned, raim), (true, false, true, true, true, false, false));
    let m = PositionB {
        repeat: rep as u8,
        mmsi: mmsi as u32,
        sog,
        accuracy: acc,
        lon: Some(lon as f64 / 600_000.0),
        lat: Some(lat as f64 / 600_000.0),
        cog,
        heading: hdg as u16,
        second: sec as u8,
        cs,
        display,
        dsc,
        band,
        msg22,
        assigned,
        raim,
        radio: radio as u32,
    };
    assert_eq!(one(&m.bits(), 'A'), s);
}

#[test]
fn type_19_matches_a_real_extended_class_b_report() {
    let s = "!AIVDM,1,1,,B,C5N3SRgPEnJGEBT>NhWAwwo862PaLELTBJ:V00000000S0D:R220,0*0B";
    let mut r = Rd::new(payload(s));
    let (ty, rep, mmsi, res) = (r.u(6), r.u(2), r.u(30), r.u(8));
    let (sog, acc, lon, lat, cog, hdg, sec, res2) = (r.u(10), r.b(), r.i(28), r.i(27), r.u(12), r.u(9), r.u(6), r.u(4));
    let name = r.text(20);
    let (ship_type, bow, stern, port, stbd, epfd, raim, dte, assigned) = (r.u(8), r.u(9), r.u(9), r.u(6), r.u(6), r.u(4), r.b(), r.b(), r.b());
    // pyais: mmsi 367059850, speed 8.7, accuracy 0, lat 29.543695, course 335.9, heading 511,
    // second 46, CAPT.J.RIMES, cargo, 5/21/4/4, GPS, dte 0, not assigned
    assert_eq!((ty, mmsi, sog, acc, cog, hdg, sec), (19, 367059850, 87, false, 3359, 511, 46));
    assert!((lat as f64 / 600_000.0 - 29.543695).abs() < 1e-6);
    assert_eq!((trimmed(&name), ship_type, bow, stern, port, stbd, epfd, dte, assigned), ("CAPT.J.RIMES", 70, 5, 21, 4, 4, 1, false, false));
    let m = ExtendedB {
        repeat: rep as u8,
        mmsi: mmsi as u32,
        // this transponder fills the regional bits
        regional: (res as u8, res2 as u8),
        sog,
        accuracy: acc,
        lon: Some(lon as f64 / 600_000.0),
        lat: Some(lat as f64 / 600_000.0),
        cog,
        heading: hdg as u16,
        second: sec as u8,
        name,
        ship_type: ship_type as u8,
        to_bow: bow as u16,
        to_stern: stern as u16,
        to_port: port as u8,
        to_starboard: stbd as u8,
        epfd: epfd as u8,
        raim,
        dte,
        assigned,
    };
    assert_eq!(one(&m.bits(), 'B'), s);
}

#[test]
fn type_24_parts_a_and_b_match_published_sentences() {
    // part A, 168 bits (M.1371-5): pyais decodes mmsi 123456789, part 0
    let a = "!AIVDO,1,1,,A,H1mg=5@EP4m0hF1<PU0000000000,0*45";
    let mut r = Rd::new(payload(a));
    let (ty, rep, mmsi, part) = (r.u(6), r.u(2), r.u(30), r.u(2));
    let name = r.text(20);
    assert_eq!((ty, mmsi, part), (24, 123456789, 0));
    let m = StaticA { repeat: rep as u8, mmsi: mmsi as u32, name };
    assert_eq!(one(&m.bits(), 'A'), a.replace("AIVDO", "AIVDM").replace("*45", &format!("*{}", ais::checksum(&a[1..a.len() - 3].replace("AIVDO", "AIVDM")))));
    // pyais's own encoder: mmsi 338091445, "HMS FooBar" (sent upper case)
    let m = StaticA { repeat: 0, mmsi: 338091445, name: "HMS FooBar".into() };
    assert_eq!(ais::sentences(&m.bits(), 'A', 0, None, 60)[0].trim_end(), "!AIVDM,1,1,,A,H52KMe@Pm>0Htt85800000000000,0*1A".replace("*1A", &format!("*{}", ais::checksum("AIVDM,1,1,,A,H52KMe@Pm>0Htt85800000000000,0"))));
    assert_eq!(payload(&ais::sentences(&m.bits(), 'A', 0, None, 60)[0]), "H52KMe@Pm>0Htt85800000000000");
    // part B: pyais decodes mmsi 368008030, part 1, pleasure craft, vendor SRT, WDJ7574, 6/19/3/3
    let b = "!ANVDM,1,1,,B,H5NuKGTUCBD8SaUG4:omol0hC33t,0*54";
    let mut r = Rd::new(payload(b));
    let (ty, rep, mmsi, part, ship_type) = (r.u(6), r.u(2), r.u(30), r.u(2), r.u(8));
    let (vendor, model, serial) = (r.text(3), r.u(4), r.u(20));
    let callsign = r.text(7);
    let (bow, stern, port, stbd, epfd) = (r.u(9), r.u(9), r.u(6), r.u(6), r.u(4));
    assert_eq!((ty, mmsi, part, ship_type, vendor.as_str(), trimmed(&callsign)), (24, 368008030, 1, 37, "SRT", "WDJ7574"));
    assert_eq!((bow, stern, port, stbd), (6, 19, 3, 3));
    let m = StaticB {
        repeat: rep as u8,
        mmsi: mmsi as u32,
        ship_type: ship_type as u8,
        vendor,
        model: model as u8,
        serial: serial as u32,
        callsign,
        to_bow: bow as u16,
        to_stern: stern as u16,
        to_port: port as u8,
        to_starboard: stbd as u8,
        epfd: epfd as u8,
    };
    assert_eq!(payload(&one(&m.bits(), 'B')), payload(b));
}

#[test]
fn tag_blocks_carry_their_own_checksum() {
    // pyais tests/test_tag_block.py: \s:2573535,c:1671533231*08\ and \s:2573545,c:1671533231*0F\
    let mut bits = ais::Bits::new();
    bits.u(6, 1).u(162, 0);
    let l = &ais::sentences(&bits, 'A', 0, Some(("2573535", 1671533231)), 60)[0];
    assert!(l.starts_with("\\s:2573535,c:1671533231*08\\!AIVDM,"), "{l}");
    let l = &ais::sentences(&bits, 'A', 0, Some(("2573545", 1671533231)), 60)[0];
    assert!(l.starts_with("\\s:2573545,c:1671533231*0F\\!AIVDM,"), "{l}");
}

#[test]
fn sbs_lines_have_dump1090s_fields_and_formats() {
    // 2026-06-14T17:00:00.050Z generated, .123 logged
    let base = SbsMsg { session: 3, icao: 0xA1B2C3, t_gen_ms: 1_781_456_400_050, t_log_ms: 1_781_456_400_123, ground: Some(false), ..Default::default() };
    let pos = SbsMsg { msg_type: 3, alt_ft: Some(3500), lat: Some(37.7), lon: Some(-122.35), ..base.clone() };
    assert_eq!(pos.line(), "MSG,3,3,1,A1B2C3,1,2026/06/14,17:00:00.050,2026/06/14,17:00:00.123,,3500,,,37.70000,-122.35000,,,,,,0\r\n");
    let vel = SbsMsg { msg_type: 4, gs_kt: Some(249.6), track_deg: Some(236.2), vr_fpm: Some(-1024), ..base.clone() };
    assert_eq!(vel.line(), "MSG,4,3,1,A1B2C3,1,2026/06/14,17:00:00.050,2026/06/14,17:00:00.123,,,250,236,,,-1024,,,,,0\r\n");
    let id = SbsMsg { msg_type: 1, callsign: Some(pad_callsign("UAL123")), ..base.clone() };
    assert_eq!(id.line(), "MSG,1,3,1,A1B2C3,1,2026/06/14,17:00:00.050,2026/06/14,17:00:00.123,UAL123  ,,,,,,,,,,,0\r\n");
    let sq = SbsMsg { msg_type: 6, squawk: Some(7700), alert: Some(false), emergency: Some(true), spi: Some(false), ..base.clone() };
    assert_eq!(sq.line(), "MSG,6,3,1,A1B2C3,1,2026/06/14,17:00:00.050,2026/06/14,17:00:00.123,,,,,,,,7700,0,-1,0,0\r\n");
    let surface = SbsMsg { msg_type: 2, gs_kt: Some(12.0), track_deg: Some(298.0), lat: Some(37.61), lon: Some(-122.37), ground: Some(true), ..base.clone() };
    assert_eq!(surface.line(), "MSG,2,3,1,A1B2C3,1,2026/06/14,17:00:00.050,2026/06/14,17:00:00.123,,,12,298,37.61000,-122.37000,,,,,,-1\r\n");
    let anon = SbsMsg { msg_type: 8, non_icao: true, ..base };
    assert_eq!(anon.line(), "MSG,8,3,1,~A1B2C3,1,2026/06/14,17:00:00.050,2026/06/14,17:00:00.123,,,,,,,,,,,,0\r\n");
    // 22 fields every time
    for l in [pos.line(), vel.line(), id.line(), sq.line()] {
        assert_eq!(l.trim_end().split(',').count(), 22, "{l}");
    }
}
