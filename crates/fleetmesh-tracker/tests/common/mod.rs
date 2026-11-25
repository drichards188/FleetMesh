//! Test helpers: encoders for SBS-1, AIVDM and the JSON lines, and spherical geometry, written
//! independently of the crate's decoders and projection.

#![allow(dead_code, clippy::too_many_arguments)]

use serde_json::json;

pub const T0: i64 = 1_781_456_400_000; // 2026-06-14T17:00:00Z
const A: f64 = 6_378_137.0;
const E2: f64 = 0.006_694_379_990_14;
/// Effective Earth radius for radio paths (4/3 Earth).
pub const R43: f64 = 4.0 / 3.0 * 6_371_000.0;

fn radii(lat: f64) -> (f64, f64) {
    let s = lat.to_radians().sin();
    let w = 1.0 - E2 * s * s;
    (A * (1.0 - E2) / w.powf(1.5), A / w.sqrt())
}

// ------------------------------------------------------------ geometry
//
// Ellipsoidal (WGS84) plane geometry at the midpoint latitude, good to about a metre over 50 km,
// with the initial bearing corrected for the meridians' convergence.

/// Move from (lat, lon) by east and north metres.
pub fn offset(lat: f64, lon: f64, east: f64, north: f64) -> (f64, f64) {
    let (m0, _) = radii(lat);
    let mid = lat + (north / (2.0 * m0)).to_degrees();
    let (m, n) = radii(mid);
    (
        lat + (north / m).to_degrees(),
        lon + (east / (n * mid.to_radians().cos())).to_degrees(),
    )
}

fn delta(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> (f64, f64, f64) {
    let mid = 0.5 * (lat1 + lat2);
    let (m, n) = radii(mid);
    let dn = (lat2 - lat1).to_radians() * m;
    let de = (lon2 - lon1).to_radians() * n * mid.to_radians().cos();
    (de, dn, mid)
}

pub fn distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (de, dn, _) = delta(lat1, lon1, lat2, lon2);
    (de * de + dn * dn).sqrt()
}

/// Initial true bearing, degrees.
pub fn bearing(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (de, dn, mid) = delta(lat1, lon1, lat2, lon2);
    let conv = 0.5 * (lon2 - lon1).to_radians() * mid.to_radians().sin();
    ((de.atan2(dn) - conv).to_degrees() + 360.0) % 360.0
}

/// A small deterministic noise source (xorshift) giving roughly normal values.
pub struct Noise(u64);

