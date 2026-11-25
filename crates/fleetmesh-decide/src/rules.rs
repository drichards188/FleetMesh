//! Rules (Phase C.2): pure functions of `(intent, &WorldSnapshot, &RulesConfig)`.
//!
//! Fail-closed: a rule that can't evaluate (vehicle unknown, fact missing or stale, area
//! file not loaded, terrain point unanswered) returns `Fail` and says why. Two
//! exceptions, both recorded in the intent's checks:
//! - `NotEvaluated`: deliberately skipped. The domain rule with no terrain oracle
//!   installed (PLAN.md), and an rtl whose return leg can't be checked (rtl is the safety
//!   command, as in the readiness exemption). Blocks only consequential kinds
//! - `Unverified`: the rule can't verify the condition, and a human must judge it: the
//!   reserves rule on a platform with no endurance figure. It doesn't block the rules
//!   gate, but policy never auto-approves it ([`crate::policy::decide`]).
//!
//! Output order is fixed (rule order, then the intent's uid order) and detail strings use
//! fixed precision, so recomputing a decision gives byte-identical results.
//!
//! Space kinds (collect, cover) target no vehicle, so the vehicle rules don't apply to them;
//! they get their own, which read only the logged space answer ([`crate::space`]):
//! `space_query` (the segment answered this intent's question), then `feasibility` for a
//! collect (an option is chosen and still feasible: pass in the future, lead time and
//! deadline met) or `coverage` for a cover. A rule set that produced no result at all
//! fails ("no rule evaluated"): an empty check list never passes.

use serde::{Deserialize, Serialize};

