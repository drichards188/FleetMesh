//! NMEA 0183 `!AIVDM` sentences for AIS messages 1, 2, 3, 5, 18, 19 and 24 (ITU-R M.1371-5),
//! with the 6-bit payload armouring, multi-sentence fragmentation, checksums and an NMEA 4.10
//! tag block (`\s:<source>,c:<unix seconds>*hh\`) on each sentence.

/// Bits, most significant first.
#[derive(Default, Clone, Debug)]
pub struct Bits {
    bits: Vec<bool>,
}

impl Bits {
    pub fn new() -> Bits {
        Bits::default()
    }

    pub fn len(&self) -> usize {
        self.bits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    /// The low `n` bits of `v` (two's complement for a negative value).
    pub fn u(&mut self, n: u32, v: u64) -> &mut Bits {
        for i in (0..n).rev() {
            self.bits.push(i < 64 && (v >> i) & 1 == 1);
        }
        self
    }

    pub fn i(&mut self, n: u32, v: i64) -> &mut Bits {
        let mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
        self.u(n, (v as u64) & mask)
    }

    pub fn b(&mut self, v: bool) -> &mut Bits {
        self.bits.push(v);
        self
    }

    /// Text in the AIS 6-bit character set, padded with `@` (0) to `chars` characters.
    pub fn text(&mut self, chars: usize, s: &str) -> &mut Bits {
        let mut it = s.chars();
        for _ in 0..chars {
            let v = it.next().map_or(0, sixbit_char);
            self.u(6, u64::from(v));
        }
        self
    }

    /// The armoured payload and its fill bits.
    pub fn armour(&self) -> (String, u8) {
        let fill = (6 - self.bits.len() % 6) % 6;
        let mut out = String::with_capacity(self.bits.len().div_ceil(6));
        for chunk in self.bits.chunks(6) {
            let mut v = 0u8;
            for k in 0..6 {
                v <<= 1;
                if chunk.get(k).copied().unwrap_or(false) {
                    v |= 1;
                }
            }
            out.push(armour_char(v));
        }
        (out, fill as u8)
    }
}

/// A character of the AIS 6-bit text set (ITU-R M.1371 table 47): `@A-Z[\]^_` then space to
/// `?`; a lowercase letter is sent as upper case, anything else as a space.
pub fn sixbit_char(c: char) -> u8 {
    let c = c.to_ascii_uppercase();
    let b = if c.is_ascii() { c as u8 } else { b' ' };
    match b {
        64..=95 => b - 64,
        32..=63 => b,
        _ => 32,
    }
}

/// The payload character for a 6-bit value (`0`-`W`, then `` ` ``-`w`).
pub fn armour_char(v: u8) -> char {
    let v = v & 0x3f;
    (if v < 40 { v + 48 } else { v + 56 }) as char
}

/// XOR of the bytes, as two upper-case hex digits.
pub fn checksum(s: &str) -> String {
    let x = s.bytes().fold(0u8, |a, b| a ^ b);
    format!("{x:02X}")
}

/// The sentences of one message: `!AIVDM,<count>,<n>,<seq>,<channel>,<payload>,<fill>*hh`, each
/// prefixed with the tag block, each ending in CR LF. A payload longer than `max_chars` is
/// split (60 characters a fragment is what most transponders do, and keeps every sentence
/// within NMEA's 82 characters).
pub fn sentences(bits: &Bits, channel: char, seq: u8, tag: Option<(&str, i64)>, max_chars: usize) -> Vec<String> {
    let (payload, fill) = bits.armour();
    let chunks: Vec<&str> = if payload.len() <= max_chars {
        vec![payload.as_str()]
    } else {
        payload.as_bytes().chunks(max_chars).map(|c| std::str::from_utf8(c).unwrap_or("")).collect()
    };
    let count = chunks.len();
    let block = tag.map(|(s, c)| {
        let body = format!("s:{s},c:{c}");
        format!("\\{body}*{}\\", checksum(&body))
    });
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let seq_field = if count > 1 { ((b'0' + seq % 10) as char).to_string() } else { String::new() };
            let f = if i + 1 == count { fill } else { 0 };
            let body = format!("AIVDM,{count},{},{seq_field},{channel},{chunk},{f}", i + 1);
            format!("{}!{body}*{}\r\n", block.as_deref().unwrap_or(""), checksum(&body))
        })
        .collect()
}

