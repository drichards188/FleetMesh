//! The world as the decision layer sees it: a snapshot the bridge builds from its world
//! model. Every vehicle fact carries the time it was observed, so rules can refuse stale
//! facts and the log shows how old each input was.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::areas::AreaSet;
use crate::catalog::{AutopilotKind, PlatformFacts};
use crate::geo::LonLat;
use crate::ids::{IntentId, OperatorId, Uid};
use crate::intent::IntentSpec;
use crate::rules::{RulesConfig, needs_terrain};
use crate::terrain::{Surface, TerrainOracle};
use crate::time::Time;

/// A value and when it was observed (for telemetry: when the message carrying it arrived).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fact<T> {
    pub value: T,
    pub t: Time,
}

impl<T> Fact<T> {
    pub fn new(value: T, t: Time) -> Self {
        Fact { value, t }
    }
}

/// What the bridge knows about one vehicle. `None` means "not known", which rules treat as
/// a reason to fail, never as a default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VehicleFacts {
    /// From the catalog, via the fleet manifest. None if the vehicle isn't in the manifest.
    pub platform: Option<PlatformFacts>,
    /// From the heartbeat's MAV_TYPE.
    pub autopilot: Option<Fact<AutopilotKind>>,
    /// Navigation ready (GPS + EKF, or the EKF flags for submerged vehicles).
    pub nav_ready: Option<Fact<bool>>,
    pub position: Option<Fact<LonLat>>,
    pub home: Option<Fact<LonLat>>,
    /// 0..100.
    pub battery_pct: Option<Fact<f64>>,
    /// What the position rests on. A submerged AUV navigates on dead reckoning (inertial +
    /// DVL, no GNSS), which the bridge counts as navigation-ready for subs only; the
    /// readiness rule records it. Absent in logs written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_source: Option<Fact<NavSource>>,
    /// How often this vehicle's link is expected to report, in ms: 1000 streaming, 20000 on
    /// an acoustic (HIGH_LATENCY2) link. Freshness limits scale with it
    /// (`rules.fact_age_intervals`). Absent in logs written before it existed and for
    /// vehicles whose cadence is unknown; both mean the plain `max_fact_age_s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_interval_ms: Option<u32>,
    /// Its velocity over ground, observed with the position: the friendly keep-out carries a
    /// silent vehicle along it. Absent when not reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity_mps: Option<Fact<[f64; 2]>>,
    /// The GPS receiver's fix type (GPS_RAW_INT: 3 is a 3D fix), and whether the
    /// release needs both (Phase E). Absent in logs written before they existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gps_fix: Option<Fact<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gps_glitch: Option<Fact<bool>>,
    /// Where `gps_fix` comes from when not from the receiver's own report: a HIGH_LATENCY2
    /// report carries no fix type, so one without its GPS failure flag counts as a 3D fix,
    /// and says so here. Absent: the receiver's report (GPS_RAW_INT). Rules 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gps_fix_from: Option<GpsFixFrom>,
    /// The circle the vehicle is flying (an orbit task on station, a fixed wing loitering or
    /// keep-out predicts it anywhere on its disc instead of along its momentary heading.
    /// Absent: not known to be circling. Rules 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circling: Option<Fact<Circle>>,
    /// Altitude above mean sea level (negative: depth), metres, stamped with the position it
    /// in 3D where it and the effect's altitude are known; absent (or stale), the keep-out
    /// stays 2D for this vehicle. Absent in logs written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_msl_m: Option<Fact<f64>>,
    /// When the autopilot booted (engine time), estimated by the bridge from the time since
    /// boot its messages carry, and replaced when it detects a reboot. A queued order
    /// compares it with its approval: a vehicle that booted since lost its mission and mode.
    /// Absent before rules 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub booted_t: Option<Time>,
    /// Rules 25 ([`crate::rules::DAMAGE_SINCE`]): the damage the vehicle reported of itself after
    /// A report, never truth: the bridge fills it from what the vehicle said over its link; a
    /// vehicle destroyed says nothing (it goes silent). Damage never heals in a run. The engine
    /// drops it before logging under older rules. Absent: none reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage: Option<Fact<VehicleDamage>>,
    /// Rules 30 ([`crate::rules::NODE_LOSS_SINCE`]): what the vehicle's autopilot does on its own
    /// when it loses its ground station (its GCS failsafe), stamped with when the bridge read it
    /// from the vehicle's parameters (at first contact and after a reboot) or, where the link
    /// cannot carry a read, configured it ([`LostLinkPlan::source`]). A silent vehicle's predicted
    /// region reads it (docs/NODE-LOSS.md section 3). The engine drops it before logging under
    /// older rules. Absent: not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_link: Option<Fact<LostLinkPlan>>,
    /// Rules 32 ([`crate::rules::REACH_NOW_SINCE`]): its sensors' reach under the conditions where
    /// it is now, as the physical sensing model predicts it (the bridge's `tracks::reach`, the
    /// reach display's computation; [`crate::reach`]), stamped with when the bridge computed it. An
    /// clear-day reach for every sensor whose facts carry one. The bridge fills it in a run on the
    /// physical model only; the engine drops it before logging under older rules and from every
    /// decision that does not read it. Absent: not known (the legacy model has no reach that
    /// changes with the conditions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach_now: Option<Fact<crate::reach::ReachNow>>,
}

