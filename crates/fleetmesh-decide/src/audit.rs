//! Append-only audit log (Phase C.4): one JSON object per line,
//! `{seq, t, prev, type, ...}`.
//!
//! Decision events (`check.result`, `policy.decision`) carry the exact inputs they read,
//! the world facts with their timestamps, so [`crate::replay`] can recompute them. The
//! area set is logged once (in `run.started`, and as `areas.loaded` whenever it changes)
//! with its SHA-256 content hash; decision inputs reference the hash. Space answers
//! (collect, cover) work the same way: each is logged once as `space.query` (query, answer,
//! `time_map`, segment identity with `cache_meta`) and referenced by hash; the segment in
//! use is logged as `space.loaded` when the engine is given one. The engine writes an event before applying the change it records; if the write
//! fails, the change doesn't happen.
//!
//! ## Hash chain
//!
//! Every event's `prev` is the SHA-256 (hex) of the previous event's line as written (its
//! canonical serialization: compact `serde_json` of [`Event`], without the newline); the
//! first event's `prev` is [`crate::hash::GENESIS`]. Replay recomputes the chain, so
//! deleting, inserting, reordering or editing any event that has a successor breaks it,
//! including deleting a whole intent's events and renumbering `seq`.
//!
//! This makes the log **tamper-evident, not tamper-proof**. Whoever can write the file can
//! recompute every hash after an edit and produce a consistent chain, and truncating the
//! tail (dropping the last events) leaves a valid shorter chain. The fix for both is to
//! anchor the head hash ([`crate::Engine::head_hash`], printed by `fleetmesh-replay`) outside
//! the writer's control. [`crate::anchor`] writes run-time anchors to a separate file and
//! hands them to a command that can post them elsewhere; an anchor kept where the log's
//! writer can also write is no protection against that writer (a separate trust domain
//! is needed: another machine, a signed checkpoint, a transparency log, none of which is
//! here).

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::areas::AreaSet;
use crate::authority::{AuthMethod, Grants, Roster, StepUpRef};
use crate::ids::{AgentId, IntentId, OperatorId, SessionId, Uid};
use crate::intent::{IntentSpec, State, Task, Transition};
use crate::policy::{Policy, PolicyDecision};
use crate::queue::{Basis, CarriedApproval, Reach, ResumedFrom, Verdict};
use crate::rules::CheckResult;
use crate::time::Time;
use crate::world::LoggedInputs;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub t: Time,
    /// SHA-256 of the previous event's line ([`crate::hash::GENESIS`] for the first).
    pub prev: String,
    #[serde(flatten)]
    pub body: EventBody,
}

/// An area set and its content hash ([`AreaSet::content_hash`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoggedAreas {
    pub sha256: String,
    pub areas: AreaSet,
}

/// When a check ran: at submission, again at approval against the then-current world,
/// (rules 5) when a queued intent's vehicles are reachable again, before it may go, or
/// (rules 11) on a dispatched collect or cover against a later space answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckPhase {
    Submit,
    Approve,
    Dequeue,
    Dispatched,
}

