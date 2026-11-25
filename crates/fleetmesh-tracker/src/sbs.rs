//! SBS-1 BaseStation CSV, as dump1090 serves on port 30003 and FleetMesh's traffic simulator
//! serves on `FLEETMESH_TRAFFIC_SBS_PORT`.
//!
//! Field order (22 fields for `MSG`): message type, transmission type, session id, aircraft id,
//! hex ident, flight id, date generated, time generated, date logged, time logged, callsign,
//! altitude (ft, pressure), ground speed (kt), track (deg), latitude, longitude, vertical rate
//! (ft/min), squawk, alert, emergency, SPI, on ground. Flags are `0`, `1` or `-1` (true in
//! BaseStation's own output). dump1090 marks a non-ICAO (anonymous or TIS-B) address with a
//! leading `~`.

use crate::geo::days_from_civil;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SbsError {
    Empty,
    UnknownType(String),
    TooFewFields(usize),
    BadHex(String),
    BadNumber(&'static str),
}

impl std::fmt::Display for SbsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbsKind {
    /// `MSG`: a transmission from an aircraft.
    Msg,
    /// `SEL`: selection change (BaseStation user interface).
    Sel,
    /// `ID`: new callsign for an aircraft.
    Id,
    /// `AIR`: new aircraft.
    Air,
    /// `STA`: status change.
    Sta,
    /// `CLK`: clock.
    Clk,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SbsMessage {
    pub kind: SbsKind,
    /// 1 identification, 2 surface position, 3 airborne position, 4 airborne velocity,
    /// 5 surveillance altitude, 6 surveillance identity (squawk), 7 air to air, 8 all-call.
    pub transmission: Option<u8>,
    pub session_id: Option<String>,
    pub aircraft_id: Option<String>,
    /// 24-bit address.
    pub icao: Option<u32>,
    /// The address was marked non-ICAO (`~` prefix): anonymous, TIS-B or otherwise not an
    /// aircraft's own registered address.
    pub non_icao: bool,
    pub flight_id: Option<String>,
    /// "Date/time generated", Unix ms, read as UTC plus the caller's offset.
    pub generated_ms: Option<i64>,
    /// "Date/time logged", Unix ms, likewise.
    pub logged_ms: Option<i64>,
    pub callsign: Option<String>,
    pub altitude_ft: Option<f64>,
    pub ground_speed_kt: Option<f64>,
    pub track_deg: Option<f64>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub vertical_rate_fpm: Option<f64>,
    pub squawk: Option<String>,
    pub alert: Option<bool>,
    pub emergency: Option<bool>,
    pub spi: Option<bool>,
    pub on_ground: Option<bool>,
}

fn opt(s: Option<&&str>) -> Option<String> {
    s.map(|v| v.trim_end_matches(['\r', '\n']))
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.to_string())
}

fn num(s: Option<&&str>, what: &'static str) -> Result<Option<f64>, SbsError> {
    match s.map(|v| v.trim()).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => v
            .parse::<f64>()
            .map(Some)
            .map_err(|_| SbsError::BadNumber(what)),
    }
}

fn flag(s: Option<&&str>) -> Option<bool> {
    match s.map(|v| v.trim()).filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => v.parse::<i64>().ok().map(|n| n != 0),
    }
}

/// "2008/11/28" and "14:53:50.594" to Unix ms, read as UTC shifted by `utc_offset_min` (the
/// minutes the writer's clock is ahead of UTC; 0 for the simulator).
pub fn parse_datetime(date: &str, time: &str, utc_offset_min: i32) -> Option<i64> {
    let mut d = date.trim().split(['/', '-']);
    let y: i64 = d.next()?.parse().ok()?;
    let mo: u32 = d.next()?.parse().ok()?;
    let da: u32 = d.next()?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) {
        return None;
    }
    let mut t = time.trim().split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let sec: f64 = t.next()?.parse().ok()?;
    if h > 23 || mi > 59 || !(0.0..61.0).contains(&sec) {
        return None;
    }
    let days = days_from_civil(y, mo, da);
    let ms = ((days * 86_400 + h * 3600 + mi * 60) * 1000) as f64 + (sec * 1000.0).round();
    Some(ms as i64 - utc_offset_min as i64 * 60_000)
}

pub fn parse(line: &str, utc_offset_min: i32) -> Result<SbsMessage, SbsError> {
    let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if line.is_empty() {
        return Err(SbsError::Empty);
    }
    let f: Vec<&str> = line.split(',').collect();
    let kind = match f[0].trim() {
        "MSG" => SbsKind::Msg,
        "SEL" => SbsKind::Sel,
        "ID" => SbsKind::Id,
        "AIR" => SbsKind::Air,
        "STA" => SbsKind::Sta,
        "CLK" => SbsKind::Clk,
        other => return Err(SbsError::UnknownType(other.chars().take(8).collect())),
    };
    if f.len() < 5 {
        return Err(SbsError::TooFewFields(f.len()));
    }
    let transmission = if kind == SbsKind::Msg {
        Some(
            f[1].trim()
                .parse::<u8>()
                .map_err(|_| SbsError::BadNumber("transmission type"))?,
        )
    } else {
        None
    };
    if kind == SbsKind::Msg && f.len() < 16 {
        return Err(SbsError::TooFewFields(f.len()));
    }
    let hex_raw = f[4].trim();
    let (non_icao, hex) = match hex_raw.strip_prefix('~') {
        Some(h) => (true, h),
        None => (false, hex_raw),
    };
    let icao = if hex.is_empty() {
        None
    } else {
        let v = u32::from_str_radix(hex, 16)
            .map_err(|_| SbsError::BadHex(hex.chars().take(8).collect()))?;
        if v > 0xFF_FFFF {
            return Err(SbsError::BadHex(hex.to_string()));
        }
        Some(v)
    };
    let dt = |di: usize, ti: usize| match (f.get(di), f.get(ti)) {
        (Some(d), Some(t)) if !d.trim().is_empty() && !t.trim().is_empty() => {
            parse_datetime(d, t, utc_offset_min)
        }
        _ => None,
    };
    let lat = num(f.get(14), "latitude")?;
    let lon = num(f.get(15), "longitude")?;
    let (lat, lon) = match (lat, lon) {
        (Some(a), Some(o)) if a.abs() <= 90.0 && o.abs() <= 180.0 && !(a == 0.0 && o == 0.0) => {
            (Some(a), Some(o))
        }
        _ => (None, None),
    };
    // ID lines carry the callsign in field 10 too.
    let callsign = opt(f.get(10))
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    Ok(SbsMessage {
        kind,
        transmission,
        session_id: opt(f.get(2)).map(|s| s.trim().to_string()),
        aircraft_id: opt(f.get(3)).map(|s| s.trim().to_string()),
        icao,
        non_icao,
        flight_id: opt(f.get(5)).map(|s| s.trim().to_string()),
        generated_ms: dt(6, 7),
        logged_ms: dt(8, 9),
        callsign,
        altitude_ft: num(f.get(11), "altitude")?,
        ground_speed_kt: num(f.get(12), "ground speed")?,
        track_deg: num(f.get(13), "track")?,
        lat,
        lon,
        vertical_rate_fpm: num(f.get(16), "vertical rate")?,
        squawk: opt(f.get(17)).map(|s| s.trim().to_string()),
        alert: flag(f.get(18)),
        emergency: flag(f.get(19)),
        spi: flag(f.get(20)),
        on_ground: flag(f.get(21)),
    })
}

/// Text for an address as the track JSON carries it: six lower-case hex digits.
pub fn icao_hex(icao: u32) -> String {
    format!("{icao:06x}")
}
