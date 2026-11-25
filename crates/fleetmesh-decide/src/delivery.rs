//! Delivery data: the places a delivery fleet works from (hubs and drop zones), the inputs the
//! delivery rules read beyond a vehicle's own facts, and the knobs those rules take from the
//! policy file (docs/design/delivery-orders.md, delivery-rules.md).
//!
//! Everything here is plain data. The rules themselves are in [`crate::delivery_rules`]. All of it
//! is logged by value with the decision that read it (`delivery` in the decision's inputs), so
//! replay recomputes every delivery check from the log alone and queries nothing. Unknown is
//! `None` or absent, never a default.
//!
//! Hubs and drop zones are named points with kinds, loaded from `sim/hubs.toml` ([`Places`]):
//!
//! ```toml
//! [[hub]]
//! id = "hub-presidio"
//! lon = -122.4650
//! lat = 37.8040
//! radius_m = 60        # its landing zone
//! pads = 4
//! packs = 6            # charged battery packs in stock
//!
//! [[dropzone]]
//! id = "dz-harbour-17"
//! kind = "doorstep"    # pad | doorstep | locker | dock | kerb
//! lon = -122.4119
//! lat = 37.8087
//! radius_m = 4
//! surface = "land"     # land | water
//! populated = true
//! approval = "customer" # survey | customer | withdrawn; absent: a new delivery point
//! ```
//!
//! Rules 2 ([`crate::rules::STANDING_APPROVALS_SINCE`]): a drop zone's `approval` says it was
//! approved ahead of time as a delivery point, as real operators do (a surveyed delivery zone
//! with marked points; a spot the customer chose), and stands with no time limit. A person
//! approves any other zone once with `confirm_drop_zone`, and that approval stands too.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::geo::LonLat;
use crate::ids::{IntentId, Uid};
use crate::intent::TaskKind;
use crate::time::Time;
use crate::world::{Fact, LostLinkAction};

/// A hub: where vehicles start, load, land, swap batteries and come home to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hub {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub lon: f64,
    pub lat: f64,
    /// Its landing zone: a point within this many metres of the hub is a planned landing place.
    #[serde(default = "default_hub_radius")]
    pub radius_m: f64,
    #[serde(default = "default_pads")]
    pub pads: u32,
    /// Charged battery packs in stock (`swap_battery` takes one).
    #[serde(default)]
    pub packs: u32,
    #[serde(default = "yes")]
    pub open: bool,
}

fn default_hub_radius() -> f64 {
    50.0
}
fn default_pads() -> u32 {
    2
}
fn yes() -> bool {
    true
}

impl Hub {
    pub fn at(&self) -> LonLat {
        LonLat::new(self.lon, self.lat)
    }
}

/// What a drop zone is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropZoneKind {
    Pad,
    Doorstep,
    Locker,
    Dock,
    Kerb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface2 {
    Land,
    Water,
}

/// A place a vehicle may release a parcel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropZone {
    pub id: String,
    pub kind: DropZoneKind,
    pub lon: f64,
    pub lat: f64,
    #[serde(default = "default_dz_radius")]
    pub radius_m: f64,
    pub surface: Surface2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_m: Option<f64>,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub populated: bool,
    /// The hub a `pad` zone belongs to (a hub pad needs no confirmation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    /// Terrain slope at the zone, degrees, where known (a land release is refused above the
    /// policy's limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slope_deg: Option<f64>,
    /// A dock's deck level above the tide datum, m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock_level_m: Option<f64>,
    /// Rules 2 ([`crate::rules::STANDING_APPROVALS_SINCE`]): how the zone was approved as a
    /// delivery point ahead of time, if it was. Absent: a new delivery point, which a person
    /// approves once (`confirm_drop_zone`). Not read under rules 1, whose logs never carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<StandingApproval>,
}

/// How a drop zone came to be an approved delivery point before any delivery to it (rules 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandingApproval {
    /// Surveyed and marked in advance: an obstacle-free delivery zone with a reachable point in
    /// it (as Amazon Prime Air surveys its delivery zones).
    Survey,
    /// The customer chose the spot in advance (as Wing's customers pick theirs: about 6 by 6 ft
    /// with nothing overhead); it changes only when the customer picks another.
    Customer,
    /// It was approved and is not any more (the customer moved, the survey lapsed): a person
    /// approves it again before a delivery goes there.
    Withdrawn,
}