/// Longitude and latitude in 1/10 000 minute, unavailable as 181 and 91 degrees.
fn lon_field(lon: Option<f64>) -> i64 {
    lon.map_or(0x6791AC0, |v| (v * 600_000.0).round() as i64)
}

fn lat_field(lat: Option<f64>) -> i64 {
    lat.map_or(0x3412140, |v| (v * 600_000.0).round() as i64)
}

/// Speed over ground in 0.1 knot, 1023 unavailable, 1022 for 102.2 knots or more.
pub fn sog_field(kn: Option<f64>) -> u64 {
    kn.map_or(1023, |v| ((v * 10.0).round().max(0.0) as u64).min(1022))
}

/// Course over ground in 0.1 degree, 3600 unavailable.
pub fn cog_field(deg: Option<f64>) -> u64 {
    deg.map_or(3600, |v| ((crate::geo::wrap360(v) * 10.0).round() as u64) % 3600)
}

/// Rate of turn as M.1371 encodes it: 4.733 sqrt(degrees a minute), signed, +-127 for a turn
/// beyond 708 degrees a minute; -128 when there is no turn indicator.
pub fn rot_field(deg_per_min: Option<f64>) -> i64 {
    match deg_per_min {
        None => -128,
        Some(r) => {
            let v = (4.733 * r.abs().sqrt()).round().min(126.0);
            (if r < 0.0 { -v } else { v }) as i64
        }
    }
}

/// Messages 1, 2 and 3: the Class A position report.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionA {
    pub msg_type: u8,
    pub repeat: u8,
    pub mmsi: u32,
    pub status: u8,
    pub rot: i64,
    pub sog: u64,
    pub accuracy: bool,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
    pub cog: u64,
    pub heading: u16,
    pub second: u8,
    pub maneuver: u8,
    pub raim: bool,
    /// the 19-bit communication state (SOTDMA for 1 and 2, ITDMA for 3)
    pub radio: u32,
}

impl PositionA {
    pub fn bits(&self) -> Bits {
        let mut b = Bits::new();
        b.u(6, u64::from(self.msg_type))
            .u(2, u64::from(self.repeat))
            .u(30, u64::from(self.mmsi))
            .u(4, u64::from(self.status))
            .i(8, self.rot)
            .u(10, self.sog)
            .b(self.accuracy)
            .i(28, lon_field(self.lon))
            .i(27, lat_field(self.lat))
            .u(12, self.cog)
            .u(9, u64::from(self.heading))
            .u(6, u64::from(self.second))
            .u(2, u64::from(self.maneuver))
            .u(3, 0)
            .b(self.raim)
            .u(19, u64::from(self.radio));
        b
    }
}

/// Message 5: static and voyage related data (424 bits, two sentences).
#[derive(Clone, Debug, PartialEq)]
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
    pub eta_month: u8,
    pub eta_day: u8,
    pub eta_hour: u8,
    pub eta_minute: u8,
    /// 0.1 metre
    pub draught_dm: u8,
    pub destination: String,
    pub dte: bool,
}

impl StaticVoyage {
    pub fn bits(&self) -> Bits {
        let mut b = Bits::new();
        b.u(6, 5)
            .u(2, u64::from(self.repeat))
            .u(30, u64::from(self.mmsi))
            .u(2, u64::from(self.ais_version))
            .u(30, u64::from(self.imo))
            .text(7, &self.callsign)
            .text(20, &self.name)
            .u(8, u64::from(self.ship_type))
            .u(9, u64::from(self.to_bow))
            .u(9, u64::from(self.to_stern))
            .u(6, u64::from(self.to_port))
            .u(6, u64::from(self.to_starboard))
            .u(4, u64::from(self.epfd))
            .u(4, u64::from(self.eta_month))
            .u(5, u64::from(self.eta_day))
            .u(5, u64::from(self.eta_hour))
            .u(6, u64::from(self.eta_minute))
            .u(8, u64::from(self.draught_dm))
            .text(20, &self.destination)
            .b(self.dte)
            .u(1, 0);
        b
    }
}

