//! Tracker configuration, sensor sites and the company's own fleet.

use crate::track::{Class, Domain, SourceKind};
use crate::wire::SensorKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How long a source may be silent, as multiples of its normal reporting interval with floors.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    /// The interval when the report itself does not say (AIS does: by speed and status).
    pub nominal_ms: i64,
    /// Flag `stale` when silent longer than this factor times the interval.
    pub stale_factor: f64,
    /// Coast (extrapolate) after this factor times the interval, and at least `coast_floor_ms`.
    pub coast_factor: f64,
    pub coast_floor_ms: i64,
    /// Drop after this factor times the interval, and at least `drop_floor_ms`.
    pub drop_factor: f64,
    pub drop_floor_ms: i64,
}

impl Timing {
    pub fn stale_ms(&self, interval_ms: i64) -> i64 {
        (self.stale_factor * interval_ms as f64) as i64
    }
    pub fn coast_ms(&self, interval_ms: i64) -> i64 {
        ((self.coast_factor * interval_ms as f64) as i64).max(self.coast_floor_ms)
    }
    pub fn drop_ms(&self, interval_ms: i64) -> i64 {
        ((self.drop_factor * interval_ms as f64) as i64).max(self.drop_floor_ms)
    }
}

/// A receiver or sensor site, as the simulator's `/sensors` lists them. An entry of `/sensors`
/// deserialises directly (`num` is the SBS session id, `nominal_range_m` the range; other fields
/// are ignored).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorSite {
    /// The id used in tag blocks (`s:`), Remote ID `rx` and `det` `platform` fields.
    pub id: String,
    /// The number the SBS session-id field carries for this receiver, when it is numbered.
    #[serde(default, alias = "num", deserialize_with = "opt_string_or_number")]
    pub sbs_session: Option<String>,
    pub kind: SiteKind,
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lon: Option<f64>,
    #[serde(default)]
    pub alt_m: Option<f64>,
    /// Nominal range, metres.
    #[serde(default, alias = "nominal_range_m")]
    pub range_m: Option<f64>,
    /// Interval between looks, for radars (ms).
    #[serde(default)]
    pub scan_ms: Option<i64>,
    /// Which traffic a ground radar watches (default air).
    #[serde(default)]
    pub domain: Option<Domain>,
    /// Owner company or network.
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteKind {
    #[serde(rename = "adsb_1090")]
    Adsb1090,
    AdsbUat,
    /// A commercial aggregated ADS-B feed.
    AdsbFeed,
    Ais,
    AisFeed,
    #[serde(alias = "rid")]
    RemoteId,
    Daa,
    MarineRadar,
    GroundRadar,
}

fn opt_string_or_number<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Text(String),
        Int(i64),
    }
    Ok(Option::<Raw>::deserialize(d)?.map(|r| match r {
        Raw::Text(s) => s,
        Raw::Int(i) => i.to_string(),
    }))
}

/// The company's own vehicles: their broadcasts are not "other traffic", and their own sensors'
/// detections of each other are dropped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OwnFleet {
    pub icao: Vec<String>,
    pub mmsi: Vec<u32>,
    pub uas_ids: Vec<String>,
}