impl StandingApproval {
    pub fn as_str(self) -> &'static str {
        match self {
            StandingApproval::Survey => "survey",
            StandingApproval::Customer => "customer",
            StandingApproval::Withdrawn => "withdrawn",
        }
    }
}

fn default_dz_radius() -> f64 {
    4.0
}

impl DropZone {
    pub fn at(&self) -> LonLat {
        LonLat::new(self.lon, self.lat)
    }

    /// A pad at a hub: a planned landing place, approved with its hub.
    pub fn hub_pad(&self) -> bool {
        self.kind == DropZoneKind::Pad && self.hub.is_some()
    }

    /// Rules 2: approved ahead of time (a survey or the customer's choice) or a hub pad. A zone
    /// whose approval was withdrawn is not.
    pub fn standing_approved(&self) -> bool {
        self.hub_pad() || matches!(self.approval, Some(StandingApproval::Survey | StandingApproval::Customer))
    }
}

/// The hubs and drop zones of the world, by id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Places {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hubs: BTreeMap<String, Hub>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drop_zones: BTreeMap<String, DropZone>,
}

#[derive(Debug, thiserror::Error)]
pub enum PlacesError {
    #[error("places {0}: {1}")]
    Io(String, std::io::Error),
    #[error("places {0}: {1}")]
    Invalid(String, String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacesFile {
    #[serde(default)]
    hub: Vec<Hub>,
    #[serde(default)]
    dropzone: Vec<DropZone>,
}

impl Places {
    pub fn from_toml(source: &str, text: &str) -> Result<Places, PlacesError> {
        let bad = |m: String| PlacesError::Invalid(source.to_string(), m);
        let f: PlacesFile = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
        let mut p = Places::default();
        for h in f.hub {
            if !h.at().is_valid() || !(h.radius_m.is_finite() && h.radius_m > 0.0) {
                return Err(bad(format!("hub {}: position or radius out of range", h.id)));
            }
            if p.hubs.insert(h.id.clone(), h.clone()).is_some() {
                return Err(bad(format!("hub {} appears twice", h.id)));
            }
        }
        for z in f.dropzone {
            if !z.at().is_valid() || !(z.radius_m.is_finite() && z.radius_m > 0.0) {
                return Err(bad(format!("drop zone {}: position or radius out of range", z.id)));
            }
            if let Some(h) = &z.hub
                && !p.hubs.contains_key(h)
            {
                return Err(bad(format!("drop zone {}: its hub {h} is not defined", z.id)));
            }
            if p.drop_zones.insert(z.id.clone(), z.clone()).is_some() {
                return Err(bad(format!("drop zone {} appears twice", z.id)));
            }
        }
        Ok(p)
    }

    pub fn load(path: &Path) -> Result<Places, PlacesError> {
        let text = std::fs::read_to_string(path).map_err(|e| PlacesError::Io(path.display().to_string(), e))?;
        Places::from_toml(&path.display().to_string(), &text)
    }

    pub fn is_empty(&self) -> bool {
        self.hubs.is_empty() && self.drop_zones.is_empty()
    }

    /// The open hub nearest `p` (ties: the lower id), with its distance in metres.
    pub fn nearest_hub(&self, p: LonLat) -> Option<(&Hub, f64)> {
        self.hubs
            .values()
            .filter(|h| h.open)
            .map(|h| (h, crate::geo::haversine_m(p, h.at())))
            .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.id.cmp(&b.0.id)))
    }

    /// The hub whose landing zone holds `p`, if any.
    pub fn hub_landing_at(&self, p: LonLat) -> Option<&Hub> {
        self.hubs.values().find(|h| crate::geo::haversine_m(p, h.at()) <= h.radius_m)
    }
}

/// A parcel as the dispatch layer (a later step) tells the engine of it. `pickup` and `deliver`
/// take its id; the payload rule reads its mass and class.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParcelFact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass_kg: Option<f64>,
    /// `normal`, `hazmat` or `excluded`.
    #[serde(default = "normal_class")]
    pub class: String,
    pub state: ParcelState,
    /// The vehicle it is assigned to or aboard, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<Uid>,
}