/// Rules 30: a vehicle's own lost-link failsafe ([`VehicleFacts::lost_link`]): what it does when
/// its ground station has been silent for `timer_s`, and where the bridge has that from. The
/// plan is read from the vehicle's parameters or configured, as [`LostLinkSource`] says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LostLinkPlan {
    pub action: LostLinkAction,
    /// How long the link must be silent before the failsafe acts, s (the firmware's GCS failsafe
    /// timeout). Absent: not known, or nothing to time (`off`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timer_s: Option<f64>,
    pub source: LostLinkSource,
    /// A fixed wing's circle when it holds or reaches home, m: its loiter radius (ArduPlane's
    /// `WP_LOITER_RAD`, or `RTL_RADIUS` when that is set), which the bridge reads with the plan. A
    /// silent fixed wing's region circles at it ([`crate::silent`]). Absent: not known (or not a
    /// fixed wing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circle_m: Option<f64>,
}

/// Rules 30: what a GCS failsafe does ([`LostLinkPlan::action`]), each firmware's parameters mapped
/// onto one of these by the bridge (docs/NODE-LOSS.md section 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LostLinkAction {
    /// The failsafe is off or only warns: nothing changes.
    Off,
    /// It carries on with its mission.
    Continue,
    /// It returns home (or to a rally point).
    Rtl,
    /// It retraces its own way home (SmartRTL), or returns home where it cannot.
    SmartRtl,
    /// It holds where it is (a rover's or a boat's hold).
    Hold,
    /// It circles or loiters where it is.
    Loiter,
    /// It stops where it is (a copter's brake).
    Brake,
    /// It lands where it is.
    Land,
    /// It glides down, its motor off (a fixed wing).
    Glide,
    /// It holds its depth (a submarine).
    DepthHold,
    /// It surfaces (a submarine).
    Surface,
    /// It disarms: on the water it drifts.
    Disarm,
}

/// Rules 30: where a vehicle's [`LostLinkPlan`] comes from (decision D4: read where the link can
/// carry a read).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LostLinkSource {
    /// Read from the vehicle's own parameters, as a ground station reads them.
    Read,
    /// Configured (the link profile's lost-link timer and the platform's kind, sim/links.toml),
    /// where the link cannot carry a read.
    Configured,
}

