//! Intents (Phase C.1): every action, from a human or an agent, as a record with a
//! decision trail.
//!
//! An [`IntentSpec`] is the immutable request (what, to whom, why, from whom). An
//! [`Intent`] adds where it stands: state, check results, policy decision, approval. Its
//! fields are private and change only through the [`crate::Engine`], which logs every change
//! before making it. The legal state changes are the table in [`next_state`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::geo::{LonLat, TaskPath, circle_points};
use crate::queue::{Basis, QueueState, ResumedFrom};
use crate::ids::{AgentId, IntentId, OperatorId, SessionId, Uid};
use crate::policy::PolicyDecision;
use crate::rules::CheckResult;
use crate::time::Time;

/// Waypoints in an orbit ring for non-fixed-wing vehicles (as the Python bridge flies it).
pub const ORBIT_RING_POINTS: usize = 12;

/// Rules 34: the most points a transit's route names (a mission upload carries each one).
pub const MAX_TRANSIT_POINTS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Orbit,
    Patrol,
    Hold,
    Rtl,
    /// Rules 34 ([`crate::rules::GROUP_ORDER_SINCE`]): go along a route and hold at its end by
    /// the vehicle's type (a copter hovers, a fixed wing loiters, a boat or a sub holds station, a
    /// ground vehicle stops); for a group, stations along the route or a block round its end
    /// ([`crate::group::Spread::Stations`], [`crate::group::Spread::Block`]).
    Transit,
    /// Delivery order (docs/design/delivery-orders.md 5.1): fly, drive or sail to a site and load
    /// a parcel there.
    Pickup,
    /// Delivery order 5.2: go to a drop zone and release the parcel.
    Deliver,
    /// Delivery order 5.4: replace the remaining route of a running order.
    Reroute,
    /// Delivery order 5.5: go to a hub and land or dock.
    ReturnToHub,
    /// Delivery order 5.6: bring the vehicle to the ground (or a stop) as soon as it is safe.
    LandNow,
    /// Delivery order 5.7: stop an order in progress and do something safe instead.
    Abort,
    /// Delivery order 5.8: replace a landed vehicle's battery at a hub.
    SwapBattery,
    /// Delivery order 5.9: a person says the drop zone is fit for this delivery now. Always a
    /// person's: no policy tier lets it run without one ([`TaskKind::is_human_only`]).
    ConfirmDropZone,
}

impl TaskKind {
    /// Every kind, in a fixed order.
    pub const ALL: [TaskKind; 13] = [
        TaskKind::Orbit,
        TaskKind::Patrol,
        TaskKind::Hold,
        TaskKind::Rtl,
        TaskKind::Transit,
        TaskKind::Pickup,
        TaskKind::Deliver,
        TaskKind::Reroute,
        TaskKind::ReturnToHub,
        TaskKind::LandNow,
        TaskKind::Abort,
        TaskKind::SwapBattery,
        TaskKind::ConfirmDropZone,
    ];

    /// The kind named `name` (its snake_case name), if there is one.
    pub fn from_name(name: &str) -> Option<TaskKind> {
        TaskKind::ALL.into_iter().find(|k| k.as_str() == name)
    }

    /// The nine delivery kinds ([`TaskKind::is_delivery`]), in the order the design lists them.
    pub const DELIVERY: [TaskKind; 9] = [
        TaskKind::Pickup,
        TaskKind::Deliver,
        TaskKind::Hold,
        TaskKind::Reroute,
        TaskKind::ReturnToHub,
        TaskKind::LandNow,
        TaskKind::Abort,
        TaskKind::SwapBattery,
        TaskKind::ConfirmDropZone,
    ];

    /// One of the nine delivery orders.
    pub fn is_delivery(self) -> bool {
        Self::DELIVERY.contains(&self)
    }

    /// A kind no policy tier may route to `auto` (the policy loader refuses a file that does, and
    /// [`crate::policy::decide`] holds the floor whatever the file says).
    pub fn is_human_only(self) -> bool {
        self == TaskKind::ConfirmDropZone
    }

    /// A delivery kind that moves no vehicle and is checked by the order rules alone
    /// (`swap_battery`: the vehicle is on a pad; `confirm_drop_zone`: not a vehicle order).
    pub fn is_ground_order(self) -> bool {
        matches!(self, TaskKind::SwapBattery | TaskKind::ConfirmDropZone)
    }

    /// The safety orders: `hold`, `return_to_hub`, `land_now` and `abort` skip the energy, payload
    /// and weather rules (delivery-rules.md, principle 5).
    pub fn is_safety_order(self) -> bool {
        matches!(
            self,
            TaskKind::Hold | TaskKind::Rtl | TaskKind::ReturnToHub | TaskKind::LandNow | TaskKind::Abort
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Orbit => "orbit",
            TaskKind::Patrol => "patrol",
            TaskKind::Hold => "hold",
            TaskKind::Rtl => "rtl",
            TaskKind::Transit => "transit",
            TaskKind::Pickup => "pickup",
            TaskKind::Deliver => "deliver",
            TaskKind::Reroute => "reroute",
            TaskKind::ReturnToHub => "return_to_hub",
            TaskKind::LandNow => "land_now",
            TaskKind::Abort => "abort",
            TaskKind::SwapBattery => "swap_battery",
            TaskKind::ConfirmDropZone => "confirm_drop_zone",
        }
    }