/// An own vehicle's position now (from the bridge's state), so a sensor's detection of it is not
/// taken for traffic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnVehicle {
    pub uid: String,
    pub t: i64,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Origin of the local metres (default: the middle of the FleetMesh region).
    pub origin_lat: f64,
    pub origin_lon: f64,

    // Time handling.
    /// Minutes the SBS writer's clock is ahead of UTC (0 for the simulator; a dump1090 writes
    /// local time).
    pub sbs_utc_offset_min: i32,
    /// A message's own time is used when it is at most this far before its receipt time (a
    /// commercial feed's delay) ...
    pub max_feed_delay_ms: i64,
    /// ... and at most this far after it (clock skew). Otherwise the receipt time is used.
    pub max_clock_skew_ms: i64,
    /// Identical reports within this window are duplicates (the same transmission heard by
    /// several receivers).
    pub dedup_window_ms: i64,
    /// Reports up to this much older than a track are fused approximately; older ones are not.
    pub oosm_max_ms: i64,
    /// A multi-sentence AIS message must complete within this time.
    pub nmea_fragment_ms: i64,
    pub nmea_require_checksum: bool,

    // Measurement errors (1 sigma) when the source does not say.
    pub adsb_pos_sigma_m: f64,
    pub adsb_alt_sigma_m: f64,
    pub adsb_speed_sigma_mps: f64,
    pub adsb_track_sigma_deg: f64,
    pub adsb_vrate_sigma_mps: f64,
    /// AIS position: accuracy flag set (better than 10 m) and not set.
    pub ais_pos_sigma_m: [f64; 2],
    pub ais_speed_sigma_mps: f64,
    pub ais_course_sigma_deg: f64,
    pub rid_pos_sigma_m: f64,
    pub rid_alt_sigma_m: f64,
    pub rid_speed_sigma_mps: f64,
    /// Geoid height (WGS84 ellipsoid above mean sea level) for Remote ID's ellipsoid heights;
    /// about -32 m on San Francisco Bay. Set 0 if a source already gives MSL.
    pub geoid_n_m: f64,
    pub bearing_sigma_deg: BTreeMap<SensorKind, f64>,
    pub elev_sigma_deg: f64,
    pub range_sigma_m: BTreeMap<SensorKind, f64>,
    pub radial_speed_sigma_mps: f64,

    // Process noise: white-noise acceleration spectral density, m^2/s^3.
    pub q_air: f64,
    pub q_air_vertical: f64,
    pub q_surface: f64,
    pub q_noncoop_air: f64,
    pub q_noncoop_surface: f64,

    // Altitude references.
    /// QNH in hPa when known (from weather); baro and geometric altitudes are compared through
    /// it. Unknown, the offset is taken as 0 with a wide sigma.
    pub qnh_hpa: Option<f64>,
    pub baro_geo_sigma_known_m: f64,
    pub baro_geo_sigma_unknown_m: f64,

    // Association and lifecycle.
    /// Gate probability for detection-to-track association (chi-square).
    pub gate_probability: f64,
    /// Added to a tentative track's association cost, so a confirmed track that also gates
    /// keeps the detection.
    pub tentative_penalty: f64,
    /// Non-cooperative confirmation: M hits in N looks.
    pub confirm_m: u32,
    pub confirm_n: u32,
    /// Cooperative tracks are confirmed after this many position reports (AIS, ADS-B, RID).
    pub coop_confirm_ais: u32,
    pub coop_confirm_adsb: u32,
    pub coop_confirm_rid: u32,
    pub timing: BTreeMap<SourceKind, Timing>,
    /// Bearing-only tracks: coast and drop after these silences.
    pub bearing_coast_ms: i64,
    pub bearing_drop_ms: i64,
    /// Default nominal ranges by sensor kind, metres.
    pub sensor_range_m: BTreeMap<SensorKind, f64>,
    /// DAA detection ranges by target size (small, medium, large, unknown), metres: the far end of
    /// a bearing-only line.
    pub daa_range_by_size_m: [f64; 4],
    pub scan_ms: BTreeMap<SensorKind, i64>,

    // Position jumps.
    /// A cooperative report this many sigmas (squared, 2-D) from the prediction and ...
    pub jump_chi2: f64,
    /// ... further than this from it is a jump.
    pub jump_min_m: f64,
    /// How long the flag stays after the last jump.
    pub jump_flag_ms: i64,

    // Sensor disagreement (see README).
    pub disagree_radius_m: f64,
    pub disagree_min_speed_mps: f64,
    /// The sensed track's velocity must be known this well (1-sigma, m/s) to compare.
    pub disagree_max_vel_sigma_mps: f64,
    pub disagree_persist_ms: i64,
    pub disagree_hold_ms: i64,

    // Anonymous identities.
    /// Address ranges (inclusive, hex) treated as anonymous (for example a Privacy ICAO Address
    /// pool the operator knows).
    pub anonymous_icao_ranges: Vec<(String, String)>,
    /// A new anonymous identity starting within this time after an old one went quiet, at its
    /// predicted position, continues the old track.
    pub handover_ms: i64,
    pub handover_chi2: f64,

    /// ICAO addresses on the LADD list: flagged, callsign withheld.
    pub ladd_icao: Vec<String>,
    /// Address to class, from an aircraft registry the operator holds.
    pub aircraft_classes: BTreeMap<String, Class>,
    /// Read an airline-style callsign (three letters and a flight number) as an airliner.
    pub classify_airline_callsigns: bool,

    // Bearing-only triangulation.
    pub tri_min_obs: usize,
    pub tri_window_ms: i64,
    pub tri_min_cross_deg: f64,
    /// Accept a fix when its 1-sigma semi-major axis is below max(abs, rel * range).
    pub tri_max_sigma_abs_m: f64,
    pub tri_max_sigma_rel: f64,
    pub tri_max_speed_mps: f64,
    /// The fitted range must beat ranges 40% shorter and 60% longer by this chi-square margin.
    pub tri_profile_dchi2: f64,

    /// Own vehicles' positions are this old at most to be used for self-detection checks.
    pub own_vehicle_max_age_ms: i64,
    pub own_vehicle_sigma_m: f64,

    /// Output: an altitude whose sigma exceeds this is reported as unknown.
    pub max_alt_sigma_out_m: f64,
}

