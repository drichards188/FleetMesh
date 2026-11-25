//! Identities and equipage, drawn per object at realistic rates. Every name, number and address
//! here is invented for the simulation (illustrative); the formats are the real ones: ICAO 24-bit
//! addresses from each state's block, N-numbers, airline designators, MMSIs with real maritime
//! identification digits, IMO numbers with their check digit, ANSI/CTA-2063-A serial numbers.

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::{Adsb, Ais, Class, Equip, Ident, RidLink};
use crate::config::EquipCfg;

const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
const ALNUM: &[u8] = b"0123456789ABCDEFGHJKLMNPRSTUVWXYZ";

fn pick<'a>(rng: &mut ChaCha8Rng, xs: &'a [&'a str]) -> &'a str {
    xs[rng.random_range(0..xs.len())]
}

fn chars(rng: &mut ChaCha8Rng, set: &[u8], n: usize) -> String {
    (0..n).map(|_| set[rng.random_range(0..set.len())] as char).collect()
}

/// The US block of ICAO addresses: civil N-numbers A00001 to ADF7C7.
pub fn icao_us(rng: &mut ChaCha8Rng) -> u32 {
    rng.random_range(0xA0_0001..=0xAD_F7C7)
}

/// An address in a block `[lo, hi]` (a privacy pool, the military block).
pub fn icao_in(rng: &mut ChaCha8Rng, block: [u32; 2]) -> u32 {
    let [lo, hi] = block;
    if hi > lo { rng.random_range(lo..=hi) } else { lo }
}

/// A flight with a Privacy ICAO Address: an address from the pool, fresh for this flight, and
/// a third-party callsign instead of its registration (FAA PIA programme).
pub fn apply_privacy(rng: &mut ChaCha8Rng, a: &crate::config::AddressCfg, id: &mut Ident, eq: &Equip) {
    if eq.pia {
        id.icao24 = Some(icao_in(rng, a.pia_block));
        id.callsign = Some(format!("{}{}", a.pia_callsign_prefix, rng.random_range(100..10000)));
    }
}

/// An address in the block of the airline's state (ICAO Annex 10 Vol III, table 9-1).
pub fn icao_for_airline(rng: &mut ChaCha8Rng, prefix: &str) -> u32 {
    let (lo, hi): (u32, u32) = match prefix {
        "ANA" | "JAL" => (0x84_0000, 0x87_FFFF),
        "BAW" | "VIR" => (0x40_0000, 0x43_FFFF),
        "DLH" => (0x3C_0000, 0x3F_FFFF),
        "AFR" => (0x38_0000, 0x3B_FFFF),
        "KLM" => (0x48_0000, 0x48_7FFF),
        "ACA" | "WJA" => (0xC0_0000, 0xC3_FFFF),
        "CPA" | "CCA" | "CES" | "CSN" => (0x78_0000, 0x7B_FFFF),
        "EVA" | "CAL" | "SJX" => (0x89_9000, 0x89_93FF),
        "KAL" | "AAR" => (0x71_8000, 0x71_FFFF),
        "QFA" => (0x7C_0000, 0x7F_FFFF),
        "SIA" => (0x76_8000, 0x76_BFFF),
        "UAE" => (0x89_6000, 0x89_6FFF),
        "AMX" | "VOI" | "VIV" => (0x0D_0000, 0x0D_7FFF),
        "PAL" => (0x75_8000, 0x75_FFFF),
        "ANZ" => (0xC8_0000, 0xC8_7FFF),
        "THY" => (0x4B_8000, 0x4B_FFFF),
        _ => (0xA0_0001, 0xAD_F7C7),
    };
    rng.random_range(lo..=hi)
}

/// An N-number: N, a digit 1-9, up to three more digits, up to two letters.
pub fn n_number(rng: &mut ChaCha8Rng) -> String {
    let mut s = format!("N{}", rng.random_range(1..=9));
    let digits = rng.random_range(1..=3);
    for _ in 0..digits {
        s.push((b'0' + rng.random_range(0..=9u8)) as char);
    }
    let letters = rng.random_range(0..=2);
    s.push_str(&chars(rng, LETTERS, letters));
    s
}

pub fn flight_number(rng: &mut ChaCha8Rng, prefix: &str) -> String {
    let n: u32 = match rng.random_range(0..10) {
        0..=4 => rng.random_range(1..1000),
        5..=7 => rng.random_range(1000..2600),
        _ => rng.random_range(2600..6000),
    };
    format!("{prefix}{n}")
}

/// A discrete transponder code: four octal digits, never 1200, 7500, 7600 or 7700.
pub fn discrete_squawk(rng: &mut ChaCha8Rng) -> u16 {
    loop {
        let d: Vec<u16> = (0..4).map(|_| rng.random_range(0..=7u16)).collect();
        let s = d[0] * 1000 + d[1] * 100 + d[2] * 10 + d[3];
        if d[0] != 0 && d[0] != 7 && s != 1200 {
            return s;
        }
    }
}

/// An MMSI: a US one (MIDs 366 to 369, 338) for a US-flag vessel, else one of the flags deep-sea
/// ships in San Francisco Bay often fly.
pub fn mmsi(rng: &mut ChaCha8Rng, us: bool) -> u32 {
    let mid: u32 = if us {
        [366, 367, 368, 369, 338][rng.random_range(0..5)]
    } else {
        [636, 636, 538, 538, 477, 563, 255, 249, 352, 353, 355, 370, 371, 311, 431, 440, 412, 416, 241, 219][rng.random_range(0..20)]
    };
    mid * 1_000_000 + rng.random_range(0..1_000_000)
}

fn mid(mmsi: u32) -> u32 {
    mmsi / 1_000_000
}

/// A call sign in the style of the vessel's flag.
pub fn ship_callsign(rng: &mut ChaCha8Rng, mmsi: u32) -> String {
    let m = mid(mmsi);
    let p = match m {
        636 => "D5",
        538 => "V7",
        477 => "VR",
        563 => "9V",
        255 => "CQ",
        249 => "9H",
        352 | 353 | 355 | 370 | 371 => "3E",
        311 => "C6",
        431 => "7J",
        440 => "D7",
        412 => "BO",
        416 => "BR",
        241 => "SV",
        219 => "OX",
        _ => "",
    };
    if p.is_empty() {
        // US: WD, a letter, four digits (small craft licences); K or W and three letters (older ships)
        if rng.random_bool(0.7) {
            format!("WD{}{}", chars(rng, LETTERS, 1), rng.random_range(1000..10000))
        } else {
            format!("{}{}", if rng.random_bool(0.5) { "K" } else { "W" }, chars(rng, LETTERS, 3))
        }
    } else {
        format!("{p}{}", chars(rng, ALNUM, 3))
    }
}

/// A 7-digit IMO number with its check digit.
pub fn imo(rng: &mut ChaCha8Rng) -> u32 {
    let base: u32 = rng.random_range(930_000..=999_999);
    let digits: Vec<u32> = base.to_string().bytes().map(|b| u32::from(b - b'0')).collect();
    let check = digits.iter().zip([7, 6, 5, 4, 3, 2]).map(|(d, w)| d * w).sum::<u32>() % 10;
    base * 10 + check
}

const NAME_A: &[&str] = &[
    "PACIFIC", "OCEAN", "NORTHERN", "GOLDEN", "SILVER", "CAPE", "MERIDIAN", "HARBOR", "CORAL", "TASMAN", "ORIENT", "TRANS",
    "SEA", "STAR", "BLUE", "GRAND", "ISLAND", "EASTERN", "WESTERN", "ARCTIC",
];
const NAME_B: &[&str] = &[
    "ARROW", "SPIRIT", "CREST", "VOYAGER", "HARMONY", "TRADER", "PIONEER", "HORIZON", "DAWN", "PEARL", "EXPRESS", "GRACE",
    "LEGEND", "KESTREL", "FALCON", "MARINER", "ENDEAVOUR", "PROMISE", "BRIDGE", "LINK",
];
const SMALL_NAMES: &[&str] = &[
    "SEA DOG", "WINDSONG", "BLUE HERON", "SALTY", "MARGARET ANN", "LUCKY STAR", "OSPREY", "FOG CUTTER", "SEA BREEZE", "KESTREL",
    "PELICAN", "ZEPHYR", "SERENITY", "DRIFTER", "HALCYON", "SKYLARK", "TIDE RUNNER", "WAVE DANCER", "ALBATROSS", "SEAGULL",
];

pub fn ship_name(rng: &mut ChaCha8Rng, class: Class) -> String {
    match class {
        Class::Cargo | Class::Tanker | Class::Passenger => format!("{} {}", pick(rng, NAME_A), pick(rng, NAME_B)),
        Class::Tug => format!("{} {}", pick(rng, &["BAY", "DELTA", "GOLDEN", "ALCATRAZ", "PACIFIC", "MARIN"]), pick(rng, NAME_B)),
        Class::Pilot => format!("PILOT {}", pick(rng, &["DRAKE", "BONITA", "FARALLON", "LIME POINT", "MILE ROCK"])),
        _ => {
            let n = pick(rng, SMALL_NAMES);
            if rng.random_bool(0.3) { format!("{n} {}", ["II", "III", "IV"][rng.random_range(0..3)]) } else { n.to_string() }
        }
    }
}

/// An ANSI/CTA-2063-A serial number: a four-character manufacturer code, the serial's length
/// in one hex character, the serial.
pub fn uas_serial(rng: &mut ChaCha8Rng) -> String {
    let mfr = match rng.random_range(0..100) {
        0..=59 => "1581",
        60..=74 => "1596",
        75..=84 => "1748",
        85..=94 => "1668",
        _ => "1787",
    };
    let len = rng.random_range(10..=15usize);
    format!("{mfr}{:X}{}", len, chars(rng, ALNUM, len))
}

/// AIS ship type, length and beam, draught, by class.
pub fn hull(rng: &mut ChaCha8Rng, class: Class, length: Option<[f64; 2]>) -> (u8, [u16; 4], u8) {
    let (ty, l, b, draught): (u8, [f64; 2], f64, [f64; 2]) = match class {
        Class::Cargo => ([70, 70, 70, 70, 71, 72][rng.random_range(0..6)], [180.0, 366.0], 0.14, [9.5, 14.5]),
        Class::Tanker => ([80, 80, 80, 81, 82, 83][rng.random_range(0..6)], [170.0, 275.0], 0.17, [8.0, 15.0]),
        Class::Passenger => (60, [250.0, 300.0], 0.12, [7.5, 8.5]),
        Class::Ferry => ([60, 60, 40, 69][rng.random_range(0..4)], [30.0, 70.0], 0.25, [1.5, 3.0]),
        Class::Tug => (52, [24.0, 35.0], 0.35, [4.0, 5.5]),
        Class::Pilot => (50, [16.0, 28.0], 0.28, [1.5, 2.5]),
        Class::Fishing => (30, [12.0, 26.0], 0.3, [1.8, 3.0]),
        Class::Pleasure => (37, [8.0, 20.0], 0.3, [0.8, 1.8]),
        Class::Sailing => (36, [8.0, 16.0], 0.3, [1.5, 2.6]),
        _ => (0, [4.0, 6.0], 0.2, [0.2, 0.4]),
    };
    let [l0, l1] = length.unwrap_or(l);
    let len = if l1 > l0 { rng.random_range(l0..l1) } else { l0 };
    let beam = (len * b).clamp(1.0, 63.0);
    // GNSS antenna aft of amidships on most ships, near the wheelhouse on small craft
    let aft = match class {
        Class::Cargo | Class::Tanker => rng.random_range(0.12..0.3),
        Class::Passenger => 0.75,
        _ => rng.random_range(0.3..0.5),
    };
    let to_stern = (len * aft).round();
    let to_bow = (len - to_stern).round();
    let port = (beam / 2.0).round();
    let starboard = (beam - port).round();
    let dims = [to_bow.min(511.0) as u16, to_stern.min(511.0) as u16, port.min(63.0) as u16, starboard.min(63.0) as u16];
    let [d0, d1] = draught;
    let dr = if d1 > d0 { rng.random_range(d0..d1) } else { d0 };
    (ty, dims, (dr * 10.0).round().min(255.0) as u8)
}

/// Draw equipage for a class.
pub fn equip(rng: &mut ChaCha8Rng, class: Class, c: &EquipCfg) -> Equip {
    let x: f64 = rng.random();
    let adsb = if x < c.adsb_1090 {
        Adsb::Es1090
    } else if x < c.adsb_1090 + c.uat {
        Adsb::Uat
    } else if x < c.adsb_1090 + c.uat + c.mode_s_only {
        Adsb::ModeS
    } else {
        Adsb::None
    };
    let y: f64 = rng.random();
    let ais = if y < c.ais_a {
        Ais::A
    } else if y < c.ais_a + c.ais_b {
        Ais::B
    } else {
        Ais::None
    };
    let rid = (rng.random::<f64>() < c.rid).then(|| match rng.random_range(0..10) {
        0..=3 => RidLink::Wifi,
        4..=7 => RidLink::Bt5,
        _ => RidLink::Bt4,
    });
    let pia = adsb == Adsb::Es1090 && rng.random::<f64>() < c.pia;
    let ladd = adsb != Adsb::None && rng.random::<f64>() < c.ladd;
    let anonymous = adsb == Adsb::Uat && rng.random::<f64>() < c.uat_anon;
    let ais_off = ais != Ais::None && rng.random::<f64>() < c.ais_off;
    let type19 = ais == Ais::B && rng.random::<f64>() < c.type19;
    let big = matches!(class, Class::Cargo | Class::Tanker | Class::Passenger | Class::Ferry);
    let nav_sigma_m = match class {
        Class::Airliner | Class::Bizjet => 3.0,
        Class::Drone => 2.0,
        _ if class.air() => 6.0,
        _ => 3.0,
    };
    Equip {
        adsb,
        ais,
        rid,
        pia,
        ladd,
        anonymous,
        ais_off,
        type19,
        nav_sigma_m,
        rot_indicator: big,
        gyro: big || class == Class::Tug,
        dgnss: big || rng.random_bool(0.6),
        pos_type: match rng.random_range(0..100) {
            0..=84 => 1,
            85..=96 => 3,
            _ => 2,
        },
    }
}

/// A vessel's identity for its class.
pub fn vessel(rng: &mut ChaCha8Rng, class: Class, us_flag: bool, length: Option<[f64; 2]>, name: Option<&str>) -> Ident {
    let m = mmsi(rng, us_flag);
    let (ship_type, dims, draught_dm) = hull(rng, class, length);
    let deep = matches!(class, Class::Cargo | Class::Tanker | Class::Passenger);
    let vendor = (
        ["SRT", "TRU", "VSP", "GMN", "ACR", "EMT"][rng.random_range(0..6)].to_string(),
        rng.random_range(1..15u8),
        rng.random_range(1..1_000_000u32),
    );
    Ident {
        mmsi: Some(m),
        imo: deep.then(|| imo(rng)),
        name: Some(name.map_or_else(|| ship_name(rng, class), str::to_string)),
        ship_callsign: Some(ship_callsign(rng, m)),
        ship_type,
        dims,
        draught_dm,
        vendor: Some(vendor),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng;

    #[test]
    fn identities_have_their_real_formats() {
        let mut r = rng::chacha(1, &[2]);
        for _ in 0..200 {
            let i = imo(&mut r);
            let d: Vec<u32> = i.to_string().bytes().map(|b| u32::from(b - b'0')).collect();
            assert_eq!(d.len(), 7);
            let check = (0..6).map(|k| d[k] * (7 - k as u32)).sum::<u32>() % 10;
            assert_eq!(check, d[6], "{i}");
            let a = icao_us(&mut r);
            assert!((0xA00001..=0xADF7C7).contains(&a));
            let p = icao_in(&mut r, [0xADF800, 0xADFBFF]);
            assert!((0xADF800..=0xADFBFF).contains(&p));
            let s = discrete_squawk(&mut r);
            assert!(s.to_string().bytes().all(|b| b <= b'7') && s != 1200);
            let n = n_number(&mut r);
            assert!(n.starts_with('N') && n.len() <= 7, "{n}");
            let m = mmsi(&mut r, true);
            assert!((338_000_000..370_000_000).contains(&m));
            let u = uas_serial(&mut r);
            let len = usize::from_str_radix(&u[4..5], 16).expect("hex length");
            assert_eq!(u.len(), 5 + len, "{u}");
        }
    }
}