    /// Kinds for which every rule must actually evaluate (a "not evaluated" or "unverified"
    /// outcome blocks: fail closed, delivery-rules.md section 1). `deliver` is the consequential
    /// order; `abort` over people and `land_now` outside a hub are refused by their own rules
    /// when a rule could not be evaluated.
    pub fn is_consequential(self) -> bool {
        matches!(self, TaskKind::Deliver)
    }

    /// The kinds whose approval chain needs the second signer with extra authority. None yet.
    pub fn needs_consequential_authority(self) -> bool {
        false
    }
}

/// The task and its parameters (today's task params). The kind is the variant, so a kind
/// with the wrong params can't be built. Serialized as `{"kind": ..., "params": {...}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum Task {
    Orbit {
        center: LonLat,
        #[serde(alias = "radius")]
        radius_m: f64,
    },
    /// Flown as a loop back to the first point.
    Patrol {
        route: Vec<LonLat>,
    },
    /// Stay where you are; after `max_s` (when given) the vehicle's own lost-link style return
    /// applies.
    Hold {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_s: Option<u32>,
    },
    Rtl {},
    /// Rules 34: flown along `route` once, its first point to its last, and held at the last by
    /// the vehicle's type (never looped back). One point: go there and hold.
    Transit {
        route: Vec<LonLat>,
    },
    /// Go to `site` (a hub, or another registered pickup point) and load `parcel`.
    Pickup {
        parcel: String,
        site: Place,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        load_by_s: Option<u32>,
    },
    /// Go to drop zone `drop_zone` (at `at`, as the world's places give it) along `route` (the
    /// engine plans one when it is empty) and release `parcel` the way `release` says.
    Deliver {
        parcel: String,
        drop_zone: String,
        at: LonLat,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        route: Vec<LonLat>,
        release: Release,
        /// RFC 3339: the latest time the parcel may still be delivered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_end: Option<String>,
    },
    /// Replace the remaining route of the running order on the vehicle with `route`.
    Reroute {
        route: Vec<LonLat>,
        cause: RerouteCause,
    },
    /// Go to `hub` (default: the nearest by energy cost with a free pad) and land or dock.
    ReturnToHub {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub: Option<Place>,
    },
    /// Land (or stop) as soon as it is safe, at `site` or where the vehicle is.
    LandNow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        site: Option<LonLat>,
    },
    /// Stop intent `target` and do `then` instead.
    Abort {
        target: IntentId,
        #[serde(default)]
        then: AbortThen,
    },
    /// Swap the battery of a vehicle landed on a pad of `hub`.
    SwapBattery {
        hub: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pad: Option<String>,
    },
    /// A person's confirmation that `drop_zone` is fit for `parcel` now, on `evidence` (ids of
    /// the observations that bear on it). The approval is the decision.
    ConfirmDropZone {
        drop_zone: String,
        parcel: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<String>,
    },
}

/// A named place (a hub, by its id) with where it is, or a bare point: what `pickup` and
/// `return_to_hub` name. The bridge fills `at` from the hub registry when a proposal names only
/// an id; the order rules check that an id and its point agree with the world's places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub at: LonLat,
}

/// How a parcel leaves the vehicle at the drop zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Release {
    Land,
    Winch,
    Dock,
    Slip,
}

/// Why a route is replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RerouteCause {
    KeepOut,
    Weather,
    Conflict,
    Battery,
    Operator,
}

/// What an `abort` does after it stops its target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortThen {
    #[default]
    ReturnToHub,
    Hold,
    LandNow,
}

/// A parcel or drop zone id: short, printable, no spaces.
fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

impl Task {
    /// A hold with no time limit.
    pub fn hold() -> Task {
        Task::Hold { max_s: None }
    }

    pub fn kind(&self) -> TaskKind {
        match self {
            Task::Orbit { .. } => TaskKind::Orbit,
            Task::Patrol { .. } => TaskKind::Patrol,
            Task::Hold { .. } => TaskKind::Hold,
            Task::Rtl {} => TaskKind::Rtl,
            Task::Transit { .. } => TaskKind::Transit,
            Task::Pickup { .. } => TaskKind::Pickup,
            Task::Deliver { .. } => TaskKind::Deliver,
            Task::Reroute { .. } => TaskKind::Reroute,
            Task::ReturnToHub { .. } => TaskKind::ReturnToHub,
            Task::LandNow { .. } => TaskKind::LandNow,
            Task::Abort { .. } => TaskKind::Abort,
            Task::SwapBattery { .. } => TaskKind::SwapBattery,
            Task::ConfirmDropZone { .. } => TaskKind::ConfirmDropZone,
        }
    }


