//! SBS-1 BaseStation CSV lines, as dump1090 serves them on port 30003 (dump1090-fa's
//! `modesSendSBSOutput`, readsb's of the same name):
//!
//! ```text
//! MSG,<type>,<session>,1,<hex>,1,<date gen>,<time gen>,<date logged>,<time logged>,<callsign>,
//! <altitude ft>,<ground speed kt>,<track>,<lat>,<lon>,<vertical rate ft/min>,<squawk>,
//! <alert>,<emergency>,<spi>,<on ground>\r\n
//! ```
//!
//! Types: 1 identification, 2 surface position, 3 airborne position, 4 airborne velocity (the
//! four from ADS-B extended squitters), 5 surveillance altitude (DF4/20), 6 surveillance
//! identity (DF5/21), 7 air-to-air (DF0/16), 8 all-call reply (DF11). Flags are `-1` for true
//! and `0` for false, empty when the message does not carry them. Latitude and longitude have
//! five decimals, ground speed and track none, the squawk four digits, the callsign is as
//! broadcast (eight characters, padded with spaces).
//!
//! FleetMesh's own conventions on top (docs/design/traffic.md section 5): the session-id field
//! (third) is the receiver's number in `/sensors`; dates and times are UTC (readsb writes UTC;
//! dump1090-fa writes the host's local time, so run it with `TZ=UTC`); "generated" is the
//! transmission time and "logged" the receipt time; a non-ICAO address (a UAT anonymous,
//! self-assigned address) is written with a `~` prefix, as readsb does.

use chrono::{DateTime, Utc};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SbsMsg {
    pub msg_type: u8,
    pub session: u32,
    pub icao: u32,
    pub non_icao: bool,
    pub t_gen_ms: i64,
    pub t_log_ms: i64,
    pub callsign: Option<String>,
    pub alt_ft: Option<i32>,
    pub gs_kt: Option<f64>,
    pub track_deg: Option<f64>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub vr_fpm: Option<i32>,
    /// four octal digits as written (1200, 7700)
    pub squawk: Option<u16>,
    pub alert: Option<bool>,
    pub emergency: Option<bool>,
    pub spi: Option<bool>,
    pub ground: Option<bool>,
}

fn date_time(ms: i64) -> (String, String) {
    let t: DateTime<Utc> = DateTime::from_timestamp_millis(ms).unwrap_or_default();
    (t.format("%Y/%m/%d").to_string(), t.format("%H:%M:%S%.3f").to_string())
}

fn flag(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "-1",
        Some(false) => "0",
        None => "",
    }
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

impl SbsMsg {
    pub fn line(&self) -> String {
        let (dg, tg) = date_time(self.t_gen_ms);
        let (dl, tl) = date_time(self.t_log_ms);
        let hex = format!("{}{:06X}", if self.non_icao { "~" } else { "" }, self.icao & 0xFF_FFFF);
        let pos = match (self.lat, self.lon) {
            (Some(la), Some(lo)) => format!("{la:.5},{lo:.5}"),
            _ => ",".to_string(),
        };
        format!(
            "MSG,{},{},1,{hex},1,{dg},{tg},{dl},{tl},{},{},{},{},{pos},{},{},{},{},{},{}\r\n",
            self.msg_type,
            self.session,
            self.callsign.as_deref().unwrap_or(""),
            opt(self.alt_ft),
            self.gs_kt.map(|v| format!("{v:.0}")).unwrap_or_default(),
            self.track_deg.map(|v| format!("{v:.0}")).unwrap_or_default(),
            opt(self.vr_fpm),
            self.squawk.map(|s| format!("{s:04}")).unwrap_or_default(),
            flag(self.alert),
            flag(self.emergency),
            flag(self.spi),
            flag(self.ground),
        )
    }
}

/// A callsign padded to the eight characters ADS-B carries.
pub fn pad_callsign(c: &str) -> String {
    format!("{:<8}", c.chars().take(8).collect::<String>())
}

/// Pressure altitude as ADS-B reports it: feet, 25-foot steps.
pub fn alt_25ft(ft: f64) -> i32 {
    ((ft / 25.0).round() * 25.0) as i32
}

/// Vertical rate as ADS-B reports it: 64 ft/min steps.
pub fn vr_64fpm(fpm: f64) -> i32 {
    ((fpm / 64.0).round() * 64.0) as i32
}