// `run.started` (the policy, the roster, the areas and, rules 12, the carry) is the largest
// variant by far and is written once per run: boxing it would only reshape every match.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventBody {
    /// The engine started; the policy in force for every decision that follows, and the
    /// area set if one was given at start.
    #[serde(rename = "run.started")]
    RunStarted {
        crate_version: String,
        /// The decision rules this run decides under (`crate::RULES_VERSION`) and the policy
        /// file schema it was given (`crate::POLICY_SCHEMA`). Absent in logs written before
        /// they existed: version 1 of both. Replay recomputes with these rules.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rules_version: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy_schema: Option<u32>,
        /// The build that wrote the log: its commit, and whether the working tree differed
        /// from it. Absent in logs written before it existed (rules 1 and 2).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build: Option<BuildInfo>,
        /// Who may act as an operator, and with consequential authority (rules 4 on; absent
        /// before). Replay checks every session against it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roster: Option<Roster>,
        policy: Policy,
        areas: Option<LoggedAreas>,
        /// Rules 12 ([`crate::rules::REVIEW_FOLLOWUPS_SINCE`]): what the bridge carried over at
        /// start from earlier runs' logs, in one line (a crash can't leave half of it): the log
        /// its `.last-run-<port>` pointer named, then any log whose carry failed before and that
        /// nobody wrote off, each with its head and last seq as read and what it still owed
        /// (the platforms' own lost-link values, and the logs it had not carried), or why it
        /// could not be read. The platforms' values seed this run's ledger. Absent when no
        /// pointer named a log (a first start), in a scenario and before rules 12.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        carried_from: Vec<crate::replay::CarriedFrom>,
        /// Rules 15 ([`crate::rules::CARRY_FOLLOWUPS_SINCE`]): the `.last-run` pointer of the
        /// station this run is (its file name, `.last-run-<http port>`): the pointer it read at
        /// start and rewrites to name this log. A later start of the station whose pointer can't
        /// be read falls back to the newest log naming it whose chain and anchors check. Absent
        /// without a pointer (a scenario, a test) and before rules 15.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pointer: Option<String>,
        /// Rules 33 ([`crate::rules::SCENARIO_SETTINGS_SINCE`]): the files the policy was built
        /// from, the one the run was given first, then each file it extends in turn, each path as
        /// written (never absolute) with the SHA-256 of its bytes. Absent for a policy not read
        /// from a file (a test) and before rules 33.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        policy_layers: Vec<crate::policy::Layer>,
    },

    /// A new area set is in force (the first one, if none was given at start, or a
    /// change). Decision inputs after this reference it by hash.
    #[serde(rename = "areas.loaded")]
    AreasLoaded { sha256: String, areas: AreaSet },




    #[serde(rename = "intent.created")]
    IntentCreated {
        intent: IntentSpec,
        /// Rules 8: a human's intent is made in a live session of theirs (absent for an
        /// agent's, and before rules 8); replay checks it is open and the proposer's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },

    /// Rules 8, a credentials run: a login for an operator on the roster was refused (a wrong
    /// password; `locked_s`: the id is now locked that long). Never the password. A refused
    /// step-up is `operator.action step_up.refused` under its session instead.
    #[serde(rename = "auth.refused")]
    AuthRefused {
        operator: OperatorId,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locked_s: Option<u64>,
    },

    #[serde(rename = "check.result")]
    CheckResult {
        intent: IntentId,
        phase: CheckPhase,
        inputs: LoggedInputs,
        results: Vec<CheckResult>,
        passed: bool,
        /// Rules 34, a group order: its layout on the same inputs (each member's share, the
        /// layers, the spacing; [`crate::group::Layout`]), which replay recomputes and binds each
        /// member order to. Absent on any other check.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<Box<crate::group::Layout>>,
    },

    #[serde(rename = "policy.decision")]
    PolicyDecision {
        intent: IntentId,
        inputs: LoggedInputs,
        decision: PolicyDecision,
    },

    /// Every state change of every intent.
    #[serde(rename = "intent.state")]
    IntentState {
        intent: IntentId,
        transition: Transition,
        from: State,
        to: State,
        reason: String,
    },

    /// A human must decide by `expires_t` (engine time). `until` says when the deadline
    /// is not the plain `expire_s` (a collect's tasking lead time, in scenario time).
    #[serde(rename = "approval.requested")]
    ApprovalRequested {
        intent: IntentId,
        expires_t: Time,
    },

    /// The approval that moves an intent to `approved` (the last signature when the tier
    /// needs more than one).
    #[serde(rename = "approval.granted")]
    ApprovalGranted {
        intent: IntentId,
        by: OperatorId,
        session: SessionId,
        /// The approver is the human who proposed the intent. Allowed where the tier lets
        /// the proposer approve, and recorded so evaluation can measure it; replay recomputes
        /// it.
        self_approved: bool,
        note: Option<String>,
        /// Older orders on the same vehicles this approval superseded (the approver
        /// acknowledged them). Absent when none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        supersedes: Vec<IntentId>,
        /// When the plan the approver signed was last checked (the plan's freeze time is read
        /// against it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checked_t: Option<Time>,
        /// Rules 8: the step-up this signature quoted (the signer's credential re-entered at
        /// signing: the `operator.action step_up` event under the same session). Required in
        /// a credentials run when the decision says `step_up`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_up: Option<StepUpRef>,
    },

    /// A signature that is not yet enough: signature `n` of the `of` distinct approvers the
    /// tier needs (two-person approval). The intent stays `pending_approval`.
    #[serde(rename = "approval.concurred")]
    ApprovalConcurred {
        intent: IntentId,
        by: OperatorId,
        session: SessionId,
        n: u32,
        of: u32,
        note: Option<String>,
        /// As on `approval.granted`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        supersedes: Vec<IntentId>,
        /// As on `approval.granted`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checked_t: Option<Time>,
        /// Rules 8: as on `approval.granted`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_up: Option<StepUpRef>,
    },

    /// A plan's approval was reset: the plan went stale (its checks no longer hold, or the
    /// signatures took longer than `max_signing_s`), so every signature so far
    /// (`dropped`) is dropped with the plan and a new plan must be signed. At a signature the
    /// world is that signature's re-check (`inputs` absent); while pending it is the world
    /// then (`inputs` present). Replay recomputes the failed checks.
    #[serde(rename = "approval.reset")]
    ApprovalReset {
        intent: IntentId,
        dropped: Vec<OperatorId>,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inputs: Option<LoggedInputs>,
    },

    /// A human submitted an intent whose policy tier needs a human and lets the submitter
    /// authorize (`submitter_authorizes`): the submission is the human decision. Distinct
    /// from `approval.granted` (an inbox approval) and from `auto_approved` (no human), so
    /// evaluation can tell the three apart. Never for an agent-sourced intent.
    #[serde(rename = "approval.at_submission")]
    ApprovalAtSubmission {
        intent: IntentId,
        by: OperatorId,
        session: SessionId,
        /// The approver is the submitter: always true, recorded like `approval.granted`'s.
        self_approved: bool,
        /// The policy tier that allowed it (`policy.decision`'s `rule`).
        rule: String,
    },

    #[serde(rename = "approval.rejected")]
    ApprovalRejected {
        intent: IntentId,
        by: OperatorId,
        session: SessionId,
        note: Option<String>,
    },

    #[serde(rename = "approval.expired")]
    ApprovalExpired { intent: IntentId, expires_t: Time },

    #[serde(rename = "intent.dispatched")]
    IntentDispatched {
        intent: IntentId,
        task: Task,
        uids: Vec<Uid>,
        /// Rules 22: each vehicle's transit waypoints, a plan's route around the wrong surface or
        /// a no-go area (replay recomputes them from the last check's plans).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        via: BTreeMap<Uid, Vec<crate::geo::LonLat>>,
        /// Rules 22: a patrol's route with the waypoints its legs are routed through.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<Vec<crate::geo::LonLat>>,
        /// Rules 5, a kind the queue holds, dispatched straight from its approval: the
        /// vehicle facts that showed every vehicle reachable (`reach`, recomputed by replay).
        /// Absent when it comes out of the queue (its `queue.recheck` holds them).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inputs: Option<LoggedInputs>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reach: Option<Vec<Reach>>,
        /// Rules 34, a member order: the share it flies (its spec's: slot, layer, altitude, where
        /// it enters), handed to the task layer with the task.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        share: Option<crate::group::Share>,
    },

    /// Rules 34: a group order goes out as its member orders, one per member sent or waiting,
    /// each created and approved under it just before (`intent.created` with `parent`, the
    /// transition `under`); the group's own dispatch follows at once, and each member order is
    /// then dispatched or queued on its own.
    #[serde(rename = "group.dispatched")]
    GroupDispatched {
        intent: IntentId,
        /// Each member's member order, by uid.
        members: BTreeMap<Uid, IntentId>,
    },

    /// Rules 34: a group order in force re-formed itself inside its envelope and tier (docs/SWARMS.md
    /// section 4.3), its `reform`th re-form: on `cause` (the members on its layout that dropped, the
    /// members a re-form had left out that are back), the facts of every member of its selection
    /// (`inputs`, with the terrain along each moved member's new share), the new layout, each
    /// member moved or back with its new member order (created and approved under the group's
    /// approval right after, then dispatched or queued on its own), the members back whose share
    /// failed its checks (left out for good), the moved members' checks, and the largest gap
    /// against the even spacing. Replay recomputes all of it.
    #[serde(rename = "group.reformed")]
    GroupReformed {
        intent: IntentId,
        reform: u32,
        inputs: LoggedInputs,
        cause: crate::group::Cause,
        layout: Box<crate::group::Layout>,
        moved: BTreeMap<Uid, IntentId>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        left_out: BTreeMap<Uid, String>,
        results: Vec<CheckResult>,
        largest_gap_m: f64,
        even_spacing_m: f64,
    },

    /// Rules 34: a re-form due was refused (it would leave the envelope: a layout that does not
    /// hold, a moved member's share failing its checks; or raise the tier): nothing moved, the
    /// members on their shares fly on, the person decides. On the same facts as a re-form.
    #[serde(rename = "group.reform_refused")]
    GroupReformRefused {
        intent: IntentId,
        inputs: LoggedInputs,
        cause: crate::group::Cause,
        why: String,
    },

    /// Rules 34: a group order in force fell below its minimum (its members on their shares and
    /// back, `strength`, fewer than `min`, its own less one for each member a newer order took):
    /// it re-forms no more; the person re-forms it or orders anew (decision 4).
    #[serde(rename = "group.suspended")]
    GroupSuspended {
        intent: IntentId,
        inputs: LoggedInputs,
        cause: crate::group::Cause,
        strength: u32,
        min: u32,
    },

    /// Rules 5: an approved intent held for vehicles that can't be reached now (`reach`,
    /// recomputed by replay from `inputs`), until `expires_t` at the latest.
    #[serde(rename = "intent.queued")]
    IntentQueued {
        intent: IntentId,
        inputs: LoggedInputs,
        reach: Vec<Reach>,
        expires_t: Time,
    },

    /// Rules 5: a queued intent's vehicles are reachable again and its checks ran on the
    /// world now (the `check.result`, phase `dequeue`, just before): the policy decision on
    /// those inputs (absent when a check failed) and what follows (`verdict`: dispatch, back
    /// to the inbox, void), with what changed since the approval. Replay recomputes all of it.
    #[serde(rename = "queue.recheck")]
    QueueRecheck {
        intent: IntentId,
        reach: Vec<Reach>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<PolicyDecision>,
        verdict: Verdict,
        changes: Vec<String>,
    },

    /// Rules 5: a queued intent's deadline passed before its vehicles could be reached. It
    /// expires, or (`inbox`: a human's approval held past `[queue] human_expire_s`) goes back
    /// to the inbox for a new decision (`approval.requested` follows).
    #[serde(rename = "queue.expired")]
    QueueExpired {
        intent: IntentId,
        expires_t: Time,
        reason: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        inbox: bool,
    },

    /// Rules 5: a dispatched intent whose link was lost to every vehicle before any started
    /// it (each vehicle's last `op.state` is `requeued`, none ever started it) goes back to
    /// the queue until `expires_t` (`inbox` as in `queue.expired`), re-checked before it is
    /// sent again.
    #[serde(rename = "intent.requeued")]
    IntentRequeued {
        intent: IntentId,
        expires_t: Time,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        inbox: bool,
    },

    /// Rules 5: an intent the previous run left open (queued, approved, or dispatched and
    /// still running), carried into this one after a bridge restart: its spec and id, where
    /// it comes from (that log, its head and last seq), the state it continues in here
    /// (`queued`: re-checked before it goes; `dispatched`: some vehicle had started it, so it
    /// is followed, never sent again), what its approval rested on, and per vehicle its last
    /// op state there. Replay checks it against the previous log when given it.
    #[serde(rename = "intent.resumed")]
    IntentResumed {
        intent: IntentSpec,
        from: ResumedFrom,
        state: State,
        basis: Basis,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<PolicyDecision>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval: Option<CarriedApproval>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_t: Option<Time>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        ops: BTreeMap<Uid, String>,
        /// The vehicles that ever started it, across every run it went through.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        committed: Vec<Uid>,
        /// It was dispatched in some run before: a vehicle may hold it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        dispatched_before: bool,
        /// Each vehicle's platform as the previous run last checked it.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        platforms: BTreeMap<Uid, String>,
        /// The vehicles that may have started it (its start went out, nothing confirmed it).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        maybe_started: Vec<Uid>,
    },

    /// Rules 29: an order the fleet file hands over at the start (`start` on a unit or a swarm,
    /// sim/swarm.py): one vehicle's starting task, the previous watch's order, which its
    /// vehicle spawned on station for. It enters queued (re-checked on the world before it goes,
    /// delivered once) until `expires_t` (`[queue] expire_s` from now, whatever its kind); its
    /// source is the fleet file (`from`: its name and content hash) and its basis the previous
    /// watch's approval, which no person or policy of this run decides again. Replay checks it
    /// against the fleet the run logged (`<log stem>.fleet.json`) when given it.
    #[serde(rename = "intent.handed_over")]
    IntentHandedOver {
        intent: IntentSpec,
        from: crate::queue::HandedOverFrom,
        expires_t: Time,
    },

    /// Rules 5: a vehicle rebooted, and lost the mode and armed state (and, on a stand-in,
    /// the mission) its orders had set: its time since boot went back, or (an outage longer
    /// than its uptime before) the boot instant it implies jumped later than the one before
    /// (`booted_before_t`). `booted_t` is the new boot instant the bridge reports from here on.
    #[serde(rename = "vehicle.rebooted")]
    VehicleRebooted {
        uid: Uid,
        boot_ms_before: u64,
        boot_ms: u64,
        booted_t: Time,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        booted_before_t: Option<Time>,
    },

    /// Per-vehicle task progress from the task layer (sent, acked, active, done, failed).
    /// Rules 7: a vehicle task's `starting` is logged ahead of the start it records (`ahead`):
    /// written before the mission mode, the hold or rtl mode, or new items to a vehicle
    /// already flying a mission go out, so a bridge that stops right after still finds that
    /// the order may have started.
    #[serde(rename = "op.state")]
    OpState {
        intent: IntentId,
        uid: Uid,
        state: String,
        detail: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        ahead: bool,
    },

    /// Dispatch (docs/design/dispatch.md): the plan the dispatcher's assigner made at `tick`,
    /// with the whole input it read, by value. Replay re-runs [`crate::dispatch::assign`] on
    /// `input` and compares it with `plan`; the dispatcher's proposals that follow are ordinary
    /// intents.
    #[serde(rename = "dispatch.plan")]
    DispatchPlan {
        tick: u64,
        input: Box<crate::dispatch::AssignInput>,
        plan: crate::dispatch::Plan,
    },

    /// What happened to a delivery's parts outside a decision: a parcel changed state
    /// (`parcel`), a drop zone was looked at (`observation`), a hub's packs changed (`hub`), the
    /// navigation monitor saw a vehicle's fix go bad (`nav`), a dispatcher's order was withdrawn
    /// (`withdrawn`). `data` is the fact by value.
    #[serde(rename = "delivery.event")]
    DeliveryEvent {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uid: Option<Uid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parcel: Option<String>,
        detail: String,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        data: serde_json::Value,
    },

    /// Rules 7 (docs/design/watch-officer.md): the AI watch officer came on watch (an MCP call
    /// by the policy's `[officer] agent`) or went off it (no call for `absent_after_s`). Going off
    /// releases every item it held (`triage.released`, `officer_absent`).
    #[serde(rename = "triage.watch")]
    TriageWatch { agent: AgentId, on: bool },

    /// Rules 7: an Inbox item appeared while the officer was on watch, and where it went: to the
    /// officer with its deadline, or straight to the person ([`crate::officer::route`] on `input`,
    /// which replay re-runs against the logged policy). `item` is `intent:<id>` or
    /// `alert:<id>`; `about` says what it is, for the audit, and no routing reads it.
    #[serde(rename = "triage.routed")]
    TriageRouted {
        item: String,
        input: crate::officer::TriageInput,
        route: crate::officer::Route,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        about: serde_json::Value,
    },

    /// Rules 7: the officer's verdict on an item it held: handled (within `[officer.handle]`),
    /// escalated to the person with its recommendation, or turned down (a dispatcher's order the
    /// dispatcher then withdrew). `intents`: the orders it proposed or turned down for it.
    #[serde(rename = "triage.verdict")]
    TriageVerdict {
        item: String,
        by: AgentId,
        verdict: crate::officer::Verdict,
        note: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recommendation: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        intents: Vec<IntentId>,
    },

    /// Rules 7: an item the officer held reached the person without its verdict: its deadline
    /// passed, or the officer went off watch.
    #[serde(rename = "triage.released")]
    TriageReleased { item: String, why: crate::officer::Release },

    /// Rules 7: an item the officer held left the Inbox before a verdict or a release (the order
    /// was decided, withdrawn or expired; the alert's cause cleared).
    #[serde(rename = "triage.closed")]
    TriageClosed { item: String, why: String },

    /// Rules 8 ([`crate::rules::PLAYS_SINCE`], docs/design/incidents.md): a person adopted a play,
    /// a standing decision for a pattern at a hub, in force until it ends.
    #[serde(rename = "play.adopted")]
    PlayAdopted {
        play: crate::plays::Play,
        session: SessionId,
    },

    /// Rules 8: a play ended: its end condition cleared (the bridge's, `by` none), a person lifted
    /// it (`by`, `session`), or its time ran out (`by` none, at its `until_t`).
    #[serde(rename = "play.ended")]
    PlayEnded {
        play: u64,
        why: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<OperatorId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },

    /// Mode change, failsafe, STATUSTEXT and the like.
    #[serde(rename = "vehicle.event")]
    VehicleEvent {
        uid: Uid,
        kind: String,
        text: String,
    },

    #[serde(rename = "operator.action")]
    OperatorAction {
        operator: OperatorId,
        session: SessionId,
        action: String,
        detail: String,
        /// `session.opened`: what the session may authorize beyond tasking (absent: nothing).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grants: Option<Grants>,
        /// Rules 8, on `session.opened` and `step_up`: how the operator proved who they are
        /// (`password`: checked by the bridge; `asserted`: a name the request gave, dev mode).
        /// `session.closed` (detail: why) ends the session; nothing may follow under it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<AuthMethod>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_up: Option<StepUpRef>,
        /// Rules 10, on `session.opened` and `step_up` with `auth: passkey`: the assertion,
        /// what it was for, the nonce and issue time its challenge was derived from, the
        /// credential and its counter (nothing secret). Replay recomputes the challenge and
        /// verifies the signature against the run's logged public key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passkey: Option<Box<crate::passkey::PasskeyUse>>,
    },

    /// Rules 10, a credentials run: a passkey was registered for `key.operator` while the run
    /// was up (its public key, never private material): through a signed-in session, which
    /// re-entered the operator's credential first (`step_up`, under `session`), or through a
    /// one-time invitation `fleetmesh-auth passkey-invite` issued (`via: invitation`, no
    /// session). Usable from here on.
    #[serde(rename = "passkey.registered")]
    PasskeyRegistered {
        key: crate::passkey::PasskeyKey,
        via: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_up: Option<StepUpRef>,
    },

    /// Rules 10: a passkey was removed from the credential file (`fleetmesh-auth
    /// passkey-remove`) while the run was up; nothing may use it from here on.
    #[serde(rename = "passkey.removed")]
    PasskeyRemoved {
        operator: OperatorId,
        credential: String,
        reason: String,
    },

    #[serde(rename = "agent.action")]
    AgentAction {
        agent: AgentId,
        action: String,
        detail: String,
    },




















    /// Rules 30 ([`crate::rules::NODE_LOSS_SINCE`], docs/NODE-LOSS.md): one of ours is presumed
    /// lost: silent on its facts at `world_t` (`vehicle`; its last report at `since`, none: it
    /// never reported a position) with evidence (damage it reported of itself). A flag for a
    /// person's review: it decides nothing. Replay recomputes it.
    #[serde(rename = "node.presumed")]
    NodePresumed {
        uid: Uid,
        #[serde(default)]
        since: Option<Time>,
        evidence: Vec<crate::nodes::Evidence>,
        world_t: Time,
        vehicle: Box<crate::world::VehicleFacts>,
    },

    /// Rules 30: a presumption of loss withdrawn: the vehicle was heard again since the report it
    /// was made on (its facts at `world_t`; `detail` says how). Replay recomputes it.
    #[serde(rename = "node.presumption_cleared")]
    NodePresumptionCleared {
        uid: Uid,
        #[serde(default)]
        since: Option<Time>,
        world_t: Time,
        vehicle: Box<crate::world::VehicleFacts>,
        detail: String,
    },

    /// Rules 30: an operator with consequential authority (`by`, under `session`; never an agent)
    /// confirmed one of ours lost, on the silence they saw: silent since exactly its report at
    /// of the presumption in force (empty: none presumed it, and `note` says why: decision D2),
    /// the vehicle as the bridge knew it (`identity`: a later run's carry holds only it) and, in a
    /// credentials run, the step-up made for it. From here on it leaves the friendly keep-outs,
    /// lost-link values end next (voids, `op.state` lost, `restore.closed` lost). Replay
    /// recomputes every condition.
    #[serde(rename = "node.lost")]
    NodeLost {
        uid: Uid,
        #[serde(default)]
        since: Option<Time>,
        by: OperatorId,
        session: SessionId,
        note: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<crate::nodes::Evidence>,
        world_t: Time,
        vehicle: Box<crate::world::VehicleFacts>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_up: Option<StepUpRef>,
    },

    /// Rules 30: a loss revoked (`cause`): automatically when the vehicle is heard again, logged
    /// before any decision reads the report, or by any operator with a note. Its protection is
    /// back at once and the values its confirmation closed are owed back again. `from`: the
    /// earlier log a loss carried over was confirmed in.
    #[serde(rename = "node.loss_revoked")]
    NodeLossRevoked {
        uid: Uid,
        #[serde(default)]
        since: Option<Time>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<String>,
        cause: crate::nodes::Revocation,
    },









    #[serde(rename = "operator.view")]
    OperatorView {
        operator: OperatorId,
        session: SessionId,
        intent: IntentId,
        view: OperatorView,
    },
}