fn normal_class() -> String {
    "normal".into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParcelState {
    Ready,
    Loading,
    Loaded,
    Released,
}

/// One weather sample: what [`fleetmesh_weather`'s `Sample`] says at a point (wind east and north,
/// gust, visibility, precipitation), the water's current and level for a boat, and where it
/// comes from. The bridge samples at the origin, the drop zone, the hub and along the route, and
/// the decision logs the values it read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WxFact {
    pub at: LonLat,
    pub t: Time,
    pub wind_e_mps: f64,
    pub wind_n_mps: f64,
    pub gust_mps: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precip_mmh: Option<f64>,
    /// The water's current east and north, m/s (tidewater), for a boat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_e_mps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_n_mps: Option<f64>,
    /// The tide level above the datum, m, at the time the sample is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tide_m: Option<f64>,
    /// The water depth at the point at that level, m (a boat's keel clearance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_m: Option<f64>,
    /// The ground's elevation above the datum, m (the flood rule for a car).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_m: Option<f64>,
    /// Where it came from (`archive`, `tide_prediction`, `vehicle`, `forecast`).
    #[serde(default)]
    pub source: String,
}

impl WxFact {
    pub fn wind_mps(&self) -> f64 {
        self.wind_e_mps.hypot(self.wind_n_mps)
    }
}

/// What the latest look at a drop zone saw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DzObservation {
    pub t: Time,
    pub state: ObservedState,
    #[serde(default)]
    pub count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedState {
    Clear,
    Occupied,
    Unknown,
}

/// Another vehicle's position and route, for the separation rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrafficFact {
    pub position: Fact<LonLat>,
    /// Altitude above the ground, m (air); absent for ground and surface vehicles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_mps: Option<[f64; 2]>,
    /// Its dispatched or approved route from where it is now, if it has one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route: Vec<LonLat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_mps: Option<f64>,
    pub domain: crate::catalog::Domain,
}

/// A vehicle's navigation fix as the bridge reads it from the GNSS state the vehicle reports
/// (the stand-ins' receiver: satellites tracked, HDOP, horizontal accuracy) and from the
/// consistency checks the bridge makes on its track (delivery-rules.md section 9).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sats: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdop: Option<f64>,
    /// Reported horizontal accuracy, m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h_acc_m: Option<f64>,
    /// How far the new position is from the previous one carried along the velocity, m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump_m: Option<f64>,
    /// Disagreement between the slant range to the serving station and the range the reported
    /// position implies, m (cellular and LOS only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_mismatch_m: Option<f64>,
    /// The position is on the wrong surface for the platform.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrong_surface: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_mps: Option<f64>,
    /// The state the vehicle was in at the last evaluation and how long the better state's
    /// conditions have held since, s: the hysteresis (a vehicle returns to a better state only
    /// after `nav.recover_s`). Absent: not tracked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_state: Option<NavTrust>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub good_for_s: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavTrust {
    Trusted,
    Degraded,
    Untrusted,
    Unknown,
}

impl NavTrust {
    pub fn as_str(self) -> &'static str {
        match self {
            NavTrust::Trusted => "trusted",
            NavTrust::Degraded => "degraded",
            NavTrust::Untrusted => "untrusted",
            NavTrust::Unknown => "unknown",
        }
    }
}

/// The link prediction along a vehicle's route (the link emulator's `route_answer`): whether its
/// failsafe timer trips on the route, how long it would be without a link, whether the energy to
/// return from the trip point is there, and whether a backup link covers it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkAlong {
    pub t: Time,
    pub trips: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_s: Option<f64>,
    /// The return from the trip point is within the vehicle's energy.
    pub return_ok: bool,
    #[serde(default)]
    pub backup_covers: bool,
    /// The main link is down and the backup carries now.
    #[serde(default)]
    pub on_backup: bool,
    /// The backup carries a low data rate (iridium).
    #[serde(default)]
    pub backup_rate_low: bool,
}

/// What the engine knows of an intent an `abort` names (engine-filled).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetFact {
    pub kind: TaskKind,
    pub state: crate::intent::State,
    pub uids: Vec<Uid>,
    /// A `deliver`'s parcel and drop zone, when the target is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parcel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_zone: Option<String>,
    /// The parcel has been released (a released parcel cannot be un-released).
    #[serde(default)]
    pub released: bool,
}