impl Noise {
    pub fn new(seed: u64) -> Noise {
        Noise(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn uniform(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Sum of 12 uniforms minus 6: mean 0, sigma 1.
    pub fn normal(&mut self) -> f64 {
        (0..12).map(|_| self.uniform()).sum::<f64>() - 6.0
    }
}

// ------------------------------------------------------------ SBS

pub fn sbs_time(ms: i64) -> (String, String) {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (h, mi, s, msr) = (
        rem / 3_600_000,
        (rem / 60_000) % 60,
        (rem / 1000) % 60,
        rem % 1000,
    );
    (
        format!("{y:04}/{m:02}/{d:02}"),
        format!("{h:02}:{mi:02}:{s:02}.{msr:03}"),
    )
}

#[derive(Default, Clone)]
pub struct SbsFields {
    pub callsign: Option<String>,
    pub alt_ft: Option<f64>,
    pub gs_kt: Option<f64>,
    pub track: Option<f64>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub vr_fpm: Option<f64>,
    pub squawk: Option<String>,
    pub on_ground: bool,
}

/// A dump1090-style MSG line.
pub fn sbs(tx: u8, session: &str, hex: &str, t_gen: i64, t_log: i64, f: &SbsFields) -> String {
    let (dg, tg) = sbs_time(t_gen);
    let (dl, tl) = sbs_time(t_log);
    let o = |v: Option<f64>, p: usize| v.map(|x| format!("{x:.p$}")).unwrap_or_default();
    format!(
        "MSG,{tx},{session},1,{hex},1,{dg},{tg},{dl},{tl},{},{},{},{},{},{},{},{},0,0,0,{}",
        f.callsign.clone().unwrap_or_default(),
        o(f.alt_ft, 0),
        o(f.gs_kt, 1),
        o(f.track, 1),
        o(f.lat, 5),
        o(f.lon, 5),
        o(f.vr_fpm, 0),
        f.squawk.clone().unwrap_or_default(),
        if f.on_ground { -1 } else { 0 },
    )
}

/// Airborne position (MSG 3) and velocity (MSG 4) lines for an aircraft.
pub fn adsb_pair(
    session: &str,
    hex: &str,
    t: i64,
    lat: f64,
    lon: f64,
    alt_ft: f64,
    gs_kt: f64,
    track: f64,
) -> [String; 2] {
    let p = SbsFields {
        alt_ft: Some(alt_ft),
        lat: Some(lat),
        lon: Some(lon),
        ..Default::default()
    };
    let v = SbsFields {
        gs_kt: Some(gs_kt),
        track: Some(track),
        vr_fpm: Some(0.0),
        ..Default::default()
    };
    [
        sbs(3, session, hex, t, t + 20, &p),
        sbs(4, session, hex, t + 5, t + 25, &v),
    ]
}

// ------------------------------------------------------------ AIS

#[derive(Default)]
pub struct BitWriter {
    bits: Vec<u8>,
}

impl BitWriter {
    pub fn u(&mut self, v: u64, n: usize) -> &mut Self {
        for k in (0..n).rev() {
            self.bits.push(((v >> k) & 1) as u8);
        }
        self
    }
    pub fn i(&mut self, v: i64, n: usize) -> &mut Self {
        let mask = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
        self.u((v as u64) & mask, n)
    }
    pub fn text(&mut self, s: &str, n: usize) -> &mut Self {
        const T: &[u8; 64] = b"@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_ !\"#$%&'()*+,-./0123456789:;<=>?";
        let up = s.to_uppercase();
        let b = up.as_bytes();
        for k in 0..n {
            let c = b.get(k).copied().unwrap_or(b'@');
            let v = T.iter().position(|&x| x == c).unwrap_or(0) as u64;
            self.u(v, 6);
        }
        self
    }
    /// Armoured payload and fill bits.
    pub fn payload(&self) -> (String, u8) {
        let mut bits = self.bits.clone();
        let fill = (6 - bits.len() % 6) % 6;
        bits.extend(std::iter::repeat_n(0, fill));
        let mut s = String::new();
        for c in bits.chunks(6) {
            let v = c.iter().fold(0u8, |a, &b| (a << 1) | b);
            let ch = if v < 40 { v + 48 } else { v + 56 };
            s.push(ch as char);
        }
        (s, fill as u8)
    }
}

fn nmea_checksum(s: &str) -> u8 {
    s.bytes().fold(0, |a, b| a ^ b)
}

/// NMEA sentences for a payload (split at 60 characters), each with a tag block naming the
/// receiver and the time in Unix seconds.
pub fn aivdm(payload: &str, fill: u8, channel: char, seq: u8, rx: &str, t_ms: i64) -> Vec<String> {
    let chunks: Vec<&str> = payload
        .as_bytes()
        .chunks(60)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    let n = chunks.len();
    let mut out = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        let f = if i + 1 == n { fill } else { 0 };
        let seqs = if n > 1 {
            seq.to_string()
        } else {
            String::new()
        };
        let body = format!("AIVDM,{n},{},{seqs},{channel},{c},{f}", i + 1);
        let tag = format!("s:{rx},c:{}", t_ms / 1000);
        out.push(format!(
            "\\{tag}*{:02X}\\!{body}*{:02X}",
            nmea_checksum(&tag),
            nmea_checksum(&body)
        ));
    }
    out
}

pub struct ClassA {
    pub mmsi: u32,
    pub nav: u8,
    pub sog_kn: f64,
    pub lat: f64,
    pub lon: f64,
    pub cog: f64,
    pub heading: u16,
    pub second: u8,
}

pub fn ais_type1(a: &ClassA) -> (String, u8) {
    let mut w = BitWriter::default();
    w.u(1, 6)
        .u(0, 2)
        .u(a.mmsi as u64, 30)
        .u(a.nav as u64, 4)
        .i(-128, 8);
    w.u((a.sog_kn * 10.0).round() as u64, 10).u(1, 1);
    w.i((a.lon * 600_000.0).round() as i64, 28)
        .i((a.lat * 600_000.0).round() as i64, 27);
    w.u((a.cog * 10.0).round() as u64, 12)
        .u(a.heading as u64, 9)
        .u(a.second as u64, 6);
    w.u(0, 2).u(0, 3).u(0, 1).u(0, 19);
    w.payload()
}

pub fn ais_type18(
    mmsi: u32,
    sog_kn: f64,
    lat: f64,
    lon: f64,
    cog: f64,
    second: u8,
) -> (String, u8) {
    let mut w = BitWriter::default();
    w.u(18, 6)
        .u(0, 2)
        .u(mmsi as u64, 30)
        .u(0, 8)
        .u((sog_kn * 10.0).round() as u64, 10)
        .u(0, 1);
    w.i((lon * 600_000.0).round() as i64, 28)
        .i((lat * 600_000.0).round() as i64, 27);
    w.u((cog * 10.0).round() as u64, 12)
        .u(511, 9)
        .u(second as u64, 6);
    w.u(0, 2)
        .u(1, 1)
        .u(0, 1)
        .u(1, 1)
        .u(1, 1)
        .u(1, 1)
        .u(0, 1)
        .u(0, 1)
        .u(0, 20);
    w.payload()
}

pub fn ais_type5(
    mmsi: u32,
    callsign: &str,
    name: &str,
    ship_type: u8,
    dims: [u16; 4],
    draught: f64,
    dest: &str,
) -> (String, u8) {
    let mut w = BitWriter::default();
    w.u(5, 6)
        .u(0, 2)
        .u(mmsi as u64, 30)
        .u(0, 2)
        .u(9_000_001, 30)
        .text(callsign, 7)
        .text(name, 20);
    w.u(ship_type as u64, 8)
        .u(dims[0] as u64, 9)
        .u(dims[1] as u64, 9)
        .u(dims[2] as u64, 6)
        .u(dims[3] as u64, 6);
    w.u(1, 4)
        .u(6, 4)
        .u(14, 5)
        .u(18, 5)
        .u(30, 6)
        .u((draught * 10.0).round() as u64, 8)
        .text(dest, 20)
        .u(0, 1)
        .u(0, 1);
    w.payload()
}

pub fn ais_type24a(mmsi: u32, name: &str) -> (String, u8) {
    let mut w = BitWriter::default();
    w.u(24, 6).u(0, 2).u(mmsi as u64, 30).u(0, 2).text(name, 20);
    w.payload()
}

pub fn ais_type24b(mmsi: u32, ship_type: u8, callsign: &str, dims: [u16; 4]) -> (String, u8) {
    let mut w = BitWriter::default();
    w.u(24, 6)
        .u(0, 2)
        .u(mmsi as u64, 30)
        .u(1, 2)
        .u(ship_type as u64, 8)
        .text("ABC", 3)
        .u(0, 24);
    w.text(callsign, 7)
        .u(dims[0] as u64, 9)
        .u(dims[1] as u64, 9)
        .u(dims[2] as u64, 6)
        .u(dims[3] as u64, 6);
    w.u(1, 4).u(0, 2);
    w.payload()
}

// ------------------------------------------------------------ JSON

pub fn det(
    sensor: &str,
    platform: &str,
    t: i64,
    s: (f64, f64, f64),
    bearing: f64,
    elev: Option<f64>,
    range: Option<f64>,
    sigma_b: f64,
) -> String {
    json!({"type":"det","sensor":sensor,"platform":platform,"t":t,"sensor_lat":s.0,"sensor_lon":s.1,
        "sensor_alt_m":s.2,"bearing_deg":bearing,"sigma_bearing_deg":sigma_b,"elev_deg":elev,
        "sigma_elev_deg":elev.map(|_| 1.0),"range_m":range,"sigma_range_m":range.map(|_| 20.0),
        "radial_speed_mps":null,"size":"medium"})
    .to_string()
}

pub fn rid(
    rx: &str,
    t: i64,
    uas_id: &str,
    id_type: &str,
    lat: f64,
    lon: f64,
    alt_hae: f64,
    speed: f64,
    track: f64,
) -> String {
    json!({"type":"rid","rx":rx,"t_rx":t + 40,"t":t,"uas_id":uas_id,"id_type":id_type,"ua_type":"multirotor",
        "status":"airborne","lat":lat,"lon":lon,"alt_geo_m":alt_hae,"alt_baro_m":alt_hae,"height_agl_m":alt_hae,
        "speed_mps":speed,"track_deg":track,"vspeed_mps":0.0,"h_acc_m":3.0,"v_acc_m":4.0})
    .to_string()
}

/// A detection of a target from a sensor, with noise: bearing (true), elevation and range.
pub struct Look {
    pub bearing: f64,
    pub elev: f64,
    pub range: f64,
}

pub fn look(s: (f64, f64, f64), t: (f64, f64, f64)) -> Look {
    let ground = distance(s.0, s.1, t.0, t.1);
    // The target sits lower by the Earth's curvature (4/3 Earth for refraction).
    let dz = t.2 - s.2 - ground * ground / (2.0 * R43);
    Look {
        bearing: bearing(s.0, s.1, t.0, t.1),
        elev: dz.atan2(ground).to_degrees(),
        range: (ground * ground + dz * dz).sqrt(),
    }
}