/// Rules 30: one of ours an operator confirmed lost and no report has revoked since (`node.lost`),
/// as [`WorldSnapshot::nodes_lost`] carries it: lost since its report at `since` (the last position
/// the confirmation was made on; none: it never reported one), confirmed by `by` at `t`. One carried
/// over from an earlier run's log (`from`) names the vehicle it was confirmed on (`lost_on`) and the
/// one under the uid now as the bridge says (`now`): it holds only that very vehicle
/// ([`LostNode::holds`], `crate::nodes`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LostNode {
    pub since: Option<Time>,
    pub by: OperatorId,
    pub t: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// Rules 25: what a vehicle reported damaged in itself ([`VehicleFacts::damage`]). `mobility`: its
/// propulsion is lost (it cannot move: a boat drifts, a ground vehicle stands, an aircraft comes
/// down); `payload_loss`: its payload is out of action.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleDamage {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mobility: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub payload_loss: bool,
}

impl VehicleDamage {
    /// What it reported, in words.
    pub fn describe(&self) -> String {
        match (self.mobility, self.payload_loss) {
            (true, true) => "its propulsion lost and its payload out of action".into(),
            (true, false) => "its propulsion lost".into(),
            (false, true) => "its payload out of action".into(),
            (false, false) => "no damage".into(),
        }
    }
}

impl VehicleFacts {
    /// Rules 15: when the vehicle was last heard, as its facts show: the newest of its
    /// heartbeat (`autopilot`), position and navigation status reports. None: never heard.
    pub fn heard_t(&self) -> Option<Time> {
        [
            self.autopilot.as_ref().map(|f| f.t),
            self.position.as_ref().map(|f| f.t),
            self.nav_ready.as_ref().map(|f| f.t),
        ]
        .into_iter()
        .flatten()
        .max()
    }
}

/// A circle a vehicle flies: its center and its radius with the tolerance it is flown to
/// (the vehicle stays within `radius_m` of `center`). The friendly keep-out trusts it only
/// while the vehicle's position is on it (rules 3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    pub center: LonLat,
    pub radius_m: f64,
    /// Where the bridge knows it from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CircleSource>,
    /// When its center was reported (LOITER and RTL: NAV_CONTROLLER_OUTPUT's target); the
    /// fact is stamped with the older of this and the position. None: a task's own center.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_t: Option<Time>,
}

/// See [`Circle::source`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircleSource {
    /// An orbit task on station, still in the mission's mode.
    OrbitTask,
    /// A fixed wing in LOITER around its navigation target.
    Loiter,
    /// A fixed wing in RTL circling its navigation target (home, or a rally point).
    Rtl,
}

/// See [`VehicleFacts::gps_fix_from`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpsFixFrom {
    /// A HIGH_LATENCY2 report without HL_FAILURE_FLAG_GPS.
    HighLatency2,
}

/// Which autopilot a vehicle runs: its heartbeat's, else the catalog's.
pub fn platform_autopilot(v: &VehicleFacts) -> Option<AutopilotKind> {
    v.autopilot.as_ref().map(|a| a.value).or_else(|| v.platform.as_ref().and_then(|p| p.autopilot))
}

/// Where a vehicle's horizontal position comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavSource {
    /// Satellite fix: the navigation filter has an absolute position.
    Gnss,
    /// Relative position only (inertial plus DVL or odometry), error growing with distance.
    DeadReckoning,
    /// No usable position estimate.
    None,
}

impl NavSource {
    pub fn as_str(self) -> &'static str {
        match self {
            NavSource::Gnss => "gnss",
            NavSource::DeadReckoning => "dead_reckoning",
            NavSource::None => "none",
        }
    }
}

/// Terrain answers for a task's waypoints. The engine fills this in from a
/// [`crate::TerrainOracle`] before evaluating rules; the bridge leaves it `NotQueried`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TerrainInputs {
    #[default]
    NotQueried,
    /// No target needs a land/water check, or the task has no waypoints.
    NotNeeded,
    /// No oracle installed or it failed as a whole: the domain rule records "not evaluated".
    Unavailable {
        reason: String,
    },
    Sampled {
        samples: Vec<TerrainSample>,
        /// Rules 17: the answers along each leg of the vehicles' routes ([`LegSurfaces`]).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        along: Vec<LegSurfaces>,
    },
}