/// Message 18: the Class B position report.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionB {
    pub repeat: u8,
    pub mmsi: u32,
    pub sog: u64,
    pub accuracy: bool,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
    pub cog: u64,
    pub heading: u16,
    pub second: u8,
    /// carrier-sense unit
    pub cs: bool,
    pub display: bool,
    pub dsc: bool,
    pub band: bool,
    pub msg22: bool,
    pub assigned: bool,
    pub raim: bool,
    /// the communication state selector flag and the 19-bit state
    pub radio: u32,
}

/// The fixed communication state a Class B CS unit sends (selector 1, `1100000000000000110`).
pub const CS_RADIO: u32 = (1 << 19) | 0b110_0000_0000_0000_0110;

impl PositionB {
    pub fn bits(&self) -> Bits {
        let mut b = Bits::new();
        b.u(6, 18)
            .u(2, u64::from(self.repeat))
            .u(30, u64::from(self.mmsi))
            .u(8, 0)
            .u(10, self.sog)
            .b(self.accuracy)
            .i(28, lon_field(self.lon))
            .i(27, lat_field(self.lat))
            .u(12, self.cog)
            .u(9, u64::from(self.heading))
            .u(6, u64::from(self.second))
            .u(2, 0)
            .b(self.cs)
            .b(self.display)
            .b(self.dsc)
            .b(self.band)
            .b(self.msg22)
            .b(self.assigned)
            .b(self.raim)
            .u(20, u64::from(self.radio));
        b
    }
}

/// Message 19: the extended Class B position report (312 bits).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtendedB {
    pub repeat: u8,
    pub mmsi: u32,
    /// regional reserved bits (8 after the MMSI, 4 after the time stamp): 0 unless a regional
    /// application uses them
    pub regional: (u8, u8),
    pub sog: u64,
    pub accuracy: bool,
    pub lon: Option<f64>,
    pub lat: Option<f64>,
    pub cog: u64,
    pub heading: u16,
    pub second: u8,
    pub name: String,
    pub ship_type: u8,
    pub to_bow: u16,
    pub to_stern: u16,
    pub to_port: u8,
    pub to_starboard: u8,
    pub epfd: u8,
    pub raim: bool,
    pub dte: bool,
    pub assigned: bool,
}

impl ExtendedB {
    pub fn bits(&self) -> Bits {
        let mut b = Bits::new();
        b.u(6, 19)
            .u(2, u64::from(self.repeat))
            .u(30, u64::from(self.mmsi))
            .u(8, u64::from(self.regional.0))
            .u(10, self.sog)
            .b(self.accuracy)
            .i(28, lon_field(self.lon))
            .i(27, lat_field(self.lat))
            .u(12, self.cog)
            .u(9, u64::from(self.heading))
            .u(6, u64::from(self.second))
            .u(4, u64::from(self.regional.1 & 0xf))
            .text(20, &self.name)
            .u(8, u64::from(self.ship_type))
            .u(9, u64::from(self.to_bow))
            .u(9, u64::from(self.to_stern))
            .u(6, u64::from(self.to_port))
            .u(6, u64::from(self.to_starboard))
            .u(4, u64::from(self.epfd))
            .b(self.raim)
            .b(self.dte)
            .b(self.assigned)
            .u(4, 0);
        b
    }
}

/// Message 24 part A: the name (168 bits: 160 and 8 spare, M.1371-5).
#[derive(Clone, Debug, PartialEq)]
pub struct StaticA {
    pub repeat: u8,
    pub mmsi: u32,
    pub name: String,
}

impl StaticA {
    pub fn bits(&self) -> Bits {
        let mut b = Bits::new();
        b.u(6, 24).u(2, u64::from(self.repeat)).u(30, u64::from(self.mmsi)).u(2, 0).text(20, &self.name).u(8, 0);
        b
    }
}