use crate::areas::Area;
use crate::catalog::{AutopilotKind, Domain};
use crate::geo::haversine_m;
use crate::ids::Uid;
use crate::intent::{IntentSpec, Task, TaskKind};
use crate::route::{Segment, route_for};
use crate::terrain::Surface;
use crate::world::{Fact, NavSource, TerrainInputs, VehicleFacts, WorldSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleId {
    Capability,
    Readiness,
    PlatformLimits,
    Domain,
    Geofence,
    Reserves,
    /// Rules 34: a group order: at least its minimum of members can take it (sent or waiting).
    Strength,
    /// Rules 34: a sub member's layer clears the seabed margin at its share's waypoints.
    Depth,
    /// Rules 34: the layout puts no two members of one domain within both minima on their
    /// shares (more members than its layers or rings hold refuses the group).
    Separation,
    /// Delivery orders: what the order itself needs before any rule reads the world: the parcel,
    /// the place, the order it names (delivery-orders.md section 5).
    Order,
    /// Delivery: battery and range reserve with wind (delivery-rules.md section 5).
    Energy,
    /// Delivery: the parcel against the platform's payload limit (section 6).
    Payload,
    /// Delivery: wind, gusts, visibility, precipitation, tide and current (section 7).
    Weather,
    /// Delivery: the drop zone's state and conditions (section 10).
    Dropzone,
    /// Delivery: whether the position fix can be trusted (section 9).
    NavIntegrity,
    /// Delivery: the failsafe plan and the link along the route (section 11).
    LostLink,
}

impl RuleId {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleId::Capability => "capability",
            RuleId::Readiness => "readiness",
            RuleId::PlatformLimits => "platform_limits",
            RuleId::Domain => "domain",
            RuleId::Geofence => "geofence",
            RuleId::Reserves => "reserves",
            RuleId::Strength => "strength",
            RuleId::Depth => "depth",
            RuleId::Separation => "separation",
            RuleId::Order => "order",
            RuleId::Energy => "energy",
            RuleId::Payload => "payload",
            RuleId::Weather => "weather",
            RuleId::Dropzone => "dropzone",
            RuleId::NavIntegrity => "nav_integrity",
            RuleId::LostLink => "lost_link",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    /// Deliberately skipped (no terrain oracle; an rtl's unchecked return leg). Recorded;
    /// blocks consequential kinds only.
    NotEvaluated,
    /// Could not be verified, and a human must judge it (endurance unknown). Doesn't block
    /// the rules gate (except for consequential kinds); policy routes it to a human and
    /// never auto-approves it.
    Unverified,
    /// A delivery check passed the order's hard limits but found something a person should
    /// judge (a marginal reserve, a gust above its soft limit, an unconfirmed drop zone). It
    /// blocks nothing, and policy never auto-approves an order with one: the order goes to the
    /// inbox whatever its kind's tier says. Unlike `Unverified` (the check could not tell), the
    /// reason codes say what is marginal (delivery-rules.md section 1).
    Hold,
}

/// A change that would make a failing intent pass. Suggested, never applied silently.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fix", rename_all = "snake_case")]
pub enum Fix {
    WidenRadius { radius_m: f64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub rule: RuleId,
    /// The vehicle this result is about (every rule today is per vehicle).
    pub uid: Option<Uid>,
    pub outcome: Outcome,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    /// Rules 17: what a person must see though the check passed: a safety command (rtl) whose
    /// way home crosses land goes as ordered, and the bridge raises an inbox alert from this
    /// (`path_crosses_land`) while the order runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// Rules 34: on a group order's failing result for one member, how it classes that member
    /// (waiting or excluded, [`crate::group::MemberClass`]) instead of failing the group, whose
    /// `strength` rule counts what is left. Absent on every other result. Logged as `member`.
    #[serde(default, rename = "member", skip_serializing_if = "Option::is_none")]
    pub class: Option<crate::group::MemberClass>,
    /// Delivery rules: the stable reason codes behind a `hold` or a `fail` (`GEOFENCE_NO_GO`,
    /// `ENERGY_MARGIN_THIN`, ...). `detail` states the numbers; the code is the key a proposer
    /// or a measure reads. Empty on a pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codes: Vec<String>,
}

impl CheckResult {
    pub fn new(rule: RuleId, uid: Option<&Uid>, outcome: Outcome, detail: impl Into<String>) -> Self {
        CheckResult {
            rule,
            uid: uid.cloned(),
            outcome,
            detail: detail.into(),
            fix: None,
            warning: None,
            class: None,
            codes: Vec::new(),
        }
    }

    /// This result with its reason codes.
    pub fn with_codes(mut self, codes: &[&str]) -> Self {
        self.codes = codes.iter().map(|c| (*c).to_string()).collect();
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownEndurance {
    /// No endurance figure: the result is `Unverified` ("not evaluated: endurance
    /// unknown"), which policy routes to a human. A battery already below the reserve
    /// still fails.
    #[default]
    Human,
    /// No endurance figure: the reserves rule can't evaluate, so it fails.
    Fail,
    /// No endurance figure: pass if the battery is above the reserve now (a weaker check,
    /// stated in the detail).
    BatteryNow,
}

/// The version of the decision rules: bumped whenever a change alters a decision (a new
/// check, another threshold or model, another detail text), so replay recomputes every log
/// with the rules it was produced under. `run.started` records it. FleetMesh starts at 1;
/// new behaviour is gated on a named `*_SINCE` constant set to the version that adds it
/// (never a literal), and replay is taught the same gate in the same change.
pub const RULES_VERSION: u32 = FLOOD_ALONG_ROUTE_SINCE;

/// Rules 2: delivery points are approved ahead of time and a person is asked only about an
/// exception (docs/DECISIONS.md, 2026-11-25, "Routine deliveries need no person"). A drop zone
/// that carries a standing approval (`approval = "survey"` or `"customer"` in the places file), a
/// hub pad, or a zone a person approved with `confirm_drop_zone` passes the confirmation part of
/// the drop-zone rule with no time limit; any other zone holds `DZ_NOT_APPROVED` (it was
/// `DZ_UNCONFIRMED` within `confirm_valid_s` under rules 1). A `confirm_drop_zone` approves a
/// delivery point and needs no delivery waiting for it. `[rules.delivery.dropzone]
/// standing_approvals = false` restores the rules 1 procedure (the comparison setting).
pub const STANDING_APPROVALS_SINCE: u32 = 2;

/// Rules 3: the fleet keeps clear of the traffic it does not control, as the company's picture
/// shows it (docs/design/traffic.md, docs/DECISIONS.md 2026-11-25, "Other traffic: automation keeps
/// clear, a person hears of issues"). A delivery order's decision reads the picture
/// (`DeliveryInputs::picture`, cut to the tracks and receivers near the order, logged by value),
/// and the separation rule keeps a drone well clear of manned aircraft (2,000 ft and 250 ft) and
/// clear of other drones and of air tracks that broadcast nothing, a boat clear of ships under way
/// (a 0.25 nm closest approach, and out of a ship's way ahead of it) and of other vessels, each
/// along the tracker's prediction widened by its uncertainty; it holds an order whose route the
/// picture no longer covers or whose picture is stale. `[rules.delivery.separation.traffic]` holds
/// the figures (policy schema 3).
pub const TRAFFIC_SEPARATION_SINCE: u32 = 3;

/// Rules 4: every vehicle belongs to a home base, a hub, as a delivery fleet's vehicles belong to
/// their nest, hub or depot (docs/design/fleet-organisation.md, docs/DECISIONS.md 2026-11-25,
/// "The fleet is organised by home base"). The assigner's input names each candidate's home hub
/// (`dispatch.plan`'s `input.vehicles[].home`), and the way back of a trip it costs goes to that
/// hub, or to the hub nearest the drop zone when the battery would not cover the way home (under
/// rules 3 it went to the nearest hub always). No check reads it, and the policy file does not
/// change.
pub const HOME_BASE_SINCE: u32 = 4;

/// Rules 5: a hub sequences its own arrivals and departures, as a vertiport or a nest does, and
/// the separation rule predicts the fleet's own vehicles as they will move (docs/DECISIONS.md
/// 2026-11-25, "Hubs sequence their own traffic"; delivery-rules.md section 8). Who lands or leaves
/// first at a hub is automation's, never a person's:
///
/// - **Prediction.** Another of the fleet's vehicles with a dispatched route
///   (`DeliveryInputs::traffic[..].route`, the bridge fills it from rules 5) is predicted along it
///   at its platform's cruise speed (or faster, as it goes) and at rest at its end; one with no
///   route that is still (on the ground, or under [`STILL_MPS`]) is predicted where it is, never
///   carried off by the noise of its velocity estimate over the horizon; and the position
///   uncertainty that grows with time (`drift_mps`) stops growing once both vehicles of a pair are
///   at rest (the order's at its route's end, the other still or at its own). Before rules 5 every
///   other vehicle was carried along its velocity for the whole horizon, and two vehicles standing
///   still 150 m apart met at +300 s.
/// - **Parked vehicles.** One parked on a hub's stand (still, not in the air, inside the hub's
///   radius) is the hub's: not counted against an order while it is on the same hub's stands, nor
///   against an order that leaves or reaches that hub (its bays, berths and pads keep them clear);
///   one whose route ends on a hub's stand counts as parked there from the time it arrives.
/// - **Stands.** An order that does not move its vehicle (a pickup where it stands) has no route to
///   separate, and a pickup or a return may name a point within the hub's radius (the vehicle's
///   own stand, or a pad automation gave it) instead of the hub's centre point: the order rule
///   accepts a point within `radius_m` of the hub (5 m before rules 5).
/// - **Sequencing.** A meeting within `hub_zone_m` of a hub holds `SEP_HUB_ZONE` as before, and its
///   words say the hub sequences it (before rules 5: "a person decides who waits"); the dispatcher
///   waits it out and never sends the order to a person for it.
///
/// No minimum, threshold or policy key changed; the policy schema stays 3.
pub const HUB_SEQUENCING_SINCE: u32 = 5;

/// Rules 5: a vehicle of the fleet moving slower than this, m/s, with no route, is predicted at
/// rest ([`HUB_SEQUENCING_SINCE`]): a parked or hovering vehicle's velocity estimate is noise of
/// a few decimetres a second, which the old prediction carried for the whole horizon (90 m over
/// 300 s at 0.3 m/s) through a hub. The traffic rule's `underway_mps` for a vessel is the same
/// figure.
pub const STILL_MPS: f64 = 0.5;

/// Rules 6: the weather rule reads the ground and the water where the order takes its vehicle
/// (the samples within [`FLOOD_NEAR_M`] of its position, its route's points and its way back's
/// end), not every sample the world has (docs/DECISIONS.md 2026-11-25): a car's flood check
/// (`WX_FLOOD_RISK`) and a boat's depth under the keel (`TIDE_SHALLOW`, `TIDE_AGROUND`). Under
/// rules 5 and before, a water sample anywhere (a boat dock's seabed, 4 m under the tide) held
/// every car's order `WX_FLOOD_RISK`, and the shallowest water anywhere judged every boat's. The
/// wind, gusts, visibility and current are still the worst of every fresh sample, and no
/// threshold changed.
pub const FLOOD_ALONG_ROUTE_SINCE: u32 = 6;

/// Rules 6: how near the vehicle's position or one of its order's points a weather sample must be
/// for the ground and water checks to read it, m ([`FLOOD_ALONG_ROUTE_SINCE`]): the samples are
/// taken at the hubs, the open drop zones and the vehicles, so this keeps the order's own and
/// leaves a neighbouring dock's seabed out.
pub const FLOOD_NEAR_M: f64 = 60.0;

/// The fields each rules version added to what a log carries: (the version that added it, where
/// it is). Replay flags a log whose `run.started` claims an older version but that carries one of
/// them (an event that could not have existed then: a newer log given an older header).
pub const RULES_MARKERS: &[(u32, &str)] = &[
    (STANDING_APPROVALS_SINCE, "policy.rules.delivery.dropzone.standing_approvals"),
    (STANDING_APPROVALS_SINCE, "check.result.inputs.delivery.places.drop_zones.approval"),
    (TRAFFIC_SEPARATION_SINCE, "policy.rules.delivery.separation.traffic"),
    (TRAFFIC_SEPARATION_SINCE, "check.result.inputs.delivery.picture"),
    (HOME_BASE_SINCE, "dispatch.plan.input.vehicles.home"),
    (HUB_SEQUENCING_SINCE, "check.result.inputs.delivery.traffic.route"),
];

/// The newest decision rules this build can decide under and replay: [`RULES_VERSION`] itself,
/// except on a branch that builds the next version in stages. Replay accepts a log of any
/// version up to this one and flags one newer.
pub const RULES_NEWEST: u32 = RULES_VERSION;

/// How far a collect's space answer may have been computed from the check that decides on
/// it, ms: the engine takes an answer prepared outside its lock only within this, and from
/// the `space_query` rule fails a collect whose logged answer
/// was computed farther from its check (a cover's question does not depend on when it was
/// asked, so its age is no concern).
pub const PREPARED_SPACE_MAX_AGE_MS: i64 = 5_000;

/// The version of the policy file's schema: bumped when a key is added, renamed or removed.
/// A renamed or removed key stays an optional field of its own, so an old log both parses
/// and re-serializes byte for byte; `run.started` records it (absent: 1), and replay checks
/// This is the one a run of the default rules logs; a run under older rules logs the schema they
/// logged ([`RulesConfig::policy_schema`]: 9 under rules 34, 8 under rules 33, 7 below them, whose
/// keys it does not log). Schema 2 (rules 2, [`STANDING_APPROVALS_SINCE`]) adds
/// `[rules.delivery.dropzone] standing_approvals`; schema 3 (rules 3,
/// [`TRAFFIC_SEPARATION_SINCE`]) adds `[rules.delivery.separation.traffic]`.
pub const POLICY_SCHEMA: u32 = TRAFFIC_SEPARATION_SCHEMA;

/// The policy schema rules 3 logs: [`STANDING_APPROVALS_SCHEMA`] and the traffic figures.
pub const TRAFFIC_SEPARATION_SCHEMA: u32 = 3;

/// The policy schema rules 2 logs: [`POLICY_SCHEMA_V1`] and `standing_approvals`.
pub const STANDING_APPROVALS_SCHEMA: u32 = 2;

/// The policy schema rules 1 logged.
pub const POLICY_SCHEMA_V1: u32 = 1;

/// The newest policy schema this build reads and replay accepts: [`POLICY_SCHEMA`], or the one
/// rules being built behind [`RULES_NEWEST`] add (none now).
pub const POLICY_SCHEMA_NEWEST: u32 = POLICY_SCHEMA;

fn current_rules_version() -> u32 {
    RULES_VERSION
}

/// Rule settings, from `[rules]` in `sim/policy.toml`. Logged with the policy at run start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RulesConfig {
    /// The rules version the checks run under: [`RULES_VERSION`] for a run, the logged one
    /// in a replay. Not a policy key, and not logged with the policy (`run.started` carries
    /// it on its own).
    #[serde(skip, default = "current_rules_version")]
    pub rules_version: u32,
    /// A fact older than this (against the snapshot time) can't be relied on.
    pub max_fact_age_s: u32,
    /// Link-aware freshness: a vehicle's limit is the larger of `max_fact_age_s` and this many
    /// of its expected reporting intervals (3 x 20 s = 60 s on an acoustic link). Absent: no
    /// scaling, `max_fact_age_s` for everyone (the behaviour before the key existed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fact_age_intervals: Option<u32>,
    /// Orbit radius must be at least this times the minimum turn radius.
    pub orbit_radius_margin: f64,
    /// For fixed-wing platforms with no catalog turn figures: ArduPlane's WP_LOITER_RAD default.
    pub fixed_wing_default_min_turn_radius_m: f64,
    /// Endurance left after the task, percent of full endurance.
    pub reserve_pct: f64,
    /// Time on task assumed by the reserves estimate (orbits and patrols loop until told
    /// otherwise, so the estimate needs a horizon).
    pub task_duration_s: u32,
    pub unknown_endurance: UnknownEndurance,
    /// Rules 33 (policy schema 8): `[rules.silent]`, the figures for a silent vehicle (its
    /// drift); the defaults (today's constants) when absent. Not logged under older rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silent: Option<crate::silent::SilentRules>,
    /// `[rules.delivery]`: the numbers of the delivery rules (docs/design/delivery-rules.md);
    /// the first-draft defaults when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<crate::delivery::DeliveryRules>,
}

impl RulesConfig {
    /// `[rules.delivery]`, or its defaults.
    pub fn delivery_rules(&self) -> crate::delivery::DeliveryRules {
        self.delivery.clone().unwrap_or_default()
    }

    /// Freshness limit in ms for a vehicle reporting every `interval_ms` (None: unknown), and
    /// whether the link-aware part set it.
    pub fn fact_age_limit_ms(&self, interval_ms: Option<u32>) -> (i64, bool) {
        let base = i64::from(self.max_fact_age_s) * 1000;
        match (self.fact_age_intervals, interval_ms) {
            (Some(n), Some(i)) if i64::from(n) * i64::from(i) > base => {
                (i64::from(n) * i64::from(i), true)
            }
            _ => (base, false),
        }
    }


    /// `[rules.silent]`, or its defaults.
    pub fn silent_rules(&self) -> crate::silent::SilentRules {
        self.silent.clone().unwrap_or_default()
    }

    /// How fast the water or the wind carries one of ours that makes no way, by domain: the
    /// policy's `[rules.silent] drift_mps`.
    pub fn silent_drift(&self) -> crate::silent::SilentDrift {
        self.silent_rules().drift_mps
    }

    /// The policy schema a run under these rules logs (`run.started`'s `policy_schema`): 3 from
    /// rules 3 (the traffic figures), 2 from rules 2 (`standing_approvals`), 1 before.
    pub fn policy_schema(&self) -> u32 {
        if self.at_least(TRAFFIC_SEPARATION_SINCE) {
            TRAFFIC_SEPARATION_SCHEMA
        } else if self.at_least(STANDING_APPROVALS_SINCE) {
            STANDING_APPROVALS_SCHEMA
        } else {
            POLICY_SCHEMA_V1
        }
    }

    /// Whether these rules include the behaviour a `*_SINCE` constant names (rules-versioning.md
    /// section 5): the one way code branches on the version.
    pub fn at_least(&self, since: u32) -> bool {
        self.rules_version >= since
    }

    /// Rules 2: whether a drop zone's standing approval stands in for a person's confirmation
    /// ([`STANDING_APPROVALS_SINCE`]): from rules 2 unless the policy sets
    /// `[rules.delivery.dropzone] standing_approvals = false` (the rules 1 procedure, kept as the
    /// comparison setting); never under rules 1.
    pub fn standing_approvals(&self) -> bool {
        self.at_least(STANDING_APPROVALS_SINCE)
            && self.delivery.as_ref().is_none_or(|d| d.dropzone.standing_approvals != Some(false))
    }
}

impl Default for RulesConfig {
    fn default() -> Self {
        RulesConfig {
            rules_version: RULES_VERSION,
            max_fact_age_s: 10,
            fact_age_intervals: None,
            orbit_radius_margin: 1.1,
            fixed_wing_default_min_turn_radius_m: 60.0,
            reserve_pct: 20.0,
            task_duration_s: 900,
            unknown_endurance: UnknownEndurance::Human,
            silent: None,
            delivery: None,
        }
    }
}

/// All rules, in order.
pub fn evaluate(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    // rules 34: a group order's check, member by member on each one's own share
    if crate::group::is_group(spec, cfg) {
        return crate::group::evaluate(spec, world, cfg).results;
    }
    // delivery orders (and a hold, where the world gives delivery inputs) have their own rule set
    if crate::delivery_rules::applies(spec, world) {
        return crate::delivery_rules::evaluate(spec, world, cfg);
    }
    let mut out = Vec::new();
    out.extend(capability(spec, world, cfg));
    out.extend(readiness(spec, world, cfg));
    out.extend(platform_limits(spec, world, cfg));
    out.extend(domain(spec, world, cfg));
    out.extend(geofence(spec, world, cfg));
    out.extend(reserves(spec, world, cfg));
    if out.is_empty() {
        out.push(CheckResult::new(
            RuleId::Capability,
            None,
            Outcome::Fail,
            "no rule evaluated (no target)",
        ));
    }
    out
}

/// Whether a set of results lets the intent proceed.
pub fn passed(results: &[CheckResult], kind: TaskKind) -> bool {
    // rules 34: a group member's own failure classes that member (its `class`), and the group's
    // `strength` rule decides whether enough are left
    results.iter().filter(|r| r.class.is_none()).all(|r| match r.outcome {
        Outcome::Pass | Outcome::Hold => true,
        Outcome::Fail => false,
        Outcome::NotEvaluated | Outcome::Unverified => !kind.is_consequential(),
    })
}

/// The results a human must judge (outcome `Unverified`), as `"rule uid: detail"`.
pub fn unverified(results: &[CheckResult]) -> Vec<String> {
    results
        .iter()
        .filter(|r| r.outcome == Outcome::Unverified)
        .map(|r| match &r.uid {
            Some(u) => format!("{} {u}: {}", r.rule.as_str(), r.detail),
            None => format!("{}: {}", r.rule.as_str(), r.detail),
        })
        .collect()
}

/// The results a person must judge because a delivery check came out `hold`, as
/// `"rule uid: CODE, CODE: detail"`. Policy sends such an order to the inbox whatever its tier.
pub fn held(results: &[CheckResult]) -> Vec<String> {
    results
        .iter()
        .filter(|r| r.outcome == Outcome::Hold)
        .map(|r| {
            let who = r.uid.as_ref().map(|u| format!(" {u}")).unwrap_or_default();
            format!("{}{who}: {}: {}", r.rule.as_str(), r.codes.join(", "), r.detail)
        })
        .collect()
}

pub(crate) fn per_vehicle(
    spec: &IntentSpec,
    world: &WorldSnapshot,
    rule: RuleId,
    f: impl Fn(&Uid, &VehicleFacts) -> CheckResult,
) -> Vec<CheckResult> {
    spec.uids
        .iter()
        .map(|uid| match world.vehicles.get(uid) {
            Some(v) => f(uid, v),
            None => CheckResult::new(
                rule,
                Some(uid),
                Outcome::Fail,
                "vehicle not in the world model",
            ),
        })
        .collect()
}

/// A fact that exists and is fresh enough, or why not.
pub(crate) fn fresh<'a, T>(
    fact: &'a Option<Fact<T>>,
    v: &VehicleFacts,
    what: &str,
    world: &WorldSnapshot,
    cfg: &RulesConfig,
) -> Result<&'a T, String> {
    let f = fact
        .as_ref()
        .ok_or_else(|| format!("cannot evaluate: no {what} reported"))?;
    let age_ms = world.t.ms_since(f.t);
    let (max_ms, scaled) = cfg.fact_age_limit_ms(v.report_interval_ms);
    if age_ms > max_ms {
        let why = match (scaled, v.report_interval_ms) {
            (true, Some(i)) => format!(": link reports every {} s", f64::from(i) / 1000.0),
            _ => String::new(),
        };
        return Err(format!(
            "cannot evaluate: {what} is {:.1} s old (limit {} s{why})",
            age_ms as f64 / 1000.0,
            max_ms as f64 / 1000.0
        ));
    }
    Ok(&f.value)
}

pub(crate) fn is_safety_command(kind: TaskKind) -> bool {
    matches!(
        kind,
        TaskKind::Hold | TaskKind::Rtl | TaskKind::ReturnToHub | TaskKind::LandNow | TaskKind::Abort
    )
}

/// The platform kind supports the task (the KINDS table), the platform is in the catalog,
/// and a track target exists. Rules 34: a transit is among a mover's tasks; the list a refusal
/// gives names it under rules 34 only, so an older log's details read as they were.
pub fn capability(spec: &IntentSpec, world: &WorldSnapshot, _cfg: &RulesConfig) -> Vec<CheckResult> {
    let r = RuleId::Capability;
    let kind = spec.kind();
    per_vehicle(spec, world, r, |uid, v| {
        let fail = |d: String| CheckResult::new(r, Some(uid), Outcome::Fail, d);
        let Some(platform) = &v.platform else {
            return fail("cannot evaluate: platform not in the catalog".into());
        };
        let Some(ap) = &v.autopilot else {
            return fail("cannot evaluate: autopilot type unknown (no heartbeat yet)".into());
        };
        let ap = ap.value;
        // rules 27: a ground vehicle or a sub lays mines (a lay order exists from rules 27 only)
        let lays = false && matches!(ap, AutopilotKind::Ugv);
        if !ap.supports(kind) && !lays {
            // rules 34: a transit exists from rules 34 only (refused before the checks under
            // older rules), so their details name none
            let list: Vec<_> = ap
                .tasks()
                .iter()
                .filter(|k| **k != TaskKind::Transit || true)
                .map(|k| k.as_str())
                .collect();
            let can = if list.is_empty() {
                "nothing".to_string()
            } else {
                list.join(", ")
            };
            return fail(format!(
                "{} ({}) can't {}; supports: {}",
                platform.id,
                ap.as_str(),
                kind.as_str(),
                can
            ));
        }
        CheckResult::new(
            r,
            Some(uid),
            Outcome::Pass,
            format!("{} supports {}", ap.as_str(), kind.as_str()),
        )
    })
}

/// Navigation is ready and the status is fresh. Hold and RTL are exempt: they are the
/// safety commands, and the autopilot applies its own checks to them. On dead reckoning (no
/// GNSS fix) a sub passes, a fixed-wing (plane, VTOL) is `Unverified` (a human judges it),
/// anything else fails.
pub fn readiness(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let r = RuleId::Readiness;
    let kind = spec.kind();
    per_vehicle(spec, world, r, |uid, v| {
        let res = |o, d: String| CheckResult::new(r, Some(uid), o, d);
        // rules 30: one an operator confirmed lost takes no order, a hold or rtl included (no
        // one can command it, and a lost vehicle is out of the keep-outs); a report from it
        // revokes the loss before this reads it
        if let Some(l) = world.nodes_lost.get(uid).filter(|l| l.holds()) {
            return res(
                Outcome::Fail,
                format!("{}: a vehicle confirmed lost takes no order (a report from it revokes the loss)", l.describe()),
            );
        }
        if is_safety_command(kind) || kind.is_ground_order() {
            return res(Outcome::Pass, format!("not required for {}", kind.as_str()));
        }
        match fresh(&v.nav_ready, v, "navigation status", world, cfg) {
            Err(e) => return res(Outcome::Fail, e),
            Ok(false) => return res(Outcome::Fail, "navigation not ready (GPS/EKF)".into()),
            Ok(true) => {}
        }
        // dead reckoning (no GNSS) is how a submerged AUV navigates. A fixed-wing flies on
        // on airspeed dead reckoning (ArduPlane has no EKF failsafe for it), so it stays
        // taskable, out of a jammer for one, but a human judges a position that drifts.
        // Anyone else needs a fix (their EKF gives up the position, as ArduPilot's does).
        let dead_reckoning =
            v.nav_source.as_ref().map(|f| f.value) == Some(NavSource::DeadReckoning);
        let ap = v.autopilot.as_ref().map(|f| f.value);
        let is_sub = false;
        let fixed_wing = ap.is_some_and(AutopilotKind::is_fixed_wing);
        if dead_reckoning && !is_sub && !fixed_wing {
            return res(
                Outcome::Fail,
                "navigation on dead reckoning only (no GNSS fix)".into(),
            );
        }
        if dead_reckoning && fixed_wing {
            return res(
                Outcome::Unverified,
                "navigation on dead reckoning (no GNSS fix): position error grows with time".into(),
            );
        }
        if dead_reckoning {
            return res(
                Outcome::Pass,
                "navigation ready (dead reckoning, no GNSS fix: position error grows with distance)"
                    .into(),
            );
        }
        res(Outcome::Pass, "navigation ready".into())
    })
}

/// Orbit radius against the platform's minimum turn radius. Returns a suggested radius
/// instead of widening silently (the Python bridge widened silently).
pub fn platform_limits(
    spec: &IntentSpec,
    world: &WorldSnapshot,
    cfg: &RulesConfig,
) -> Vec<CheckResult> {
    let r = RuleId::PlatformLimits;
    per_vehicle(spec, world, r, |uid, v| {
        let res = |o, d: String| CheckResult::new(r, Some(uid), o, d);
        let Task::Orbit { radius_m, .. } = &spec.task else {
            return res(
                Outcome::Pass,
                format!("no platform limit applies to {}", spec.kind().as_str()),
            );
        };
        let (Some(platform), Some(ap)) = (&v.platform, &v.autopilot) else {
            return res(
                Outcome::Fail,
                "cannot evaluate: platform or autopilot unknown".into(),
            );
        };
        let (min_r, why) = match platform.min_turn_radius_m {
            Some(m) => (m, "minimum turn radius"),
            None if ap.value.is_fixed_wing() => (
                cfg.fixed_wing_default_min_turn_radius_m,
                "default fixed-wing loiter radius",
            ),
            None => {
                return res(
                    Outcome::Pass,
                    format!("{} has no minimum turn radius", ap.value.as_str()),
                );
            }
        };
        let need = (min_r * cfg.orbit_radius_margin).ceil();
        if *radius_m >= need {
            res(
                Outcome::Pass,
                format!(
                    "radius {radius_m:.0} m >= {need:.0} m ({why} {min_r:.0} m x {})",
                    cfg.orbit_radius_margin
                ),
            )
        } else {
            let mut c = res(
                Outcome::Fail,
                format!(
                    "radius {radius_m:.0} m is below {need:.0} m ({why} {min_r:.0} m x {})",
                    cfg.orbit_radius_margin
                ),
            );
            c.fix = Some(Fix::WidenRadius { radius_m: need });
            c
        }
    })
}

/// Which surface a platform's waypoints must be on, if any.
pub fn required_surface(domain: Domain, _autopilot: Option<AutopilotKind>) -> Option<&'static str> {
    match domain {
        Domain::Surface => Some("water"),
        Domain::Ground => Some("land"),
        _ => None,
    }
}