/// What the engine knows of the order a `reroute` replaces (engine-filled): the running
/// `deliver`, `pickup` or `return_to_hub` on the vehicle, and the length of its route.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RerouteBase {
    pub intent: IntentId,
    pub kind: TaskKind,
    pub route_m: f64,
}

/// Everything the delivery rules read besides the vehicles' own facts: the world's places and
/// parcels, the weather and drop-zone observations, other traffic, and per vehicle its
/// navigation fix and link prediction. Logged by value with each decision that reads it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryInputs {
    #[serde(default, skip_serializing_if = "Places::is_empty")]
    pub places: Places,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parcels: BTreeMap<String, ParcelFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weather: Vec<WxFact>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub observations: BTreeMap<String, DzObservation>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub traffic: BTreeMap<Uid, TrafficFact>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nav: BTreeMap<Uid, NavFacts>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub link: BTreeMap<Uid, LinkAlong>,
    /// Engine-filled: when each drop zone was last confirmed (an approved `confirm_drop_zone`, or
    /// an approved `deliver` held for `DZ_UNCONFIRMED`, or from rules 2 `DZ_NOT_APPROVED`). Under
    /// rules 1 a confirmation stands for `dropzone.confirm_valid_s`; from rules 2 it approves the
    /// zone as a delivery point and stands from then on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub confirmed: BTreeMap<String, Time>,
    /// Engine-filled for an `abort`: the order it names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, TargetFact>,
    /// Engine-filled for a `reroute`: the order it replaces, by vehicle.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reroute_base: BTreeMap<Uid, RerouteBase>,
    /// Engine-filled for a `confirm_drop_zone`: the `deliver` waiting on that zone and parcel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_deliver: Option<IntentId>,
}

// ---- knobs ------------------------------------------------------------------------------------

/// `[rules.delivery]` in the policy file: the numbers of the delivery rules, each with the
/// first-draft default of docs/design/delivery-rules.md. Logged with the policy; replay decides
/// with that copy.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliveryRules {
    pub geofence: GeofenceKnobs,
    pub energy: EnergyKnobs,
    pub payload: PayloadKnobs,
    pub weather: WeatherKnobs,
    pub separation: SeparationKnobs,
    pub nav: NavKnobs,
    pub dropzone: DropZoneKnobs,
    pub lost_link: LostLinkKnobs,
    pub reroute: RerouteKnobs,
    pub abort: AbortKnobs,
    pub hub: HubKnobs,
}

/// A figure by domain.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByDomain {
    pub air: f64,
    pub ground: f64,
    pub surface: f64,
}

