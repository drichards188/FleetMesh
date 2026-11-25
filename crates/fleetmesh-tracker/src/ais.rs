//! AIS payload decoding (ITU-R M.1371): the 6-bit armouring and message types 1, 2, 3 (Class A
//! position), 5 (Class A static and voyage), 18 (Class B position), 19 (Class B extended) and 24
//! (Class B static, parts A and B). Field layouts follow the gpsd "AIVDM/AIVDO protocol decoding"
//! document. Other types decode to [`AisMessage::Other`] with their MMSI.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AisError {
    BadChar(char),
    TooShort { msg_type: u8, bits: usize },
}

impl std::fmt::Display for AisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// The payload as a bit string.
pub struct Bits {
    bits: Vec<u8>,
}

impl Bits {
    pub fn from_payload(payload: &str, fill: u8) -> Result<Bits, AisError> {
        let mut bits = Vec::with_capacity(payload.len() * 6);
        for c in payload.chars() {
            let mut v = (c as u32).wrapping_sub(48);
            if v > 40 {
                v = v.wrapping_sub(8);
            }
            // Valid armour characters are '0'..'W' and '`'..'w'.
            if !(('0'..='W').contains(&c) || ('`'..='w').contains(&c)) || v > 63 {
                return Err(AisError::BadChar(c));
            }
            for k in (0..6).rev() {
                bits.push(((v >> k) & 1) as u8);
            }
        }
        let keep = bits.len().saturating_sub(fill as usize);
        bits.truncate(keep);
        Ok(Bits { bits })
    }

    pub fn len(&self) -> usize {
        self.bits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    pub fn u(&self, start: usize, len: usize) -> Option<u64> {
        if start + len > self.bits.len() {
            return None;
        }
        Some(
            self.bits[start..start + len]
                .iter()
                .fold(0u64, |a, &b| (a << 1) | b as u64),
        )
    }

    /// Like [`Bits::u`] but a field the payload is too short for reads as zero (receivers trim
    /// trailing spare bits).
    fn uz(&self, start: usize, len: usize) -> u64 {
        self.u(start, len).unwrap_or(0)
    }

    pub fn i(&self, start: usize, len: usize) -> Option<i64> {
        let u = self.u(start, len)?;
        let sign = 1u64 << (len - 1);
        Some(if u & sign != 0 {
            u as i64 - (1i64 << len)
        } else {
            u as i64
        })
    }

    /// Six-bit text, `nchar` characters, trailing `@` and spaces removed. Characters past the end
    /// of a short payload are left off.
    pub fn text(&self, start: usize, nchar: usize) -> String {
        const T: &[u8; 64] = b"@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_ !\"#$%&'()*+,-./0123456789:;<=>?";
        let mut s = String::with_capacity(nchar);
        for k in 0..nchar {
            match self.u(start + 6 * k, 6) {
                Some(v) => s.push(T[v as usize] as char),
                None => break,
            }
        }
        // Text ends at the first '@' (padding), and trailing spaces are padding too.
        if let Some(at) = s.find('@') {
            s.truncate(at);
        }
        s.trim_end().to_string()
    }
}

/// Position report: types 1, 2, 3 (Class A) and 18, 19 (Class B).
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub msg_type: u8,
    pub repeat: u8,
    pub mmsi: u32,
    /// Navigation status (Class A only): 0 under way using engine, 1 at anchor, 5 moored, ...
    pub nav_status: Option<u8>,
    /// Rate of turn, raw ITU value (Class A only); None when not available (-128).
    pub rot_raw: Option<i8>,
    pub sog_kn: Option<f64>,
    /// Position accuracy flag: true is better than 10 m (DGNSS).
    pub accuracy: bool,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
    pub cog_deg: Option<f64>,
    pub heading_deg: Option<u16>,
    /// UTC second of the report, 0..59; 60 and above are "not available" or special.
    pub second: u8,
    pub raim: bool,
    /// Class B only: true for a carrier-sense (CS) unit.
    pub class_b_cs: Option<bool>,
}