/// Whether the engine needs to ask the terrain oracle for this intent.
pub fn needs_terrain(spec: &IntentSpec, world: &WorldSnapshot) -> bool {
    spec.task.path().is_some()
        && spec.uids.iter().any(|u| {
            world
                .vehicles
                .get(u)
                .and_then(|v| v.platform.as_ref())
                .is_some_and(|p| required_surface(p.domain, p.autopilot).is_some())
        })
}

/// Boats and subs keep to water, UGVs to land: up to rules 16 their waypoints; from rules 17
/// the waypoints and every leg of the route ([`domain_route`]).
pub fn domain(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    domain_route(spec, world, cfg)
}

/// Rules 17: the waypoints as before (each answered exactly), then the points along every
/// straight leg of each vehicle's route ([`crate::route::Route::legs`], [`crate::route::along`]),
/// in travel order: the transit from its position, the task path's legs, the return home (an
/// the wrong surface refuses the order, naming the leg, the point and how far along it is; a leg
/// that starts at the vehicle may leave the wrong surface within [`crate::route::LEG_ENDS_M`],
/// and one that ends at home may reach it there (a pier, a slipway, a grid edge at the berth).
/// A hold or an rtl, the safety commands, are never refused for their way: an rtl whose way home
/// crosses the wrong surface passes with a warning (the bridge alerts on it while it runs), and
/// one whose way can't be checked is not evaluated.
fn domain_route(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    use crate::route::{LEG_ENDS_M, LEG_STEP_M, along};
    let r = RuleId::Domain;
    let kind = spec.kind();
    let safety = is_safety_command(kind);
    per_vehicle(spec, world, r, |uid, v| {
        let res = |o, d: String| CheckResult::new(r, Some(uid), o, d);
        let not_checked = |why: String| {
            if safety {
                res(
                    Outcome::NotEvaluated,
                    format!("path not checked ({} is a safety command): {why}", kind.as_str()),
                )
            } else {
                res(Outcome::Fail, why)
            }
        };
        let Some(platform) = &v.platform else {
            return res(
                Outcome::Fail,
                "cannot evaluate: platform not in the catalog".into(),
            );
        };
        let Some(need) = required_surface(platform.domain, platform.autopilot) else {
            return res(
                Outcome::Pass,
                "no surface constraint for this domain".into(),
            );
        };
        let route = match route_for(spec, v, world, cfg) {
            Ok(route) => route,
            Err(e) => return not_checked(e),
        };
        if route.is_empty() {
            return res(Outcome::Pass, format!("no path for {}", kind.as_str()));
        }
        match &world.terrain {
            TerrainInputs::Unavailable { reason } => {
                return res(
                    Outcome::NotEvaluated,
                    format!("terrain unavailable: {reason}"),
                );
            }
            TerrainInputs::NotQueried | TerrainInputs::NotNeeded => {
                return not_checked("cannot evaluate: terrain was not sampled".into());
            }
            TerrainInputs::Sampled { .. } => {}
        }
        let is = if need == "water" { "not water" } else { "not land" };
        // the waypoints, each answered exactly, as up to rules 16
        let waypoints = route.task.as_ref().map_or(0, |p| p.points.len());
        if let Some(path) = &route.task {
            for (i, p) in path.points.iter().enumerate() {
                let at = format!("waypoint {} ({:.5}, {:.5})", i + 1, p.lon, p.lat);
                let ok = match world.terrain.lookup(*p) {
                    None => {
                        return res(
                            Outcome::Fail,
                            format!("cannot evaluate: no terrain sample for {at}"),
                        );
                    }
                    Some(Surface::OutsideCoverage) => {
                        return res(
                            Outcome::Fail,
                            format!("cannot evaluate: {at} is outside terrain coverage"),
                        );
                    }
                    Some(Surface::Land) => need == "land",
                    Some(Surface::Water { depth_m }) => need == "water" && *depth_m > 0.0,
                };
                if !ok {
                    return res(
                        Outcome::Fail,
                        format!("{at} is {is}; {} needs {need}", platform.id),
                    );
                }
            }
        }
        // the legs, in travel order
        let good = if need == "water" { 'w' } else { 'l' };
        let mut points = 0usize;
        let mut ends: Vec<String> = Vec::new();
        for leg in route.legs() {
            let Some(pts) = along(leg.from, leg.to) else {
                return not_checked(format!(
                    "cannot evaluate: the {} is {:.0} km, longer than the {:.0} km the rule samples",
                    leg.name,
                    haversine_m(leg.from, leg.to) / 1000.0,
                    crate::route::LEG_MAX_M / 1000.0
                ));
            };
            if pts.is_empty() {
                continue;
            }
            let Some(answers) = world.terrain.along_for(leg.from, leg.to) else {
                return not_checked(format!("cannot evaluate: no terrain answers along the {}", leg.name));
            };
            let letters: Vec<char> = answers.surfaces.chars().collect();
            if letters.len() != pts.len() {
                return not_checked(format!(
                    "cannot evaluate: {} terrain answers along the {}, which has {} points",
                    letters.len(),
                    leg.name,
                    pts.len()
                ));
            }
            points += pts.len();
            let len = haversine_m(leg.from, leg.to);
            // a leg from the vehicle may leave the wrong surface it starts on, and one to home
            // may reach it there, each within LEG_ENDS_M and only before (after) the right one
            let first_good = letters.iter().position(|c| *c == good).unwrap_or(letters.len());
            let last_good = letters.iter().rposition(|c| *c == good);
            for (i, ((p, d), c)) in pts.iter().zip(&letters).enumerate() {
                if *c == good {
                    continue;
                }
                if leg.departs && i < first_good && *d <= LEG_ENDS_M {
                    let e = format!("the {} starts off {need} within {LEG_ENDS_M:.0} m of the vehicle", leg.name);
                    if !ends.contains(&e) {
                        ends.push(e);
                    }
                    continue;
                }
                if leg.arrives_home && last_good.is_none_or(|g| i > g) && len - d <= LEG_ENDS_M {
                    let e = format!("the {} ends off {need} within {LEG_ENDS_M:.0} m of home", leg.name);
                    if !ends.contains(&e) {
                        ends.push(e);
                    }
                    continue;
                }
                let at = format!("({:.5}, {:.5}), {d:.0} m along it", p.lon, p.lat);
                if *c == 'o' {
                    return not_checked(format!("cannot evaluate: the {} leaves terrain coverage at {at}", leg.name));
                }
                let crosses = match (need, *c) {
                    ("water", 'd') => "water no deeper than 0 m",
                    ("water", _) => "land",
                    _ => "water",
                };
                let detail = format!("{} crosses {crosses} at {at}; {} needs {need}{}", leg.name, platform.id, plan_note(world, uid, false, false));
                if safety {
                    // the safety command goes as ordered, and a person is told
                    let mut c = res(
                        Outcome::Pass,
                        format!("{detail}: the {} goes as ordered (a safety command)", kind.as_str()),
                    );
                    c.warning = Some(format!(
                        "{} path crosses {crosses} at ({:.5}, {:.5}), {d:.0} m along the {}: send a safer order",
                        kind.as_str(),
                        p.lon,
                        p.lat,
                        leg.name
                    ));
                    return c;
                }
                return res(Outcome::Fail, detail);
            }
        }
        let parts = route.parts();
        let mut detail = if waypoints > 0 {
            format!("all {waypoints} waypoints on {need}, and the {parts} ({points} points every {LEG_STEP_M:.0} m)")
        } else {
            format!("the {parts} on {need} ({points} points every {LEG_STEP_M:.0} m)")
        };
        for e in ends {
            detail.push_str("; ");
            detail.push_str(&e);
        }
        detail.push_str(&plan_note(world, uid, true, false));
        res(Outcome::Pass, detail)
    })
}