/// One console display event for a pending intent (see [`EventBody::OperatorView`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperatorView {
    /// `open` (the card was rendered), `close` (it went away undecided), `decide` (approve
    /// or reject was clicked; `action` says which).
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// What the card showed at that moment.
    pub shown: ViewShown,
    /// How long the card had been open, milliseconds (absent for `open`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_ms: Option<u64>,
    /// The client's own clock, milliseconds since the epoch, when it happened (the event's
    /// `t` is the bridge's clock when it was logged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_t: Option<i64>,
}

impl OperatorView {
    pub const EVENTS: [&'static str; 3] = ["open", "close", "decide"];
    pub const ACTIONS: [&'static str; 2] = ["approve", "reject"];
    /// Bounds on what a view may carry: it is instrumentation, not a place to store data.
    pub const MAX_ITEMS: usize = 64;
    pub const MAX_TEXT: usize = 256;

    /// A well-formed view: a known event, an action only on `decide` and then a known one,
    /// at most [`Self::MAX_ITEMS`] checks and warnings, each at most
    /// [`Self::MAX_TEXT`] bytes.
    pub fn validate(&self) -> Result<(), String> {
        if !Self::EVENTS.contains(&self.event.as_str()) {
            return Err(format!("event must be one of {}", Self::EVENTS.join(", ")));
        }
        for (name, items) in [("checks", &self.shown.checks), ("warnings", &self.shown.warnings)] {
            if items.len() > Self::MAX_ITEMS {
                return Err(format!("{name}: at most {} entries", Self::MAX_ITEMS));
            }
            if items.iter().any(|t| t.len() > Self::MAX_TEXT) {
                return Err(format!("{name}: entries of at most {} bytes", Self::MAX_TEXT));
            }
        }
        match (self.event.as_str(), self.action.as_deref()) {
            ("decide", Some(a)) if Self::ACTIONS.contains(&a) => Ok(()),
            ("decide", Some(a)) => Err(format!(
                "action {a:?}: must be one of {}",
                Self::ACTIONS.join(", ")
            )),
            ("decide", None) => Err("a decide event needs an action".into()),
            (_, Some(_)) => Err("only a decide event carries an action".into()),
            (_, None) => Ok(()),
        }
    }
}

/// What was visible on an intent's card: the rule ids of the checks shown, the warning
/// lines ("why this needs a human"), and whether the rationale was shown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewShown {
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub rationale: bool,
}

impl EventBody {
    /// The inputs of every decision this event logs (none, one, or one per intent): their
    /// vehicles' facts are the reports a loss is revoked on before the event is logged, and
    /// their `nodes_lost` replay compares with its ledger.
    pub fn decision_inputs(&self) -> Vec<&LoggedInputs> {
        match self {
            EventBody::CheckResult { inputs, .. }
            | EventBody::PolicyDecision { inputs, .. }
            | EventBody::IntentQueued { inputs, .. } => vec![inputs],
            EventBody::ApprovalReset { inputs, .. } | EventBody::IntentDispatched { inputs, .. } => {
                inputs.iter().collect()
            }
            _ => Vec::new(),
        }
    }
}

/// The build a log was written by (`run.started`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    /// The commit the binary was built from ("unknown" outside a git checkout).
    pub commit: String,
    /// The working tree had changes to tracked files (None: unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
}

