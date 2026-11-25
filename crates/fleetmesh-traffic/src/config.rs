//! The two configuration files: the shared world (`sim/traffic.toml`: region, terrain, airports
//! and their flows, the other traffic streams, equipage rates, receivers and sensors) and a
//! scenario's own (`sim/scenarios/<name>/traffic.toml`: seed, length, rate multipliers per
//! stream, scripted issue events).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::terrain::Hill;

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldConfig {
    pub world: WorldSection,
    #[serde(default)]
    pub terrain: TerrainCfg,
    #[serde(default)]
    pub addresses: AddressCfg,
    #[serde(default)]
    pub airport: Vec<AirportCfg>,
    #[serde(default)]
    pub ships: ShipsCfg,
    #[serde(default)]
    pub stream: Vec<StreamCfg>,
    #[serde(default)]
    pub equipage: BTreeMap<String, EquipCfg>,
    pub receivers: ReceiversCfg,
    #[serde(default)]
    pub network: Vec<NetworkCfg>,
    #[serde(default)]
    pub onboard: OnboardCfg,
    #[serde(default)]
    pub ground_radar: Vec<GroundRadarCfg>,
    /// the directory the file was read from (relative paths in it resolve against this)
    #[serde(skip)]
    pub dir: PathBuf,
}

impl WorldConfig {
    pub fn load(path: &Path) -> Result<WorldConfig, String> {
        let mut c: WorldConfig = read_toml(path)?;
        c.dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(c)
    }

    pub fn resolve(&self, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.is_absolute() { p.to_path_buf() } else { self.dir.join(p) }
    }

