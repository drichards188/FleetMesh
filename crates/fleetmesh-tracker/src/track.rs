//! The track the tracker produces: the JSON of docs/design/traffic.md section 5.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Air,
    Surface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Airliner,
    Bizjet,
    Ga,
    Helicopter,
    Seaplane,
    Drone,
    UnknownAir,
    Cargo,
    Tanker,
    Passenger,
    Ferry,
    Tug,
    Pilot,
    Fishing,
    Sailing,
    Pleasure,
    SmallCraft,
    UnknownSurface,
}

impl Class {
    pub fn unknown(domain: Domain) -> Class {
        match domain {
            Domain::Air => Class::UnknownAir,
            Domain::Surface => Class::UnknownSurface,
        }
    }

    pub fn is_unknown(self) -> bool {
        matches!(self, Class::UnknownAir | Class::UnknownSurface)
    }

    /// Class from an AIS ship type code. High-speed craft (40-49) are read as ferries: on San
    /// Francisco Bay the high-speed craft are the ferry fleet.
    pub fn from_ship_type(t: u8) -> Class {
        match t {
            30 => Class::Fishing,
            31 | 32 | 52 => Class::Tug,
            36 => Class::Sailing,
            37 => Class::Pleasure,
            40..=49 => Class::Ferry,
            50 => Class::Pilot,
            60..=69 => Class::Passenger,
            70..=79 => Class::Cargo,
            80..=89 => Class::Tanker,
            _ => Class::UnknownSurface,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AltRef {
    /// Pressure altitude (standard setting 1013.25 hPa), metres.
    Baro,
    /// Geometric altitude above mean sea level, metres.
    Geo,
    /// No altitude (surface traffic, or an air track whose altitude is unknown).
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackState {
    Tentative,
    Confirmed,
    Coasting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[serde(rename = "adsb_1090")]
    Adsb1090,
    AdsbUat,
    AisA,
    AisB,
    RemoteId,
    Daa,
    MarineRadar,
    GroundRadar,
}

impl SourceKind {
    pub fn cooperative(self) -> bool {
        !matches!(
            self,
            SourceKind::Daa | SourceKind::MarineRadar | SourceKind::GroundRadar
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub rx: String,
    pub last_t: i64,
}

/// Quality and integrity flags, serialised as strings in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// The track's own source is overdue (more than its stale factor times its normal interval).
    Stale,
    /// The identity is anonymous: a non-ICAO or Privacy ICAO Address, or a Remote ID session id.
    Anonymous,
    /// The aircraft is on the LADD list: its callsign is withheld from the output.
    Ladd,
    /// The anonymous identity changed and the track was carried over to the new one.
    IdChanged,
    /// A cooperative position disagrees with the company's own sensors (see README).
    SensorDisagree,
    /// A cooperative report jumped further than the track could have moved.
    PositionJump,
    InvalidMmsi,
    OnGround,
    Moored,
    AtAnchor,
    /// An emergency squawk (7500, 7600, 7700), the emergency bit, or a Remote ID emergency.
    Emergency,
    /// Bearing-only: the position is somewhere along a line (see `bearing_only`).
    BearingOnly,
    /// Position from bearing lines that crossed (several sensors, or one that moved).
    Triangulated,
}

/// Identity of a cooperative track. Absent fields are left out of the JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ids {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub icao24: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mmsi: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub uas_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub callsign: Option<String>,
    pub anonymous: bool,
}

/// What static messages gave: AIS types 5, 19 and 24 for vessels, plus a few Remote ID and
/// ADS-B items. Absent fields are left out, so a track with none is `{}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Static {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub callsign: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub imo: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ship_type: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ship_type_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub length_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub beam_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub draught_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub destination: Option<String>,
    /// ETA as sent, "MM-DDTHH:MMZ" (no year in AIS).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub eta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nav_status: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nav_status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mothership_mmsi: Option<u32>,
    /// ADS-B: the last squawk.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub squawk: Option<String>,
    /// Remote ID: `serial`, `caa`, `utm` or `session`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ua_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub op_lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub op_lon: Option<f64>,
}

/// The geometry of a bearing-only track: the line it lies on. Present only on tracks flagged
/// `bearing_only` (an addition to section 5, see README).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BearingLine {
    pub sensor_lat: f64,
    pub sensor_lon: f64,
    pub sensor_alt_m: f64,
    pub bearing_deg: f64,
    pub sigma_bearing_deg: f64,
    pub elev_deg: Option<f64>,
    /// Range interval along the line, metres: [min, max].
    pub range_m: [f64; 2],
    pub t: i64,
}

/// State behind a track, kept so [`crate::Tracker::predict`] can project it. Not serialised.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kinematics {
    /// [x, y, z, vx, vy, vz] in local metres at `t`.
    pub x: [f64; 6],
    pub p: [[f64; 6]; 6],
    pub qh: f64,
    pub qv: f64,
    pub surface: bool,
    pub valid: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    /// Time of this state, Unix ms (the picture's time, the last `update`).
    pub t: i64,
    pub domain: Domain,
    pub class: Class,
    pub cooperative: bool,
    pub ids: Ids,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: Option<f64>,
    pub alt_ref: AltRef,
    /// Horizontal 1-sigma, the semi-major axis of the position ellipse, metres.
    pub h_sigma_m: f64,
    pub v_sigma_m: Option<f64>,
    /// Velocity over ground [east, north], m/s.
    pub vel_mps: Option<[f64; 2]>,
    pub vs_mps: Option<f64>,
    /// AIS true heading when sent, else course over ground when moving, degrees.
    pub heading_deg: Option<f64>,
    pub state: TrackState,
    pub sources: Vec<Source>,
    pub flags: Vec<Flag>,
    #[serde(rename = "static")]
    pub static_: Static,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bearing_only: Option<BearingLine>,
    #[serde(skip)]
    pub kin: Kinematics,
}

impl Track {
    pub fn has_flag(&self, f: Flag) -> bool {
        self.flags.contains(&f)
    }
}

/// A track's predicted position.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    pub t: i64,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: Option<f64>,
    /// 1-sigma semi-major axis, metres.
    pub h_sigma_m: f64,
    /// 1-sigma semi-minor axis, metres.
    pub h_sigma_minor_m: f64,
    /// Direction of the semi-major axis, degrees true.
    pub h_sigma_dir_deg: f64,
    pub v_sigma_m: Option<f64>,
}

/// A cooperative track whose broadcast position disagrees with what the company's own sensors
/// see: a non-cooperative track nearby moving with it, while the sensor that sees that track does
/// not see anything at the broadcast position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Disagreement {
    /// The cooperative track.
    pub track: String,
    /// The non-cooperative track the sensors see instead.
    pub sensed: String,
    /// Distance between them, metres.
    pub offset_m: f64,
    /// The sensor that sees `sensed` and not `track`.
    pub sensor: String,
    pub since_t: i64,
}