    /// The waypoints the task commits a vehicle to; None for tasks flown in place.
    /// The transit leg to them and the return leg home are added by [`crate::route`].
    pub fn path(&self) -> Option<TaskPath> {
        match self {
            Task::Orbit { center, radius_m } => Some(TaskPath {
                points: circle_points(*center, *radius_m, ORBIT_RING_POINTS),
                closed: true,
            }),
            Task::Patrol { route } => Some(TaskPath {
                points: route.clone(),
                closed: true,
            }),
            // rules 34: the route, flown once, held at its end
            Task::Transit { route } => Some(TaskPath {
                points: route.clone(),
                closed: false,
            }),
            // delivery: to the site, then (a deliver) along the route to the drop zone
            Task::Pickup { site, .. } => Some(TaskPath { points: vec![site.at], closed: false }),
            Task::Deliver { route, at, .. } => {
                let mut points = route.clone();
                points.push(*at);
                Some(TaskPath { points, closed: false })
            }
            Task::Reroute { route, .. } => Some(TaskPath { points: route.clone(), closed: false }),
            Task::ReturnToHub { hub: Some(h) } => Some(TaskPath { points: vec![h.at], closed: false }),
            Task::LandNow { site: Some(p) } => Some(TaskPath { points: vec![*p], closed: false }),
            Task::Hold { .. }
            | Task::Rtl {}
            | Task::ReturnToHub { hub: None }
            | Task::LandNow { site: None }
            | Task::Abort { .. }
            | Task::SwapBattery { .. }
            | Task::ConfirmDropZone { .. } => None,
        }
    }

    fn validate(&self) -> Result<(), ProposalError> {
        let bad = |m: &str| Err(ProposalError::BadParams(m.to_string()));
        match self {
            Task::Orbit { center, radius_m } => {
                if !center.is_valid() {
                    return bad("orbit center out of range");
                }
                if !(radius_m.is_finite() && *radius_m > 0.0) {
                    return bad("orbit radius must be a positive number of metres");
                }
            }
            Task::Patrol { route } => {
                if route.len() < 2 {
                    return bad("patrol needs a route of at least 2 points");
                }
                if !route.iter().all(LonLat::is_valid) {
                    return bad("patrol point out of range");
                }
            }
            Task::Transit { route } => {
                if route.is_empty() || route.len() > MAX_TRANSIT_POINTS {
                    return bad("transit needs a route of 1 to 50 points [lon, lat], held at its last");
                }
                if !route.iter().all(LonLat::is_valid) {
                    return bad("transit point out of range");
                }
            }
            Task::Hold { max_s } => {
                if *max_s == Some(0) {
                    return bad("hold max_s must be at least 1 s (omit it to hold until superseded)");
                }
            }
            Task::Rtl {} => {}
            Task::Pickup { parcel, site, load_by_s } => {
                if !valid_id(parcel) {
                    return bad("pickup needs a parcel id (letters, digits, - _ . :)");
                }
                if !site.at.is_valid() || site.id.as_deref().is_some_and(|i| !valid_id(i)) {
                    return bad("pickup site out of range or its id malformed");
                }
                if *load_by_s == Some(0) {
                    return bad("pickup load_by_s must be at least 1 s");
                }
            }
            Task::Deliver { parcel, drop_zone, at, route, window_end, .. } => {
                if !valid_id(parcel) || !valid_id(drop_zone) {
                    return bad("deliver needs a parcel id and a drop zone id (letters, digits, - _ . :)");
                }
                if !at.is_valid() || !route.iter().all(LonLat::is_valid) {
                    return bad("deliver point out of range");
                }
                if route.len() >= MAX_TRANSIT_POINTS {
                    return bad("deliver route has at most 49 points before the drop zone");
                }
                if let Some(w) = window_end
                    && chrono::DateTime::parse_from_rfc3339(w).is_err()
                {
                    return bad("deliver window_end must be an RFC 3339 time");
                }
            }
            Task::Reroute { route, .. } => {
                if route.is_empty() || route.len() > MAX_TRANSIT_POINTS || !route.iter().all(LonLat::is_valid) {
                    return bad("reroute needs a route of 1 to 50 points [lon, lat]");
                }
            }
            Task::ReturnToHub { hub } => {
                if let Some(h) = hub
                    && (!h.at.is_valid() || h.id.as_deref().is_some_and(|i| !valid_id(i)))
                {
                    return bad("return_to_hub hub out of range or its id malformed");
                }
            }
            Task::LandNow { site } => {
                if site.is_some_and(|p| !p.is_valid()) {
                    return bad("land_now site out of range");
                }
            }
            Task::Abort { .. } => {}
            Task::SwapBattery { hub, pad } => {
                if !valid_id(hub) || pad.as_deref().is_some_and(|p| !valid_id(p)) {
                    return bad("swap_battery needs a hub id (and optionally a pad id)");
                }
            }
            Task::ConfirmDropZone { drop_zone, parcel, evidence } => {
                if !valid_id(drop_zone) || !valid_id(parcel) {
                    return bad("confirm_drop_zone needs a drop zone id and a parcel id");
                }
                if evidence.len() > 20 || evidence.iter().any(|e| e.trim().is_empty() || e.len() > 128) {
                    return bad("confirm_drop_zone evidence: at most 20 observation ids");
                }
            }
        }
        Ok(())
    }
}