/// Rules 17: what the land/water source answered at the points along one straight leg of a
/// route ([`crate::route::along`]), one letter a point in travel order ([`Surface::letter`]:
/// `w` water deeper than 0, `d` water no deeper, `l` land, `o` outside coverage). The rule works
/// the points out again from the leg's ends, which the logged facts give.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegSurfaces {
    /// The leg's start and end, `[lon, lat]` at MAVLink's 1e-7 degree resolution.
    pub from_e7: [i64; 2],
    pub to_e7: [i64; 2],
    pub surfaces: String,
    /// The farthest a point's answer was taken from the point, m, when the source answered
    /// some from a sample near them (at most [`crate::route::LEG_NEAR_M`]); absent: every point
    /// was answered itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_m: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainSample {
    pub lon_e7: i64,
    pub lat_e7: i64,
    pub surface: Surface,
    /// Where a nearby sample the answer was taken from lies: which sample was used, and how far
    /// from the point it was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<SampleFrom>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampleFrom {
    pub lon_e7: i64,
    pub lat_e7: i64,
    pub distance_m: f64,
}

impl TerrainInputs {
    /// Rules 34: a group order's terrain, each member's own answers (its share's waypoints and
    /// legs) put together: unavailable if any member's was, else every sample and leg once.
    pub fn merge(&mut self, other: TerrainInputs) {
        match (std::mem::take(self), other) {
            (TerrainInputs::Unavailable { reason }, _) | (_, TerrainInputs::Unavailable { reason }) => {
                *self = TerrainInputs::Unavailable { reason };
            }
            (TerrainInputs::Sampled { mut samples, mut along }, TerrainInputs::Sampled { samples: s2, along: a2 }) => {
                for x in s2 {
                    if !samples.iter().any(|y| y.lon_e7 == x.lon_e7 && y.lat_e7 == x.lat_e7) {
                        samples.push(x);
                    }
                }
                for l in a2 {
                    if !along.iter().any(|m| m.from_e7 == l.from_e7 && m.to_e7 == l.to_e7) {
                        along.push(l);
                    }
                }
                *self = TerrainInputs::Sampled { samples, along };
            }
            (a @ TerrainInputs::Sampled { .. }, _) | (_, a @ TerrainInputs::Sampled { .. }) => *self = a,
            (TerrainInputs::NotQueried, b) => *self = b,
            (a, _) => *self = a,
        }
    }

    pub fn lookup(&self, p: LonLat) -> Option<&Surface> {
        let (lon_e7, lat_e7) = p.e7();
        match self {
            TerrainInputs::Sampled { samples, .. } => samples
                .iter()
                .find(|s| s.lon_e7 == lon_e7 && s.lat_e7 == lat_e7)
                .map(|s| &s.surface),
            _ => None,
        }
    }

    /// Rules 17: the answers along the leg from `from` to `to`.
    pub fn along_for(&self, from: LonLat, to: LonLat) -> Option<&LegSurfaces> {
        let (f, t) = (from.e7(), to.e7());
        match self {
            TerrainInputs::Sampled { along, .. } => along
                .iter()
                .find(|l| l.from_e7 == [f.0, f.1] && l.to_e7 == [t.0, t.1]),
            _ => None,
        }
    }
}