/// Type 5: Class A static and voyage data.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticVoyage {
    pub repeat: u8,
    pub mmsi: u32,
    pub ais_version: u8,
    pub imo: u32,
    pub callsign: String,
    pub name: String,
    pub ship_type: u8,
    pub to_bow: u16,
    pub to_stern: u16,
    pub to_port: u8,
    pub to_starboard: u8,
    pub epfd: u8,
    /// ETA month, day, hour, minute as sent (0, 0, 24, 60 mean not available).
    pub eta: (u8, u8, u8, u8),
    pub draught_m: f64,
    pub destination: String,
    pub dte: bool,
}

/// Type 19: Class B extended position and static data.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassBExtended {
    pub position: Position,
    pub name: String,
    pub ship_type: u8,
    pub to_bow: u16,
    pub to_stern: u16,
    pub to_port: u8,
    pub to_starboard: u8,
    pub epfd: u8,
}

/// Type 24 part B.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticB {
    pub repeat: u8,
    pub mmsi: u32,
    pub ship_type: u8,
    pub vendor_id: String,
    pub callsign: String,
    pub to_bow: u16,
    pub to_stern: u16,
    pub to_port: u8,
    pub to_starboard: u8,
    /// For an auxiliary craft (MMSI 98xxxxxxx) the dimension bits carry the mother ship.
    pub mothership_mmsi: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AisMessage {
    Position(Position),
    StaticVoyage(StaticVoyage),
    ClassBExtended(ClassBExtended),
    StaticA { repeat: u8, mmsi: u32, name: String },
    StaticB(StaticB),
    Other { msg_type: u8, mmsi: Option<u32> },
}

impl AisMessage {
    pub fn mmsi(&self) -> Option<u32> {
        match self {
            AisMessage::Position(p) => Some(p.mmsi),
            AisMessage::StaticVoyage(s) => Some(s.mmsi),
            AisMessage::ClassBExtended(e) => Some(e.position.mmsi),
            AisMessage::StaticA { mmsi, .. } => Some(*mmsi),
            AisMessage::StaticB(b) => Some(b.mmsi),
            AisMessage::Other { mmsi, .. } => *mmsi,
        }
    }
}

fn lon_lat(lon_raw: i64, lat_raw: i64) -> (Option<f64>, Option<f64>) {
    let lon = lon_raw as f64 / 600_000.0;
    let lat = lat_raw as f64 / 600_000.0;
    // 181 and 91 degrees mean "not available".
    if lon.abs() > 180.0 || lat.abs() > 90.0 {
        (None, None)
    } else {
        (Some(lon), Some(lat))
    }
}

fn sog(raw: u64) -> Option<f64> {
    if raw == 1023 {
        None
    } else {
        Some(raw as f64 / 10.0)
    }
}

fn cog(raw: u64) -> Option<f64> {
    if raw >= 3600 {
        None
    } else {
        Some(raw as f64 / 10.0)
    }
}

fn heading(raw: u64) -> Option<u16> {
    if raw >= 360 { None } else { Some(raw as u16) }
}

pub fn decode(payload: &str, fill: u8) -> Result<AisMessage, AisError> {
    let b = Bits::from_payload(payload, fill)?;
    decode_bits(&b)
}