/// Operator-set limits. They can only tighten what the policy file sets, never loosen it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    /// Endurance reserve after the task, percent of full endurance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_pct: Option<f64>,
    /// Time on task for the reserves estimate, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<u32>,
}

/// Why an intent is wanted. Required for agents (the type has no empty value), optional for
/// humans.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Rationale(String);

impl Rationale {
    pub fn new(text: impl Into<String>) -> Result<Self, ProposalError> {
        let s: String = text.into();
        if s.trim().is_empty() {
            Err(ProposalError::EmptyRationale)
        } else {
            Ok(Rationale(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Rationale {
    type Error = ProposalError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Rationale::new(s)
    }
}

impl From<Rationale> for String {
    fn from(r: Rationale) -> String {
        r.0
    }
}

/// Who asked. Set by the engine from the path the request came in on (an operator's
/// authority or an agent port), never from the request body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Human { id: OperatorId },
    Agent { id: AgentId },
    /// Rules 29: the previous watch, through the fleet file that handed the order over at the
    /// start (`intent.handed_over`; `id` names that file): no operator or agent of this run.
    Handover { id: String },
}

impl Source {
    pub fn is_agent(&self) -> bool {
        matches!(self, Source::Agent { .. })
    }
}

/// A request to act, before the engine gives it an id. Deliberately has no `source` field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    #[serde(flatten)]
    pub task: Task,
    /// The vehicles, for vehicle kinds. Space kinds (collect, cover) take none; the field
    /// may then be omitted.
    #[serde(default)]
    pub uids: Vec<Uid>,
    #[serde(default)]
    pub constraints: Constraints,
    #[serde(default)]
    pub rationale: Option<Rationale>,
    /// Rules 34 ([`crate::rules::GROUP_ORDER_SINCE`]): how a group order to these vehicles is laid
    /// out (its spread, minimum, layer base, wait, whether it re-forms). From rules 34 an order of
    /// an ordinary kind naming more than one vehicle is a group order whether or not it gives one
    /// (the engine fills the defaults); refused under older rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<crate::group::GroupSpec>,
}

impl Proposal {
    pub fn new(task: Task, uids: Vec<Uid>) -> Self {
        Proposal {
            task,
            uids,
            constraints: Constraints::default(),
            rationale: None,
            group: None,
        }
    }

    pub fn validate(&self) -> Result<(), ProposalError> {
        if self.uids.is_empty() {
            return Err(ProposalError::NoVehicles);
        }
        let mut seen = self.uids.clone();
        seen.sort();
        seen.dedup();
        if seen.len() != self.uids.len() {
            return Err(ProposalError::DuplicateVehicle);
        }
        if let Some(r) = self.constraints.reserve_pct
            && !(r.is_finite() && (0.0..=100.0).contains(&r))
        {
            return Err(ProposalError::BadParams(
                "reserve_pct must be 0..100".into(),
            ));
        }
        // rules 34: a group order's own spec
        if let Some(why) = self.group.as_ref().and_then(|g| g.invalid(self.task.kind(), self.uids.len())) {
            return Err(ProposalError::BadParams(why));
        }
        self.task.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProposalError {
    #[error("an intent needs at least one vehicle uid")]
    NoVehicles,
    #[error("a vehicle uid appears twice")]
    DuplicateVehicle,
    #[error("rationale must not be empty")]
    EmptyRationale,
    #[error("bad params: {0}")]
    BadParams(String),
}

/// The immutable request, as logged in `intent.created`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntentSpec {
    pub id: IntentId,
    pub created_t: Time,
    pub source: Source,
    #[serde(flatten)]
    pub task: Task,
    pub uids: Vec<Uid>,
    pub constraints: Constraints,
    pub rationale: Option<Rationale>,
    /// Rules 34: a group order's spec, or a member order's group and share
    /// ([`crate::group::Grouping`]); nothing at all on any other intent.
    #[serde(flatten)]
    pub grouping: crate::group::Grouping,
}

impl IntentSpec {
    pub fn kind(&self) -> TaskKind {
        self.task.kind()
    }

    /// Rules 34: a group order's spec, when it is one.
    pub fn group(&self) -> Option<&crate::group::GroupSpec> {
        self.grouping.group.as_ref()
    }

    /// Rules 34: the group order a member order is a share of, when it is one.
    pub fn parent(&self) -> Option<IntentId> {
        self.grouping.parent
    }

    /// Rules 34: a member order's share, when it is one.
    pub fn share(&self) -> Option<&crate::group::Share> {
        self.grouping.share.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Proposed,
    Checked,
    AutoApproved,
    PendingApproval,
    Approved,
    Rejected,
    Expired,
    Voided,
    Dispatched,
    Effected,
    /// Rules 5: approved (or auto-approved), but a vehicle it is for can't be reached now (a
    /// silent link, an AUV whose acoustic modem can't carry a mission upload): held, with
    /// an expiry, until it can. Dispatched only after its checks pass again on the world
    /// then; back to the inbox when a human approved a picture that has since changed.
    Queued,
    /// Stopped after dispatch (an operator's abort, a void when the picture changed, link loss);
    /// the platform was sent its recorded fallback.
    Aborted,
    Done,
    Failed,
}

impl State {
    pub const ALL: [State; 14] = [
        State::Proposed,
        State::Checked,
        State::AutoApproved,
        State::PendingApproval,
        State::Approved,
        State::Rejected,
        State::Expired,
        State::Voided,
        State::Dispatched,
        State::Effected,
        State::Queued,
        State::Aborted,
        State::Done,
        State::Failed,
    ];

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            State::Rejected
                | State::Expired
                | State::Voided
                | State::Aborted
                | State::Done
                | State::Failed
        )
    }
}

/// The events that move an intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    /// Rules evaluated.
    Check,
    /// A rule failed: rejected by the rules, before policy.
    ChecksFailed,
    /// Policy tier auto.
    AutoApprove,
    /// Policy tier human: into the inbox.
    RequireApproval,
    /// A human approved (needs a `HumanAuthority`).
    Approve,
    /// Policy tier human, and the human who submitted it is the authorizing human: the
    /// submission itself is the approval (needs the submitter's `HumanAuthority`; never
    /// for an agent's proposal).
    AuthorizeAtSubmission,
    /// A human rejected.
    Reject,
    /// Nobody decided in time.
    Expire,
    /// Withdrawn, cancelled, or invalidated by a changed world.
    Void,
    /// Handed to the task layer.
    Dispatch,
    Release,
    Abort,
    Complete,
    Fail,
    /// Rules 5: held for a vehicle that can't be reached now (`intent.queued`).
    Queue,
    /// Rules 5: dispatched, but the link to every vehicle was lost before any started it:
    /// back to the queue (`intent.requeued`), re-checked before it is sent again.
    Requeue,
    /// Rules 11: a dispatched collect or cover withdrawn by an operator (`operator.action
    /// intent.cancel`, then the engine's `op.state cancelled`): `aborted`.
    Cancel,
    /// Rules 34: a member order approved under its group order's approval (proposed to approved):
    /// the group's checks, policy decision and approval cover it, and it is dispatched or queued
    /// on its own from here.
    Under,
}

impl Transition {
    pub const ALL: [Transition; 18] = [
        Transition::Check,
        Transition::ChecksFailed,
        Transition::AutoApprove,
        Transition::RequireApproval,
        Transition::Approve,
        Transition::AuthorizeAtSubmission,
        Transition::Reject,
        Transition::Expire,
        Transition::Void,
        Transition::Dispatch,
        Transition::Release,
        Transition::Abort,
        Transition::Complete,
        Transition::Fail,
        Transition::Queue,
        Transition::Requeue,
        Transition::Cancel,
        Transition::Under,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal transition {transition:?} from state {from:?}")]
pub struct IllegalTransition {
    pub from: State,
    pub transition: Transition,
}

/// The state machine:
///
/// ```text
/// proposed -check-> checked -checks_failed-> rejected
///                           -auto_approve-> auto_approved
///                           -require_approval-> pending_approval
///                           -authorize_at_submission-> approved
/// pending_approval -approve-> approved | -reject-> rejected | -expire-> expired
/// pending_approval | auto_approved | approved -void-> voided
/// auto_approved | approved -dispatch-> dispatched -complete-> done | -fail-> failed
/// auto_approved | approved -queue-> queued                               (rules 5)
/// queued -dispatch-> dispatched | -require_approval-> pending_approval
///        | -void-> voided | -expire-> expired                           (rules 5)
/// dispatched -requeue-> queued                                           (rules 5)
/// proposed -under-> approved                                (rules 34, a member order)
/// ```
///
/// A queued intent leaves the queue only through a re-check against the world at that
/// moment (`queue.recheck`): dispatched when it passes (and, for a human's approval, the
/// picture it rested on still holds), back to the inbox when a human must look again, void
/// when a check fails; expired at its deadline (or, for a human's approval held past
/// `[queue] human_expire_s`, back to the inbox). A dispatched order whose link was lost to
/// every vehicle before any started it goes back to the queue (`requeue`).
///
/// Approved is reachable only through `Approve` and `AuthorizeAtSubmission`, which the
/// engine performs only with a `HumanAuthority` (the second only for the submitter's own
/// intent, on the human submit path).
pub fn next_state(from: State, transition: Transition) -> Result<State, IllegalTransition> {
    use State as S;
    use Transition as T;
    let to = match (from, transition) {
        (S::Proposed, T::Check) => S::Checked,
        (S::Proposed, T::Under) => S::Approved,
        (S::Checked, T::ChecksFailed) => S::Rejected,
        (S::Checked, T::AutoApprove) => S::AutoApproved,
        (S::Checked, T::RequireApproval) => S::PendingApproval,
        (S::Checked, T::AuthorizeAtSubmission) => S::Approved,
        (S::PendingApproval, T::Approve) => S::Approved,
        (S::PendingApproval, T::Reject) => S::Rejected,
        (S::PendingApproval, T::Expire) => S::Expired,
        (S::PendingApproval | S::AutoApproved | S::Approved | S::Queued, T::Void) => S::Voided,
        (S::AutoApproved | S::Approved | S::Queued, T::Dispatch) => S::Dispatched,
        (S::AutoApproved | S::Approved, T::Queue) => S::Queued,
        (S::Queued, T::RequireApproval) => S::PendingApproval,
        (S::Queued, T::Expire) => S::Expired,
        (S::Dispatched, T::Requeue) => S::Queued,
        (S::Dispatched, T::Complete) => S::Done,
        (S::Dispatched, T::Fail) => S::Failed,
        (S::Dispatched, T::Release) => S::Effected,
        (S::Dispatched, T::Abort) => S::Aborted,
        (S::Dispatched, T::Cancel) => S::Aborted,
        (S::Effected, T::Complete) => S::Done,
        (S::Effected, T::Fail) => S::Failed,
        _ => return Err(IllegalTransition { from, transition }),
    };
    Ok(to)
}

/// A recorded human approval. Serialize-only: it can't be fabricated from JSON.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Approval {
    pub by: OperatorId,
    pub session: SessionId,
    pub t: Time,
    /// The approver proposed the intent too (no second person looked at it).
    pub self_approved: bool,
    /// The approval is the submission itself: a human submitted the intent and its policy
    /// tier lets the submitter authorize (`approval.at_submission`). Always self-approved.
    pub at_submission: bool,
    pub note: Option<String>,
    /// Rules 8: the approver re-entered their credential to sign (a step-up).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub step_up: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StateChange {
    pub state: State,
    pub t: Time,
    pub reason: String,
}

/// An intent and its decision trail. Serialize-only, read-only outside the engine.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Intent {
    #[serde(flatten)]
    spec: IntentSpec,
    state: State,
    checks: Vec<CheckResult>,
    policy: Option<PolicyDecision>,
    approval: Option<Approval>,
    /// For pending approvals: when it expires.
    expires_t: Option<Time>,
    history: Vec<StateChange>,
    /// Signatures recorded before the last one, when the tier needs more than one approver
    /// (two-person approval): each a distinct operator, none the proposer where the tier
    /// excludes the proposer. The last signature is `approval`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    concurrences: Vec<Approval>,
    /// Rules 5: held for an unreachable vehicle (or once was): when, until when, why, and
    /// the last re-check's verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    queue: Option<Box<QueueState>>,
    /// What the approval rested on (checks, policy, who), for the dequeue re-check. Not
    /// serialized: the checks and policy are already on the intent.
    #[serde(skip)]
    basis: Option<Box<Basis>>,
    /// Rules 5: carried over from the previous run after a bridge restart.
    #[serde(skip_serializing_if = "Option::is_none")]
    resumed: Option<Box<Resumed>>,
    /// Rules 29: handed over at the start by the fleet file (the previous watch's order).
    #[serde(skip_serializing_if = "Option::is_none")]
    handed_over: Option<Box<crate::queue::HandedOverFrom>>,
    /// Rules 5: the vehicles that ever accepted and started it (an op `acked`, `active` or
    /// `done`, in this run or one before): never sent it again.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    committed: BTreeSet<Uid>,
    /// Rules 5: it was dispatched before (in this run or one before), so a vehicle may hold
    /// it: its next delivery asks the vehicle what it holds first.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dispatched_before: bool,
    /// Rules 5: its basis (a human's approval) comes from the run before a bridge restart.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    basis_carried: bool,
    /// Rules 5: the vehicles that may have started it (its start went out, an op `starting`,
    /// and nothing confirmed it): its next delivery there only follows it if the vehicle runs
    /// it. Cleared when it goes back to a human.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    maybe_started: BTreeSet<Uid>,
    /// The vehicles its current delivery only confirms (`maybe_started` when it went out):
    /// one that does not run it sends it to a human.
    #[serde(skip)]
    confirming: BTreeSet<Uid>,
    /// Rules 7: the vehicles whose start (or follow) on the current delivery was logged ahead
    /// of it.
    #[serde(skip)]
    started_ahead: BTreeSet<Uid>,
    /// The vehicles whose op on the current delivery reported its start going out
    /// (`starting`, `unconfirmed`, or started; after a restart, those that had started it):
    /// rules 7 refuses such an op ending `requeued` (it may be running).
    #[serde(skip)]
    start_went_out: BTreeSet<Uid>,
    /// Each vehicle's last op state in this run (op.state), and its detail.
    #[serde(skip)]
    ops: BTreeMap<Uid, String>,
    #[serde(skip)]
    op_details: BTreeMap<Uid, String>,
    /// Rules 22: the routed legs its last passing check planned (what it is dispatched with).
    #[serde(skip)]
    route_plans: BTreeMap<Uid, Vec<crate::plan::PlannedLeg>>,
    #[serde(skip)]
    identity_before: Option<String>,
    /// Rules 34, a group order: its last passing check (its layout, each member's class), what it
    /// is dispatched with.
    #[serde(skip)]
    group_eval: Option<Box<crate::group::GroupEval>>,
    /// Rules 34, a group order once dispatched: its member orders, by uid.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    members: BTreeMap<Uid, IntentId>,
}

/// An intent carried over from the previous run: where from, and each vehicle's last op
/// state there (a vehicle that had started it is followed, never sent it again).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Resumed {
    pub from: ResumedFrom,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub ops: BTreeMap<Uid, String>,
}