/// The input to every rule and policy decision. Logged (reduced to what one intent can
/// read) with each decision, so replay recomputes from exactly this.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    /// When the snapshot was taken; staleness is measured against it.
    pub t: Time,
    pub vehicles: BTreeMap<Uid, VehicleFacts>,
    /// None: no area file loaded. Rules that need areas then fail closed.
    pub areas: Option<AreaSet>,
    #[serde(default)]
    pub terrain: TerrainInputs,
    /// Engine-filled (rules 22, [`crate::plan`]): the legs of the intent's vehicles routed around
    /// the wrong surface or a no-go area, each with its grid, or why no route exists. The route
    /// layout follows them ([`crate::route::route_for`]). Absent when none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub route_plans: BTreeMap<Uid, Vec<crate::plan::PlannedLeg>>,
    /// Engine-filled: the intent's vehicles with a consequential order in play (pending,
    /// approved or dispatched), by that order's id. An agent's order on one of them always goes
    /// to a human. Absent when empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub in_play: BTreeMap<Uid, IntentId>,
    /// Engine-filled (rules 5): the intent's vehicles that have an order a human approved
    /// waiting in the queue, by that intent's id. An agent's order on one of them goes to a
    /// human (it would supersede the human's order). Absent when empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waiting: BTreeMap<Uid, IntentId>,
    /// Engine-filled (rules 30, [`crate::rules::NODE_LOSS_SINCE`]): our vehicles an operator
    /// confirmed lost and no report has revoked since, from the engine's ledger (`node.lost`). The
    /// bridge never gives them; the engine drops them before logging under older rules. Absent
    /// when none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nodes_lost: BTreeMap<Uid, LostNode>,
    /// Engine-filled (rules 30): for each of the intent's vehicles that is silent (its position
    /// not fresh), the orders the engine's record says may be running on it, which the friendly
    /// keep-out lays its region out from ([`crate::silent::in_force`]). The bridge never gives
    /// them. Absent when none is silent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub silent_orders: BTreeMap<Uid, crate::silent::InForce>,
    /// What the delivery rules read besides the vehicles' own facts: places, parcels, weather,
    /// traffic, navigation and link predictions ([`crate::delivery::DeliveryInputs`]). The
    /// bridge fills the world's; the engine adds what it knows of its own orders. Absent: none
    /// given (the delivery rules then report each input they need as unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<crate::delivery::DeliveryInputs>,
    /// Engine-filled (rules 8, [`crate::rules::PLAYS_SINCE`]): the plays a person adopted that are
    /// in force ([`crate::plays`]), which the policy routes by. The bridge never gives them.
    /// Absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plays: Vec<crate::plays::Play>,
}

/// A decision's inputs as logged: the [`WorldSnapshot`] with the area set replaced by its
/// content hash ([`AreaSet::content_hash`]). The set itself is logged once, in `run.started` or
/// `areas.loaded`, and replay resolves the reference.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoggedInputs {
    pub t: Time,
    pub vehicles: BTreeMap<Uid, VehicleFacts>,
    /// None: no area set was loaded.
    pub areas_sha256: Option<String>,
    #[serde(default)]
    pub terrain: TerrainInputs,
    /// Its vehicles' routed legs (rules 22; absent when none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub route_plans: BTreeMap<Uid, Vec<crate::plan::PlannedLeg>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub in_play: BTreeMap<Uid, IntentId>,
    /// The intent's vehicles with a human-approved order waiting in the queue (absent when
    /// none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waiting: BTreeMap<Uid, IntentId>,
    /// Our vehicles confirmed lost (rules 30; absent when none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nodes_lost: BTreeMap<Uid, LostNode>,
    /// The orders that may be running on our silent vehicles (rules 30; absent when none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub silent_orders: BTreeMap<Uid, crate::silent::InForce>,
    /// What the delivery rules read (absent when none was given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<crate::delivery::DeliveryInputs>,
    /// The plays in force (rules 8; absent when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plays: Vec<crate::plays::Play>,
}

impl LoggedInputs {
    /// `world` with its area set replaced by `areas_sha256` (the caller hashed it and logged what
    /// it names).
    pub fn from_world(world: &WorldSnapshot, areas_sha256: Option<String>) -> Self {
        LoggedInputs {
            t: world.t,
            vehicles: world.vehicles.clone(),
            areas_sha256,
            terrain: world.terrain.clone(),
            route_plans: world.route_plans.clone(),
            in_play: world.in_play.clone(),
            waiting: world.waiting.clone(),
            nodes_lost: world.nodes_lost.clone(),
            silent_orders: world.silent_orders.clone(),
            delivery: world.delivery.clone(),
            plays: world.plays.clone(),
        }
    }