impl Default for Config {
    fn default() -> Self {
        let t =
            |nominal_ms, stale_factor, coast_factor, coast_floor_ms, drop_factor, drop_floor_ms| {
                Timing {
                    nominal_ms,
                    stale_factor,
                    coast_factor,
                    coast_floor_ms,
                    drop_factor,
                    drop_floor_ms,
                }
            };
        let timing = BTreeMap::from([
            // ADS-B: 2 Hz broadcasts, about 1 Hz received at range.
            (
                SourceKind::Adsb1090,
                t(1_000, 3.0, 5.0, 5_000, 30.0, 30_000),
            ),
            (SourceKind::AdsbUat, t(1_000, 3.0, 5.0, 5_000, 30.0, 30_000)),
            // AIS: the interval comes from each report (2 s to 3 min); slot loss at range is
            // common, so drop late.
            (SourceKind::AisA, t(10_000, 2.5, 3.5, 30_000, 6.0, 360_000)),
            (SourceKind::AisB, t(30_000, 2.5, 3.5, 60_000, 6.0, 360_000)),
            (
                SourceKind::RemoteId,
                t(1_000, 3.0, 4.0, 4_000, 20.0, 20_000),
            ),
            (SourceKind::Daa, t(1_000, 2.5, 3.0, 3_000, 8.0, 8_000)),
            (
                SourceKind::MarineRadar,
                t(2_500, 2.5, 3.0, 7_500, 8.0, 20_000),
            ),
            (
                SourceKind::GroundRadar,
                t(1_000, 2.5, 3.0, 4_000, 10.0, 15_000),
            ),
        ]);
        Config {
            origin_lat: 37.75,
            origin_lon: -122.50,
            sbs_utc_offset_min: 0,
            max_feed_delay_ms: 30_000,
            max_clock_skew_ms: 2_000,
            dedup_window_ms: 1_000,
            oosm_max_ms: 2_000,
            nmea_fragment_ms: 2_000,
            nmea_require_checksum: true,
            adsb_pos_sigma_m: 20.0,
            adsb_alt_sigma_m: 8.0,
            adsb_speed_sigma_mps: 1.0,
            adsb_track_sigma_deg: 1.0,
            adsb_vrate_sigma_mps: 1.0,
            ais_pos_sigma_m: [6.0, 25.0],
            ais_speed_sigma_mps: 0.3,
            ais_course_sigma_deg: 3.0,
            rid_pos_sigma_m: 10.0,
            rid_alt_sigma_m: 8.0,
            rid_speed_sigma_mps: 0.6,
            geoid_n_m: -32.0,
            bearing_sigma_deg: BTreeMap::from([
                (SensorKind::Daa, 2.0),
                (SensorKind::MarineRadar, 1.0),
                (SensorKind::GroundRadar, 0.5),
            ]),
            elev_sigma_deg: 2.0,
            range_sigma_m: BTreeMap::from([
                (SensorKind::Daa, 30.0),
                (SensorKind::MarineRadar, 25.0),
                (SensorKind::GroundRadar, 15.0),
            ]),
            radial_speed_sigma_mps: 1.0,
            q_air: 4.0,
            q_air_vertical: 1.0,
            q_surface: 0.05,
            q_noncoop_air: 9.0,
            q_noncoop_surface: 0.2,
            qnh_hpa: None,
            baro_geo_sigma_known_m: 15.0,
            baro_geo_sigma_unknown_m: 120.0,
            gate_probability: 0.999,
            tentative_penalty: 3.0,
            confirm_m: 3,
            confirm_n: 5,
            coop_confirm_ais: 1,
            coop_confirm_adsb: 2,
            coop_confirm_rid: 2,
            timing,
            bearing_coast_ms: 3_000,
            bearing_drop_ms: 8_000,
            sensor_range_m: BTreeMap::from([
                (SensorKind::Daa, 2_000.0),
                (SensorKind::MarineRadar, 7_400.0),
                (SensorKind::GroundRadar, 6_000.0),
            ]),
            daa_range_by_size_m: [600.0, 1_500.0, 2_500.0, 2_000.0],
            scan_ms: BTreeMap::from([
                (SensorKind::Daa, 1_000),
                (SensorKind::MarineRadar, 2_500),
                (SensorKind::GroundRadar, 1_000),
            ]),
            jump_chi2: 40.0,
            jump_min_m: 300.0,
            jump_flag_ms: 60_000,
            disagree_radius_m: 3_000.0,
            disagree_min_speed_mps: 3.0,
            disagree_max_vel_sigma_mps: 8.0,
            disagree_persist_ms: 4_000,
            disagree_hold_ms: 15_000,
            anonymous_icao_ranges: Vec::new(),
            handover_ms: 30_000,
            handover_chi2: 13.8,
            ladd_icao: Vec::new(),
            aircraft_classes: BTreeMap::new(),
            classify_airline_callsigns: true,
            tri_min_obs: 4,
            tri_window_ms: 30_000,
            tri_min_cross_deg: 8.0,
            tri_max_sigma_abs_m: 120.0,
            tri_max_sigma_rel: 0.15,
            tri_max_speed_mps: 200.0,
            tri_profile_dchi2: 9.0,
            own_vehicle_max_age_ms: 5_000,
            own_vehicle_sigma_m: 25.0,
            max_alt_sigma_out_m: 300.0,
        }
    }
}