pub fn decode_bits(b: &Bits) -> Result<AisMessage, AisError> {
    let msg_type = b.u(0, 6).ok_or(AisError::TooShort {
        msg_type: 0,
        bits: b.len(),
    })? as u8;
    let short = |need: usize| -> Result<(), AisError> {
        if b.len() < need {
            Err(AisError::TooShort {
                msg_type,
                bits: b.len(),
            })
        } else {
            Ok(())
        }
    };
    let repeat = b.uz(6, 2) as u8;
    let mmsi = b.u(8, 30).map(|v| v as u32);
    match msg_type {
        1..=3 => {
            // 168 bits nominal; the radio status at the end is not needed.
            short(149)?;
            let (lon, lat) = lon_lat(b.i(61, 28).unwrap_or(0), b.i(89, 27).unwrap_or(0));
            let rot = b.i(42, 8).unwrap_or(-128);
            Ok(AisMessage::Position(Position {
                msg_type,
                repeat,
                mmsi: mmsi.unwrap_or(0),
                nav_status: Some(b.uz(38, 4) as u8),
                rot_raw: if rot == -128 { None } else { Some(rot as i8) },
                sog_kn: sog(b.uz(50, 10)),
                accuracy: b.uz(60, 1) == 1,
                lon,
                lat,
                cog_deg: cog(b.uz(116, 12)),
                heading_deg: heading(b.uz(128, 9)),
                second: b.uz(137, 6) as u8,
                raim: b.uz(148, 1) == 1,
                class_b_cs: None,
            }))
        }
        5 => {
            // 424 bits nominal; many encoders send 420 or 422, so accept from the draught on.
            short(302)?;
            Ok(AisMessage::StaticVoyage(StaticVoyage {
                repeat,
                mmsi: mmsi.unwrap_or(0),
                ais_version: b.uz(38, 2) as u8,
                imo: b.uz(40, 30) as u32,
                callsign: b.text(70, 7),
                name: b.text(112, 20),
                ship_type: b.uz(232, 8) as u8,
                to_bow: b.uz(240, 9) as u16,
                to_stern: b.uz(249, 9) as u16,
                to_port: b.uz(258, 6) as u8,
                to_starboard: b.uz(264, 6) as u8,
                epfd: b.uz(270, 4) as u8,
                eta: (
                    b.uz(274, 4) as u8,
                    b.uz(278, 5) as u8,
                    b.uz(283, 5) as u8,
                    b.uz(288, 6) as u8,
                ),
                draught_m: b.uz(294, 8) as f64 / 10.0,
                destination: b.text(302, 20),
                dte: b.uz(422, 1) == 1,
            }))
        }
        18 | 19 => {
            short(if msg_type == 18 { 148 } else { 301 })?;
            let (lon, lat) = lon_lat(b.i(57, 28).unwrap_or(0), b.i(85, 27).unwrap_or(0));
            let position = Position {
                msg_type,
                repeat,
                mmsi: mmsi.unwrap_or(0),
                nav_status: None,
                rot_raw: None,
                sog_kn: sog(b.uz(46, 10)),
                accuracy: b.uz(56, 1) == 1,
                lon,
                lat,
                cog_deg: cog(b.uz(112, 12)),
                heading_deg: heading(b.uz(124, 9)),
                second: b.uz(133, 6) as u8,
                raim: b.uz(if msg_type == 18 { 147 } else { 305 }, 1) == 1,
                class_b_cs: if msg_type == 18 {
                    Some(b.uz(141, 1) == 1)
                } else {
                    None
                },
            };
            if msg_type == 18 {
                return Ok(AisMessage::Position(position));
            }
            Ok(AisMessage::ClassBExtended(ClassBExtended {
                position,
                name: b.text(143, 20),
                ship_type: b.uz(263, 8) as u8,
                to_bow: b.uz(271, 9) as u16,
                to_stern: b.uz(280, 9) as u16,
                to_port: b.uz(289, 6) as u8,
                to_starboard: b.uz(295, 6) as u8,
                epfd: b.uz(301, 4) as u8,
            }))
        }
        24 => {
            short(40)?;
            let part = b.uz(38, 2);
            let mmsi = mmsi.unwrap_or(0);
            match part {
                0 => {
                    short(158)?;
                    Ok(AisMessage::StaticA {
                        repeat,
                        mmsi,
                        name: b.text(40, 20),
                    })
                }
                1 => {
                    short(162)?;
                    let auxiliary = mmsi / 10_000_000 == 98;
                    Ok(AisMessage::StaticB(StaticB {
                        repeat,
                        mmsi,
                        ship_type: b.uz(40, 8) as u8,
                        vendor_id: b.text(48, 3),
                        callsign: b.text(90, 7),
                        to_bow: if auxiliary { 0 } else { b.uz(132, 9) as u16 },
                        to_stern: if auxiliary { 0 } else { b.uz(141, 9) as u16 },
                        to_port: if auxiliary { 0 } else { b.uz(150, 6) as u8 },
                        to_starboard: if auxiliary { 0 } else { b.uz(156, 6) as u8 },
                        mothership_mmsi: if auxiliary {
                            Some(b.uz(132, 30) as u32)
                        } else {
                            None
                        },
                    }))
                }
                _ => Ok(AisMessage::Other {
                    msg_type,
                    mmsi: Some(mmsi),
                }),
            }
        }
        _ => Ok(AisMessage::Other { msg_type, mmsi }),
    }
}