    /// The snapshot again, with `areas` the set the reference resolved to. A space answer
    /// reference can't be resolved here and becomes `Unavailable` (replay uses
    /// [`Self::resolve_with_space`]).
    pub fn resolve(&self, areas: Option<AreaSet>) -> WorldSnapshot {
        WorldSnapshot {
            t: self.t,
            vehicles: self.vehicles.clone(),
            areas,
            terrain: self.terrain.clone(),
            route_plans: self.route_plans.clone(),
            in_play: self.in_play.clone(),
            waiting: self.waiting.clone(),
            nodes_lost: self.nodes_lost.clone(),
            silent_orders: self.silent_orders.clone(),
            delivery: self.delivery.clone(),
            plays: self.plays.clone(),
        }
    }
}

impl WorldSnapshot {
    pub fn new(t: Time) -> Self {
        WorldSnapshot {
            t,
            ..Default::default()
        }
    }

    /// The part of the world an intent's rules can read: its vehicles,
    /// the areas and terrain. This is what gets logged (as [`LoggedInputs`], the area set
    /// replaced by its hash).
    pub fn subset_for(&self, spec: &IntentSpec) -> WorldSnapshot {
        self.subset_reaching(spec, false)
    }

    /// [`Self::subset_for`], its vehicles' sensors' reach now kept only with `reach_now` (rules 32).
    fn subset_reaching(&self, spec: &IntentSpec, reach_now: bool) -> WorldSnapshot {
        let wanted: Vec<&Uid> = spec.uids.iter().collect();
        let mut vehicles: BTreeMap<Uid, VehicleFacts> = {
            wanted
                .into_iter()
                .filter_map(|u| self.vehicles.get(u).map(|v| (u.clone(), v.clone())))
                .collect()
        };
        if !reach_now {
            for v in vehicles.values_mut() {
                v.reach_now = None;
            }
        }
        WorldSnapshot {
            t: self.t,
            vehicles,
            areas: self.areas.clone(),
            terrain: self.terrain.clone(),
            // engine-owned (rules 22): planned for this intent
            route_plans: BTreeMap::new(),
            // engine-owned: filled from its own intents
            in_play: BTreeMap::new(),
            waiting: BTreeMap::new(),
            // engine-owned (rules 30): filled from its ledger of our vehicles confirmed lost
            nodes_lost: BTreeMap::new(),
            // engine-owned (rules 30): filled from its record of our orders
            silent_orders: BTreeMap::new(),
            // the world's delivery inputs: the checks of a delivery order read them (the engine
            // adds what it knows of its own orders)
            delivery: self.delivery.clone(),
            // engine-owned (rules 8): the plays in force
            plays: Vec::new(),
        }
    }

    /// What the engine logs and evaluates for one intent: [`Self::subset_for`] plus terrain
    /// answers for its waypoints from `oracle` (only if some target needs a land/water
    /// check), and from rules 17 for the points along its vehicles' routes. Public so the
    /// bridge can build the same view, e.g. to prefetch terrain.
    pub fn for_intent(&self, spec: &IntentSpec, oracle: &dyn TerrainOracle) -> WorldSnapshot {
        self.for_intent_with(spec, oracle, &RulesConfig::default())
    }