impl Config {
    pub(crate) fn timing(&self, k: SourceKind) -> Timing {
        self.timing.get(&k).copied().unwrap_or(Timing {
            nominal_ms: 1_000,
            stale_factor: 3.0,
            coast_factor: 4.0,
            coast_floor_ms: 5_000,
            drop_factor: 20.0,
            drop_floor_ms: 30_000,
        })
    }
}

/// The normal Class A reporting interval (ITU-R M.1371 table 1), from speed, rate of turn and
/// navigation status.
pub fn ais_a_interval_ms(sog_kn: Option<f64>, nav_status: Option<u8>, turning: bool) -> i64 {
    let sog = sog_kn.unwrap_or(0.0);
    let anchored = matches!(nav_status, Some(1) | Some(5));
    if anchored && sog <= 3.0 {
        return 180_000;
    }
    match sog {
        s if s > 23.0 => 2_000,
        s if s > 14.0 => {
            if turning {
                2_000
            } else {
                6_000
            }
        }
        _ => {
            if turning {
                3_333
            } else {
                10_000
            }
        }
    }
}

/// The normal Class B interval: SOTDMA units by speed, carrier-sense units 30 s moving.
pub fn ais_b_interval_ms(sog_kn: Option<f64>, cs: bool) -> i64 {
    let sog = sog_kn.unwrap_or(0.0);
    if sog <= 2.0 {
        return 180_000;
    }
    if cs {
        return 30_000;
    }
    match sog {
        s if s > 23.0 => 5_000,
        s if s > 14.0 => 15_000,
        _ => 30_000,
    }
}