/// Whether an MMSI is a plausible identity for a vessel sending position reports.
///
/// Accepted: a ship station (first digit 2 to 7, its three-digit MID inside an ITU region's
/// allocation: 201-279, 301-379, 401-477, 501-578, 601-679, 701-775), a craft associated with a
/// parent ship (98 + MID), a handheld (8 + MID), a SAR aircraft (111 + MID) and AIS-SART, MOB and
/// EPIRB-AIS (970, 972, 974). Rejected: anything else, including the common placeholders
/// (000000000, 123456789, repeated digits).
pub fn mmsi_valid(mmsi: u32) -> bool {
    if !(100_000_000..=999_999_999).contains(&mmsi) {
        return false;
    }
    let s = mmsi.to_string();
    let b = s.as_bytes();
    if b.iter().all(|&c| c == b[0]) || mmsi == 123_456_789 || mmsi == 987_654_321 {
        return false;
    }
    let mid_ok = |mid: u32| matches!(mid, 201..=279 | 301..=379 | 401..=477 | 501..=578 | 601..=679 | 701..=775);
    let first = mmsi / 100_000_000;
    match first {
        2..=7 => mid_ok(mmsi / 1_000_000),
        8 => mid_ok((mmsi / 100_000) % 1000),
        9 => {
            let p3 = mmsi / 1_000_000;
            matches!(p3, 970 | 972 | 974)
                || (mmsi / 10_000_000 == 98 && mid_ok((mmsi / 10_000) % 1000))
        }
        1 => mmsi / 1_000_000 == 111 && mid_ok((mmsi / 1000) % 1000),
        _ => false,
    }
}

/// Navigation status text (ITU-R M.1371 table 45).
pub fn nav_status_text(s: u8) -> &'static str {
    match s {
        0 => "under way using engine",
        1 => "at anchor",
        2 => "not under command",
        3 => "restricted manoeuverability",
        4 => "constrained by her draught",
        5 => "moored",
        6 => "aground",
        7 => "engaged in fishing",
        8 => "under way sailing",
        11 => "power-driven vessel towing astern",
        12 => "power-driven vessel pushing ahead or towing alongside",
        14 => "AIS-SART, MOB or EPIRB active",
        15 => "not defined",
        _ => "reserved",
    }
}

/// Ship type text (ITU-R M.1371 table 53), coarse.
pub fn ship_type_text(t: u8) -> &'static str {
    match t {
        0 => "not available",
        20..=29 => "wing in ground",
        30 => "fishing",
        31 | 32 => "towing",
        33 => "dredging or underwater ops",
        34 => "diving ops",
        35 => "military ops",
        36 => "sailing",
        37 => "pleasure craft",
        40..=49 => "high speed craft",
        50 => "pilot vessel",
        51 => "search and rescue vessel",
        52 => "tug",
        53 => "port tender",
        54 => "anti-pollution equipment",
        55 => "law enforcement",
        58 => "medical transport",
        59 => "noncombatant ship",
        60..=69 => "passenger",
        70..=79 => "cargo",
        80..=89 => "tanker",
        90..=99 => "other",
        _ => "reserved",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_bit_alphabet() {
        let b = Bits::from_payload("0W`w", 0).expect("valid");
        assert_eq!(b.u(0, 6), Some(0));
        assert_eq!(b.u(6, 6), Some(39));
        assert_eq!(b.u(12, 6), Some(40));
        assert_eq!(b.u(18, 6), Some(63));
        assert!(Bits::from_payload("X", 0).is_err());
        assert_eq!(Bits::from_payload("0000", 2).expect("valid").len(), 22);
    }

    #[test]
    fn mmsi_rules() {
        assert!(mmsi_valid(366_999_712));
        assert!(mmsi_valid(371_798_000));
        assert!(mmsi_valid(970_123_456));
        assert!(mmsi_valid(111_366_001));
        assert!(!mmsi_valid(123_456_789));
        assert!(!mmsi_valid(0));
        assert!(!mmsi_valid(1_193_046));
        assert!(!mmsi_valid(999_999_999));
        assert!(!mmsi_valid(200_000_001));
    }
}