    pub fn for_intent_with(
        &self,
        spec: &IntentSpec,
        oracle: &dyn TerrainOracle,
        cfg: &RulesConfig,
    ) -> WorldSnapshot {
        let identify = false;
        let mut w = self.subset_reaching(spec, identify);
        let routed = |w: &mut WorldSnapshot| {
            // rules 22: legs the domain rule or the geofence would refuse are routed first, and
            // the points along the routed legs sampled again
            if !matches!(w.terrain, TerrainInputs::Unavailable { .. }) {
                let plans = crate::plan::plans_for(spec, w, oracle, cfg);
                if !plans.is_empty() {
                    w.route_plans = plans;
                    Self::sample_legs(w, spec, oracle, cfg);
                }
            }
        };
        w.terrain = match spec.task.path() {
            Some(path) if needs_terrain(spec, &w) => match oracle.sample(w.t, &path.points) {
                Ok(answers) if answers.len() == path.points.len() => TerrainInputs::Sampled {
                    samples: path
                        .points
                        .iter()
                        .zip(answers)
                        .map(|(p, surface)| {
                            let (lon_e7, lat_e7) = p.e7();
                            TerrainSample {
                                lon_e7,
                                lat_e7,
                                surface,
                                from: None,
                            }
                        })
                        .collect(),
                    along: Vec::new(),
                },
                // A wrong-length answer is a broken oracle, not a missing one: no samples,
                // so the domain rule fails rather than skipping.
                Ok(_) => TerrainInputs::Sampled {
                    samples: Vec::new(),
                    along: Vec::new(),
                },
                Err(reason) => TerrainInputs::Unavailable { reason },
            },
            _ => TerrainInputs::NotNeeded,
        };
        Self::sample_legs(&mut w, spec, oracle, cfg);
        routed(&mut w);
        // rules 3: only the part of the traffic picture the order's checks read is kept (and logged)
        crate::traffic::trim(&mut w, spec, cfg);
        w
    }


    /// Rules 17 ([`crate::rules::ROUTE_SURFACE_SINCE`]): the answers along every leg the domain
    /// rule judges ([`crate::route::surface_legs`]), in one query, beside the waypoints' (or the
    /// it does for a waypoint (the rule records "not evaluated"). Nothing under older rules.
    fn sample_legs(w: &mut WorldSnapshot, spec: &IntentSpec, oracle: &dyn TerrainOracle, cfg: &RulesConfig) {
        if matches!(w.terrain, TerrainInputs::Unavailable { .. }) {
            return;
        }
        let legs: Vec<(crate::route::RouteLeg, Vec<(LonLat, f64)>)> = crate::route::surface_legs(spec, w, cfg)
            .into_iter()
            .filter_map(|l| {
                let pts = crate::route::along(l.from, l.to)?;
                (!pts.is_empty()).then_some((l, pts))
            })
            .collect();
        if legs.is_empty() {
            return;
        }
        let points: Vec<LonLat> = legs.iter().flat_map(|(_, pts)| pts.iter().map(|(p, _)| *p)).collect();
        let answers = match oracle.sample_along(w.t, &points, crate::route::LEG_NEAR_M) {
            Ok(a) if a.len() == points.len() => a,
            // a wrong-length answer is a broken oracle, not a missing one: no answers along the
            // legs, so the rule fails rather than skipping (as for the waypoints)
            Ok(_) => Vec::new(),
            Err(reason) => {
                w.terrain = TerrainInputs::Unavailable { reason };
                return;
            }
        };
        let mut answers = answers.into_iter();
        let along: Vec<LegSurfaces> = legs
            .iter()
            .map(|(l, pts)| {
                let mut near: f64 = 0.0;
                let surfaces: String = answers
                    .by_ref()
                    .take(pts.len())
                    .map(|a| {
                        near = near.max(a.distance_m);
                        a.surface.letter()
                    })
                    .collect();
                let (f, t) = (l.from.e7(), l.to.e7());
                LegSurfaces {
                    from_e7: [f.0, f.1],
                    to_e7: [t.0, t.1],
                    surfaces,
                    near_m: (near > 0.0).then(|| (near * 10.0).round() / 10.0),
                }
            })
            .collect();
        match &mut w.terrain {
            TerrainInputs::Sampled { along: a, .. } => *a = along,
            other => {
                *other = TerrainInputs::Sampled {
                    samples: Vec::new(),
                    along,
                }
            }
        }
    }
}