/// Rules 22: what the plans for vehicle `uid` did, for a check's detail: `routed` the legs
/// routed around what they met (a passing check), else the ones no route exists for (a failing
/// one), each `"; <summary>"`; `no_go` the ones round a no-go area (the geofence's), else round
/// the wrong surface (the domain rule's); empty when none.
fn plan_note(world: &WorldSnapshot, uid: &Uid, routed: bool, no_go: bool) -> String {
    world
        .route_plans
        .get(uid)
        .into_iter()
        .flatten()
        .filter(|p| p.no_route.is_none() == routed && p.met.starts_with("no-go") == no_go)
        .map(|p| format!("; {}", p.summary()))
        .collect()
}

/// No part of a vehicle's route enters a no-go area: the transit leg from its position to
/// the task, the task path, and the return leg home (see [`crate::route`]). A leg that
/// starts at the vehicle's position inside a no-go area is leaving it, which is allowed
/// (otherwise a vehicle that strayed in could never be brought out). For rtl, a return leg
/// that can't be checked is `NotEvaluated` rather than a failure: rtl is the safety command.
pub fn geofence(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let r = RuleId::Geofence;
    let kind = spec.kind();
    per_vehicle(spec, world, r, |uid, v| {
        let res = |o, d: String| CheckResult::new(r, Some(uid), o, d);
        let not_checked = |why: String| {
            if kind == TaskKind::Rtl {
                res(
                    Outcome::NotEvaluated,
                    format!("return leg not checked (rtl is a safety command): {why}"),
                )
            } else {
                res(Outcome::Fail, why)
            }
        };
        let route = match route_for(spec, v, world, cfg) {
            Ok(route) => route,
            Err(e) => return not_checked(e),
        };
        if route.is_empty() {
            return res(Outcome::Pass, format!("no path for {}", kind.as_str()));
        }
        let Some(areas) = &world.areas else {
            return not_checked("cannot evaluate: no area set loaded".into());
        };
        let no_go: Vec<&Area> = areas.no_go().collect();
        // rules 22: through the waypoints a plan routes its legs through
        if let Some(path) = &route.task_routed() {
            for area in &no_go {
                if let Some(why) = area.intrusion(path) {
                    return res(
                        Outcome::Fail,
                        format!("task path: {why} (no-go: {}){}", area.name, plan_note(world, uid, false, true)),
                    );
                }
            }
        }
        let mut leaving: Vec<&str> = Vec::new();
        for seg in &route.segments() {
            for area in &no_go {
                if let Some(why) = segment_enters(area, seg) {
                    return res(Outcome::Fail, format!("{why} (no-go: {}){}", area.name, plan_note(world, uid, false, true)));
                }
                if seg.from_vehicle
                    && area.contains(seg.from)
                    && !leaving.contains(&area.id.as_str())
                {
                    leaving.push(&area.id);
                }
            }
        }
        let mut detail = format!("clear of {} no-go areas ({}){}", no_go.len(), route.parts(), plan_note(world, uid, true, true));
        if !leaving.is_empty() {
            detail.push_str(&format!(
                "; starts inside {} and leaves it",
                leaving.join(", ")
            ));
        }
        res(Outcome::Pass, detail)
    })
}