impl Intent {
    pub(crate) fn new(spec: IntentSpec) -> Self {
        let t = spec.created_t;
        Intent {
            spec,
            state: State::Proposed,
            checks: Vec::new(),
            policy: None,
            approval: None,
            expires_t: None,
            history: vec![StateChange {
                state: State::Proposed,
                t,
                reason: "created".into(),
            }],
            concurrences: Vec::new(),
            queue: None,
            basis: None,
            resumed: None,
            handed_over: None,
            committed: BTreeSet::new(),
            dispatched_before: false,
            basis_carried: false,
            maybe_started: BTreeSet::new(),
            confirming: BTreeSet::new(),
            started_ahead: BTreeSet::new(),
            start_went_out: BTreeSet::new(),
            ops: BTreeMap::new(),
            op_details: BTreeMap::new(),
            route_plans: BTreeMap::new(),
            identity_before: None,
            group_eval: None,
            members: BTreeMap::new(),
        }
    }

    pub fn id(&self) -> IntentId {
        self.spec.id
    }
    pub fn spec(&self) -> &IntentSpec {
        &self.spec
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn play_state(&self) -> State {
        self.state
    }
    pub fn checks(&self) -> &[CheckResult] {
        &self.checks
    }
    pub fn policy(&self) -> Option<&PolicyDecision> {
        self.policy.as_ref()
    }
    pub fn approval(&self) -> Option<&Approval> {
        self.approval.as_ref()
    }
    pub fn expires_t(&self) -> Option<Time> {
        self.expires_t
    }
    pub fn history(&self) -> &[StateChange] {
        &self.history
    }
    /// Signatures before the last (see the field).
    pub fn concurrences(&self) -> &[Approval] {
        &self.concurrences
    }
    /// Rules 5: where it stands in the queue (or stood).
    pub fn queue(&self) -> Option<&QueueState> {
        self.queue.as_deref()
    }
    /// What the approval rested on (the dequeue re-check compares the world with it).
    pub fn basis(&self) -> Option<&Basis> {
        self.basis.as_deref()
    }
    /// Carried over from the previous run after a bridge restart.
    pub fn resumed(&self) -> Option<&Resumed> {
        self.resumed.as_deref()
    }
    /// Rules 29: handed over at the start by the fleet file (the previous watch's order).
    pub fn handed_over(&self) -> Option<&crate::queue::HandedOverFrom> {
        self.handed_over.as_deref()
    }
    /// The vehicles that ever accepted and started it (this run or one before).
    pub fn committed(&self) -> &BTreeSet<Uid> {
        &self.committed
    }
    /// Dispatched before, in this run or one before: a vehicle may hold it.
    pub fn dispatched_before(&self) -> bool {
        self.dispatched_before
    }
    /// Its basis (a human's approval) is from the run before a bridge restart.
    pub fn basis_carried(&self) -> bool {
        self.basis_carried
    }
    /// Each vehicle's last op state in this run.
    pub fn ops(&self) -> &BTreeMap<Uid, String> {
        &self.ops
    }
    /// Each vehicle's last op detail in this run.
    pub fn op_details(&self) -> &BTreeMap<Uid, String> {
        &self.op_details
    }
    /// The vehicles that may have started it (see the field).
    pub fn maybe_started(&self) -> &BTreeSet<Uid> {
        &self.maybe_started
    }
    /// The vehicles its current delivery only confirms.
    pub fn confirming(&self) -> &BTreeSet<Uid> {
        &self.confirming
    }
    /// Rules 7: the vehicles whose start in the current delivery was logged ahead of it.
    pub fn started_ahead(&self) -> &BTreeSet<Uid> {
        &self.started_ahead
    }
    /// The vehicles whose op on the current delivery reported its start going out.
    pub fn start_went_out(&self) -> &BTreeSet<Uid> {
        &self.start_went_out
    }

    pub(crate) fn apply(
        &mut self,
        transition: Transition,
        t: Time,
        reason: String,
    ) -> Result<State, IllegalTransition> {
        let to = next_state(self.state, transition)?;
        self.state = to;
        self.history.push(StateChange {
            state: to,
            t,
            reason,
        });
        Ok(to)
    }

    pub(crate) fn set_checks(&mut self, checks: Vec<CheckResult>) {
        self.checks = checks;
    }
    pub(crate) fn set_policy(&mut self, p: PolicyDecision) {
        self.policy = Some(p);
    }
    pub(crate) fn set_approval(&mut self, a: Approval) {
        self.approval = Some(a);
    }
    pub(crate) fn set_expires(&mut self, t: Time) {
        self.expires_t = Some(t);
    }
    pub(crate) fn push_concurrence(&mut self, a: Approval) {
        self.concurrences.push(a);
    }

    /// Rules 34, a group order: its last passing check (layout and member classes).
    pub fn group_eval(&self) -> Option<&crate::group::GroupEval> {
        self.group_eval.as_deref()
    }

    pub(crate) fn set_group_eval(&mut self, eval: Option<crate::group::GroupEval>) {
        self.group_eval = eval.map(Box::new);
    }

    /// Rules 34, a group order once dispatched: its member orders, by uid.
    pub fn members(&self) -> &BTreeMap<Uid, IntentId> {
        &self.members
    }

    pub(crate) fn set_members(&mut self, members: BTreeMap<Uid, IntentId>) {
        self.members = members;
    }

    /// Rules 22: the routed legs its last passing check planned.
    pub fn route_plans(&self) -> &BTreeMap<Uid, Vec<crate::plan::PlannedLeg>> {
        &self.route_plans
    }

    pub(crate) fn set_route_plans(&mut self, plans: BTreeMap<Uid, Vec<crate::plan::PlannedLeg>>) {
        self.route_plans = plans;
    }

    pub(crate) fn set_queue(&mut self, q: Option<QueueState>) {
        self.queue = q.map(Box::new);
    }
    pub(crate) fn queue_mut(&mut self) -> Option<&mut QueueState> {
        self.queue.as_deref_mut()
    }
    pub(crate) fn set_basis(&mut self, b: Basis) {
        self.basis = Some(Box::new(b));
        self.basis_carried = false;
    }
    pub(crate) fn set_carried(
        &mut self,
        committed: BTreeSet<Uid>,
        dispatched_before: bool,
        basis_carried: bool,
        maybe_started: BTreeSet<Uid>,
    ) {
        self.committed = committed;
        self.dispatched_before = dispatched_before;
        self.basis_carried = basis_carried;
        self.maybe_started = maybe_started;
        // followed after a restart: the vehicles that had started it
        self.start_went_out = self.committed.clone();
    }
    /// Dispatched (again): a vehicle may hold it from now on; its op states start over, and
    /// the vehicles that may have started it are only confirmed this time.
    pub(crate) fn mark_dispatched(&mut self) {
        self.dispatched_before = true;
        self.ops.clear();
        self.op_details.clear();
        self.started_ahead.clear();
        self.start_went_out.clear();
        self.confirming = self.maybe_started.clone();
    }
    /// Back to a human: their next approval decides, starting it again included.
    pub(crate) fn clear_maybe_started(&mut self) {
        self.maybe_started.clear();
    }
    /// An op state reported for `uid` (`committed`: the vehicle accepted and started it;
    /// `ahead`: a start logged ahead of sending it).
    pub(crate) fn record_op(&mut self, uid: &Uid, state: &str, detail: &str, committed: bool, ahead: bool) {
        if ahead {
            self.started_ahead.insert(uid.clone());
        }
        self.ops.insert(uid.clone(), state.to_string());
        self.op_details.insert(uid.clone(), detail.to_string());
        if committed {
            self.committed.insert(uid.clone());
        }
        if crate::queue::maybe_started_op(state) {
            self.maybe_started.insert(uid.clone());
        }
        if crate::queue::maybe_started_op(state) || committed {
            self.start_went_out.insert(uid.clone());
        }
    }
    /// Back to the inbox from the queue: a new decision is needed, so the old approval and
    /// any signatures are dropped (the history keeps them).
    pub(crate) fn clear_approval(&mut self) {
        self.approval = None;
        self.concurrences.clear();
    }
    pub(crate) fn set_resumed(&mut self, r: Resumed) {
        self.resumed = Some(Box::new(r));
    }
    pub(crate) fn set_handed_over(&mut self, from: crate::queue::HandedOverFrom) {
        self.handed_over = Some(Box::new(from));
    }
    /// A resumed intent enters this run in `state` (queued or dispatched) directly: its
    /// approval was logged in the previous run.
    pub(crate) fn resume_in(&mut self, state: State, t: Time, reason: String) {
        self.state = state;
        self.history.push(StateChange { state, t, reason });
    }
}