impl BuildInfo {
    /// This binary's build (`build.rs`).
    pub fn current() -> BuildInfo {
        BuildInfo {
            commit: env!("FLEETMESH_BUILD_COMMIT").to_string(),
            dirty: match env!("FLEETMESH_BUILD_DIRTY") {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            },
        }
    }
}

/// Where events go. Implementations must append, never rewrite.
pub trait AuditSink {
    fn append(&mut self, line: &str) -> std::io::Result<()>;
}

/// In-memory log; clones share the buffer, so a test can read what the engine wrote.
#[derive(Clone, Debug, Default)]
pub struct MemorySink(Arc<Mutex<Vec<String>>>);

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn text(&self) -> String {
        let mut s = self.lines().join("\n");
        s.push('\n');
        s
    }
}

impl AuditSink for MemorySink {
    fn append(&mut self, line: &str) -> std::io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(line.to_string());
        Ok(())
    }
}

/// A JSONL file opened in append mode (e.g. `logs/audit/<run-id>.jsonl`). Each event is
/// written and flushed before the engine acts on it.
#[derive(Debug)]
pub struct JsonlFile(File);

impl JsonlFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(JsonlFile(
            OpenOptions::new().create(true).append(true).open(path)?,
        ))
    }
}

impl AuditSink for JsonlFile {
    fn append(&mut self, line: &str) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        self.0.write_all(&buf)?;
        self.0.flush()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("line {line}: {err}")]
pub struct LogError {
    pub line: usize,
    pub err: serde_json::Error,
}

/// Parse a JSONL log. Blank lines are skipped.
pub fn parse_log(text: &str) -> Result<Vec<Event>, LogError> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|err| LogError { line: i + 1, err }))
        .collect()
}