/// Message 24 part B: type, vendor, call sign and dimensions (168 bits).
#[derive(Clone, Debug, PartialEq)]
pub struct StaticB {
    pub repeat: u8,
    pub mmsi: u32,
    pub ship_type: u8,
    pub vendor: String,
    pub model: u8,
    pub serial: u32,
    pub callsign: String,
    pub to_bow: u16,
    pub to_stern: u16,
    pub to_port: u8,
    pub to_starboard: u8,
    pub epfd: u8,
}

impl StaticB {
    pub fn bits(&self) -> Bits {
        let mut b = Bits::new();
        b.u(6, 24)
            .u(2, u64::from(self.repeat))
            .u(30, u64::from(self.mmsi))
            .u(2, 1)
            .u(8, u64::from(self.ship_type))
            .text(3, &self.vendor)
            .u(4, u64::from(self.model))
            .u(20, u64::from(self.serial))
            .text(7, &self.callsign)
            .u(9, u64::from(self.to_bow))
            .u(9, u64::from(self.to_stern))
            .u(6, u64::from(self.to_port))
            .u(6, u64::from(self.to_starboard))
            .u(4, u64::from(self.epfd))
            .u(2, 0);
        b
    }
}

/// A SOTDMA communication state (messages 1 and 2): UTC direct sync, the slot time-out
/// counting down 7 to 0 frame by frame, and the sub-message it selects (received stations
/// for 3, 5 and 7; the slot number for 2, 4 and 6; the UTC hour and minute for 1; the slot
/// offset for 0).
pub fn sotdma(timeout: u8, slot: u16, stations: u16, utc_hour: u8, utc_min: u8, offset: u16) -> u32 {
    let sub: u32 = match timeout {
        3 | 5 | 7 => u32::from(stations.min(16383)),
        2 | 4 | 6 => u32::from(slot % 2250),
        1 => (u32::from(utc_hour) << 9) | (u32::from(utc_min) << 2),
        _ => u32::from(offset),
    };
    (u32::from(timeout & 7) << 14) | (sub & 0x3fff)
}

/// An ITDMA communication state (message 3): UTC direct sync, a slot increment, the number of
/// slots and the keep flag.
pub fn itdma(increment: u16, slots: u8, keep: bool) -> u32 {
    (u32::from(increment & 0x1fff) << 4) | (u32::from(slots & 7) << 1) | u32::from(keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armour_and_text_follow_the_tables() {
        assert_eq!(armour_char(0), '0');
        assert_eq!(armour_char(39), 'W');
        assert_eq!(armour_char(40), '`');
        assert_eq!(armour_char(63), 'w');
        assert_eq!(sixbit_char('@'), 0);
        assert_eq!(sixbit_char('A'), 1);
        assert_eq!(sixbit_char(' '), 32);
        assert_eq!(sixbit_char('9'), 57);
        assert_eq!(rot_field(Some(0.0)), 0);
        assert_eq!(rot_field(Some(-30.0)), -26);
        assert_eq!(rot_field(None), -128);
    }

    #[test]
    fn a_tag_block_has_its_own_checksum_and_long_payloads_split() {
        let mut b = Bits::new();
        for _ in 0..424 {
            b.b(true);
        }
        let s = sentences(&b, 'A', 3, Some(("hub-pier", 1781456400)), 60);
        assert_eq!(s.len(), 2);
        assert!(s[0].starts_with("\\s:hub-pier,c:1781456400*"));
        assert!(s[0].contains("!AIVDM,2,1,3,A,"));
        assert!(s[1].contains("!AIVDM,2,2,3,A,"));
        assert!(s[1].trim_end().contains(",2*"), "424 bits leave two fill bits: {}", s[1]);
        for line in &s {
            assert!(line.ends_with("\r\n"));
            let bang = line.find('!').expect("sentence");
            let star = line.rfind('*').expect("checksum");
            assert_eq!(&line[star + 1..star + 3], checksum(&line[bang + 1..star]));
        }
    }
}
