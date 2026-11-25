//! The JSON lines of docs/design/traffic.md section 5: Remote ID (`"type":"rid"`) and the
//! company's own sensor detections (`"type":"det"`). Unknown fields are ignored and absent
//! measurements are null or missing.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum JsonLine {
    #[serde(rename = "rid")]
    Rid(Rid),
    #[serde(rename = "det")]
    Det(Det),
}

/// Broadcast Remote ID (the ASTM F3411 fields).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Rid {
    /// The receiver.
    #[serde(deserialize_with = "string_or_number")]
    pub rx: String,
    /// Receipt time, Unix ms.
    pub t_rx: i64,
    /// The message's own timestamp, Unix ms.
    #[serde(default)]
    pub t: Option<i64>,
    pub uas_id: String,
    /// `serial`, `caa` (registration), `utm` or `session`.
    #[serde(default)]
    pub id_type: Option<String>,
    #[serde(default)]
    pub ua_type: Option<String>,
    /// `ground`, `airborne`, `emergency`, `failure` or `undeclared`.
    #[serde(default)]
    pub status: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Geodetic altitude (F3411: WGS84 ellipsoid height).
    #[serde(default)]
    pub alt_geo_m: Option<f64>,
    /// Pressure altitude.
    #[serde(default)]
    pub alt_baro_m: Option<f64>,
    #[serde(default)]
    pub height_agl_m: Option<f64>,
    #[serde(default)]
    pub speed_mps: Option<f64>,
    #[serde(default)]
    pub track_deg: Option<f64>,
    #[serde(default)]
    pub vspeed_mps: Option<f64>,
    /// Horizontal accuracy, metres (95%).
    #[serde(default)]
    pub h_acc_m: Option<f64>,
    /// Vertical accuracy, metres (95%).
    #[serde(default)]
    pub v_acc_m: Option<f64>,
    #[serde(default)]
    pub op_lat: Option<f64>,
    #[serde(default)]
    pub op_lon: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    /// Onboard detect-and-avoid; `platform` is the own vehicle's uid.
    Daa,
    /// Radar on an own boat.
    MarineRadar,
    /// Radar at a site; `platform` is the site id.
    GroundRadar,
}

impl SensorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SensorKind::Daa => "daa",
            SensorKind::MarineRadar => "marine_radar",
            SensorKind::GroundRadar => "ground_radar",
        }
    }
}

/// A detection by one of the company's own sensors.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Det {
    pub sensor: SensorKind,
    #[serde(deserialize_with = "string_or_number")]
    pub platform: String,
    /// Detection time, Unix ms.
    pub t: i64,
    #[serde(default)]
    pub sensor_lat: Option<f64>,
    #[serde(default)]
    pub sensor_lon: Option<f64>,
    #[serde(default)]
    pub sensor_alt_m: Option<f64>,
    /// True bearing from the sensor, degrees.
    pub bearing_deg: Option<f64>,
    #[serde(default)]
    pub sigma_bearing_deg: Option<f64>,
    #[serde(default)]
    pub elev_deg: Option<f64>,
    #[serde(default)]
    pub sigma_elev_deg: Option<f64>,
    #[serde(default)]
    pub range_m: Option<f64>,
    #[serde(default)]
    pub sigma_range_m: Option<f64>,
    /// Range rate, positive opening.
    #[serde(default)]
    pub radial_speed_mps: Option<f64>,
    #[serde(default)]
    pub sigma_radial_speed_mps: Option<f64>,
    /// `small`, `medium` or `large`.
    #[serde(default)]
    pub size: Option<String>,
}

fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Text(String),
        Int(i64),
        Float(f64),
    }
    Ok(match Raw::deserialize(d)? {
        Raw::Text(s) => s,
        Raw::Int(i) => i.to_string(),
        Raw::Float(f) => f.to_string(),
    })
}

pub fn parse(line: &str) -> Result<JsonLine, serde_json::Error> {
    serde_json::from_str(line.trim())
}