/// Why a single leg enters a no-go area, naming the leg, or None.
fn segment_enters(area: &Area, seg: &Segment) -> Option<String> {
    let name = seg.name();
    if area.contains(seg.to) {
        return Some(format!("{name}: {} is inside {}", seg.to_label, area.id));
    }
    if area.contains(seg.from) {
        if seg.from_vehicle {
            // Already inside: the leg leaves it.
            return None;
        }
        return Some(format!("{name}: {} is inside {}", seg.from_label, area.id));
    }
    if area.segment_touches_boundary(seg.from, seg.to) {
        return Some(format!("{name} crosses {}", area.id));
    }
    None
}

/// Endurance left after the task stays above the reserve. Estimate: the transit leg to
/// waypoint 1, time on task (at least one lap, at least the configured horizon), the return
/// leg home (the same legs the geofence checks), all at cruise speed, against battery x
/// full endurance. Hold and rtl are exempt. With no endurance figure the result depends on
/// `unknown_endurance` (default: `Unverified`, routed to a human).
pub fn reserves(spec: &IntentSpec, world: &WorldSnapshot, cfg: &RulesConfig) -> Vec<CheckResult> {
    let r = RuleId::Reserves;
    per_vehicle(spec, world, r, |uid, v| {
        let res = |o, d: String| CheckResult::new(r, Some(uid), o, d);
        let fail = |d: String| res(Outcome::Fail, d);
        let Some(path) = spec.task.path().filter(|_| !is_safety_command(spec.kind())) else {
            return res(
                Outcome::Pass,
                format!("not required for {}", spec.kind().as_str()),
            );
        };
        let Some(platform) = &v.platform else {
            return fail("cannot evaluate: platform not in the catalog".into());
        };
        let reserve_pct = spec
            .constraints
            .reserve_pct
            .map_or(cfg.reserve_pct, |c| c.max(cfg.reserve_pct));
        let battery = match fresh(&v.battery_pct, v, "battery level", world, cfg) {
            Ok(b) => *b,
            Err(e) => return fail(e),
        };
        let Some(endurance_h) = platform.endurance_h.filter(|e| *e > 0.0) else {
            return match cfg.unknown_endurance {
                UnknownEndurance::Human if battery >= reserve_pct => res(
                    Outcome::Unverified,
                    format!(
                        "not evaluated: endurance unknown for {}; battery {battery:.0}% >= reserve {reserve_pct:.0}% now, task cost not estimated; needs a human",
                        platform.id
                    ),
                ),
                UnknownEndurance::Fail => fail(format!(
                    "cannot evaluate: no endurance figure for {}",
                    platform.id
                )),
                UnknownEndurance::BatteryNow if battery >= reserve_pct => res(
                    Outcome::Pass,
                    format!(
                        "battery {battery:.0}% >= reserve {reserve_pct:.0}% now (no endurance figure, task cost not estimated)"
                    ),
                ),
                UnknownEndurance::Human | UnknownEndurance::BatteryNow => fail(format!(
                    "battery {battery:.0}% below reserve {reserve_pct:.0}%"
                )),
            };
        };
        let Some(cruise) = platform.cruise_mps.filter(|c| *c > 0.0) else {
            return fail(format!(
                "cannot evaluate: no cruise speed for {}",
                platform.id
            ));
        };
        let route = match route_for(spec, v, world, cfg) {
            Ok(route) => route,
            Err(e) => return fail(e),
        };
        let leg_m = |s: &Option<Segment>| s.as_ref().map_or(0.0, |s| haversine_m(s.from, s.to));
        let lap_m: f64 = path
            .legs()
            .iter()
            .map(|(_, a, b)| haversine_m(*a, *b))
            .sum();
        let on_task_s = (lap_m / cruise)
            .max(f64::from(cfg.task_duration_s))
            .max(f64::from(spec.constraints.duration_s.unwrap_or(0)));
        // rules 22: the transit through the waypoints a plan routes it through
        let used_s = route.transit_m() / cruise + on_task_s + leg_m(&route.ret) / cruise;
        let full_s = endurance_h * 3600.0;
        let left_s = battery / 100.0 * full_s - used_s;
        let reserve_s = reserve_pct / 100.0 * full_s;
        let detail = format!(
            "{:.0} min left after task (task {:.0} min, battery {battery:.0}% of {:.0} min), reserve {:.0} min ({reserve_pct:.0}%)",
            left_s / 60.0,
            used_s / 60.0,
            full_s / 60.0,
            reserve_s / 60.0
        );
        if left_s >= reserve_s {
            res(Outcome::Pass, detail)
        } else {
            fail(detail)
        }
    })
}

// ---- space kinds (collect, cover) -------------------------------------------------------