    pub fn stream(&self, name: &str) -> Option<&StreamCfg> {
        self.stream.iter().find(|s| s.name == name)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldSection {
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
    /// local time offset from UTC, hours (-7 for Pacific daylight time)
    #[serde(default)]
    pub utc_offset_h: f64,
    /// altimeter setting, for pressure altitudes
    #[serde(default = "d_qnh")]
    pub qnh_hpa: f64,
    /// how long before the start the schedules begin, so the world starts warm
    #[serde(default = "d_warmup")]
    pub warmup_s: f64,
    /// the schedules' length when the scenario does not give one (live runs)
    #[serde(default = "d_horizon")]
    pub horizon_s: f64,
    /// geoid height above the WGS84 ellipsoid (EGM96 about -32 m on the Bay): Remote ID's
    /// geodetic altitude is ellipsoid height = mean sea level height + this
    #[serde(default = "d_geoid")]
    pub geoid_n_m: f64,
}

fn d_geoid() -> f64 {
    -32.0
}

/// Address blocks the identities are drawn from.
#[derive(Debug, Clone, Deserialize)]
pub struct AddressCfg {
    /// Privacy ICAO Addresses: one contiguous block, a fresh address each flight (illustrative:
    /// the FAA does not publish its pool)
    #[serde(default = "d_pia")]
    pub pia_block: [u32; 2],
    /// the third-party callsign a PIA flight uses instead of its registration
    #[serde(default = "d_pia_prefix")]
    pub pia_callsign_prefix: String,
    /// US government and military aircraft (a Coast Guard helicopter)
    #[serde(default = "d_mil")]
    pub military_block: [u32; 2],
}

fn d_pia() -> [u32; 2] {
    [0xAD_F800, 0xAD_FBFF]
}
fn d_pia_prefix() -> String {
    "XPV".into()
}
fn d_mil() -> [u32; 2] {
    [0xAE_0000, 0xAF_FFFF]
}

impl Default for AddressCfg {
    fn default() -> Self {
        AddressCfg { pia_block: d_pia(), pia_callsign_prefix: d_pia_prefix(), military_block: d_mil() }
    }
}

fn d_qnh() -> f64 {
    1013.25
}
fn d_warmup() -> f64 {
    4.0 * 3600.0
}
fn d_horizon() -> f64 {
    48.0 * 3600.0
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TerrainCfg {
    /// the baked surface grid, relative to the config file
    pub grid: Option<String>,
    #[serde(default = "d_k")]
    pub k: f64,
    #[serde(default = "d_sample")]
    pub sample_m: f64,
    #[serde(default)]
    pub hill: Vec<Hill>,
}

fn d_k() -> f64 {
    4.0 / 3.0
}
fn d_sample() -> f64 {
    60.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunwayCfg {
    pub id: String,
    /// the threshold
    pub lat: f64,
    pub lon: f64,
    /// true heading of the runway in the direction of use
    pub heading_deg: f64,
    pub length_m: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AirportCfg {
    pub id: String,
    pub elevation_m: f64,
    pub runway: Vec<RunwayCfg>,
    /// ICAO airline designators and their shares of this airport's airline flights
    #[serde(default)]
    pub airlines: Vec<(String, f64)>,
    /// share of airline flights by foreign carriers (an ICAO address outside the US block)
    #[serde(default)]
    pub foreign_share: f64,
}

/// A route: points `[lat, lon]`, `[lat, lon, alt_ft, kt]` or `[lat, lon, alt_ft, kt, hold_s]`.
/// A negative altitude is on the ground (or water) at that height; a hold waits at the point.
#[derive(Debug, Clone, Deserialize)]
pub struct RouteCfg {
    #[serde(default)]
    pub name: String,
    #[serde(default = "d_one")]
    pub weight: f64,
    pub points: Vec<Vec<f64>>,
}

fn d_one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpotCfg {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub ground_m: f64,
    #[serde(default = "d_one")]
    pub weight: f64,
    /// the way out to open water `[[lat, lon], ...]` (a marina's channel), taken back in reverse
    #[serde(default)]
    pub via: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StopCfg {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub dwell_min: f64,
}

/// One stream of traffic. `kind` picks the generator: `arrivals`, `departures`, `overflights`,
/// `route`, `wander`, `drones`, `ships`, `ferry`, `moored`, `pilot_station`. Fields a kind does
/// not use are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StreamCfg {
    pub name: String,
    pub kind: String,
    pub enabled: Option<bool>,
    pub class: Option<String>,
    /// class shares, for streams of more than one class
    pub mix: BTreeMap<String, f64>,
    pub per_hour: f64,
    pub per_hour_out: f64,
    pub count: usize,
    pub airport: Option<String>,
    pub runways: Vec<String>,
    pub min_gap_s: f64,
    pub final_nm: f64,
    pub routes: Vec<RouteCfg>,
    /// waypoints are moved up to this far at random, per object
    pub jitter_m: f64,
    /// flown either way round
    pub bidirectional: bool,
    /// local hours the stream runs, [from, to)
    pub active_local_h: Option<[f64; 2]>,
    pub alt_ft: Option<[f64; 2]>,
    pub speed_kt: Option<[f64; 2]>,
    pub session_min: Option<[f64; 2]>,
    pub agl_m: Option<[f64; 2]>,
    /// a polygon `[[lat, lon], ...]` the stream wanders in
    pub area: Vec<[f64; 2]>,
    pub spots: Vec<SpotCfg>,
    /// circles `[lat, lon, radius_m]` a wandering craft keeps out of (islands)
    pub avoid: Vec<[f64; 3]>,
    /// `n_number`, `airline`, `prefix:<p>` (a prefix and digits), `none`
    pub callsign: Option<String>,
    /// a fixed squawk (1200 for VFR); otherwise a discrete code
    pub squawk: Option<u16>,
    pub stops: Vec<StopCfg>,
    /// the route between consecutive stops: `legs[i]` runs from `stops[i]` to `stops[i + 1]`
    pub legs: Vec<Vec<[f64; 2]>>,
    pub headway_min: f64,
    pub vessels: usize,
    pub names: Vec<String>,
    /// `[lat, lon, heading]`
    pub positions: Vec<Vec<f64>>,
    pub status: Option<String>,
    pub length_m: Option<[f64; 2]>,
    /// ships: the path from the Golden Gate to the berths, and back
    pub inbound: Vec<Vec<f64>>,
    pub outbound: Vec<Vec<f64>>,
    pub berths: Vec<Vec<f64>>,
    pub dwell_h: Option<[f64; 2]>,
    pub destination: Option<String>,
    pub tugs: usize,
    pub tug_base: Option<[f64; 2]>,
    /// the index in `inbound` where escort tugs join
    pub tug_meet: usize,
    pub wander_m: f64,
}

impl StreamCfg {
    pub fn on(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// Deep-draft approaches through the traffic separation scheme, the bar channel and the pilot
/// station, shared by every ship stream.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShipsCfg {
    pub pilot_station: Option<[f64; 2]>,
    pub pilot_base: Option<[f64; 2]>,
    pub approach: Vec<ApproachCfg>,
    /// pilot station to just inside the Golden Gate, inbound and outbound `[lat, lon, kt]`
    pub bar_inbound: Vec<Vec<f64>>,
    pub bar_outbound: Vec<Vec<f64>>,
    /// the pilot launches' way from their base out to the station `[[lat, lon], ...]`
    pub launch_route: Vec<[f64; 2]>,
    pub sea_kt: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ApproachCfg {
    pub name: String,
    #[serde(default = "d_one")]
    pub weight: f64,
    /// from sea to the pilot station `[lat, lon]`
    pub inbound: Vec<[f64; 2]>,
    /// from the pilot station to sea
    pub outbound: Vec<[f64; 2]>,
}

/// Equipage shares for a class. ADS-B: `adsb_1090`, `uat`, `mode_s_only` (a transponder that
/// answers radar but broadcasts no position); the rest has none. AIS: `ais_a`, `ais_b`; the rest
/// none. `rid`: broadcast Remote ID.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EquipCfg {
    pub adsb_1090: f64,
    pub uat: f64,
    pub mode_s_only: f64,
    /// of 1090 equipped: a privacy ICAO address (PIA)
    pub pia: f64,
    /// callsign blocked from distribution (LADD)
    pub ladd: f64,
    /// of UAT equipped: the anonymous mode (self-assigned address, no callsign)
    pub uat_anon: f64,
    pub ais_a: f64,
    pub ais_b: f64,
    /// of AIS equipped: switched off for the whole run
    pub ais_off: f64,
    /// of Class B: also sends message 19
    pub type19: f64,
    pub rid: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RxKindCfg {
    pub range_m: f64,
    #[serde(default)]
    pub base_loss: f64,
    #[serde(default = "d_latency")]
    pub latency_ms: [i64; 2],
    /// AIS: Class B transmits 2 W against Class A's 12.5 W
    #[serde(default)]
    pub class_b_range_m: Option<f64>,
    /// AIS: collision loss per unit of channel load (hidden stations)
    #[serde(default)]
    pub slot_factor: f64,
}

fn d_latency() -> [i64; 2] {
    [5, 60]
}

/// The company's receivers: one of each kind at each hub in `hubs`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReceiversCfg {
    pub hubs: String,
    pub kinds: Vec<String>,
    #[serde(default = "d_mast")]
    pub mast_m: f64,
    #[serde(default = "d_ground")]
    pub ground_m: f64,
    pub adsb_1090: RxKindCfg,
    pub adsb_uat: RxKindCfg,
    pub ais: RxKindCfg,
    pub rid: RxKindCfg,
}

fn d_mast() -> f64 {
    10.0
}
fn d_ground() -> f64 {
    3.0
}

/// A commercial aggregated feed: many feeders, one stream, a few seconds late.
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkCfg {
    pub id: String,
    pub kind: String,
    pub delay_s: [f64; 2],
    #[serde(default)]
    pub base_loss: f64,
    pub range_m: f64,
    /// honours LADD: no callsign for a blocked aircraft
    #[serde(default)]
    pub ladd: bool,
    /// `[lat, lon, antenna metres above sea level]`
    pub feeders: Vec<[f64; 3]>,
    #[serde(default = "d_true")]
    pub enabled: bool,
}

fn d_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanCfg {
    pub period_s: f64,
    pub sigma_bearing_deg: f64,
    #[serde(default)]
    pub sigma_elev_deg: Option<f64>,
    /// metres plus a fraction of the range
    #[serde(default)]
    pub sigma_range_m: Option<f64>,
    #[serde(default)]
    pub sigma_range_frac: f64,
    #[serde(default)]
    pub doppler: bool,
    #[serde(default)]
    pub sigma_speed_mps: f64,
    /// range of 50 % detection by class
    pub r50_m: BTreeMap<String, f64>,
    /// how steeply detection falls off beyond r50
    #[serde(default = "d_slope")]
    pub slope: f64,
    #[serde(default)]
    pub min_range_m: f64,
    #[serde(default = "d_fov")]
    pub fov_up_deg: f64,
    #[serde(default = "d_fov")]
    pub fov_down_deg: f64,
    /// false detections a scan (Poisson mean)
    #[serde(default)]
    pub false_per_scan: f64,
    #[serde(default = "d_latency_det")]
    pub latency_ms: i64,
    /// the antenna or camera above the platform's reported altitude
    #[serde(default)]
    pub mount_m: f64,
}

fn d_slope() -> f64 {
    6.0
}
fn d_fov() -> f64 {
    90.0
}
fn d_latency_det() -> i64 {
    150
}

#[derive(Debug, Clone, Deserialize)]
pub struct OnboardCfg {
    #[serde(default = "d_true")]
    pub rid: bool,
    #[serde(default = "d_rid_range")]
    pub rid_range_m: f64,
    pub daa: Option<ScanCfg>,
    pub marine_radar: Option<ScanCfg>,
}

fn d_rid_range() -> f64 {
    800.0
}

impl Default for OnboardCfg {
    fn default() -> Self {
        OnboardCfg { rid: true, rid_range_m: d_rid_range(), daa: None, marine_radar: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroundRadarCfg {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f64,
    pub range_m: f64,
    #[serde(flatten)]
    pub scan: ScanCfg,
}

/// A scenario's traffic: `sim/scenarios/<name>/traffic.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ScenarioCfg {
    pub name: String,
    pub seed: Option<u64>,
    /// RFC 3339; the start when the caller gives none
    pub at: Option<String>,
    pub duration_s: Option<f64>,
    /// multipliers of streams' rates (and populations), by stream name
    pub rates: BTreeMap<String, f64>,
    /// streams switched off
    pub disable: Vec<String>,
    pub issue: Vec<IssueCfg>,
}

impl ScenarioCfg {
    pub fn load(path: &Path) -> Result<ScenarioCfg, String> {
        read_toml(path)
    }

    pub fn rate(&self, stream: &str) -> f64 {
        self.rates.get(stream).copied().unwrap_or(1.0)
    }
}

/// A scripted issue: `drone_no_rid`, `ship_arrival`, `adsb_bad_position`, `receiver_down`,
/// `ais_off`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IssueCfg {
    pub kind: String,
    pub name: String,
    pub at_s: f64,
    pub duration_s: f64,
    pub lat: f64,
    pub lon: f64,
    pub ground_m: f64,
    pub agl_m: f64,
    /// a receiver id (`hub-pier`, `net-adsb`), with `sensor` its kind for a hub
    pub receiver: String,
    pub sensor: String,
    /// the stream whose route an issue object follows, or a ship's destination stream
    pub stream: String,
    pub class: String,
    pub offset_m: f64,
    pub bearing_deg: f64,
    pub vessel_name: String,
}