impl ByDomain {
    pub fn get(&self, d: crate::catalog::Domain) -> f64 {
        match d {
            crate::catalog::Domain::Air => self.air,
            crate::catalog::Domain::Ground => self.ground,
            crate::catalog::Domain::Surface => self.surface,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeofenceKnobs {
    pub buffer_m: ByDomain,
    pub soon_s: u32,
    pub alt_slack_m: f64,
    pub sample_m: f64,
}

impl Default for GeofenceKnobs {
    fn default() -> Self {
        GeofenceKnobs {
            buffer_m: ByDomain { air: 30.0, ground: 10.0, surface: 20.0 },
            soon_s: 600,
            alt_slack_m: 20.0,
            sample_m: 20.0,
        }
    }
}

/// A figure by autopilot kind: the multirotor, the fixed wing, the car, the boat.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByKind {
    pub copter: f64,
    pub plane: f64,
    pub car: f64,
    pub boat: f64,
}

impl ByKind {
    pub fn get(&self, k: crate::catalog::AutopilotKind) -> f64 {
        use crate::catalog::AutopilotKind::*;
        match k {
            Copter | Blimp => self.copter,
            Plane | Vtol => self.plane,
            Ugv => self.car,
            Boat => self.boat,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnergyKnobs {
    pub reserve_pct: ByKind,
    pub wind_margin: f64,
    pub hold_band_pct: f64,
    /// A battery fact older than this is unknown: streaming link, then slow link (satcom).
    pub max_age_s: f64,
    pub max_age_slow_s: f64,
    pub min_ground_speed_frac: f64,
    pub drop_hover_s: f64,
    pub payload_load: ByKind,
}

impl Default for EnergyKnobs {
    fn default() -> Self {
        EnergyKnobs {
            reserve_pct: ByKind { copter: 25.0, plane: 20.0, car: 15.0, boat: 20.0 },
            wind_margin: 0.15,
            hold_band_pct: 5.0,
            max_age_s: 10.0,
            max_age_slow_s: 60.0,
            min_ground_speed_frac: 0.3,
            drop_hover_s: 20.0,
            payload_load: ByKind { copter: 0.35, plane: 0.15, car: 0.10, boat: 0.10 },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PayloadKnobs {
    pub derate_from_mps: f64,
    pub derate_floor: f64,
    pub hold_frac: f64,
}

impl Default for PayloadKnobs {
    fn default() -> Self {
        PayloadKnobs { derate_from_mps: 7.0, derate_floor: 0.6, hold_frac: 0.9 }
    }
}

/// Soft and hard weather limits of one platform. A soft limit holds, a hard limit refuses.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WxLimits {
    pub wind_soft_mps: Option<f64>,
    pub wind_hard_mps: Option<f64>,
    pub gust_soft_mps: Option<f64>,
    pub gust_hard_mps: Option<f64>,
    /// Visibility: below the soft value holds, below the hard value refuses.
    pub vis_soft_m: Option<f64>,
    pub vis_hard_m: Option<f64>,
    pub precip_soft_mmh: Option<f64>,
    pub precip_hard_mmh: Option<f64>,
    /// A boat: the current as a fraction of its cruise speed.
    pub current_soft_ratio: Option<f64>,
    pub current_hard_ratio: Option<f64>,
}

impl WxLimits {
    /// The first-draft limits by autopilot kind (delivery-rules.md section 7).
    pub fn defaults(k: crate::catalog::AutopilotKind) -> WxLimits {
        use crate::catalog::AutopilotKind::*;
        let mut l = WxLimits::default();
        match k {
            Copter | Blimp => {
                (l.wind_soft_mps, l.wind_hard_mps) = (Some(8.0), Some(10.0));
                (l.gust_soft_mps, l.gust_hard_mps) = (Some(11.0), Some(14.0));
                (l.vis_soft_m, l.vis_hard_m) = (Some(1500.0), Some(800.0));
                (l.precip_soft_mmh, l.precip_hard_mmh) = (Some(2.0), Some(5.0));
            }
            Plane | Vtol => {
                (l.wind_soft_mps, l.wind_hard_mps) = (Some(10.0), Some(12.0));
                (l.gust_soft_mps, l.gust_hard_mps) = (Some(13.0), Some(16.0));
                (l.vis_soft_m, l.vis_hard_m) = (Some(2000.0), Some(1000.0));
                (l.precip_soft_mmh, l.precip_hard_mmh) = (Some(4.0), Some(8.0));
            }
            Boat => {
                (l.wind_soft_mps, l.wind_hard_mps) = (Some(10.0), Some(13.0));
                (l.gust_soft_mps, l.gust_hard_mps) = (Some(13.0), Some(16.0));
                (l.current_soft_ratio, l.current_hard_ratio) = (Some(0.5), Some(0.8));
            }
            Ugv => {
                (l.precip_soft_mmh, l.precip_hard_mmh) = (Some(8.0), Some(20.0));
                (l.vis_soft_m, l.vis_hard_m) = (Some(300.0), Some(100.0));
            }
        }
        l
    }

    /// `self` over `base`: what `self` sets, else `base`'s.
    pub fn over(&self, base: &WxLimits) -> WxLimits {
        WxLimits {
            wind_soft_mps: self.wind_soft_mps.or(base.wind_soft_mps),
            wind_hard_mps: self.wind_hard_mps.or(base.wind_hard_mps),
            gust_soft_mps: self.gust_soft_mps.or(base.gust_soft_mps),
            gust_hard_mps: self.gust_hard_mps.or(base.gust_hard_mps),
            vis_soft_m: self.vis_soft_m.or(base.vis_soft_m),
            vis_hard_m: self.vis_hard_m.or(base.vis_hard_m),
            precip_soft_mmh: self.precip_soft_mmh.or(base.precip_soft_mmh),
            precip_hard_mmh: self.precip_hard_mmh.or(base.precip_hard_mmh),
            current_soft_ratio: self.current_soft_ratio.or(base.current_soft_ratio),
            current_hard_ratio: self.current_hard_ratio.or(base.current_hard_ratio),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherKnobs {
    /// Per platform id: what it sets over the first-draft limits of its kind
    /// (`[rules.delivery.weather.platform.delivery_drone] gust_soft_mps = 11`).
    pub platform: BTreeMap<String, WxLimits>,
    pub keel_clear_m: f64,
    pub keel_hard_m: f64,
    pub flood_clear_m: f64,
    pub dock_level_band_m: f64,
    pub max_age_s: f64,
    /// 0: current conditions only (the archive is a replay, not a forecast).
    pub lookahead_s: u32,
}

impl Default for WeatherKnobs {
    fn default() -> Self {
        WeatherKnobs {
            platform: BTreeMap::new(),
            keel_clear_m: 0.5,
            keel_hard_m: 0.2,
            flood_clear_m: 0.3,
            dock_level_band_m: 0.3,
            max_age_s: 120.0,
            lookahead_s: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SeparationKnobs {
    pub step_s: f64,
    pub horizon_s: f64,
    pub marginal_factor: f64,
    pub drift_mps: f64,
    pub relevant_km: f64,
    pub air_air_h_m: f64,
    pub air_air_v_m: f64,
    pub air_hub_h_m: f64,
    pub air_hub_v_m: f64,
    pub hub_zone_m: f64,
    pub ground_ground_m: f64,
    pub surface_surface_m: f64,
    /// An aircraft above this height keeps no minimum from ground and surface vehicles.
    pub air_over_ground_m: f64,
    /// A traffic fact older than this is unknown.
    pub max_age_s: f64,
}

impl Default for SeparationKnobs {
    fn default() -> Self {
        SeparationKnobs {
            step_s: 5.0,
            horizon_s: 300.0,
            marginal_factor: 1.5,
            drift_mps: 0.5,
            relevant_km: 5.0,
            air_air_h_m: 50.0,
            air_air_v_m: 30.0,
            air_hub_h_m: 20.0,
            air_hub_v_m: 15.0,
            hub_zone_m: 100.0,
            ground_ground_m: 10.0,
            surface_surface_m: 30.0,
            air_over_ground_m: 20.0,
            max_age_s: 10.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NavKnobs {
    pub trusted_acc_m: ByDomain,
    pub degraded_acc_m: ByDomain,
    pub min_sats: u8,
    pub max_hdop: f64,
    pub max_age_s: f64,
    pub jump_m: f64,
    pub range_tol_m: f64,
    pub recover_s: f64,
    /// A reported speed above this times the platform's `max_mps` is impossible.
    pub speed_factor: f64,
}

impl Default for NavKnobs {
    fn default() -> Self {
        NavKnobs {
            trusted_acc_m: ByDomain { air: 5.0, ground: 8.0, surface: 10.0 },
            degraded_acc_m: ByDomain { air: 15.0, ground: 20.0, surface: 25.0 },
            min_sats: 6,
            max_hdop: 2.0,
            max_age_s: 5.0,
            jump_m: 30.0,
            range_tol_m: 150.0,
            recover_s: 20.0,
            speed_factor: 1.2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DropZoneKnobs {
    /// How long a person's confirmation of a drop zone stands when standing approvals are off
    /// (rules 1, or `standing_approvals = false`).
    pub confirm_valid_s: u32,
    pub obs_max_age_s: f64,
    pub descent_gust_mps: f64,
    pub descent_gust_hard_mps: f64,
    pub max_slope_deg: f64,
    /// Rules 2 (policy schema 2, [`crate::rules::STANDING_APPROVALS_SINCE`]): whether a drop
    /// zone's standing approval (and a person's approval of a new delivery point) stands in for a
    /// confirmation, with no time limit. Absent: yes from rules 2. `false` restores the rules 1
    /// procedure, a person's confirmation within `confirm_valid_s` for every zone but a hub pad:
    /// the comparison setting (`sim/policy-strict.toml`). Never logged under rules 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standing_approvals: Option<bool>,
}

impl Default for DropZoneKnobs {
    fn default() -> Self {
        DropZoneKnobs {
            confirm_valid_s: 3600,
            obs_max_age_s: 30.0,
            descent_gust_mps: 8.0,
            descent_gust_hard_mps: 10.0,
            max_slope_deg: 10.0,
            standing_approvals: None,
        }
    }
}

/// The failsafe plan policy expects of a platform: the action and the timers (seconds without a
/// ground-station heartbeat) it may carry (each radio class has its own).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedPlan {
    pub action: LostLinkAction,
    pub timer_s: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LostLinkKnobs {
    pub gap_hold_s: f64,
    pub release_wait_s: f64,
    pub car_hold_s: f64,
    /// By platform id; a platform with none is checked against its kind's default.
    pub platform: BTreeMap<String, ExpectedPlan>,
}

impl Default for LostLinkKnobs {
    fn default() -> Self {
        LostLinkKnobs { gap_hold_s: 30.0, release_wait_s: 30.0, car_hold_s: 60.0, platform: BTreeMap::new() }
    }
}

impl LostLinkKnobs {
    /// The plan expected of a platform: its entry, else the kind's first-draft default
    /// (delivery-rules.md 11.1; the car uses RTL until a fleet-side timer exists).
    pub fn expected(&self, platform_id: &str, kind: crate::catalog::AutopilotKind) -> ExpectedPlan {
        use crate::catalog::AutopilotKind::*;
        if let Some(p) = self.platform.get(platform_id) {
            return p.clone();
        }
        match kind {
            Copter | Blimp => ExpectedPlan { action: LostLinkAction::Rtl, timer_s: vec![10.0] },
            Plane | Vtol => ExpectedPlan { action: LostLinkAction::Rtl, timer_s: vec![5.0, 30.0] },
            // a rover's timer is FS_GCS_TIMEOUT plus FS_TIMEOUT (1.5 s)
            Ugv => ExpectedPlan { action: LostLinkAction::Rtl, timer_s: vec![11.5] },
            Boat => ExpectedPlan { action: LostLinkAction::Rtl, timer_s: vec![11.5, 31.5] },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RerouteKnobs {
    pub auto_added_pct: f64,
    pub join_m: f64,
}

impl Default for RerouteKnobs {
    fn default() -> Self {
        RerouteKnobs { auto_added_pct: 25.0, join_m: 150.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AbortKnobs {
    pub people_buffer_m: f64,
    pub final_approach_s: f64,
}

impl Default for AbortKnobs {
    fn default() -> Self {
        AbortKnobs { people_buffer_m: 100.0, final_approach_s: 20.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HubKnobs {
    pub swap_s: u32,
    pub charge_s: u32,
}

impl Default for HubKnobs {
    fn default() -> Self {
        HubKnobs { swap_s: 120, charge_s: 2400 }
    }
}

impl DeliveryRules {
    /// The numbers must be ones a rule can work with.
    pub fn validate(&self) -> Result<(), String> {
        let pos = |name: &str, x: f64| {
            if x.is_finite() && x > 0.0 {
                Ok(())
            } else {
                Err(format!("rules.delivery.{name} must be above 0"))
            }
        };
        pos("geofence.sample_m", self.geofence.sample_m)?;
        pos("energy.max_age_s", self.energy.max_age_s)?;
        pos("energy.max_age_slow_s", self.energy.max_age_slow_s)?;
        pos("separation.step_s", self.separation.step_s)?;
        pos("separation.horizon_s", self.separation.horizon_s)?;
        pos("dropzone.obs_max_age_s", self.dropzone.obs_max_age_s)?;
        pos("weather.max_age_s", self.weather.max_age_s)?;
        if !(0.0..=1.0).contains(&self.energy.min_ground_speed_frac) {
            return Err("rules.delivery.energy.min_ground_speed_frac must be 0..1".into());
        }
        if !(0.0..=1.0).contains(&self.payload.derate_floor) || !(0.0..=1.0).contains(&self.payload.hold_frac) {
            return Err("rules.delivery.payload derate_floor and hold_frac must be 0..1".into());
        }
        if self.energy.hold_band_pct < 0.0 || self.energy.wind_margin < 0.0 {
            return Err("rules.delivery.energy hold_band_pct and wind_margin must not be negative".into());
        }
        if self.weather.keel_hard_m > self.weather.keel_clear_m {
            return Err("rules.delivery.weather.keel_hard_m must not exceed keel_clear_m".into());
        }
        Ok(())
    }
}
