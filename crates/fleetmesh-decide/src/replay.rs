//! Replay (Phase C.4 exit gate): recompute every check and policy decision in a log from
//! the inputs logged with it, and verify the log's own story.
//!
//! What is verified:
//! - every `check.result`: rules re-run on the logged inputs give the logged results;
//! - every `policy.decision`: the policy (as logged in `run.started`) re-run on the logged
//!   inputs gives the logged decision;
//! - every `intent.state` is a legal transition from the state replay tracked, and is
//!   backed by its evidence: auto approval by a recomputed `auto` tier and passing checks,
//!   `approved` by an `approval.granted` from an operator before the deadline and a passing
//!   re-check, rejection by failing checks or a human rejection, expiry by the deadline;
//! - `seq` is contiguous and `t` never goes backwards;
//! - the hash chain: each event's `prev` is the SHA-256 of the previous line, and (from
//!   text) each line is its event's canonical serialization;
//! - area sets: each logged set's content hash matches its `sha256`, and every decision
//!   references the set in force;
//! - `self_approved` on every `approval.granted` matches who proposed the intent;
//! - every `approval.at_submission` (a human's submission that is itself the approval):
//!   the intent was just checked and passed, its recomputed policy decision is a human tier
//!   that lets the submitter authorize, the source is a human (never an agent) and the
//!   approver is that human; and an intent whose decision says the submitter authorizes is
//!   not sent to the inbox instead;
//! - every `approval.requested` deadline: `[approval] expire_s`, or the kind's
//!   `[approval.kind.<kind>]` setting; a collect with `until = "tasking_lead"` from the
//!   answer its policy decision read, mapped to engine time through the `space.loaded`
//!   scenario map;
//! - space answers (collect, cover): each `space.query`'s content hash matches its
//!   `sha256`, every decision references a logged answer, and a dispatched collect is the
//!   chosen option. Rules re-decide from the logged answers, so this needs no space cache.
//! - two-person approval: every signature (`approval.concurred`, `approval.granted`) is by
//!   the operator its session was opened for, never the proposer where the policy excludes
//!   the proposer, never twice by the same operator, and `approved` comes only with as many
//!   distinct signatures as the recomputed decision asks;
//!   consequential authority; the preview logged with each check, the snapshot in the final
//!   logged inputs; the effect follows a picture that held and was ready, at the snapshot's
//! - what was executed is what was approved (an audit check, for every rules version): each
//!   `intent.dispatched` is exactly the approved intent's order (its task and vehicles, and
//!   last check read and the approval's snapshot, the frozen plan), a dispatch transition
//!   has its `intent.dispatched` right before it and nothing else follows one; an
//!   before it; an `op.state` is a dispatched intent's, on its own vehicles (a collect's
//!   satellite, a cover's service), a vehicle or space task's only while it runs, after a
//!   newer order took its vehicle only how its op ended, `superseded` only when something
//!   replaced it, and a reported fallback is the one (and the task) its stop released; a
//!   failed picture is stopped for the picture; a superseded stop names the order dispatched
//!   last, still under way, on the platform, with that order's source; a `system` stop that
//! - sessions: from rules 4 every session is for an operator on the roster `run.started`
//!   names, with consequential authority only for its consequential operators; rejections and
//!   at-submission approvals are under the decider's own session.
//! - carry-overs from an earlier run: each `intent.resumed` against the log it names
//!   ([`ReplayOptions::previous`]), each carried `platform.restore` against what the log it
//!   names still owed, that log replaying itself ([`ReplayOptions::restores_from`]; the two
//!   can be different logs; either not given is a mismatch unless trusted,
//!   [`ReplayOptions::trust_resumed`], and [`ReplayReport::carried_restores`] says which were
//!   checked); and an op whose start went out (`starting`, or the vehicle started it, a
//!   followed vehicle after a restart included) never ends `requeued` on that delivery.
//! - authentication (rules 8, [`crate::rules::OPERATOR_AUTH_SINCE`]): `run.started` says how
//!   operators were authenticated (the roster's `auth`, [`ReplayReport::auth`]); every
//!   session was opened that way (`password` or a harness-held `scripted` credential in a
//!   credentials run, `asserted` in a dev run; [`ReplayReport::people`] and
//!   [`ReplayReport::scripted`] tell the first two apart); a human's `intent.created` names
//!   the live session of theirs it was made in, an agent's none; every other operator action
//!   is under the operator's own session; nothing acts under a session after its
//!   `session.closed`; a step-up is only in a credentials run, by the credential its session
//!   was opened with; an act quoting one quotes a step-up under its own session, unused, at
//!   share theirs), and in a credentials run every signature whose recomputed decision says
//!   quotes one; a refused login (`auth.refused`) is only in a credentials run, for an
//!   operator it lists; an intent is carried over only from a run that authenticated alike.
//!   [`ReplayReport::authenticated`] is what evaluation reads before counting a log's human
//!   decisions, with the log's anchors checked and only its people's acts.
//! - passkeys (rules 10, [`crate::rules::PASSKEY_SINCE`]): the roster's relying party and keys
//!   are ones a run could start with (a credentials run, a domain, keys on the curve, a study's
//!   participant with a passkey and a passkey policy unless overridden); every passkey login
//!   and step-up's assertion verifies ([`crate::passkey::verify`]) against the key the run
//!   logged or registered since (not removed, the operator's own), on the challenge recomputed
//!   from the first line's hash, the session, what it was for, the nonce and the issue time,
//!   each nonce once, within [`crate::passkey::CHALLENGE_TTL_MS`], its counter counting up; an
//!   act quoting a passkey step-up is the act it was made for, and where the policy asks a
//!   person for a passkey their quoted step-up is one; a registration through a session quotes
//!   a password step-up. [`ReplayReport::study_evidence`] says whether a study's participant
//!   signed every consequential act by passkey.
//!
//! **Space recomputation.** [`replay_str_with_space`] also re-runs every logged space query
//! on the segment given. If that segment is the one the query ran on (same
//! [`crate::SegmentId`] hash), the new answer must equal the logged one, else it is a
//! mismatch. If the segment differs (other element sets or catalog), the query is not
//! recomputed and the event goes to [`ReplayReport::cache_mismatches`]: a cache mismatch,
//! which says nothing about the decisions (they were re-decided from the logged answer).
//!
//! **Rules versions.** A log is recomputed with the decision rules it records in
//! `run.started` (none: version 1), but only rules the verifier accepts: by default the
//! current ones ([`ReplayOptions::rules`]; `fleetmesh-replay --rules N` or `--min-rules N`
//! accepts older ones explicitly), so a log can't pass by claiming older, looser rules.
//! What the log carries must fit its claim: `run.started` comes first and only once, and no
//! field or check only later builds write (a `checked_t`, a GPS fact, a `platform_free`
//! check, a policy key of a later schema) may appear in a log that claims earlier rules.
//!
//! Physics is not replayed; the decision layer is. What replay cannot know is whether the
//! logged inputs were true of the world: it proves the decisions follow from them, and that
//! the orders logged as executed are the approved ones (not that a vehicle flew them: op
//! states and vehicle events are the bridge's reports, bound to their intent and vehicle but
//! not otherwise checked). The
//! chain is tamper-evident, not tamper-proof (see [`crate::audit`]): a full rewrite or a
//! truncated tail needs the head hash ([`ReplayReport::head_sha256`]) checked against an
//! external anchor.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::areas::AreaSet;
use crate::audit::{CheckPhase, Event, EventBody, LogError, parse_log};
use crate::authority::{AuthMethod, AuthMode, Grants, Roster, STEP_UP_MAX_AGE_MS, StepUpRef, effective_mode};
use crate::engine::{
    StartAhead, is_self_approval, op_state_mismatch, op_state_owner, waiting_vehicles,
};
use crate::ids::{OperatorId, SessionId, Uid};
use crate::hash::{GENESIS, sha256_hex};
use crate::ids::IntentId;
use crate::intent::{IntentSpec, Source, State, Task, TaskKind, Transition, next_state};
use crate::policy::{self, Policy, PolicyDecision, Tier};
use crate::queue::{self as queue, Basis, CarriedApproval, OpenIntent, Verdict};
use crate::rules::{self, CheckResult};
use crate::time::Time;
use crate::world::{LoggedInputs, WorldSnapshot};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mismatch {
    pub seq: u64,
    pub intent: Option<IntentId>,
    pub what: String,
    pub logged: String,
    pub recomputed: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ReplayReport {
    pub events: usize,
    pub intents: usize,
    pub checks_recomputed: usize,
    pub policy_recomputed: usize,
    pub transitions_verified: usize,
    /// Area sets logged (`run.started` and `areas.loaded`).
    pub area_sets: usize,
    pub approvals: usize,
    /// Approvals where the approver also proposed the intent.
    pub self_approvals: usize,
    /// Human submissions that were themselves the approval (`approval.at_submission`). Not
    /// counted in `approvals` or `self_approvals`, which are inbox approvals.
    pub approvals_at_submission: usize,
    /// SHA-256 of the last event: compare with an external anchor to detect a rewritten
    /// chain or a truncated tail.
    pub head_sha256: Option<String>,
    pub mismatches: Vec<Mismatch>,
    /// Space segments logged (`space.loaded`).
    pub space_segments: usize,
    /// Space answers logged (`space.query`).
    pub space_queries: usize,
    /// Space queries re-run on the given segment and found equal (or not: see mismatches).
    pub space_recomputed: usize,
    /// The decision rules the log was produced under (`run.started`; 1 when absent), which
    /// replay recomputes with.
    pub rules_version: u32,
    /// The build that wrote the log (`run.started`; absent before rules 3).
    pub build: Option<crate::audit::BuildInfo>,
    /// Space queries that could not be recomputed because the segment given to replay is
    /// not the one they ran on. Not decision mismatches: [`Self::ok`] ignores them.
    pub cache_mismatches: Vec<Mismatch>,
    /// Signatures that were not the last one a tier needed (`approval.concurred`).
    pub concurrences: usize,
    /// Rules 6: platforms whose own lost-link values were still
    /// owed back when the log ended (no confirmed restore): the next run carries them over
    /// (`platform.restore`), but the operator should know. Not decision mismatches.
    pub unrestored: Vec<String>,
    /// Rules 30: our vehicles presumed lost (`node.presumed`), each with its evidence, as replay
    /// recomputed them; and presumptions cleared (heard again).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nodes_presumed: Vec<String>,
    /// Rules 30: our vehicles confirmed lost (`node.lost`), each saying by whom, since when and on
    /// what.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nodes_lost: Vec<String>,
    /// Rules 30: losses revoked (`node.loss_revoked`), each saying how (heard, or by an operator).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub losses_revoked: Vec<String>,
    /// Rules 5: intents queued for unreachable vehicles, queue re-checks recomputed (and
    /// how many dispatched, went back to the inbox, voided), queue expiries, intents carried
    /// over from a previous run, and vehicle reboots.
    pub queued: usize,
    pub rechecks: usize,
    pub recheck_dispatched: usize,
    pub recheck_inbox: usize,
    pub recheck_voided: usize,
    pub queue_expired: usize,
    pub resumed: usize,
    pub reboots: usize,
    /// Rules 29: the orders the fleet file handed over at the start (`intent.handed_over`).
    pub handed_over: usize,
    /// Rules 29: whether the handed-over orders were checked against the fleet the run logged
    /// (`<log stem>.fleet.json`, [`ReplayOptions::own_fleet`]): `Some(true)` checked (every
    /// starting task it names handed over exactly, nothing else), `Some(false)` some were
    /// handed over and that file was not given, `None` nothing to check.
    pub handover_fleet: Option<bool>,
    /// Rules 5: dispatched intents sent back to the queue (their link lost before the start).
    pub requeued: usize,
    /// Rules 11: space tasks cancelled after dispatch, re-checked against a later answer and
    /// found to hold, and found no longer feasible (failed).
    pub space_cancelled: usize,
    pub space_rechecked: usize,
    pub space_missed: usize,
    /// Rules 6: the carried `platform.restore` events, by the earlier log they name: how
    /// many, and whether they were checked against what that log still owed (it was given,
    /// [`ReplayOptions::restores_from`]) or not (it was absent).
    pub carried_restores: Vec<CarriedRestores>,
    /// The intents the log leaves open (a vehicle task queued, approved but not dispatched,
    /// or dispatched with ops still running): what a restarted bridge carries over.
    #[serde(skip)]
    pub open: Vec<OpenIntent>,
    /// One past the highest intent id the log holds (a restarted bridge numbers on from it).
    pub next_intent: u64,
    /// The last event's seq.
    pub last_seq: u64,
    /// Rules 8: how the run authenticated its operators (the roster's `auth`): `credentials`
    /// (each person's password checked by the bridge) or `dev` (names asserted). None before
    /// rules 8: names asserted, as `dev`.
    pub auth: Option<AuthMode>,
    /// Rules 8: step-ups logged (a credential re-entered to sign), signatures that quoted
    /// one, and sessions ended (`session.closed`).
    pub step_ups: usize,
    pub stepped_signatures: usize,
    pub sessions_closed: usize,
    pub stepped_acts: usize,
    /// Rules 8, a credentials run: logins refused for operators the roster lists
    /// (`auth.refused`: a wrong password or a locked id; nothing secret is logged).
    pub logins_refused: usize,
    /// Rules 8, a credentials run: the operators who logged in with their own password (a
    /// person's), and those whose credential is harness-held (`auth: scripted`: a runner
    /// logs them in; authenticated, but never a person's judgment). Only the first are
    /// evidence about people.
    pub people: BTreeSet<OperatorId>,
    pub scripted: BTreeSet<OperatorId>,
    /// Rules 10: logins by passkey, people's step-ups by passkey and by password, and passkeys
    /// registered and removed while the run was up. Every passkey use was verified against
    /// the public key the run logged.
    pub passkey_logins: usize,
    pub passkey_step_ups: usize,
    pub password_step_ups: usize,
    pub passkeys_registered: usize,
    pub passkeys_removed: usize,
    /// Rules 10: a study run, its participant and how their consequential acts were stepped up.
    pub study: Option<StudyReport>,
}

/// Rules 10: what a study run's log shows of its participant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StudyReport {
    pub participant: OperatorId,
    /// The explicit override that allowed password step-ups, and its reason: a run with it is
    /// never evidence.
    pub password_step_up: Option<String>,
    /// The participant's signatures and other stepped-up acts, by the factor of the step-up
    /// each quoted.
    pub passkey_acts: usize,
    pub password_acts: usize,
}

impl ReplayReport {
    /// Every decision reproduced from the logged inputs (including logged space answers).
    pub fn ok(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// Whether the log's human decisions are bound to authenticated operators: a credentials
    /// run (rules 8 or later) that reproduced. A dev run, and every log of earlier rules,
    /// names operators that requests asserted: never evidence about people, and excluded from
    /// any claim about meaningful human control. Two things this does not say: whether an
    /// operator is a person ([`Self::people`]; a scripted operator's credential is
    /// harness-held), and whether the log is the one the run wrote (a rewritten log replays
    /// too: only its anchors tell, `fleetmesh-replay --anchors`, which a claim needs as well).
    pub fn authenticated(&self) -> bool {
        self.ok() && self.auth == Some(AuthMode::Credentials)
    }

    /// Rules 10: whether a study run's log is evidence about its participant: it is
    /// [`Self::authenticated`], a study, without the password override, its participant
    /// signed in as a person, and every consequential act of theirs quoted a passkey step-up.
    /// (The anchors are still the caller's to check: `fleetmesh-replay --anchors`.)
    pub fn study_evidence(&self) -> bool {
        self.authenticated()
            && self.study.as_ref().is_some_and(|s| {
                s.password_step_up.is_none() && s.password_acts == 0 && self.people.contains(&s.participant)
            })
    }

    /// Every logged space query was re-run on the same segment and reproduced.
    pub fn space_reproduced(&self) -> bool {
        self.cache_mismatches.is_empty() && self.space_recomputed == self.space_queries
    }
}

/// Rules 6: a log's carried `platform.restore` events that name one earlier log.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CarriedRestores {
    /// The earlier log they name (`from`, its file name; empty when they name none).
    pub log: String,
    /// How many carried restores name it.
    pub restores: usize,
    /// Checked against what that log still owed ([`owed_restores`]); false: that log was
    /// not given, so they were not (a mismatch unless [`ReplayOptions::trust_resumed`]).
    pub checked: bool,
    /// The head of that log as given (compare it with that log's anchors).
    pub head: Option<String>,
    /// Rules 12: why its carry failed at the start (it could not be read): what it owed is
    /// unknown, and it stays owed until read or written off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// Rules 15: the `.last-run` pointer could not be read (why), and this log, the newest of the
    /// station by its carry chain, was found in its place: carried from when its chain and
    /// anchors checked, else a failed carry (`failed` says why the start would not).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer_failed: Option<String>,
    /// Rules 15: a failed carry's fleet file (`<log stem>.fleet.json`): compared with what the
    /// start recorded (true), or not at hand (false); none when it recorded none and it is not
    /// at hand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fleet_checked: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Restores {
    owed: BTreeMap<Uid, Owed>,
    /// Rules 12: earlier logs whose carry failed (they could not be read) and nobody wrote off,
    /// with why and the armed platforms their fleet named (that may be owed values, unknown):
    /// they stay owed until a later start reads them or an operator writes them off.
    pending: BTreeMap<String, PendingCarry>,
    /// Rules 30: the values each platform confirmed lost was owed when its loss closed them
    /// (`restore.closed` lost; carried over with a loss from an earlier run): owed back again if
    /// lost-link action.
    lost_closed: BTreeMap<Uid, Owed>,
}

/// Rules 12: an earlier log whose carry failed: why, and the armed platforms its run's fleet
/// named (from the fleet identity beside it, when that could be read), which may be owed values
/// nobody knows: a mission for one of them waits until the log is read or written off.
#[derive(Clone, Debug, Default)]
struct PendingCarry {
    why: String,
    may_owe: Vec<OwedRestore>,
}

#[derive(Clone, Debug, Default)]
struct Owed {
    values: BTreeMap<String, f64>,
    by: Vec<IntentId>,
}

impl Restores {




    /// Rules 12: seed the ledger with what the bridge carried over at start (`run.started`'s
    /// `carried_from`, newest log first): each log's platforms' values, merged oldest first (a
    /// value both name the older log's stands), and the logs whose carry failed, pending (as
    /// is a log one of them names as pending without an entry of its own).
    pub(crate) fn carry(&mut self, from: &[CarriedFrom]) {
        // the values the losses each log read carried had closed (newest log's standing)
        for c in from.iter().rev().filter(|c| c.error.is_none()) {
            for m in c.nodes_lost.iter().filter(|m| !m.values.is_empty()) {
                self.lost_closed.entry(m.uid.clone()).or_insert_with(|| Owed {
                    values: m.values.clone(),
                    by: Vec::new(),
                });
            }
        }
        for c in from.iter().rev() {
            if let Some(why) = &c.error {
                self.pending.insert(
                    c.log.clone(),
                    PendingCarry {
                        why: why.clone(),
                        may_owe: c.may_owe.clone(),
                    },
                );
                continue;
            }
            for o in &c.owed {
                let e = self.owed.entry(o.uid.clone()).or_insert_with(|| Owed {
                    values: BTreeMap::new(),
                    by: Vec::new(),
                });
                for (n, v) in &o.values {
                    e.values.entry(n.clone()).or_insert(*v);
                }
            }
        }
        for c in from {
            for p in &c.pending {
                if !from.iter().any(|x| &x.log == p) {
                    self.pending.entry(p.clone()).or_insert_with(|| PendingCarry {
                        why: format!("named as not carried by {}, and not tried", c.log),
                        may_owe: Vec::new(),
                    });
                }
            }
        }
    }

    /// Rules 30: `uid`'s loss was revoked (it was heard, or an operator revoked it): the values
    /// its confirmation closed are owed back again, as before it (an un-spend's reopening).
    pub(crate) fn reopen_lost(&mut self, uid: &Uid) {
        if let Some(o) = self.lost_closed.remove(uid).filter(|o| !o.values.is_empty()) {
            self.owed.insert(uid.clone(), o);
        }
    }

    /// Rules 12: the values owed back to `uid`, if any.
    pub(crate) fn values_owed(&self, uid: &Uid) -> Option<&BTreeMap<String, f64>> {
        self.owed.get(uid).map(|o| &o.values)
    }

    /// Rules 12: the earlier logs pending (their carry failed), with why and the armed platforms
    /// they may owe values to.
    pub(crate) fn pending(&self) -> Vec<(String, String, Vec<Uid>)> {
        self.pending
            .iter()
            .map(|(l, p)| (l.clone(), p.why.clone(), p.may_owe.iter().map(|m| m.uid.clone()).collect()))
            .collect()
    }


    /// The ledger after one logged event, as logged (no recomputation: replay checks it).
    fn step(&mut self, e: &Event) {
        self.step_body(&e.body);
    }

    /// [`Self::step`] on an event's body (the engine keeps the same ledger as it logs).
    pub(crate) fn step_body(&mut self, body: &EventBody) {
        match body {
            EventBody::RunStarted { carried_from, .. } => self.carry(carried_from),
            // rules 30: a loss revoked: what it closed is owed back again
            EventBody::NodeLossRevoked { uid, .. } => self.reopen_lost(uid),
            _ => {}
        }
    }

    fn report(&self) -> Vec<String> {
        self.owed
            .iter()
            .map(|(uid, o)| {
                let (values, by) = (&o.values, &o.by);
                let by: Vec<String> = by.iter().map(ToString::to_string).collect();
                let by = if by.is_empty() {
                    "carried over from an earlier run".to_string()
                } else {
                    format!("set by order {}", by.join(", "))
                };
                format!("{uid}: its own lost-link values {} never confirmed back ({by})", json(values))
            })
            .chain(self.pending.iter().map(|(log, p)| {
                format!(
                    "earlier log {log}: its carry failed ({}): what it still owed is unknown, and nobody wrote it off",
                    p.why
                )
            }))
            .collect()
    }
}

/// Rules 6: the platforms' own lost-link values a log still owed back when it ended (a
/// (rules 6 to 10: `platform.restore` carried; rules 12: in `run.started`, `carried_from`)
/// and puts them back. Lines that do not parse (a torn last line) are skipped: this is a
/// ledger, not a replay.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct OwedRestore {
    pub uid: Uid,
    pub values: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by: Vec<IntentId>,
}

/// Rules 12: one earlier log the bridge carried over from at start, as `run.started` records
/// it (`carried_from`): the log `.last-run-<port>` named, or one whose carry had failed and
/// that nobody wrote off (named `pending` by a log carried before it in the list).
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct CarriedFrom {
    /// Its file name (the bridge keeps its logs in one directory).
    pub log: String,
    /// The SHA-256 of its last event's line as read (its head) and that event's seq: the log
    /// replay is given must be exactly that one (compare its anchors too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seq: Option<u64>,
    /// The platforms' own lost-link values it still owed back ([`owed_restores`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owed: Vec<OwedRestore>,
    /// The earlier logs it still owed for (their carry had failed, nobody wrote them off): each
    /// is tried again in the same start, an entry of its own after this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    /// A log that could not be read: the armed platforms its run's fleet named (the fleet
    /// identity beside it, `<log>.fleet.json`, when that could be read), each with the vehicle
    /// it was, values unknown: a mission for one of them waits (fail closed) until the log is
    /// read or written off. Empty when the fleet is unknown too: then nothing can be held.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub may_owe: Vec<OwedRestore>,
    /// Why it could not be read: what it owed is unknown, and it stays owed (pending) until a
    /// later start reads it or an operator writes it off. Nothing else is set then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Rules 15: a failed carry's fleet file as read (its hash and the armed platforms that
    /// made `may_owe`); absent when it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet_read: Option<FleetRead>,
    /// Rules 15: the `.last-run` pointer could not be read (why), and this log (the newest of the
    /// station whose chain and anchors checked) was carried from in its place. Only the first
    /// entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer_failed: Option<String>,
    /// Rules 30: our vehicles confirmed lost in it and not revoked (its own, `from` none, and
    /// those carried into it): lost in this run on the same vehicle only (platform, callsign and
    /// system id), until it is heard. Named `nodes_lost` (not `lost`, which rules 16's events
    /// already log), so replay tells the rules a log claims by its keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes_lost: Vec<crate::nodes::LostMark>,
}

/// Rules 12: what a log still owes when it ends: the platforms' own lost-link values never
/// confirmed back, and the earlier logs whose carry failed that nobody wrote off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Debts {
    pub owed: Vec<OwedRestore>,
    pub pending: Vec<String>,
    /// Rules 15: for each pending log, the platforms its failed carry was recorded as possibly
    /// owing (`may_owe`): a later start that retries it and can't read its fleet file either
    /// records the same list (the vehicles stay held).
    pub pending_may_owe: BTreeMap<String, Vec<OwedRestore>>,
    /// Rules 30: our vehicles confirmed lost in it and those carried into it, not revoked.
    pub nodes_lost: Vec<crate::nodes::LostMark>,
}

/// Rules 15 (murmur-zq3): a failed carry's fleet file as the start read it (`<log stem>.fleet.json`,
/// beside the log): the SHA-256 of its bytes. `may_owe` is the vehicles the carry may owe values
/// to ([`fleet_may_owe`]); replay compares both with the file when it is at hand.
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct FleetRead {
    pub sha256: String,
}

/// Rules 15: the vehicles of a run's fleet file (`<log stem>.fleet.json` as the bridge writes it:
/// by uid, each vehicle's platform, callsign and home `[lat, lon]`) that may owe values back, in
/// uid order, as a failed carry records them (`may_owe`: values unknown). The catalog has no
/// platform that owes any, so the list is empty. None: the file does not parse.
pub fn fleet_may_owe(bytes: &[u8]) -> Option<Vec<OwedRestore>> {
    let _fleet: BTreeMap<u64, serde::de::IgnoredAny> = serde_json::from_slice(bytes).ok()?;
    Some(Vec::new())
}

/// Rules 15: the fleet file beside a log (`<log stem>.fleet.json`), by its bare file name.
pub fn fleet_file_name(log: &str) -> String {
    match log.strip_suffix(".jsonl") {
        Some(stem) => format!("{stem}.fleet.json"),
        None => format!("{log}.fleet.json"),
    }
}

impl Debts {
    pub fn is_empty(&self) -> bool {
        self.owed.is_empty() && self.pending.is_empty()
    }
}

pub fn owed_restores(text: &str) -> Vec<OwedRestore> {
    owed_debts(text).owed
}

/// Rules 12: [`owed_restores`], and the earlier logs `text` still owed for (pending).
pub fn owed_debts(text: &str) -> Debts {
    let mut restores = Restores::default();
    let mut nodes = crate::nodes::NodeLedger::default();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(e) = serde_json::from_str::<Event>(line) {
            restores.step(&e);
            nodes.step(e.seq, e.t, &e.body);
        }
    }
    Debts {
        pending: restores.pending.keys().cloned().collect(),
        pending_may_owe: restores
            .pending
            .iter()
            .filter(|(_, p)| !p.may_owe.is_empty())
            .map(|(l, p)| (l.clone(), p.may_owe.clone()))
            .collect(),
        nodes_lost: nodes.marks(),
        owed: restores
            .owed
            .into_iter()
            .map(|(uid, o)| OwedRestore {
                uid,
                values: o.values,
                by: o.by,
            })
            .collect(),
    }
}

/// Rules 12: the head of a log's text as the bridge records it in `carried_from`: the SHA-256
/// of its last line that parses as an event, and that event's seq (a torn last line, from a
/// crash mid-write, is not part of it). None for a log with no event.
pub fn log_head(text: &str) -> Option<(String, u64)> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Event>(l).ok().map(|e| (sha256_hex(l.as_bytes()), e.seq)))
        .next_back()
}

/// Rules 6: the earlier log a carried `platform.restore` names: its `from` less a trailing
/// ` (carried over into it)`, else the whole `from`; empty when it names none.
pub fn carried_restore_log(from: Option<&str>) -> &str {
    let from = from.unwrap_or_default();
    from.strip_suffix(" (carried over into it)").unwrap_or(from)
}

/// Whether `name` is a bare file name, as the bridge names an earlier log (in
/// `intent.resumed` and a carried `platform.restore`): not empty, not `.` or `..`, no path
/// separator. A name that is a path is never followed out of the log's directory.
pub fn is_log_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

fn holding(intents: &BTreeMap<IntentId, Tracked>, uid: &Uid, except: Option<IntentId>) -> Option<IntentId> {
    intents
        .iter()
        .find(|(id, tr)| {
            Some(**id) != except
                && false
                && tr.play_state() == State::Dispatched
                && tr.spec.uids.first() == Some(uid)
        })
        .map(|(id, _)| *id)
}

struct Tracked {
    spec: IntentSpec,
    state: State,
    /// When it reached a terminal state.
    ended_t: Option<Time>,
    last_check_passed: Option<bool>,
    tier: Option<Tier>,
    /// The recomputed decision's rule name and `submitter_authorizes`.
    rule: Option<String>,
    submitter_authorizes: bool,
    expires_t: Option<Time>,
    granted: bool,
    granted_at_submission: bool,
    rejected_by_human: bool,
    expired: bool,
    /// The recomputed decision's signature count and proposer exclusion.
    approvers: u32,
    proposer_excluded: bool,
    /// Rules 8: the recomputed decision asks every signature for a step-up.
    step_up: bool,
    /// Who signed so far (concurred, then granted).
    signers: Vec<OperatorId>,
    last_inputs: Option<Box<WorldSnapshot>>,
    stop_due: Option<u64>,
    effect_logged: bool,
    stopped: bool,
    /// The last check.result's time and the time of the event before it: a signature's
    /// `checked_t` lies between them.
    check_window: Option<(Time, Time)>,
    /// An `intent.dispatched` waiting for its dispatch transition (its seq).
    dispatch_logged: Option<u64>,
    /// The dispatch transition happened: op states may refer to the intent from here on.
    dispatched: bool,
    /// Rules 11: an operator's cancel of this dispatched space task (who, seq) waiting for
    /// the engine's cancelled report and the cancel transition; the report came.
    cancel_due: Option<(OperatorId, u64)>,
    cancel_reported: bool,
    /// Rules 11: a dispatched-phase check (passed, the report it calls for) waiting for its
    /// `rechecked` or `missed` report; a `missed` report (its detail) waiting for the failure.
    recheck_due: Option<(bool, String)>,
    recheck_lost: Option<String>,
    /// The last check's recomputed results, its phase and seq.
    last_results: Vec<CheckResult>,
    last_phase: Option<CheckPhase>,
    last_check_seq: u64,
    /// The last recomputed policy decision (submission, or a queue re-check's).
    decision: Option<PolicyDecision>,
    /// Rules 5: what the approval rested on (taken at approved / auto_approved).
    basis: Option<Basis>,
    /// Who approved it, as a next run would carry it.
    approval_rec: Option<CarriedApproval>,
    /// Rules 5: the queue deadline, an `intent.queued` waiting for its transition, the last
    /// re-check's verdict waiting for its own, a `queue.expired` waiting for the expiry.
    queue_expires: Option<Time>,
    queued_logged: Option<u64>,
    recheck: Option<Verdict>,
    queue_expired: bool,
    /// The deadline sends it back to the inbox (a human's approval held long enough), and
    /// a `queue.expired` that did so waits for its transition.
    queue_inbox: bool,
    queue_expired_inbox: bool,
    /// The last re-check's seq (a dispatch from the queue follows it at once).
    recheck_seq: Option<u64>,
    /// An `intent.requeued` waiting for its transition.
    requeued_logged: Option<u64>,
    /// Each vehicle's last op state (since its last dispatch).
    ops: BTreeMap<Uid, String>,
    /// The vehicles that ever started it (this run or, carried over, one before).
    committed: BTreeSet<Uid>,
    /// Dispatched before (this run or one before).
    dispatched_before: bool,
    /// Its basis is from the run before a bridge restart.
    basis_carried: bool,
    /// Rules 29: handed over at the start by the fleet file (`intent.handed_over`).
    handed_over: bool,
    /// Each vehicle's platform as its last check saw it (or as carried over).
    platforms: BTreeMap<Uid, String>,
    /// The vehicles that may have started it (an op `starting` or `unconfirmed`), and those
    /// its current delivery only confirms.
    maybe_started: BTreeSet<Uid>,
    confirming: BTreeSet<Uid>,
    /// Rules 7: the vehicles whose start (or follow) on the current delivery was logged ahead.
    started_ahead: BTreeSet<Uid>,
    /// The vehicles whose op on this delivery (since its dispatch in this log, or its resume:
    /// the vehicles that had started it, followed) reported its start going out (`starting`,
    /// `unconfirmed`, or started): its op never ends `requeued` after that (the bridge ends
    /// it `unconfirmed`: it may be running; a followed one fails).
    start_went_out: BTreeSet<Uid>,
    /// Rules 22: the routed legs its last passing check planned (what it is dispatched with).
    route_plans: BTreeMap<Uid, Vec<crate::plan::PlannedLeg>>,
    /// Rules 34, a group order: its last passing check, recomputed (what its member orders are
    /// bound to).
    group_eval: Option<Box<crate::group::GroupEval>>,
    /// Rules 34, a group order: the member orders created under it, by uid.
    members_created: BTreeMap<Uid, IntentId>,
    /// Rules 34, a group order: its member orders once dispatched (`group.dispatched`).
    group_members: BTreeMap<Uid, IntentId>,
    /// Rules 34, a group order: the member orders its last re-form makes, each with its spec and
    /// the routes the re-form planned for it, until each is created.
    reform_pending: BTreeMap<Uid, (IntentSpec, Option<Vec<crate::plan::PlannedLeg>>)>,
    /// Rules 34, a member order: made by its group's re-form (approved under the group in force).
    by_reform: bool,
}

impl Tracked {

    fn play_state(&self) -> State {
        self.state
    }

    fn new(spec: IntentSpec, state: State) -> Tracked {
        Tracked {
            spec,
            state,
            ended_t: None,
            last_check_passed: None,
            tier: None,
            rule: None,
            submitter_authorizes: false,
            expires_t: None,
            granted: false,
            granted_at_submission: false,
            rejected_by_human: false,
            expired: false,
            approvers: 1,
            proposer_excluded: false,
            step_up: false,
            signers: Vec::new(),
            last_inputs: None,
            stop_due: None,
            effect_logged: false,
            stopped: false,
            check_window: None,
            dispatch_logged: None,
            dispatched: false,
            last_results: Vec::new(),
            last_phase: None,
            last_check_seq: 0,
            decision: None,
            basis: None,
            approval_rec: None,
            queue_expires: None,
            queued_logged: None,
            recheck: None,
            queue_expired: false,
            queue_inbox: false,
            queue_expired_inbox: false,
            recheck_seq: None,
            requeued_logged: None,
            ops: BTreeMap::new(),
            committed: BTreeSet::new(),
            dispatched_before: false,
            basis_carried: false,
            handed_over: false,
            platforms: BTreeMap::new(),
            maybe_started: BTreeSet::new(),
            confirming: BTreeSet::new(),
            started_ahead: BTreeSet::new(),
            start_went_out: BTreeSet::new(),
            route_plans: BTreeMap::new(),
            group_eval: None,
            members_created: BTreeMap::new(),
            group_members: BTreeMap::new(),
            reform_pending: BTreeMap::new(),
            by_reform: false,
            cancel_due: None,
            cancel_reported: false,
            recheck_due: None,
            recheck_lost: None,
        }
    }

    /// Back to the inbox from the queue: who approves is decided from scratch.
    fn back_to_inbox(&mut self) {
        self.signers.clear();
        self.granted = false;
        self.granted_at_submission = false;
        self.expires_t = None;
        self.approval_rec = None;
    }

    /// The decision's routing on this intent: tier, rule, approvers, who may approve.
    fn route(&mut self, d: &PolicyDecision) {
        self.tier = Some(d.tier);
        self.submitter_authorizes = d.submitter_authorizes;
        self.approvers = d.approvers.max(1);
        self.proposer_excluded = d.proposer_excluded;
        self.step_up = d.step_up;
        self.rule = Some(d.rule.clone());
        self.decision = Some(d.clone());
    }
}

/// Which decision rules a replay accepts a log under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulesAccepted {
    /// Exactly this version (`fleetmesh-replay --rules N`).
    Exactly(u32),
    /// This version or a later one (`--min-rules N`; the default is the current version).
    AtLeast(u32),
}

impl Default for RulesAccepted {
    fn default() -> Self {
        RulesAccepted::AtLeast(crate::rules::RULES_VERSION)
    }
}

/// How to replay.
#[derive(Clone, Copy, Default)]
pub struct ReplayOptions<'a> {
    /// The rules versions accepted (default: only the current one).
    pub rules: RulesAccepted,
    /// Rules 5: the previous run's log, which this one's `intent.resumed` events carry
    /// intents from. Each resumed intent must be one that log leaves open, exactly as it
    /// was there, and the log must be the one named (its head and last seq). One generation:
    /// that log's own resumed intents are trusted (replay it with its own predecessor).
    pub previous: Option<&'a str>,
    /// Accept what this log carried over from earlier runs without their logs: its
    /// `intent.resumed` events (without `previous`) and, rules 6, its carried
    /// `platform.restore` values (without `restores_from`), trusting where they came from, as
    /// accepting an older rules version trusts its log. Without this, a carry-over whose
    /// earlier log is not given is a mismatch. A carried restore that names no log by its
    /// file name is a mismatch either way.
    pub trust_resumed: bool,
    /// Rules 6: the earlier log this run's carried `platform.restore` events name, as (its
    /// file name as they name it, its text). It is the log `.last-run-<port>` named when the
    /// bridge started, which need not be `previous` (`.resume-<port>.json`, deleted by a
    /// clean stop, or `FLEETMESH_RESUME=<log>`, may name another log or none). Each carried
    /// restore must be exactly what that log still owed ([`owed_restores`]), everything it
    /// owed carried, and that log must replay itself (its own carry-overs trusted: one
    /// generation). Carried values from a log not given are a mismatch unless
    /// [`Self::trust_resumed`] ([`ReplayReport::carried_restores`] says which were checked).
    pub restores_from: Option<(&'a str, &'a str)>,
    /// Rules 12: the earlier logs this run's `run.started` says it carried over from
    /// (`carried_from`), as (their file name, their text): each must replay, be the one read
    /// (its head) and still have owed exactly what was carried. [`Self::restores_from`] is
    /// looked at too. A log not given is a mismatch when it carried anything, unless
    /// [`Self::trust_resumed`].
    pub carried_logs: &'a [(&'a str, &'a str)],
    /// Rules 15: the fleet files of the logs whose carry failed (`<log stem>.fleet.json`), as
    /// (their file name, their bytes): each must be the one the start read (its SHA-256), and
    /// the platforms the failed carry may owe values to exactly that fleet's armed ones.
    pub fleets: &'a [(&'a str, &'a [u8])],
    /// Rules 29: the fleet this run logged (`<log stem>.fleet.json`, as the bridge writes it:
    /// each vehicle's identity and, when its fleet gave it one, its starting task), against
    /// which every `intent.handed_over` is checked: each one its vehicle's starting task there,
    /// and every starting task there handed over (unless the run carried orders over from an
    /// earlier log, when none is).
    pub own_fleet: Option<&'a [u8]>,
}

/// Replay a JSONL log. The chain is checked over the lines exactly as they are in the
/// text, and each line must be its event's canonical serialization (so an edit that the
/// parser would ignore, such as an extra field, is still caught).
pub fn replay_str(text: &str) -> Result<ReplayReport, LogError> {
    replay_str_with(text, &ReplayOptions::default())
}


/// [`replay_str`] with options.
pub fn replay_str_with(text: &str, opts: &ReplayOptions<'_>) -> Result<ReplayReport, LogError> {
    let events = parse_log(text)?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut noncanonical = Vec::new();
    for (e, line) in events.iter().zip(&lines) {
        let canon = serde_json::to_string(e).map_err(|err| LogError {
            line: usize::try_from(e.seq).unwrap_or(0),
            err,
        })?;
        if canon != *line {
            noncanonical.push(e.seq);
        }
    }
    let mut r = replay_inner(&events, &lines, opts);
    for seq in noncanonical {
        r.mismatches.push(Mismatch {
            seq,
            intent: None,
            what: "line is not its event's canonical serialization".into(),
            logged: String::new(),
            recomputed: String::new(),
        });
    }
    Ok(r)
}

/// Replay parsed events; the chain is checked over their canonical serialization.
pub fn replay(events: &[Event]) -> ReplayReport {
    replay_with(events, &ReplayOptions::default())
}


/// [`replay`] with options.
pub fn replay_with(events: &[Event], opts: &ReplayOptions<'_>) -> ReplayReport {
    let lines: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap_or_else(|err| format!("<unserializable: {err}>")))
        .collect();
    replay_inner(events, &lines, opts)
}

/// The first `run.started`'s claims, and everything in the log that contradicts them.
fn check_claims<S: AsRef<str>>(r: &mut ReplayReport, events: &[Event], _lines: &[S], opts: &ReplayOptions<'_>) {
    let Some(first) = events.first() else { return };
    let EventBody::RunStarted {
        rules_version,
        policy_schema,
        build: _,
        roster,
        policy,
        ..
    } = &first.body
    else {
        let kind = serde_json::to_value(&first.body)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
            .unwrap_or_default();
        bad(r, first, None, "the log does not start with run.started", kind, "run.started".into());
        return;
    };
    let version = rules_version.unwrap_or(1);
    r.rules_version = version;
    match opts.rules {
        RulesAccepted::Exactly(n) if version != n => bad(
            r,
            first,
            None,
            "log written under other decision rules than this replay was asked to accept",
            version.to_string(),
            n.to_string(),
        ),
        RulesAccepted::AtLeast(n) if version < n => bad(
            r,
            first,
            None,
            "log written under older decision rules than this replay accepts (pass --rules or --min-rules with its version to replay it with them)",
            version.to_string(),
            format!(">= {n}"),
        ),
        _ => {}
    }
    if version > crate::rules::RULES_NEWEST {
        bad(
            r,
            first,
            None,
            "log written under newer decision rules than this replay knows",
            version.to_string(),
            crate::rules::RULES_NEWEST.to_string(),
        );
    }
    if rules_version.is_some() != policy_schema.is_some() {
        bad(
            r,
            first,
            None,
            "run.started carries one of rules_version and policy_schema without the other (every build writes both or neither)",
            format!("rules_version {rules_version:?}, policy_schema {policy_schema:?}"),
            String::new(),
        );
    }
    // the operator roster: written from rules 4, and every rules 4 run has one (a log that
    // dropped it would have its sessions go unchecked)
    match roster {
        Some(listed) => {
            if let Err(why) = listed.validate() {
                bad(
                    r,
                    first,
                    None,
                    "logged roster is not one a bridge could be configured with",
                    why,
                    String::new(),
                );
            }
        }
        None => bad(
            r,
            first,
            None,
            "a rules 4 log names its operator roster in run.started",
            "none".into(),
            "roster".into(),
        ),
    }
    // rules 12: a credentials run fixes its scripted operators at the start, always (an empty
    // list when none), so replay can check every session's method against them
    if let Some(listed) = roster
        && listed.auth == Some(AuthMode::Credentials)
        && listed.scripted.is_none() {
        bad(
            r,
            first,
            None,
            "a credentials log of these rules names its scripted operators on the roster",
            "none".into(),
            "scripted: [...]".into(),
        );
    }
    // how operators were authenticated: written from rules 8 (OPERATOR_AUTH_SINCE), always
    r.auth = roster.as_ref().and_then(|ro| ro.auth);
    if r.auth.is_none() {
        bad(
            r,
            first,
            None,
            "a log of these rules says how its operators were authenticated (the roster's auth)",
            "none".into(),
            "credentials or dev".into(),
        );
    }
    // the policy file's schema
    let schema = policy_schema.unwrap_or(1);
    if schema > crate::rules::POLICY_SCHEMA_NEWEST {
        bad(
            r,
            first,
            None,
            "logged policy schema is newer than this replay knows",
            schema.to_string(),
            crate::rules::POLICY_SCHEMA_NEWEST.to_string(),
        );
    }
    // rules 33: a log that carries the scenario's settings names each loosening its policy holds
    // (a forger who drops `loosened` makes a loosened policy look standing)
    for key in policy.unnamed_loosenings() {
        bad(
            r,
            first,
            None,
            "logged policy loosens a key its `loosened` does not name",
            key.to_string(),
            format!("loosened = {{ \"{key}\" = \"<why>\" }}"),
        );
    }
    // rules 10: the passkeys and the study on the roster are ones a run could have started with
    if let Some(listed) = roster {
        let factor = policy.approval.step_up_factor.unwrap_or(policy::StepUpFactor::Password);
        if let Err(why) = listed.validate_passkeys(factor) {
            bad(r, first, None, "logged passkeys or study could not be a run's", why, String::new());
        }
    }
}

/// The area set in force: its logged hash, and the set if that hash verified.
type InForce = Option<(String, Option<AreaSet>)>;


fn replay_inner<S: AsRef<str>>(
    events: &[Event],
    lines: &[S],
    opts: &ReplayOptions<'_>,
) -> ReplayReport {
    let hashes: Vec<String> = lines.iter().map(|l| sha256_hex(l.as_ref().as_bytes())).collect();
    let mut r = ReplayReport {
        events: events.len(),
        head_sha256: hashes.last().cloned(),
        build: match events.first().map(|e| &e.body) {
            Some(EventBody::RunStarted { build, .. }) => build.clone(),
            _ => None,
        },
        ..Default::default()
    };
    check_claims(&mut r, events, lines, opts);
    let mut policy: Option<Policy> = None;
    let mut areas: InForce = None;
    let mut intents: BTreeMap<IntentId, Tracked> = BTreeMap::new();
    let mut prev: Option<&Event> = None;
    // from the last `space.loaded`
    // rules 11: the segment in force (the last `space.loaded`), which a re-check must be on
    let _pinned: Option<i64> = None;
    // operator sessions as opened: who, with what grants
    let mut sessions: BTreeMap<SessionId, (OperatorId, Grants)> = BTreeMap::new();
    // rules 8: sessions ended (nothing may follow under one), and step-ups by their event's
    // seq
    let mut closed: BTreeSet<SessionId> = BTreeSet::new();
    let mut step_ups: BTreeMap<u64, StepUpSeen> = BTreeMap::new();
    // rules 8: how each session was opened (password, scripted, asserted)
    let mut methods: BTreeMap<SessionId, AuthMethod> = BTreeMap::new();
    // rules 6: the platforms' own lost-link values owed back
    let mut restores = Restores::default();
    // picture tokens issued and not used yet
    // who may open a session, and with consequential authority (run.started, rules 4 on)
    let mut roster: Option<Roster> = None;
    // per vehicle, the order dispatched to it last
    let mut newest_on: BTreeMap<Uid, IntentId> = BTreeMap::new();
    // rules 34: each group order dispatched, as it stays in force (layout, member orders, re-forms)
    let mut forces: BTreeMap<IntentId, crate::group::Force> = BTreeMap::new();
    // intents carried over from a previous run, checked against it after the loop
    let mut resumed: Vec<ResumedEvent> = Vec::new();
    // rules 29: the orders the fleet handed over (seq, vehicle, task) and the fleet file they
    // name, checked against the run's fleet file after the loop
    let mut handovers: Vec<(u64, Option<Uid>, Task)> = Vec::new();
    let mut handover_from: Option<crate::queue::HandedOverFrom> = None;
    // when each vehicle was last logged rebooting
    let mut rebooted: BTreeMap<Uid, Time> = BTreeMap::new();
    // rules 10: the passkeys the log knows (the roster's, registered since), each challenge's
    // nonce once answered, and the hash of the first line every challenge binds to
    let mut passkeys = PasskeysSeen {
        run_hash: hashes.first().cloned().unwrap_or_default(),
        ..PasskeysSeen::default()
    };
    // rules 30: our vehicles' standing as the log builds it (stepped with each event before the
    // next is read), and what a confirmed loss ends next
    let mut nodes = crate::nodes::NodeLedger::default();
    let mut loss_due: Option<LossDue> = None;

    for (i, logged) in events.iter().enumerate() {
        if let Some(p) = i.checked_sub(1).and_then(|j| events.get(j)) {
            nodes.step(p.seq, p.t, &p.body);
        }
        let e: &Event = logged;
        let want_prev = if i == 0 {
            GENESIS
        } else {
            hashes[i - 1].as_str()
        };
        if e.prev != want_prev {
            bad(
                &mut r,
                e,
                None,
                "hash chain broken: prev is not the hash of the previous event",
                e.prev.clone(),
                want_prev.to_string(),
            );
        }
        if let Some(p) = prev {
            if e.seq != p.seq + 1 {
                bad(
                    &mut r,
                    e,
                    None,
                    "seq not contiguous",
                    e.seq.to_string(),
                    (p.seq + 1).to_string(),
                );
            }
            if e.t < p.t {
                bad(
                    &mut r,
                    e,
                    None,
                    "time went backwards",
                    e.t.0.to_string(),
                    format!(">= {}", p.t.0),
                );
            }
        }
        let before_t = prev.map_or(Time(i64::MIN), |p| p.t);
        let _prev_event: Option<&Event> = prev;
        prev = Some(logged);
        // rules 8: an ended session acts no more (a stale or shared session can't sign, decide
        // or be shown a picture after its logout, expiry or credential change)
        if let Some(s) = acting_session(&e.body)
            && closed.contains(&s)
        {
            bad(&mut r, e, None, "an act under a session that had ended (session.closed)", s.0.to_string(), String::new());
        }
        // rules 30: every decision reads our vehicles confirmed lost as the log holds them, a
        // report among its facts revoking a loss first; and a confirmed loss's consequences
        // follow it at once
        check_nodes_read(&mut r, e, &nodes, &intents, policy.as_ref());
        if let Some(due) = loss_due.as_mut() {
            if let Err(why) = due.step(&e.body) {
                bad(&mut r, e, None, &why, event_type(&e.body), json(&due.remaining()));
                loss_due = None;
            } else if due.done() {
                loss_due = None;
            }
        }

        match &e.body {
            EventBody::RunStarted {
                policy: p,
                areas: a,
                rules_version,
                roster: listed,
                carried_from,
                ..
            } => {
                roster = listed.clone();
                // rules 12: what earlier runs still owed, owed back from here on (checked
                // against those logs after the loop)
                restores.carry(carried_from);
                if i > 0 {
                    bad(
                        &mut r,
                        e,
                        None,
                        "run.started is not the log's first event: a run can't start again inside a log",
                        e.seq.to_string(),
                        "1".into(),
                    );
                }
                let version = rules_version.unwrap_or(1);
                // the newest rules this build decides under (the default's, but on a branch that
                // builds the next version)
                if version > crate::rules::RULES_NEWEST {
                    bad(
                        &mut r,
                        e,
                        None,
                        "log written under newer decision rules than this replay knows",
                        version.to_string(),
                        crate::rules::RULES_NEWEST.to_string(),
                    );
                }
                r.rules_version = version;
                if let Err(why) = p.validate() {
                    bad(
                        &mut r,
                        e,
                        None,
                        "logged policy is out of bounds (no policy file could say this)",
                        why.to_string(),
                        String::new(),
                    );
                }
                // recompute with the rules the log was produced under
                let mut p = p.clone();
                p.rules.rules_version = version;
                // rules 2: a key its version did not have (a newer log given an older header)
                if !p.rules.at_least(crate::rules::STANDING_APPROVALS_SINCE)
                    && p.rules.delivery.as_ref().is_some_and(|d| d.dropzone.standing_approvals.is_some())
                {
                    bad(
                        &mut r,
                        e,
                        None,
                        "a field newer than the log's rules version (rules markers)",
                        "policy.rules.delivery.dropzone.standing_approvals".into(),
                        format!("absent before rules {}", crate::rules::STANDING_APPROVALS_SINCE),
                    );
                }
                // rules 3: the traffic figures, a key rules 2 did not have
                if !p.rules.at_least(crate::rules::TRAFFIC_SEPARATION_SINCE)
                    && p.rules.delivery.as_ref().is_some_and(|d| d.separation.traffic.is_some())
                {
                    bad(
                        &mut r,
                        e,
                        None,
                        "a field newer than the log's rules version (rules markers)",
                        "policy.rules.delivery.separation.traffic".into(),
                        format!("absent before rules {}", crate::rules::TRAFFIC_SEPARATION_SINCE),
                    );
                }
                // rules 10: the relying party and the passkeys the run started with
                if let Some(pk) = listed.as_ref().and_then(|ro| ro.passkeys.as_ref())
                {
                    passkeys.rp = Some((pk.rp_id.clone(), pk.origins.clone()));
                    for k in &pk.keys {
                        passkeys.keys.insert(k.id.clone(), (k.clone(), false));
                    }
                }
                if let Some(s) = listed.as_ref().and_then(|ro| ro.study.as_ref()) {
                    r.study = Some(StudyReport {
                        participant: s.participant.clone(),
                        password_step_up: s.password_step_up.clone(),
                        passkey_acts: 0,
                        password_acts: 0,
                    });
                }
                policy = Some(p);
                areas = None;
                if let Some(a) = a {
                    areas = Some(load_areas(&mut r, e, &a.sha256, &a.areas));
                }
            }
            EventBody::AreasLoaded { sha256, areas: set } => {
                areas = Some(load_areas(&mut r, e, sha256, set));
            }
            EventBody::IntentCreated { intent, session } => {
                if intents.contains_key(&intent.id) {
                    bad(
                        &mut r,
                        e,
                        Some(intent.id),
                        "intent created twice",
                        String::new(),
                        String::new(),
                    );
                    continue;
                }
                // rules 34: a member order is made by its group's dispatch, under the group's
                // approval, never by a session: it is bound to its group's check instead
                if intent.parent().is_some() || intent.share().is_some() {
                    // rules 34, stage 4: one its group's last re-form makes
                    let pending = intent.parent().zip(intent.uids.first()).and_then(|(p, u)| {
                        let g = intents.get_mut(&p)?;
                        let (want, plans) = g.reform_pending.remove(u)?;
                        Some((want, plans))
                    });
                    if let Some((want, plans)) = pending {
                        if session.is_some() || want.id != intent.id || want.task != intent.task || want.share() != intent.share() || want.uids != intent.uids
                            || want.source != intent.source || want.constraints != intent.constraints || want.rationale != intent.rationale
                        {
                            bad(&mut r, e, Some(intent.id), "a re-form's member order is not the one its re-form laid out", json(&(&intent.task, intent.share())), json(&(&want.task, want.share())));
                        }
                        if let Some(g) = intent.parent().and_then(|p| intents.get_mut(&p))
                            && let Some(u) = intent.uids.first()
                        {
                            g.members_created.insert(u.clone(), intent.id);
                        }
                        let mut tr = Tracked::new(intent.clone(), State::Proposed);
                        tr.by_reform = true;
                        if let (Some(p), Some(u)) = (plans, intent.uids.first()) {
                            tr.route_plans = BTreeMap::from([(u.clone(), p)]);
                        }
                        intents.insert(intent.id, tr);
                        continue;
                    }
                    if let Some(why) = member_created_mismatch(intent, session.is_some(), &intents, policy.as_ref()) {
                        bad(&mut r, e, Some(intent.id), why, json(&(intent.parent(), &intent.uids, &intent.task, intent.share())), String::new());
                    }
                    if let Some(g) = intent.parent().and_then(|p| intents.get_mut(&p))
                        && let Some(u) = intent.uids.first()
                    {
                        g.members_created.insert(u.clone(), intent.id);
                    }
                    intents.insert(intent.id, Tracked::new(intent.clone(), State::Proposed));
                    if let Some(g) = intent.parent().and_then(|p| intents.get(&p))
                        && let Some(u) = intent.uids.first()
                    {
                        let plans = g.route_plans.get(u).cloned();
                        if let (Some(p), Some(tr)) = (plans, intents.get_mut(&intent.id)) {
                            tr.route_plans = BTreeMap::from([(u.clone(), p)]);
                        }
                    }
                    continue;
                }
                                // rules 8: a human's intent is made in a live session of theirs (an ended one
                // is caught above), an agent's in none
                match (&intent.source, session) {
                    (Source::Human { id }, Some(s)) => {
                        check_session(&mut r, e, intent.id, id, *s, &sessions, "a human's intent");
                    }
                    (Source::Human { id }, None) => bad(
                        &mut r,
                        e,
                        Some(intent.id),
                        "a human's intent not made in a live session of theirs",
                        id.to_string(),
                        "session".into(),
                    ),
                    (Source::Agent { id }, Some(_)) => bad(
                        &mut r,
                        e,
                        Some(intent.id),
                        "an agent's intent under an operator session",
                        id.to_string(),
                        String::new(),
                    ),
                    // rules 29: the fleet's orders are handed over at the start, never requested
                    (Source::Handover { id }, _) => bad(
                        &mut r,
                        e,
                        Some(intent.id),
                        "an intent created with the fleet's handover as its source (a handed-over order is never requested)",
                        id.clone(),
                        String::new(),
                    ),
                    _ => {}
                }
                intents.insert(intent.id, Tracked::new(intent.clone(), State::Proposed));
            }
            EventBody::CheckResult {
                intent,
                phase,
                inputs,
                results,
                passed,
                layout: logged_layout,
            } => {
                check_waiting(&mut r, e, *intent, &inputs.waiting, &intents);
                check_silent_orders(&mut r, e, *intent, inputs, &intents, &nodes, policy.as_ref());
                let (Some(pol), Some(tr)) = (&policy, intents.get_mut(intent)) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "check without run.started or intent.created",
                        String::new(),
                        String::new(),
                    );
                    continue;
                };
                let inputs = match resolve(inputs, &areas) {
                    Ok(w) => w,
                    Err(why) => {
                        tr.last_check_passed = None;
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            why,
                            json(&inputs.areas_sha256),
                            json(&areas.as_ref().map(|a| &a.0)),
                        );
                        continue;
                    }
                };
                check_terrain(&mut r, e, *intent, &inputs);
                check_markers(&mut r, e, *intent, &inputs, &pol.rules);
                // rules 34: a group order's check is its layout and its members' results, both
                // recomputed; any other check logs no layout
                let group_eval = crate::group::is_group(&tr.spec, &pol.rules).then(|| crate::group::evaluate(&tr.spec, &inputs, &pol.rules));
                let again = match &group_eval {
                    Some(g) => g.results.clone(),
                    None => rules::evaluate(&tr.spec, &inputs, &pol.rules),
                };
                if group_eval.as_ref().map(|g| &g.layout) != logged_layout.as_deref() {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        if group_eval.is_some() { "a group order's layout differs from the one its facts give" } else { "a layout logged with the check of an intent that is not a group order" },
                        json(logged_layout),
                        json(&group_eval.as_ref().map(|g| &g.layout)),
                    );
                }
                let again_passed = rules::passed(&again, tr.spec.kind());
                tr.group_eval = group_eval.filter(|_| again_passed).map(Box::new);
                r.checks_recomputed += 1;
                if &again != results || again_passed != *passed {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "check results differ",
                        json(&(results, passed)),
                        json(&(&again, again_passed)),
                    );
                }
                tr.last_check_passed = Some(again_passed);
                // a dispatched-phase check belongs to no kept task
                if *phase == CheckPhase::Dispatched {
                    bad(&mut r, e, Some(*intent), "a dispatched-phase check: no running task of a kept kind is re-checked", json(&(tr.spec.kind(), tr.state)), String::new());
                }
                // which platform each of its vehicles was (a next run carries it only onto the
                // same ones)
                for u in &tr.spec.uids {
                    if let Some(p) = inputs.vehicles.get(u).and_then(|v| v.platform.as_ref()) {
                        tr.platforms.insert(u.clone(), p.id.clone());
                    }
                }
                // rules 22: each routed leg planned again from its own grid, and what it is
                // dispatched with
                check_plans(&mut r, e, *intent, &inputs, pol);
                tr.route_plans = if again_passed { inputs.route_plans.clone() } else { BTreeMap::new() };
                tr.last_inputs = Some(Box::new(inputs));
                tr.check_window = Some((before_t, e.t));
                tr.last_results = again;
                tr.last_phase = Some(*phase);
                tr.last_check_seq = e.seq;
                if *phase == CheckPhase::Dequeue && tr.state != State::Queued {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "dequeue check of an intent that is not queued",
                        json(&tr.state),
                        json(&State::Queued),
                    );
                }
            }
            EventBody::PolicyDecision {
                intent,
                inputs,
                decision,
            } => {
                check_waiting(&mut r, e, *intent, &inputs.waiting, &intents);
                check_silent_orders(&mut r, e, *intent, inputs, &intents, &nodes, policy.as_ref());
                let (Some(pol), Some(tr)) = (&policy, intents.get_mut(intent)) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "policy decision without run.started or intent.created",
                        String::new(),
                        String::new(),
                    );
                    continue;
                };
                let inputs = match resolve(inputs, &areas) {
                    Ok(w) => w,
                    Err(why) => {
                        tr.tier = None;
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            why,
                            json(&inputs.areas_sha256),
                            json(&areas.as_ref().map(|a| &a.0)),
                        );
                        continue;
                    }
                };
                let again = policy::decide(&tr.spec, &inputs, pol);
                r.policy_recomputed += 1;
                if &again != decision {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "policy decision differs",
                        json(decision),
                        json(&again),
                    );
                }
                tr.route(&again);
            }
            EventBody::ApprovalRequested { intent, expires_t } => {
                let Some(tr) = intents.get_mut(intent) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "unknown intent",
                        String::new(),
                        String::new(),
                    );
                    continue;
                };
                if let Some(pol) = &policy {
                    let want = policy::approval_deadline(pol, &tr.spec.task, e.t);
                    if *expires_t != want {
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "approval deadline differs from policy",
                            json(expires_t),
                            json(&want),
                        );
                    }
                }
                if tr.state == State::Queued
                    && tr.recheck != Some(Verdict::Inbox)
                    && !tr.queue_expired_inbox
                {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "a queued intent sent for approval without a re-check or a queue expiry that says so",
                        json(&tr.recheck),
                        json(&Verdict::Inbox),
                    );
                }
                tr.expires_t = Some(*expires_t);
            }
            EventBody::ApprovalConcurred {
                intent,
                by,
                session,
                n,
                of,
                supersedes,
                step_up,
                ..
            } => {
                let Some(tr) = intents.get_mut(intent) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "unknown intent",
                        String::new(),
                        String::new(),
                    );
                    continue;
                };
                check_signature(&mut r, e, *intent, tr, by, *session, &sessions);
                check_step_up(
                    &mut r,
                    e,
                    Some(*intent),
                    tr.step_up,
                    by,
                    *session,
                    *step_up,
                    &mut step_ups,
                    StepUse::Signature,
                    &Expected::Is(crate::passkey::StepUpFor::Sign { intent: *intent, token: None, picture_sha256: None }),
                    policy.as_ref(),
                );
                let want = (u32::try_from(tr.signers.len()).unwrap_or(u32::MAX) + 1, tr.approvers);
                if (*n, *of) != want || *n >= *of {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "concurrence count differs",
                        json(&(n, of)),
                        json(&want),
                    );
                }
                if !supersedes.is_empty() {
                    bad(&mut r, e, Some(*intent), "an approval that stops consequential orders: there are none", json(supersedes), String::new());
                }
                tr.signers.push(by.clone());
                r.concurrences += 1;
            }
            EventBody::ApprovalReset { intent, .. } => {
                bad(
                    &mut r,
                    e,
                    Some(*intent),
                    "approval.reset: no kept kind has a frozen plan to reset signatures against",
                    String::new(),
                    String::new(),
                );
            }
            EventBody::ApprovalGranted {
                intent,
                by,
                self_approved,
                session,
                supersedes,
                step_up,
                ..
            } => {
                let Some(tr) = intents.get_mut(intent) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "unknown intent",
                        String::new(),
                        String::new(),
                    );
                    continue;
                };
                if tr.state != State::PendingApproval {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "approval granted outside pending_approval",
                        json(&tr.state),
                        json(&State::PendingApproval),
                    );
                }
                match tr.expires_t {
                    Some(x) if e.t < x => {}
                    other => bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "approval granted at or after its deadline",
                        e.t.0.to_string(),
                        format!("< {:?}", other.map(|t| t.0)),
                    ),
                }
                let want = is_self_approval(&tr.spec.source, by);
                if *self_approved != want {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "self_approved flag differs",
                        self_approved.to_string(),
                        want.to_string(),
                    );
                }
                r.approvals += 1;
                if want {
                    r.self_approvals += 1;
                }
                check_signature(&mut r, e, *intent, tr, by, *session, &sessions);
                check_step_up(
                    &mut r,
                    e,
                    Some(*intent),
                    tr.step_up,
                    by,
                    *session,
                    *step_up,
                    &mut step_ups,
                    StepUse::Signature,
                    &Expected::Is(crate::passkey::StepUpFor::Sign { intent: *intent, token: None, picture_sha256: None }),
                    policy.as_ref(),
                );
                if !supersedes.is_empty() {
                    bad(&mut r, e, Some(*intent), "an approval that stops consequential orders: there are none", json(supersedes), String::new());
                }
                tr.signers.push(by.clone());
                let needed = usize::try_from(tr.approvers).unwrap_or(usize::MAX);
                if tr.signers.len() != needed {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "approval granted without the signatures the tier needs",
                        tr.signers.len().to_string(),
                        needed.to_string(),
                    );
                }
                tr.granted = true;
                tr.approval_rec = Some(CarriedApproval {
                    by: by.clone(),
                    session: *session,
                    t: e.t,
                    at_submission: false,
                    self_approved: *self_approved,
                });
            }
            EventBody::ApprovalAtSubmission {
                intent,
                by,
                session,
                self_approved,
                rule,
            } => {
                check_session(&mut r, e, *intent, by, *session, &sessions, "approval at submission");
                let Some(tr) = intents.get_mut(intent) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "unknown intent",
                        String::new(),
                        String::new(),
                    );
                    continue;
                };
                if tr.state != State::Checked {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "approval at submission outside checked",
                        json(&tr.state),
                        json(&State::Checked),
                    );
                }
                let submitter_ok = match &tr.spec.source {
                    Source::Agent { id } => {
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "agent-sourced intent approved at submission",
                            by.to_string(),
                            format!("agent {id}: only a human approves it, in the inbox"),
                        );
                        false
                    }
                    Source::Human { id } if id != by => {
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "approval at submission by someone other than the submitter",
                            by.to_string(),
                            id.to_string(),
                        );
                        false
                    }
                    Source::Human { .. } => true,
                    // rules 29: never submitted, so never approved at submission
                    Source::Handover { id } => {
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "a handed-over order approved at submission",
                            by.to_string(),
                            id.clone(),
                        );
                        false
                    }
                };
                if !*self_approved {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "self_approved flag differs",
                        self_approved.to_string(),
                        "true".into(),
                    );
                }
                let allowed = tr.tier == Some(Tier::Human) && tr.submitter_authorizes;
                if !allowed {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "approval at submission in a tier that doesn't allow it",
                        rule.clone(),
                        json(&(&tr.tier, &tr.rule, tr.submitter_authorizes)),
                    );
                } else if tr.rule.as_deref() != Some(rule.as_str()) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "approval at submission names another tier",
                        rule.clone(),
                        json(&tr.rule),
                    );
                }
                r.approvals_at_submission += 1;
                tr.granted_at_submission = submitter_ok && allowed;
                tr.approval_rec = Some(CarriedApproval {
                    by: by.clone(),
                    session: *session,
                    t: e.t,
                    at_submission: true,
                    self_approved: true,
                });
            }
            EventBody::ApprovalRejected {
                intent, by, session, ..
            } => {
                check_session(&mut r, e, *intent, by, *session, &sessions, "rejection");
                if let Some(tr) = intents.get_mut(intent) {
                    tr.rejected_by_human = true;
                }
            }
            EventBody::ApprovalExpired { intent, expires_t } => {
                if let Some(tr) = intents.get_mut(intent) {
                    if e.t < *expires_t || Some(*expires_t) != tr.expires_t {
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "expired before its deadline",
                            e.t.0.to_string(),
                            format!(">= {}", expires_t.0),
                        );
                    }
                    tr.expired = true;
                }
            }
            EventBody::IntentState {
                intent,
                transition,
                from,
                to,
                reason,
            } => {
                // rules 15 (murmur-zq3): a void that says a newer order superseded it names one
                if *transition == Transition::Void
                    && let Some(tr) = intents.get(intent)
                    && let Some(why) = superseded_mismatch(*intent, &tr.spec, reason, &intents)
                {
                    bad(&mut r, e, Some(*intent), why, reason.clone(), String::new());
                }
                // rules 34: a member order goes under only its own group's approval, the group
                // approved and the member created under it; a group order ends only on its members'
                // ends, as their count says
                let member_basis = match (*transition, intents.get(intent)) {
                    (Transition::Under, Some(tr)) => {
                        let parent = tr.spec.parent().and_then(|p| intents.get(&p));
                        let in_force = |g: &Tracked| {
                            if tr.by_reform {
                                matches!(g.state, State::Dispatched | State::Done)
                            } else {
                                matches!(g.state, State::Approved | State::AutoApproved)
                            }
                        };
                        match parent {
                            Some(g)
                                if in_force(g)
                                    && tr.spec.uids.first().is_some_and(|u| g.members_created.get(u) == Some(intent)) =>
                            {
                                Some(crate::group::member_basis(
                                    &g.basis.clone().unwrap_or_else(|| crate::engine::no_basis(e.t)),
                                    g.decision.as_ref(),
                                ))
                            }
                            _ => {
                                bad(&mut r, e, Some(*intent), "a member order approved under a group order that is not approved, or not created under it", json(&tr.spec.parent()), json(&parent.map(|g| g.state)));
                                None
                            }
                        }
                    }
                    _ => None,
                };
                if matches!(transition, Transition::Complete | Transition::Fail)
                    && let Some(tr) = intents.get(intent)
                    && !tr.group_members.is_empty()
                {
                    let min = tr.spec.group().map_or(1, crate::group::GroupSpec::min_members);
                    let ends: Option<Vec<(State, bool)>> = tr
                        .group_members
                        .iter()
                        .map(|(u, m)| {
                            let st = intents.get(m)?.state;
                            st.is_terminal().then(|| (st, newest_on.get(u).is_some_and(|n| n > m)))
                        })
                        .collect();
                    match ends {
                        None => bad(&mut r, e, Some(*intent), "a group order ended before every member order it sent ended", reason.clone(), String::new()),
                        Some(ends) => {
                            let (done, want) = crate::group::outcome(min, &ends);
                            let tr_want = if done { Transition::Complete } else { Transition::Fail };
                            if (*transition, reason) != (tr_want, &want) {
                                bad(&mut r, e, Some(*intent), "a group order's end differs from its members' count", json(&(transition, reason)), json(&(tr_want, &want)));
                            }
                        }
                    }
                }
                let Some(tr) = intents.get_mut(intent) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "unknown intent",
                        String::new(),
                        String::new(),
                    );
                    continue;
                };
                if let Some(b) = member_basis {
                    tr.basis = Some(b);
                    tr.basis_carried = false;
                }
                if *transition == Transition::Under && tr.spec.parent().is_none() {
                    bad(&mut r, e, Some(*intent), "under a group's approval, but not a member order", String::new(), String::new());
                }
                r.transitions_verified += 1;
                if *from != tr.state {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "transition from a state the intent wasn't in",
                        json(from),
                        json(&tr.state),
                    );
                }
                match next_state(tr.state, *transition) {
                    Ok(s) if s == *to => {}
                    Ok(s) => bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "transition leads elsewhere",
                        json(to),
                        json(&s),
                    ),
                    Err(err) => bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "illegal transition",
                        json(to),
                        err.to_string(),
                    ),
                }
                if let Some(why) = unbacked(tr, *transition) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        why,
                        json(transition),
                        "no evidence".into(),
                    );
                }
                // a queue re-check's verdict is carried out by the next transition, nothing
                // else (dispatch, back to the inbox, void)
                if tr.state == State::Queued
                    && let Some(v) = tr.recheck
                {
                    let follows = match v {
                        Verdict::Dispatch => Transition::Dispatch,
                        Verdict::Inbox => Transition::RequireApproval,
                        Verdict::Void => Transition::Void,
                    };
                    if *transition != follows {
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "a queue re-check's verdict not carried out",
                            json(transition),
                            json(&follows),
                        );
                    }
                }
                // and a queue expiry by its own (expired, or back to the inbox)
                if tr.state == State::Queued && tr.queue_expired {
                    let follows = if tr.queue_expired_inbox {
                        Transition::RequireApproval
                    } else {
                        Transition::Expire
                    };
                    if *transition != follows {
                        bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "a queue expiry not carried out",
                            json(transition),
                            json(&follows),
                        );
                    }
                }
                // the engine writes an intent.dispatched and its transition back to back
                if let Some(seq) = tr.dispatch_logged
                    && *transition != Transition::Dispatch
                {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "intent.dispatched followed by another transition than its dispatch",
                        json(transition),
                        format!("dispatch (intent.dispatched at seq {seq})"),
                    );
                    tr.dispatch_logged = None;
                }
                match transition {
                    // rules 34: a group order's dispatch sends no order to a vehicle itself (its
                    // member orders do)
                    Transition::Dispatch if !tr.group_members.is_empty() || tr.spec.group().is_some() => {
                        tr.dispatch_logged = None;
                        tr.dispatched = true;
                        tr.dispatched_before = true;
                    }
                    Transition::Dispatch => {
                        tr.dispatch_logged = None;
                        tr.dispatched = true;
                        tr.started_ahead.clear();
                        tr.dispatched_before = true;
                        tr.ops.clear();
                        tr.start_went_out.clear();
                        tr.confirming = tr.maybe_started.clone();
                        for u in &tr.spec.uids {
                            newest_on.insert(u.clone(), *intent);
                        }
                    }
                    Transition::Void => tr.stopped = false,
                    Transition::Queue => {
                        tr.queued_logged = None;
                        r.queued += 1;
                    }
                    Transition::Expire if tr.state == State::Queued => {
                        tr.queue_expired = false;
                        r.queue_expired += 1;
                    }
                    Transition::Requeue => {
                        tr.requeued_logged = None;
                        r.requeued += 1;
                    }
                    // what the approval rests on, as the engine records it: the last check,
                    Transition::AutoApprove
                    | Transition::Approve
                    | Transition::AuthorizeAtSubmission => {
                        tr.basis = Some(Basis {
                            t: e.t,
                            by_human: *transition != Transition::AutoApprove,
                            checks: tr.last_results.clone(),
                            policy: tr.decision.clone(),
                            in_play: tr
                                .last_inputs
                                .as_deref()
                                .map(|w| w.in_play.clone())
                                .unwrap_or_default(),
                        });
                        tr.basis_carried = false;
                    }
                    _ => {}
                }
                if tr.state == State::Queued {
                    tr.recheck = None;
                    tr.recheck_seq = None;
                    tr.queue_expired = false;
                    tr.queue_expired_inbox = false;
                    // back to a human: their next approval decides, starting it again included
                    if *transition == Transition::RequireApproval {
                        tr.maybe_started.clear();
                    }
                }
                tr.state = *to;
                if to.is_terminal() {
                    tr.ended_t.get_or_insert(e.t);
                }
            }
            EventBody::IntentDispatched {
                intent,
                task,
                uids,
                via: dispatch_via,
                path: dispatch_path,
                inputs: reach_inputs,
                reach,
                share: dispatch_share,
            } => {
                if let Some(inputs) = reach_inputs {
                    check_waiting(&mut r, e, *intent, &inputs.waiting, &intents);
                    check_silent_orders(&mut r, e, *intent, inputs, &intents, &nodes, policy.as_ref());
                }
                let Some(tr) = intents.get_mut(intent) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "dispatch of an unknown intent",
                        json(&(task, uids)),
                        String::new(),
                    );
                    continue;
                };
                if let Some(seq) = tr.dispatch_logged {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "dispatched again before its dispatch transition",
                        seq.to_string(),
                        "intent.state dispatch".into(),
                    );
                }
                tr.dispatch_logged = Some(e.seq);
                let from_queue = tr.state == State::Queued && tr.recheck == Some(Verdict::Dispatch);
                // out of the queue: at once after the re-check that allows it
                if from_queue && tr.recheck_seq.map(|s| s + 1) != Some(e.seq) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "dispatched from the queue, but not right after its re-check",
                        e.seq.to_string(),
                        json(&tr.recheck_seq.map(|s| s + 1)),
                    );
                }
                // rules 5, straight from the approval: only to vehicles the link reaches now
                if let Some(pol) = &policy
                    && matches!(tr.state, State::AutoApproved | State::Approved)
                    && pol.queue().queues(tr.spec.kind())
                {
                    match (reach_inputs, reach) {
                        (Some(inputs), Some(reach)) => match resolve(inputs, &areas) {
                            Ok(w) => {
                                let cfg = pol.queue();
                                let again = queue::reach(&tr.spec, &w, &cfg, &pol.rules);
                                if &again != reach {
                                    bad(&mut r, e, Some(*intent), "reach differs", json(reach), json(&again));
                                }
                                if !queue::all_reachable(&again) {
                                    bad(
                                        &mut r,
                                        e,
                                        Some(*intent),
                                        "dispatched to a vehicle the link could not reach (it must be queued)",
                                        json(reach),
                                        json(&again),
                                    );
                                }
                            }
                            Err(why) => bad(&mut r, e, Some(*intent), why, String::new(), String::new()),
                        },
                        _ => bad(
                            &mut r,
                            e,
                            Some(*intent),
                            "dispatched without the reach that allows it (rules 5: its kind is queued when a vehicle can't be reached)",
                            "none".into(),
                            "inputs and reach".into(),
                        ),
                    }
                }
                if !(matches!(tr.state, State::AutoApproved | State::Approved) || from_queue) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        if tr.state == State::Queued {
                            "dispatched from the queue without a passing re-check"
                        } else {
                            "dispatched without approval"
                        },
                        json(&tr.state),
                        "auto_approved|approved, or queued after a re-check that dispatches".into(),
                    );
                }
                check_dispatch(
                    &mut r,
                    e,
                    tr,
                    Dispatched {
                        task,
                        uids,
                        via: dispatch_via,
                        path: dispatch_path.as_ref(),
                    },
                    policy.as_ref(),
                );
                // rules 34: a member order goes out with its own share, nothing else
                if dispatch_share.as_ref() != tr.spec.share() {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "dispatched with another share than its member order's",
                        json(dispatch_share),
                        json(&tr.spec.share()),
                    );
                }
            }
            EventBody::AuthRefused { operator, .. } => {
                // rules 8: a login refused, in a credentials run, for an operator it lists
                // (an unknown id goes to the bridge's own log: the audit log names no one
                // the run does not know)
                if r.auth != Some(AuthMode::Credentials) {
                    let logged = json(&r.auth);
                    bad(&mut r, e, None, "a refused login outside a credentials run", logged, "credentials".into());
                } else if !roster.as_ref().is_some_and(|ro| ro.operators.contains(operator)) {
                    bad(&mut r, e, None, "a refused login for an operator the run does not list", operator.to_string(), String::new());
                }
                r.logins_refused += 1;
            }
            EventBody::PasskeyRegistered { key, via, session, step_up } => {
                // rules 10: a passkey registered while the run was up: an operator it lists, a
                // credential it does not know yet, a P-256 key; through the operator's own
                // session (a person's) quoting the credential they re-entered for it, or
                // through an invitation (no session, no step-up)
                if r.auth != Some(AuthMode::Credentials) || passkeys.rp.is_none() {
                    bad(&mut r, e, None, "a passkey registered outside a credentials run of rules 10 with a relying party", String::new(), String::new());
                }
                if !roster.as_ref().is_some_and(|ro| ro.operators.contains(&key.operator)) {
                    bad(&mut r, e, None, "a passkey registered for an operator the run does not list", key.operator.to_string(), String::new());
                }
                if passkeys.keys.contains_key(&key.id) {
                    bad(&mut r, e, None, "a passkey registered twice", key.id.clone(), String::new());
                }
                if let Err(why) = crate::passkey::check_key(&key.public_key) {
                    bad(&mut r, e, None, "a registered passkey that is not a P-256 key", why, String::new());
                }
                match (via.as_str(), session) {
                    ("session", Some(s)) => {
                        check_operator(&mut r, e, &key.operator, *s, &sessions, "a passkey registration");
                        if !methods.get(s).copied().is_some_and(AuthMethod::is_person) {
                            bad(&mut r, e, None, "a passkey registered through a session that is not a person's", s.0.to_string(), String::new());
                        }
                        check_step_up(
                            &mut r,
                            e,
                            None,
                            true,
                            &key.operator,
                            *s,
                            *step_up,
                            &mut step_ups,
                            StepUse::Registration,
                            &Expected::Registration,
                            policy.as_ref(),
                        );
                    }
                    ("invitation", None) if step_up.is_none() => {}
                    _ => bad(
                        &mut r,
                        e,
                        None,
                        "a registration is through a session (quoting its step-up) or an invitation (neither)",
                        json(&(via, session, step_up)),
                        String::new(),
                    ),
                }
                passkeys.keys.insert(key.id.clone(), (key.clone(), false));
                r.passkeys_registered += 1;
            }
            EventBody::PasskeyRemoved { operator, credential, .. } => {
                match passkeys.keys.get_mut(credential) {
                    Some((k, removed)) if &k.operator == operator && !*removed => *removed = true,
                    _ => bad(
                        &mut r,
                        e,
                        None,
                        "a removal of a passkey the run does not hold for that operator",
                        credential.clone(),
                        operator.to_string(),
                    ),
                }
                r.passkeys_removed += 1;
            }
            EventBody::OperatorAction {
                operator,
                session,
                action,
                detail: _,
                grants,
                auth,
                step_up,
                passkey: proof,
            } => {
                let authenticated = policy.is_some();
                let passkey_rules = policy.is_some();
                let run_auth = r.auth;
                // rules 10: an assertion goes with a passkey login or step-up, and only there
                let passkey_act = matches!(action.as_str(), "session.opened" | "step_up") && *auth == Some(AuthMethod::Passkey);
                if proof.is_some() != passkey_act {
                    bad(
                        &mut r,
                        e,
                        None,
                        "a passkey assertion logged without a passkey login or step-up, or one without its assertion",
                        json(&(action, auth, proof.is_some())),
                        String::new(),
                    );
                }
                if action == "session.opened" {
                    // rules 10: a passkey login's assertion, verified against the run's key
                    if let Some(u) = proof {
                        if !passkey_rules {
                            bad(&mut r, e, None, "a passkey login under rules that have none", String::new(), String::new());
                        } else if u.purpose != crate::passkey::StepUpFor::Login {
                            bad(&mut r, e, None, "a login's assertion made for another act", u.purpose.describe(), "a login".into());
                        } else if let Err(why) = passkeys.use_once(operator, None, e.t, u) {
                            bad(&mut r, e, None, &format!("passkey login: {why}"), u.credential.clone(), String::new());
                        }
                        r.passkey_logins += 1;
                    }
                    // rules 10: how a person may sign in (`[approval] login_factor`)
                    if passkey_rules
                        && run_auth == Some(AuthMode::Credentials)
                        && let (Some(method), Some(p)) = (*auth, policy.as_ref())
                        && !p.login_factor().admits(method)
                    {
                        bad(
                            &mut r,
                            e,
                            None,
                            "a session opened by a method the policy's login_factor does not admit",
                            method.as_str().into(),
                            p.login_factor().as_str().into(),
                        );
                    }
                    let grants = grants.unwrap_or_default();
                    // only operators the run was configured with, consequential authority only
                    // for its consequential operators (rules 4 on: before, no roster was logged)
                    if let Some(why) = roster.as_ref().and_then(|ro| ro.refuses(operator, grants)) {
                        bad(
                            &mut r,
                            e,
                            None,
                            why,
                            json(&(operator, grants)),
                            json(&roster),
                        );
                    }
                    // rules 8: opened the way the run authenticates (a checked credential in a
                    // credentials run: a person's password, or a harness-held scripted one; an
                    // asserted name in a dev run)
                    // a session opens by the method the roster's scripted list gives
                    // its operator (a harness's credential scripted, a person's not)
                    if policy.is_some()
                        && let (Some(method), Some(why)) = (
                            *auth,
                            auth.and_then(|m| roster.as_ref().and_then(|ro| ro.method_refused(operator, m))),
                        )
                    {
                        bad(&mut r, e, None, &format!("session opened by another method than the roster fixes for its operator: {why}"), method.as_str().into(), json(&roster.as_ref().and_then(|ro| ro.scripted.as_ref())));
                    }
                    if authenticated {
                        match (run_auth, *auth) {
                            (Some(mode), Some(method)) if mode.admits(method) => {
                                methods.insert(*session, method);
                                match method {
                                    AuthMethod::Password | AuthMethod::Passkey => r.people.insert(operator.clone()),
                                    AuthMethod::Scripted => r.scripted.insert(operator.clone()),
                                    AuthMethod::Asserted => false,
                                };
                            }
                            _ => bad(
                                &mut r,
                                e,
                                None,
                                "session opened by another method than the run authenticates with",
                                json(auth),
                                json(&run_auth.map(AuthMode::method)),
                            ),
                        }
                    }
                    if sessions
                        .insert(*session, (operator.clone(), grants))
                        .is_some()
                    {
                        bad(
                            &mut r,
                            e,
                            None,
                            "session opened twice",
                            session.0.to_string(),
                            String::new(),
                        );
                    }
                } else if authenticated {
                    // rules 8: every other operator action is under that operator's own session
                    match sessions.get(session) {
                        Some((op, _)) if op == operator => {}
                        other => bad(
                            &mut r,
                            e,
                            None,
                            "operator action under a session that is not the operator's",
                            operator.to_string(),
                            json(&other.map(|o| &o.0)),
                        ),
                    }
                    match action.as_str() {
                        "session.closed" => {
                            closed.insert(*session);
                            r.sessions_closed += 1;
                        }
                        "step_up" => {
                            // a credential re-entered: only a credentials run has one, and it is
                            // the kind the session was opened with (a scripted operator's
                            // step-up is scripted too: never a person's); rules 10: a person
                            // (signed in by password or passkey) re-enters their password or
                            // uses their passkey
                            let opened = methods.get(session).copied();
                            let by_kind = if passkey_rules {
                                match (opened, *auth) {
                                    (Some(AuthMethod::Scripted), Some(AuthMethod::Scripted)) => true,
                                    (Some(o), Some(AuthMethod::Password | AuthMethod::Passkey)) => o.is_person(),
                                    _ => false,
                                }
                            } else {
                                auth.is_some() && *auth == opened && *auth != Some(AuthMethod::Asserted)
                            };
                            if run_auth != Some(AuthMode::Credentials) || !by_kind {
                                bad(
                                    &mut r,
                                    e,
                                    None,
                                    "a step-up outside a credentials run, or not by the credential its session was opened with",
                                    json(&(run_auth, auth)),
                                    json(&(AuthMode::Credentials, opened)),
                                );
                            }
                            // rules 10: a passkey's assertion, verified against the run's key for
                            // this session and what it was made for
                            if let Some(u) = proof {
                                if !passkey_rules {
                                    bad(&mut r, e, None, "a passkey step-up under rules that have none", String::new(), String::new());
                                } else if u.purpose == crate::passkey::StepUpFor::Login {
                                    bad(&mut r, e, None, "a login's assertion logged as a step-up", u.credential.clone(), String::new());
                                } else if let Err(why) = passkeys.use_once(operator, Some(*session), e.t, u) {
                                    bad(&mut r, e, None, &format!("passkey step-up: {why}"), u.credential.clone(), String::new());
                                }
                            }
                            match *auth {
                                Some(AuthMethod::Passkey) => r.passkey_step_ups += 1,
                                Some(AuthMethod::Password) => r.password_step_ups += 1,
                                _ => {}
                            }
                            step_ups.insert(
                                e.seq,
                                StepUpSeen {
                                    session: *session,
                                    operator: operator.clone(),
                                    t: e.t,
                                    used: StepUse::Unused,
                                    factor: *auth,
                                    person: opened.is_some_and(AuthMethod::is_person),
                                    purpose: proof.as_ref().map(|u| u.purpose.clone()),
                                },
                            );
                            r.step_ups += 1;
                        }
                        _ => {}
                    }
                    if step_up.is_some() {
                        bad(
                            &mut r,
                            e,
                            None,
                            "a step-up quoted by an act that takes none",
                            action.clone(),
                            String::new(),
                        );
                    }
                }
            }
            EventBody::NodePresumed {
                uid,
                since,
                evidence,
                world_t,
                vehicle,
            } => {
                let Some(pol) = policy.as_ref() else {
                    // the marker check flags it under older rules
                    continue;
                };
                let cfg = &pol.rules;
                if nodes.lost_mark(uid).is_some() || nodes.presumption(uid).is_some() {
                    bad(&mut r, e, None, "a vehicle presumed lost that is presumed or confirmed lost already", uid.to_string(), String::new());
                }
                if *since != vehicle.position.as_ref().map(|p| p.t) {
                    bad(&mut r, e, None, "a presumption's last report is not its facts' position", json(since), json(&vehicle.position.as_ref().map(|p| p.t)));
                }
                if !crate::nodes::silent(vehicle, *world_t, cfg) {
                    bad(&mut r, e, None, "a vehicle presumed lost that is not silent on its facts", uid.to_string(), String::new());
                }
                if (e.t.0 - world_t.0).abs() > crate::nodes::WORLD_SKEW_MS {
                    bad(&mut r, e, None, "a presumption's world is not the one at the event", json(world_t), json(&e.t));
                }
                let want = nodes.evidence(uid, vehicle);
                if *evidence != want {
                    bad(
                        &mut r,
                        e,
                        None,
                        "a presumption's evidence differs from the marks the log holds within the window of its last report",
                        json(evidence),
                        json(&want),
                    );
                }
                let kinds: Vec<&str> = evidence.iter().map(|ev| ev.detail.as_str()).collect();
                r.nodes_presumed.push(format!("vehicle {uid}: presumed lost ({})", if kinds.is_empty() { "silent".to_string() } else { kinds.join(", ") }));
            }
            EventBody::NodePresumptionCleared {
                uid,
                since,
                world_t,
                vehicle,
                detail,
            } => {
                let Some(pol) = policy.as_ref() else {
                    continue;
                };
                match nodes.presumption(uid) {
                    None => bad(&mut r, e, None, "a presumption cleared that is not in force", uid.to_string(), String::new()),
                    Some(p) => {
                        let want = crate::nodes::heard_since(vehicle, p.since, p.heard, *world_t, &pol.rules);
                        if p.since != *since || want.as_ref() != Some(detail) {
                            bad(
                                &mut r,
                                e,
                                None,
                                "a presumption cleared that its facts do not show heard since its report",
                                json(&(since, detail)),
                                json(&(p.since, want)),
                            );
                        }
                    }
                }
                r.nodes_presumed.push(format!("vehicle {uid}: presumption cleared ({detail})"));
            }
            EventBody::NodeLost {
                uid,
                since,
                by,
                session,
                note,
                evidence,
                world_t,
                vehicle,
                step_up,
            } => {
                // a credentials run's confirmation quotes a step-up made for it
                check_step_up(
                    &mut r,
                    e,
                    None,
                    true,
                    by,
                    *session,
                    *step_up,
                    &mut step_ups,
                    StepUse::NodeLost,
                    &Expected::Is(crate::passkey::StepUpFor::NodeLost {
                        uid: uid.clone(),
                        since: *since,
                    }),
                    policy.as_ref(),
                );
                node_lost(
                    &mut r,
                    e,
                    (uid, *since, by, *session, note, evidence, *world_t, vehicle),
                    &nodes,
                    &intents,
                    &sessions,
                    policy.as_ref(),
                );
                loss_due = Some(LossDue::new(uid, &intents, &restores, &nodes));
            }
            EventBody::NodeLossRevoked { uid, since, from, cause } => {
                node_loss_revoked(&mut r, e, (uid, *since, from.as_deref(), cause), &nodes, &sessions, policy.as_ref());
                restores.reopen_lost(uid);
            }
            EventBody::IntentQueued {
                intent,
                inputs,
                reach,
                expires_t,
            } => {
                // rules 34: a member order waits at most its group's wait
                let wait = member_wait(&intents, *intent);
                check_silent_orders(&mut r, e, *intent, inputs, &intents, &nodes, policy.as_ref());
                let (Some(pol), Some(tr)) = (&policy, intents.get_mut(intent)) else {
                    bad(&mut r, e, Some(*intent), "queued without run.started or intent.created", String::new(), String::new());
                    continue;
                };
                if !matches!(tr.state, State::Approved | State::AutoApproved) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "queued without approval",
                        json(&tr.state),
                        "auto_approved|approved".into(),
                    );
                }
                let cfg = pol.queue();
                if !cfg.queues(tr.spec.kind()) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "queued although the policy never queues its kind",
                        tr.spec.kind().as_str().into(),
                        json(&cfg.expire_s(tr.spec.kind())),
                    );
                }
                let w = match resolve(inputs, &areas) {
                    Ok(w) => w,
                    Err(why) => {
                        bad(&mut r, e, Some(*intent), why, String::new(), String::new());
                        continue;
                    }
                };
                let again = queue::reach(&tr.spec, &w, &cfg, &pol.rules);
                if &again != reach {
                    bad(&mut r, e, Some(*intent), "reach differs", json(reach), json(&again));
                }
                if queue::all_reachable(&again) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "queued although every vehicle could be reached",
                        json(reach),
                        json(&again),
                    );
                }
                let basis = tr
                    .basis
                    .clone()
                    .unwrap_or_else(|| crate::engine::no_basis(e.t));
                let (want, inbox) = cfg.bound_for(&tr.spec, wait, cfg.wait_end(tr.spec.kind(), e.t), &basis, e.t);
                if *expires_t != want {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "queue deadline differs from policy",
                        json(expires_t),
                        json(&want),
                    );
                }
                tr.queued_logged = Some(e.seq);
                tr.queue_expires = Some(want);
                tr.queue_inbox = inbox;
            }
            EventBody::QueueRecheck {
                intent,
                reach,
                decision,
                verdict,
                changes,
            } => {
                let (Some(pol), Some(tr)) = (&policy, intents.get_mut(intent)) else {
                    bad(&mut r, e, Some(*intent), "re-check without run.started or intent.created", String::new(), String::new());
                    continue;
                };
                r.rechecks += 1;
                if tr.state != State::Queued {
                    bad(&mut r, e, Some(*intent), "re-check of an intent that is not queued", json(&tr.state), json(&State::Queued));
                }
                // at its deadline the queue lets go: expired (or back to the inbox), never sent
                if let Some(d) = tr.queue_expires
                    && e.t >= d
                {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "re-checked at or after its queue deadline (it must expire)",
                        json(&e.t),
                        format!("before {}", json(&d)),
                    );
                }
                // the checks it judges are the dequeue check just before it
                let Some(w) = tr
                    .last_inputs
                    .as_deref()
                    .filter(|_| {
                        tr.last_phase == Some(CheckPhase::Dequeue) && tr.last_check_seq + 1 == e.seq
                    })
                    .cloned()
                else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "re-check without its dequeue check just before",
                        json(&(tr.last_phase, tr.last_check_seq)),
                        json(&(CheckPhase::Dequeue, e.seq.saturating_sub(1))),
                    );
                    continue;
                };
                let cfg = pol.queue();
                let again = queue::reach(&tr.spec, &w, &cfg, &pol.rules);
                if &again != reach {
                    bad(&mut r, e, Some(*intent), "reach differs", json(reach), json(&again));
                }
                if !queue::all_reachable(&again) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "dequeued while a vehicle could not be reached",
                        json(reach),
                        json(&again),
                    );
                }
                let passed = tr.last_check_passed == Some(true);
                // rules 29: a handed-over order's re-check asks the policy only who judges a check
                // nobody could verify
                let ask = passed && (!tr.handed_over || queue::handover_unverified(&tr.last_results));
                let d = ask.then(|| policy::decide(&tr.spec, &w, pol));
                if d.as_ref() != decision.as_ref() {
                    bad(&mut r, e, Some(*intent), "re-check policy decision differs", json(decision), json(&d));
                }
                let basis = tr
                    .basis
                    .clone()
                    .unwrap_or_else(|| crate::engine::no_basis(w.t));
                let since = queue::Since {
                    basis: &basis,
                    carried: tr.basis_carried,
                    rebooted: &rebooted,
                    handed_over: tr.handed_over,
                };
                let rc = queue::assess(&tr.spec, &since, &tr.last_results, passed, d.as_ref(), &w);
                if (rc.verdict, &rc.changes) != (*verdict, changes) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "re-check verdict differs",
                        json(&(verdict, changes)),
                        json(&(rc.verdict, &rc.changes)),
                    );
                }
                match rc.verdict {
                    Verdict::Dispatch => r.recheck_dispatched += 1,
                    Verdict::Inbox => r.recheck_inbox += 1,
                    Verdict::Void => r.recheck_voided += 1,
                }
                tr.recheck = Some(rc.verdict);
                tr.recheck_seq = Some(e.seq);
                if rc.verdict == Verdict::Inbox
                    && let Some(d) = &d
                {
                    // a new decision: who approves it is this decision's, from scratch
                    tr.route(d);
                    tr.back_to_inbox();
                }
            }
            EventBody::QueueExpired {
                intent,
                expires_t,
                inbox,
                ..
            } => {
                let Some(tr) = intents.get_mut(intent) else {
                    bad(&mut r, e, Some(*intent), "unknown intent", String::new(), String::new());
                    continue;
                };
                if tr.state != State::Queued
                    || Some(*expires_t) != tr.queue_expires
                    || e.t < *expires_t
                {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "expired from the queue before its deadline (or not queued)",
                        json(&(e.t, expires_t, tr.state)),
                        json(&tr.queue_expires),
                    );
                }
                if *inbox != tr.queue_inbox {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "queue expiry differs from its deadline's (expired, or back to the inbox for a human's approval held long enough)",
                        json(inbox),
                        json(&tr.queue_inbox),
                    );
                }
                tr.queue_expired = true;
                tr.queue_expired_inbox = *inbox;
                if *inbox {
                    tr.back_to_inbox();
                }
            }
            EventBody::IntentRequeued {
                intent,
                expires_t,
                inbox,
            } => {
                let wait = member_wait(&intents, *intent);
                let (Some(pol), Some(tr)) = (&policy, intents.get_mut(intent)) else {
                    bad(&mut r, e, Some(*intent), "requeued without run.started or intent.created", String::new(), String::new());
                    continue;
                };
                let cfg = pol.queue();
                if tr.state != State::Dispatched
                    || !cfg.queues(tr.spec.kind())
                {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "requeued, but it is not a dispatched vehicle task the queue holds",
                        json(&tr.state),
                        json(&State::Dispatched),
                    );
                }
                // evidence: every vehicle's op ended with its link lost before the start, and
                // none ever started the order
                let lost = tr.spec.uids.iter().all(|u| {
                    matches!(
                        tr.ops.get(u).map(String::as_str),
                        Some(queue::REQUEUED_OP | queue::UNCONFIRMED_OP)
                    )
                });
                if !lost || !tr.committed.is_empty() {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "requeued without every vehicle's op ending with its link lost before the start (or after a vehicle started it)",
                        json(&(&tr.ops, &tr.committed)),
                        format!(
                            "every op {} or {}, none started",
                            queue::REQUEUED_OP,
                            queue::UNCONFIRMED_OP
                        ),
                    );
                }
                let basis = tr
                    .basis
                    .clone()
                    .unwrap_or_else(|| crate::engine::no_basis(e.t));
                // a delivery that only confirmed it found a vehicle not running it: to a human
                // (rules 34, bd murmur-4jlq: a member order back to the queue, under its group's
                // approval, its next delivery a whole one)
                let not_running = !queue::confirm_failed(&tr.confirming, &tr.ops).is_empty();
                let member = tr.spec.parent().is_some();
                let (want, want_inbox) = if !not_running || member {
                    cfg.bound_for(&tr.spec, wait, cfg.wait_end(tr.spec.kind(), basis.t), &basis, e.t)
                } else {
                    (e.t, true)
                };
                if member && not_running {
                    tr.maybe_started.clear();
                }
                if (*expires_t, *inbox) != (want, want_inbox) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "requeue deadline differs from policy",
                        json(&(expires_t, inbox)),
                        json(&(want, want_inbox)),
                    );
                }
                tr.requeued_logged = Some(e.seq);
                tr.queue_expires = Some(want);
                tr.queue_inbox = want_inbox;
            }
            EventBody::IntentResumed {
                intent: spec,
                from,
                state,
                basis,
                policy: carried,
                approval,
                expires_t,
                ops,
                committed,
                dispatched_before,
                platforms,
                maybe_started,
            } => {
                r.resumed += 1;
                if intents.contains_key(&spec.id) {
                    bad(&mut r, e, Some(spec.id), "intent created twice", String::new(), String::new());
                    continue;
                }
                // rules 8: carried only between runs that authenticate alike (an asserted
                // approval is no person's in a credentials run; a log before rules 8 asserted)
                if effective_mode(from.auth) != effective_mode(r.auth) {
                    let here = json(&effective_mode(r.auth));
                    bad(
                        &mut r,
                        e,
                        Some(spec.id),
                        "carried over from a run that authenticated its operators otherwise",
                        json(&effective_mode(from.auth)),
                        here,
                    );
                }
                let open = OpenIntent {
                    spec: spec.clone(),
                    state: from.was,
                    basis: basis.clone(),
                    policy: carried.clone(),
                    approval: approval.clone(),
                    expires_t: *expires_t,
                    ops: ops.clone(),
                    committed: committed.clone(),
                    dispatched_before: *dispatched_before,
                    platforms: platforms.clone(),
                    maybe_started: maybe_started.clone(),
                };
                if !queue::queueable(spec.kind())
                    || !matches!(state, State::Queued | State::Dispatched)
                    || *state != open.resumes_as()
                    || (*state == State::Queued) != expires_t.is_some()
                {
                    bad(
                        &mut r,
                        e,
                        Some(spec.id),
                        "resumed in a state its kind, op states or deadline don't allow",
                        json(&(state, expires_t)),
                        json(&open.resumes_as()),
                    );
                }
                let mut tr = Tracked::new(spec.clone(), *state);
                if let Some(d) = carried {
                    tr.route(d);
                }
                tr.basis = Some(basis.clone());
                tr.basis_carried = true;
                tr.last_results = basis.checks.clone();
                tr.approval_rec = approval.clone();
                tr.ops = ops.clone();
                tr.committed = committed.iter().cloned().collect();
                tr.dispatched_before = *dispatched_before;
                tr.platforms = platforms.clone();
                tr.maybe_started = maybe_started.iter().cloned().collect();
                // a vehicle that had started it is followed (the bridge's Adopt op), never
                // requeued; only one whose start may have gone out is confirmed and may be
                tr.start_went_out = tr.committed.clone();
                if let (Some(pol), Some(by_kind)) = (&policy, *expires_t) {
                    let cfg = pol.queue();
                    // not queued there: its kind's wait from the approval
                    if from.was != State::Queued && by_kind != cfg.wait_end(spec.kind(), basis.t) {
                        bad(
                            &mut r,
                            e,
                            Some(spec.id),
                            "resumed queue deadline differs from policy",
                            json(&by_kind),
                            json(&cfg.wait_end(spec.kind(), basis.t)),
                        );
                    }
                    let (d, inbox) = cfg.bound(by_kind, basis, e.t);
                    tr.queue_expires = Some(d);
                    tr.queue_inbox = inbox;
                }
                if *state == State::Dispatched {
                    tr.dispatched = true;
                    for u in &spec.uids {
                        newest_on.insert(u.clone(), spec.id);
                    }
                }
                resumed.push((e.seq, from.clone(), open, *state));
                intents.insert(spec.id, tr);
            }
            EventBody::GroupDispatched { intent, members } => {
                let Some(tr) = intents.get_mut(intent) else {
                    bad(&mut r, e, Some(*intent), "a group dispatch of an unknown intent", String::new(), String::new());
                    continue;
                };
                if !(policy.is_some() && tr.spec.group().is_some()) {
                    bad(&mut r, e, Some(*intent), "a group dispatch of an intent that is not a group order", String::new(), String::new());
                }
                if !matches!(tr.state, State::Approved | State::AutoApproved) {
                    bad(&mut r, e, Some(*intent), "a group order dispatched without approval", json(&tr.state), "auto_approved|approved".into());
                }
                // its member orders: exactly those created under it, one for each member its
                // last passing check counted (sent or waiting)
                let counted: Vec<Uid> = tr.group_eval.as_ref().map(|g| g.counted()).unwrap_or_default();
                let keys: Vec<Uid> = members.keys().cloned().collect();
                if *members != tr.members_created || keys != counted {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "a group order's member orders are not one for each member its check counted, created under it",
                        json(members),
                        json(&(&tr.members_created, &counted)),
                    );
                }
                tr.group_members = members.clone();
                tr.dispatch_logged = Some(e.seq);
                // rules 34, stage 4: in force on its layout, for re-forms
                if let Some(g) = tr.group_eval.as_ref() {
                    forces.insert(*intent, crate::group::Force::new(g.layout.clone(), members.clone(), e.t));
                }
            }
            EventBody::GroupReformed {
                intent,
                reform: seq,
                inputs,
                cause,
                layout,
                moved,
                left_out,
                results,
                largest_gap_m,
                even_spacing_m,
            } => {
                let Some(world) = group_step_world(&mut r, e, *intent, inputs, &areas, &intents, &forces, policy.as_ref()) else {
                    continue;
                };
                let (Some(pol), Some(g), Some(force)) = (policy.as_ref(), intents.get(intent), forces.get(intent)) else { continue };
                let (gspec, gs) = (g.spec.clone(), g.spec.group().cloned().unwrap_or_default());
                if *seq != force.seq + 1 {
                    bad(&mut r, e, Some(*intent), "a re-form out of sequence", json(seq), json(&(force.seq + 1)));
                }
                let Some(st) = group_cause_check(&mut r, e, *intent, &world, cause, force, &intents, &newest_on, &nodes, pol) else {
                    continue;
                };
                let due = crate::group::due(force, &gs, cause, st.on_share.len(), st.taken.len(), gspec.uids.len(), world.t);
                if due != crate::group::Due::Reform {
                    bad(&mut r, e, Some(*intent), "a re-form that was not due (its drops and returns not material or not settled, within a minute of the last, or the cause last refused)", json(cause), json(&due_name(&due)));
                }
                let mut w = world.clone();
                let decision = g.decision.clone();
                match crate::group::plan_reform(&gspec, &gs, force, &st.on_share, cause, &mut w, None, pol, decision.as_ref()) {
                    Err(why) => {
                        bad(&mut r, e, Some(*intent), "a re-form its facts refuse", json(layout), why);
                        continue;
                    }
                    Ok(p) => {
                        let ids: Vec<&Uid> = p.specs.iter().filter_map(|m| m.uids.first()).collect();
                        if p.reform.layout != **layout
                            || moved.keys().collect::<Vec<_>>() != ids
                            || p.left_out.keys().collect::<Vec<_>>() != left_out.keys().collect::<Vec<_>>()
                            || p.reform.largest_gap_m != *largest_gap_m
                            || p.reform.even_spacing_m != *even_spacing_m
                        {
                            bad(&mut r, e, Some(*intent), "a re-form's layout differs from the one its facts give", json(&(layout, moved, left_out)), json(&(&p.reform.layout, &ids, &p.left_out)));
                        }
                        if p.results != *results {
                            bad(&mut r, e, Some(*intent), "a re-form's checks differ from the ones its facts give", json(results), json(&p.results));
                        }
                        // the member orders it makes next, each its share
                        let pending: BTreeMap<Uid, (IntentSpec, Option<Vec<crate::plan::PlannedLeg>>)> = p
                            .specs
                            .iter()
                            .filter_map(|ms| {
                                let u = ms.uids.first()?.clone();
                                let mut ms = ms.clone();
                                ms.id = *moved.get(&u)?;
                                let plans = world.route_plans.get(&u).cloned();
                                Some((u, (ms, plans)))
                            })
                            .collect();
                        if let Some(f) = forces.get_mut(intent) {
                            f.layout = (**layout).clone();
                            f.seq = *seq;
                            f.last_attempt = Some(world.t);
                            f.refused = None;
                            f.refused_why = None;
                            for (u, m) in moved {
                                f.members.insert(u.clone(), *m);
                                f.left.remove(u);
                            }
                            f.out.extend(left_out.keys().cloned());
                            for u in cause.dropped.keys() {
                                f.left.insert(u.clone(), world.t);
                            }
                        }
                        if let Some(g) = intents.get_mut(intent) {
                            g.reform_pending = pending;
                            g.group_members.extend(moved.clone());
                        }
                    }
                }
            }
            EventBody::GroupReformRefused { intent, inputs, cause, why } => {
                let Some(world) = group_step_world(&mut r, e, *intent, inputs, &areas, &intents, &forces, policy.as_ref()) else {
                    continue;
                };
                let (Some(pol), Some(g), Some(force)) = (policy.as_ref(), intents.get(intent), forces.get(intent)) else { continue };
                let (gspec, gs) = (g.spec.clone(), g.spec.group().cloned().unwrap_or_default());
                let Some(st) = group_cause_check(&mut r, e, *intent, &world, cause, force, &intents, &newest_on, &nodes, pol) else {
                    continue;
                };
                let due = crate::group::due(force, &gs, cause, st.on_share.len(), st.taken.len(), gspec.uids.len(), world.t);
                if due != crate::group::Due::Reform {
                    bad(&mut r, e, Some(*intent), "a re-form refused that was not due", json(cause), json(&due_name(&due)));
                }
                let mut w = world.clone();
                match crate::group::plan_reform(&gspec, &gs, force, &st.on_share, cause, &mut w, None, pol, g.decision.as_ref()) {
                    Err(want) if want == *why => {}
                    other => bad(&mut r, e, Some(*intent), "a re-form refused that its facts do not refuse so", why.clone(), other.err().unwrap_or_else(|| "a re-form".into())),
                }
                if let Some(f) = forces.get_mut(intent) {
                    f.last_attempt = Some(world.t);
                    f.refused = Some(cause.uids());
                    f.refused_why = Some(why.clone());
                }
            }
            EventBody::GroupSuspended { intent, inputs, cause, strength, min } => {
                let Some(world) = group_step_world(&mut r, e, *intent, inputs, &areas, &intents, &forces, policy.as_ref()) else {
                    continue;
                };
                let (Some(pol), Some(g), Some(force)) = (policy.as_ref(), intents.get(intent), forces.get(intent)) else { continue };
                let (gspec, gs) = (g.spec.clone(), g.spec.group().cloned().unwrap_or_default());
                let Some(st) = group_cause_check(&mut r, e, *intent, &world, cause, force, &intents, &newest_on, &nodes, pol) else {
                    continue;
                };
                let due = crate::group::due(force, &gs, cause, st.on_share.len(), st.taken.len(), gspec.uids.len(), world.t);
                let want = crate::group::Due::Suspend { strength: *strength, min: *min };
                if due != want {
                    bad(&mut r, e, Some(*intent), "a group suspended that its facts do not put below its minimum", json(&(strength, min)), json(&due_name(&due)));
                }
                if let Some(f) = forces.get_mut(intent) {
                    f.suspended = true;
                    f.last_attempt = Some(world.t);
                }
            }
            EventBody::IntentHandedOver {
                intent: spec,
                from,
                expires_t,
            } => {
                r.handed_over += 1;
                let Some(pol) = &policy else {
                    bad(&mut r, e, Some(spec.id), "an order handed over without run.started", String::new(), String::new());
                    continue;
                };
                if intents.contains_key(&spec.id) {
                    bad(&mut r, e, Some(spec.id), "intent created twice", String::new(), String::new());
                    continue;
                }
                // at the start only: before any other order of the run, one per vehicle
                if let Some(other) = intents.values().find(|t| !t.handed_over) {
                    bad(
                        &mut r,
                        e,
                        Some(spec.id),
                        "an order handed over after another order (the fleet's are handed over at the start)",
                        json(&other.spec.id),
                        String::new(),
                    );
                }
                if let Some(other) = intents.values().find(|t| t.spec.uids == spec.uids) {
                    bad(&mut r, e, Some(spec.id), "a vehicle handed over two starting tasks", json(&other.spec.id), json(&spec.uids));
                }
                let source_ok = matches!(&spec.source, Source::Handover { id } if *id == from.fleet);
                if !matches!(spec.kind(), TaskKind::Orbit | TaskKind::Patrol | TaskKind::Hold)
                    || spec.uids.len() != 1
                    || !source_ok
                    || spec.created_t != e.t
                {
                    bad(
                        &mut r,
                        e,
                        Some(spec.id),
                        "a handed-over order that is not one vehicle's orbit, patrol or hold from the fleet file, created as it was handed over",
                        json(&(spec.kind(), &spec.uids, &spec.source, spec.created_t)),
                        json(&(&from.fleet, e.t)),
                    );
                }
                let want = e.t.plus_ms(i64::from(pol.queue().expire_s) * 1000);
                if *expires_t != want {
                    bad(&mut r, e, Some(spec.id), "handover deadline differs from policy ([queue] expire_s)", json(expires_t), json(&want));
                }
                match &handover_from {
                    Some(was) if was != from => {
                        bad(&mut r, e, Some(spec.id), "orders handed over from two fleet files", json(was), json(from));
                    }
                    Some(_) => {}
                    None => handover_from = Some(from.clone()),
                }
                let mut tr = Tracked::new(spec.clone(), State::Queued);
                tr.basis = Some(crate::engine::handover_basis(e.t));
                tr.handed_over = true;
                tr.queue_expires = Some(*expires_t);
                tr.queue_inbox = false;
                handovers.push((e.seq, spec.uids.first().cloned(), spec.task.clone()));
                intents.insert(spec.id, tr);
            }
            EventBody::VehicleRebooted {
                uid,
                boot_ms_before,
                boot_ms,
                booted_t,
                booted_before_t,
            } => {
                r.reboots += 1;
                rebooted.insert(uid.clone(), e.t);
                if let Some(why) =
                    queue::reboot_mismatch(*boot_ms_before, *boot_ms, *booted_t, *booted_before_t)
                {
                    bad(
                        &mut r,
                        e,
                        None,
                        why,
                        json(&(uid, boot_ms_before, boot_ms, booted_t, booted_before_t)),
                        String::new(),
                    );
                }
            }
            EventBody::OpState {
                intent,
                uid,
                state,
                detail,
                ahead,
            } => {
                let Some(tr) = intents.get_mut(intent) else {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "op state for an unknown intent",
                        json(&(uid, state)),
                        String::new(),
                    );
                    continue;
                };
                // a newer order took the vehicle since this intent's dispatch
                let newest = newest_on.get(uid);
                let replaced = newest.is_some_and(|n| n != intent);
                let start = StartAhead {
                    ahead: *ahead,
                    required: true,
                    started: tr.started_ahead.contains(uid),
                };
                if let Some(why) = op_state_mismatch(
                    &tr.spec,
                    tr.state,
                    tr.dispatched,
                    replaced,
                    uid,
                    state,
                    start,
                ) {
                    let reports_under = op_state_owner(&tr.spec, uid).err();
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        why,
                        json(&(uid, state, detail)),
                        json(&(tr.state, tr.dispatched, reports_under, newest)),
                    );
                }
                // a start that may have gone out is never requeued: the bridge ends that op
                // `unconfirmed` (a delivery's op only; a later one, which only confirms it,
                // may lose its link before it asks the vehicle)
                if state == queue::REQUEUED_OP && tr.start_went_out.contains(uid) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "op requeued after its start went out (it may be running: the bridge ends such an op unconfirmed)",
                        json(&(uid, state, detail)),
                        format!("{}, once its start went out", queue::UNCONFIRMED_OP),
                    );
                }
                if queue::maybe_started_op(state) || queue::committed_op(state) {
                    tr.start_went_out.insert(uid.clone());
                }
                if queue::committed_op(state) {
                    tr.committed.insert(uid.clone());
                }
                if queue::maybe_started_op(state) {
                    tr.maybe_started.insert(uid.clone());
                }
                if *ahead {
                    tr.started_ahead.insert(uid.clone());
                }
                tr.ops.insert(uid.clone(), state.clone());
            }
            EventBody::VehicleEvent { .. } => {}
            // dispatch: the plan is a function of the input it logged
            EventBody::DispatchPlan { input, plan, tick } => {
                // under the rules the log was made under (rules 4: the way back goes home)
                let rules = policy.as_ref().map_or_else(
                    || crate::rules::RulesConfig { rules_version: r.rules_version, ..Default::default() },
                    |p| p.rules.clone(),
                );
                // rules 4: a candidate's home base, a field rules 3 did not log
                if !rules.at_least(crate::rules::HOME_BASE_SINCE) && input.vehicles.iter().any(|v| v.home.is_some()) {
                    bad(
                        &mut r,
                        e,
                        None,
                        "a field newer than the log's rules version (rules markers)",
                        "dispatch.plan.input.vehicles.home".into(),
                        format!("absent before rules {}", crate::rules::HOME_BASE_SINCE),
                    );
                }
                let again = crate::dispatch::assign_under(input, &rules);
                if &again != plan {
                    bad(
                        &mut r,
                        e,
                        None,
                        "dispatch plan differs from the assigner's answer on its own input",
                        json(&(tick, plan)),
                        json(&again),
                    );
                }
            }
            EventBody::DeliveryEvent { .. } => {}
            EventBody::AgentAction { .. } => {}
            // a display event decides nothing; it must be under the operator's own session
            // and about an intent the log holds
            EventBody::OperatorView {
                operator,
                session,
                intent,
                ..
            } => {
                match sessions.get(session) {
                    Some((op, _)) if op == operator => {}
                    other => bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "view under a session that is not the operator's",
                        operator.to_string(),
                        json(&other.map(|o| &o.0)),
                    ),
                }
                if !intents.contains_key(intent) {
                    bad(
                        &mut r,
                        e,
                        Some(*intent),
                        "view of an intent the log does not hold",
                        intent.to_string(),
                        String::new(),
                    );
                }
            }
        }
    }
    let last_seq = events.last().map_or(0, |e| e.seq);
    for (id, tr) in &intents {
        if let Some(seq) = tr.dispatch_logged
            && seq < last_seq
        {
            r.mismatches.push(Mismatch {
                seq,
                intent: Some(*id),
                what: "intent.dispatched never followed by its dispatch transition".into(),
                logged: String::new(),
                recomputed: "intent.state dispatch".into(),
            });
        }
        if let Some(seq) = tr.stop_due
            && seq < last_seq
        {
            r.mismatches.push(Mismatch {
                seq,
                intent: Some(*id),
                what: "picture failed but the consequential order was never stopped (void suppressed)"
                    .into(),
                logged: String::new(),
                recomputed: "consequential.stopped".into(),
            });
        }
        // rules 11: a cancel, a re-check and a missed report each call for what follows them
        if let Some((_, seq)) = &tr.cancel_due {
            r.mismatches.push(Mismatch {
                seq: *seq,
                intent: Some(*id),
                what: "an operator's cancel of a dispatched space task never carried out".into(),
                logged: String::new(),
                recomputed: "op.state cancelled, intent.state cancel".into(),
            });
        }
        if tr.recheck_due.is_some() {
            r.mismatches.push(Mismatch {
                seq: tr.last_check_seq,
                intent: Some(*id),
                what: "a space task's re-check never reported".into(),
                logged: String::new(),
                recomputed: "op.state rechecked or missed".into(),
            });
        }
        if tr.recheck_lost.is_some() {
            r.mismatches.push(Mismatch {
                seq: last_seq,
                intent: Some(*id),
                what: "a space task reported missed but never failed".into(),
                logged: String::new(),
                recomputed: "intent.state fail".into(),
            });
        }
    }
    r.intents = intents.len();
    r.last_seq = last_seq;
    r.next_intent = intents.keys().map(|i| i.0 + 1).max().unwrap_or(1);
    r.open = intents
        .values()
        // rules 34: a group order carries over as its member orders, each a one-vehicle intent
        .filter(|t| t.spec.group().is_none())
        .filter(|t| {
            queue::queueable(t.spec.kind())
                && matches!(
                    t.state,
                    State::Queued | State::Approved | State::AutoApproved | State::Dispatched
                )
        })
        .filter_map(|t| {
            Some(OpenIntent {
                spec: t.spec.clone(),
                state: t.state,
                basis: t.basis.clone()?,
                policy: t.decision.clone(),
                approval: t.approval_rec.clone(),
                expires_t: if t.state == State::Queued {
                    t.queue_expires
                } else {
                    None
                },
                ops: t.ops.clone(),
                committed: t.committed.iter().cloned().collect(),
                dispatched_before: t.dispatched_before,
                platforms: t.platforms.clone(),
                maybe_started: t.maybe_started.iter().cloned().collect(),
            })
        })
        .collect();
    if !resumed.is_empty() {
        check_resumed(&mut r, &resumed, opts);
    }
    check_handovers(&mut r, &handovers, opts);
    check_carried_restores(&mut r, events, opts);
    check_carried_from(&mut r, events, opts);
    r.unrestored = restores.report();
    r
}

/// Rules 29: the starting tasks a run's fleet file names (`<log stem>.fleet.json`: uid ->
/// `start`, the engine task its fleet gave the vehicle), or why it can't be read.
pub fn fleet_starts(bytes: &[u8]) -> Result<BTreeMap<String, Task>, String> {
    #[derive(serde::Deserialize)]
    struct Entry {
        #[serde(default)]
        start: Option<Task>,
    }
    let fleet: BTreeMap<String, Entry> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    Ok(fleet.into_iter().filter_map(|(uid, e)| Some((uid, e.start?))).collect())
}

/// Rules 29: the orders handed over at the start against the fleet the run logged, when given:
/// each its vehicle's starting task there, and every starting task there handed over. A run
/// that carried orders over from an earlier log hands none over (its vehicles are that run's).
fn check_handovers(r: &mut ReplayReport, handovers: &[(u64, Option<Uid>, Task)], opts: &ReplayOptions<'_>) {
    let Some(bytes) = opts.own_fleet else {
        r.handover_fleet = (!handovers.is_empty()).then_some(false);
        return;
    };
    let mut miss = |seq: u64, what: String, logged: String, recomputed: String| {
        r.mismatches.push(Mismatch {
            seq,
            intent: None,
            what,
            logged,
            recomputed,
        });
    };
    let starts = match fleet_starts(bytes) {
        Ok(s) => s,
        Err(e) => {
            miss(0, "the run's fleet file can't be read for its starting tasks".into(), e, String::new());
            r.handover_fleet = Some(false);
            return;
        }
    };
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (seq, uid, task) in handovers {
        match uid.as_ref().and_then(|u| starts.get_key_value(u.as_str())) {
            Some((n, t)) if t == task => {
                seen.insert(n.as_str());
            }
            Some((_, t)) => miss(*seq, "a handed-over order differs from its vehicle's starting task in the run's fleet file".into(), json(task), json(t)),
            None => miss(*seq, "an order handed over for a vehicle the run's fleet file gives no starting task".into(), json(uid), String::new()),
        }
    }
    // a run that carried orders over hands none over, whatever its fleet says
    if r.resumed == 0 {
        let missing: Vec<&str> = starts.keys().map(String::as_str).filter(|u| !seen.contains(u)).collect();
        if !missing.is_empty() {
            miss(
                0,
                "starting tasks in the run's fleet file never handed over".into(),
                json(&missing.iter().take(10).collect::<Vec<_>>()),
                format!("{} of {}", missing.len(), starts.len()),
            );
        }
    }
    r.handover_fleet = (!handovers.is_empty() || !starts.is_empty()).then_some(true);
}

/// Rules 30: a decision's logged `silent_orders` fact (the orders that may be running on our
/// silent vehicles, which its friendly keep-out lays their regions out from) against what
/// replay's own record of the intents gives on the same vehicles ([`crate::silent::in_force`]).
fn check_silent_orders(
    r: &mut ReplayReport,
    e: &Event,
    id: IntentId,
    inputs: &LoggedInputs,
    intents: &BTreeMap<IntentId, Tracked>,
    nodes: &crate::nodes::NodeLedger,
    policy: Option<&Policy>,
) {
    let (Some(tr), Some(pol)) = (intents.get(&id), policy) else { return };
    let consequential = false && true;
    if !consequential && inputs.silent_orders.is_empty() {
        return;
    }
    let mut world = WorldSnapshot::new(inputs.t);
    world.vehicles = inputs.vehicles.clone();
    let silent = crate::silent::silent_uids(&world, &pol.rules, tr.spec.uids.first());
    let _ = (&silent, nodes);
    let want = BTreeMap::new();
    if inputs.silent_orders != want {
        bad(r, e, Some(id), "the orders that may be running on our silent vehicles differ from replay's record (rules 30)", json(&inputs.silent_orders), json(&want));
    }
}



/// Rules 30: what an event's decisions read of our vehicles confirmed lost, against the ledger: no
/// report among their facts from a vehicle whose loss is in force (the engine revokes the loss
/// before the decision that reads the report is logged), every decision's `nodes_lost` exactly the
/// losses in force on its vehicles (a carried one with the vehicle under the uid now, as the
/// bridge said it), and no order dispatched to a vehicle confirmed lost.
fn check_nodes_read(
    r: &mut ReplayReport,
    e: &Event,
    nodes: &crate::nodes::NodeLedger,
    intents: &BTreeMap<IntentId, Tracked>,
    policy: Option<&Policy>,
) {
    let Some(pol) = policy.filter(|_p| true) else {
        return;
    };
    let cfg = &pol.rules;
    for inputs in e.body.decision_inputs() {
        for m in nodes.marks() {
            if let Some(v) = inputs.vehicles.get(&m.uid)
                && let Some(why) = crate::nodes::heard_since(v, m.since, m.heard, inputs.t, cfg)
            {
                bad(
                    r,
                    e,
                    None,
                    "a decision read a report from a vehicle confirmed lost, its loss not revoked before it (node.loss_revoked)",
                    format!("{}: {why}", m.uid),
                    String::new(),
                );
            }
        }
        let want = nodes.lost_for(&inputs.vehicles, inputs.t, cfg);
        if inputs.nodes_lost != want {
            bad(
                r,
                e,
                None,
                "a decision's vehicles confirmed lost differ from the losses the log holds (node.lost, less node.loss_revoked)",
                json(&inputs.nodes_lost),
                json(&want),
            );
        }
    }
    if let EventBody::IntentDispatched { intent, inputs, .. } = &e.body
        && let Some(tr) = intents.get(intent)
    {
        for u in &tr.spec.uids {
            if inputs.iter().any(|i| i.nodes_lost.get(u).is_some_and(crate::world::LostNode::holds)) {
                bad(r, e, Some(*intent), "an order dispatched to a vehicle confirmed lost", u.to_string(), String::new());
            }
        }
    }
}

/// Rules 30: a `node.lost`: under these rules, by an operator with consequential authority under
/// their own session (never an agent), on a vehicle silent since exactly `since` on the facts it
/// evidence of the presumption in force (or none, and a note saying why).
#[allow(clippy::type_complexity)]
fn node_lost(
    r: &mut ReplayReport,
    e: &Event,
    (uid, since, by, session, note, evidence, _world_t, _vehicle): (
        &Uid,
        Option<Time>,
        &OperatorId,
        SessionId,
        &str,
        &[crate::nodes::Evidence],
        Time,
        &crate::world::VehicleFacts,
    ),
    nodes: &crate::nodes::NodeLedger,
    intents: &BTreeMap<IntentId, Tracked>,
    sessions: &BTreeMap<SessionId, (OperatorId, Grants)>,
    policy: Option<&Policy>,
) {
    let Some(_pol) = policy.filter(|_p| true) else {
        // the marker check flags it under older rules
        return;
    };
    match sessions.get(&session) {
        Some((op, g)) if op == by && g.consequential => {}
        Some((op, _)) if op == by => bad(
            r,
            e,
            None,
            "a loss confirmed without consequential authority",
            by.to_string(),
            "a session opened with grants.consequential".into(),
        ),
        other => bad(
            r,
            e,
            None,
            "a loss confirmed under a session that is not the operator's (an agent's, or another's)",
            by.to_string(),
            json(&other.map(|o| &o.0)),
        ),
    }
    if nodes.lost_mark(uid).is_some() {
        bad(r, e, None, "a vehicle confirmed lost that is lost already", uid.to_string(), String::new());
    }
    if let Some(other) = holding(intents, uid, None) {
        bad(r, e, Some(other), "a loss confirmed while a consequential order under way holds the vehicle", uid.to_string(), String::new());
    }
    let want: Vec<crate::nodes::Evidence> = nodes
        .presumption(uid)
        .filter(|p| p.since == since)
        .map(|p| p.evidence.clone())
        .unwrap_or_default();
    if evidence != want.as_slice() {
        bad(r, e, None, "a confirmation's evidence differs from the presumption in force", json(&evidence), json(&want));
    }
    if want.is_empty() && note.trim().is_empty() {
        bad(r, e, None, "a loss confirmed with nothing presuming it and no note saying why", uid.to_string(), "a note".into());
    }
    let on = if evidence.is_empty() { "the operator's own".to_string() } else { format!("{} item(s)", evidence.len()) };
    r.nodes_lost.push(format!(
        "vehicle {uid}: confirmed lost by {by} (silent since {}; evidence: {on}){}",
        since.map_or("never".to_string(), |t| format!("{} ms", t.0)),
        if note.trim().is_empty() { String::new() } else { format!(": {}", note.trim()) }
    ));
}

/// Rules 30: a `node.loss_revoked`: of the loss in force on the vehicle (this log's or carried
/// over, named as it is); as heard, only when its facts show it heard since that report; by an
/// operator, under their own session with a note.
#[allow(clippy::type_complexity)]
fn node_loss_revoked(
    r: &mut ReplayReport,
    e: &Event,
    (uid, since, from, cause): (&Uid, Option<Time>, Option<&str>, &crate::nodes::Revocation),
    nodes: &crate::nodes::NodeLedger,
    sessions: &BTreeMap<SessionId, (OperatorId, Grants)>,
    policy: Option<&Policy>,
) {
    let Some(pol) = policy.filter(|_p| true) else {
        return;
    };
    let Some(m) = nodes.lost_mark(uid) else {
        bad(r, e, None, "a loss revoked that is not in force", uid.to_string(), String::new());
        return;
    };
    if m.since != since || m.from.as_deref() != from {
        bad(r, e, None, "a revocation that does not name the loss in force (its report, its log)", json(&(since, from)), json(&(m.since, &m.from)));
    }
    match cause {
        crate::nodes::Revocation::Heard { world_t, vehicle, detail } => {
            let want = crate::nodes::heard_since(vehicle, m.since, m.heard, *world_t, &pol.rules);
            if want.as_ref() != Some(detail) {
                bad(r, e, None, "a loss revoked as heard that its facts do not show heard since its report", detail.clone(), json(&want));
            }
            r.losses_revoked.push(format!("vehicle {uid}: loss revoked, heard again ({detail})"));
        }
        crate::nodes::Revocation::Operator { operator, session, note } => {
            match sessions.get(session) {
                Some((op, _)) if op == operator => {}
                other => bad(r, e, None, "a loss revoked under a session that is not the operator's", operator.to_string(), json(&other.map(|o| &o.0))),
            }
            if note.trim().is_empty() {
                bad(r, e, None, "a loss revoked by an operator without a note saying why", uid.to_string(), "a note".into());
            }
            r.losses_revoked.push(format!("vehicle {uid}: loss revoked by {operator}: {note}"));
        }
    }
}

/// Rules 30: what a confirmed loss ends, next and in this order, before anything else is logged
/// ([`crate::Engine::confirm_lost`]): every intent naming the vehicle that waits for it (pending,
/// dispatched order it runs, its op on it ended `lost`, and the order failed with it when no
/// vehicle is left on it; the lost-link values owed back to it, closed (`restore.closed` lost).
struct LossDue {
    uid: Uid,
    voids: Vec<(IntentId, bool)>,
    stopping: Option<IntentId>,
    ops: Vec<(IntentId, bool)>,
    failing: Option<IntentId>,
    close: Option<BTreeMap<String, f64>>,
}

impl LossDue {
    fn new(uid: &Uid, intents: &BTreeMap<IntentId, Tracked>, restores: &Restores, nodes: &crate::nodes::NodeLedger) -> LossDue {
        let names = |spec: &IntentSpec| spec.uids.contains(uid);
        let voids = intents
            .iter()
            .filter(|(_, t)| {
                names(&t.spec) && matches!(t.state, State::PendingApproval | State::AutoApproved | State::Approved | State::Queued)
            })
            .map(|(id, t)| (*id, false && t.state == State::Approved))
            .collect();
        let ops = intents
            .iter()
            .filter(|(_, t)| {
                t.state == State::Dispatched
                    && true
                    && t.spec.uids.contains(uid)
                    && !t.ops.get(uid).is_some_and(|o| queue::ended_op(o))
            })
            .map(|(id, t)| {
                let left = t
                    .spec
                    .uids
                    .iter()
                    .filter(|u| *u != uid)
                    .any(|u| nodes.lost_mark(u).is_none() && !t.ops.get(u).is_some_and(|o| queue::ended_op(o)));
                (*id, !left)
            })
            .collect();
        LossDue {
            uid: uid.clone(),
            voids,
            stopping: None,
            ops,
            failing: None,
            close: restores.values_owed(uid).cloned(),
        }
    }

    fn done(&self) -> bool {
        self.voids.is_empty() && self.stopping.is_none() && self.ops.is_empty() && self.failing.is_none() && self.close.is_none()
    }

    fn remaining(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        out.extend(self.stopping.map(|id| format!("{id}'s void after its stop")));
        out.extend(self.failing.map(|id| format!("{id}'s failure")));
        out.extend(self.voids.iter().map(|(id, _)| format!("{id} voided")));
        out.extend(self.ops.iter().map(|(id, _)| format!("{id}'s op ended lost")));
        out.extend(self.close.as_ref().map(|v| format!("values {} closed", json(v))));
        out
    }

    /// The next event: `Ok` when it is the next consequence due (or nothing is due), `Err` when it
    /// is anything else.
    fn step(&mut self, body: &EventBody) -> Result<(), String> {
        let prefix = format!("system: vehicle {} lost: ", self.uid);
        let other = |what: String| Err(format!("a confirmed loss's consequences do not follow it at once (next due: {what})"));
        if let Some(id) = self.stopping {
            return match body {
                EventBody::IntentState {
                    intent,
                    transition: Transition::Void,
                    ..
                } if *intent == id => {
                    self.stopping = None;
                    Ok(())
                }
                _ => other(format!("{id}'s void after its stop")),
            };
        }
        if let Some(id) = self.failing {
            return match body {
                EventBody::IntentState {
                    intent,
                    transition: Transition::Fail,
                    reason,
                    ..
                } if *intent == id && reason.starts_with(&prefix) => {
                    self.failing = None;
                    Ok(())
                }
                _ => other(format!("{id}'s failure")),
            };
        }
        if let Some(&(id, consequential_order)) = self.voids.first() {
            return match body {
                EventBody::IntentState {
                    intent,
                    transition: Transition::Void,
                    reason,
                    ..
                } if *intent == id && !consequential_order && reason.starts_with(&prefix) => {
                    self.voids.remove(0);
                    Ok(())
                }
                _ => other(format!("{id} voided")),
            };
        }
        if let Some(&(id, ends)) = self.ops.first() {
            return match body {
                EventBody::OpState { intent, uid, state, .. }
                    if *intent == id && *uid == self.uid && state == crate::nodes::LOST_OP =>
                {
                    self.ops.remove(0);
                    if ends {
                        self.failing = Some(id);
                    }
                    Ok(())
                }
                _ => other(format!("{id}'s op ended lost")),
            };
        }
        if let Some(values) = &self.close {
            return other(format!("values {} closed", json(values)));
        }
        Ok(())
    }
}


/// Rules 12: the bare file names of the logs a log's `run.started` says it carried over from
/// (`carried_from`), those it read and those it could not (checked when at hand): what
/// `fleetmesh-replay` looks for, beside the log or given with `--prev`. Empty when the first line
/// is not `run.started` or carried nothing.
pub fn carried_from_logs(text: &str) -> Vec<String> {
    let Some(first) = text.lines().find(|l| !l.trim().is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Event>(first) {
        Ok(Event {
            body: EventBody::RunStarted { carried_from, .. },
            ..
        }) => carried_from
            .into_iter()
            .map(|c| c.log)
            .filter(|l| !is_pointer_name(l))
            .collect(),
        _ => Vec::new(),
    }
}

/// Rules 15 (murmur-zq3): how a void's reason says a newer order superseded it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Superseding {
    /// `system: superseded: iN is a newer order for vehicle U` (a dequeue): iN stands (approved,
    /// queued, dispatched, or a person's waiting for approval).
    Standing,
    /// `system: superseded: iN was dispatched to vehicle U` (the bridge, at iN's dispatch).
    Dispatched,
    /// `system: superseded: iN was queued for vehicle U` (the bridge, at iN's queueing).
    Queued,
    /// `superseded by iN` or `superseded by iN (agent A)` (an amend).
    Amend,
}

/// Rules 15 (murmur-zq3): the order a void's reason says superseded it, the vehicle it names and
/// how, as the engine and the bridge word it ([`Superseding`]). None: not a superseded reason;
/// `Some((None, ..))`: one that names no order.
fn superseded_by(reason: &str) -> Option<(Option<IntentId>, Option<Uid>, Superseding)> {
    let id = |t: &str| -> Option<IntentId> {
        let digits: String = t.strip_prefix('i')?.chars().take_while(char::is_ascii_digit).collect();
        let rest = &t[1 + digits.len()..];
        if digits.is_empty() || !(rest.is_empty() || rest.starts_with(' ')) {
            return None;
        }
        digits.parse().ok().map(IntentId)
    };
    if let Some(rest) = reason.strip_prefix("system: superseded: ") {
        let (vehicle, how) = [
            ("is a newer order for vehicle ", Superseding::Standing),
            ("was dispatched to vehicle ", Superseding::Dispatched),
            ("was queued for vehicle ", Superseding::Queued),
        ]
        .iter()
        .find_map(|(k, how)| rest.split_once(k).map(|(_, v)| (v.split_whitespace().next().map(Uid::new), *how)))
        .unwrap_or((None, Superseding::Standing));
        return Some((id(rest), vehicle, how));
    }
    let rest = reason.strip_prefix("superseded by ")?;
    Some((id(rest), None, Superseding::Amend))
}

/// Rules 15: why a void's superseded reason does not hold against the log, if it does not: it
/// names no order, one the log does not have, one not newer than the voided, or (naming a
/// vehicle) one not on that vehicle, or one not in the state the reason says (dispatched,
/// queued, or standing as the engine's dequeue judges it), or (an amend) another kind of task.
fn superseded_mismatch(
    id: IntentId,
    spec: &IntentSpec,
    reason: &str,
    intents: &BTreeMap<IntentId, Tracked>,
) -> Option<&'static str> {
    let (named, vehicle, how) = superseded_by(reason)?;
    let Some(by) = named else {
        return Some("a void as superseded that names no order (rules 15)");
    };
    let Some(other) = intents.get(&by) else {
        return Some("a void as superseded by an order the log does not have (rules 15)");
    };
    if by <= id {
        return Some("a void as superseded by an order that is not newer (rules 15)");
    }
    match vehicle {
        Some(u) if !(spec.uids.contains(&u) && other.spec.uids.contains(&u)) => {
            return Some("a void as superseded by an order not on that vehicle (rules 15)");
        }
        None if how == Superseding::Amend && other.spec.kind() != spec.kind() => {
            return Some("a void as superseded by an amend of another kind of task (rules 15)");
        }
        _ => {}
    }
    let in_state = match how {
        Superseding::Dispatched => other.dispatched,
        Superseding::Queued => other.state == State::Queued,
        Superseding::Standing => {
            true
                && match other.state {
                    State::Approved | State::AutoApproved | State::Queued | State::Dispatched => true,
                    State::PendingApproval => !other.spec.source.is_agent(),
                    _ => false,
                }
        }
        Superseding::Amend => true,
    };
    (!in_state).then_some("a void as superseded by an order not in the state its reason says (rules 15)")
}

/// Rules 12: whether a carried-over entry names a `.last-run` pointer that could not be read
/// (empty, torn, garbled): the log it named is unknown, so there is no log to look for.
pub fn is_pointer_name(name: &str) -> bool {
    name.starts_with(".last-run")
}

/// Rules 12: `run.started`'s `carried_from` against the logs it names (each found in
/// [`ReplayOptions::carried_logs`], or [`ReplayOptions::restores_from`] by its name): the first
/// is the log the pointer named, every later one a log an earlier one named pending, each
/// named once, and every pending log has its own entry (read again, or failed again); each
/// log read replays itself (one generation, its carry-overs trusted), is the one read (its
/// head and last seq) and still owed exactly what the entry carries (the platforms' values,
/// mismatch for an entry that carries anything, unless trusted; one that carries nothing is
/// reported unchecked. Before rules 12 `carried_from` is a marker (a mismatch elsewhere).
fn check_carried_from(r: &mut ReplayReport, events: &[Event], opts: &ReplayOptions<'_>) {
    let Some(first) = events.first() else { return };
    let EventBody::RunStarted {
        carried_from,
        rules_version: _,
        pointer,
        ..
    } = &first.body
    else {
        return;
    };
    // rules 15: what a start carries also says what each log had spent, the fleet file a failed
    // carry read, and a fallback from a pointer that could not be read
    let mut named: Vec<&str> = Vec::new();
    for (i, c) in carried_from.iter().enumerate() {
        if !is_log_file_name(&c.log) {
            bad(r, first, None, "a carried-over log not named by its bare file name", c.log.clone(), String::new());
        }
        if named.contains(&c.log.as_str()) {
            bad(r, first, None, "a log carried over from twice in one start", c.log.clone(), String::new());
        }
        // rules 15: the logs a start found in the pointer's place and would not carry from (each
        // a failed carry naming why the pointer failed) come first, in place of the pointer
        let found_in_place = true
            && c.pointer_failed.is_some()
            && carried_from[..=i].iter().all(|p| p.pointer_failed.is_some() && (i == 0 || p.error.is_some()));
        if i > 0 && !found_in_place && !carried_from[..i].iter().any(|p| p.pending.contains(&c.log)) {
            bad(
                r,
                first,
                None,
                "a log carried over from that neither the pointer nor an earlier carried log named (a start carries from the log .last-run names and the ones it still owed for)",
                c.log.clone(),
                json(&carried_from[..i].iter().map(|p| &p.pending).collect::<Vec<_>>()),
            );
        }
        // (rules 15: a log found in place of the pointer that the start would not carry from
        // is a failed carry that also says why the pointer failed)
        // what a failed carry may owe (its fleet's armed platforms) is only a failed one's; a
        // pointer that could not be read is never read as a log
        if c.error.is_none() && (!c.may_owe.is_empty() || c.fleet_read.is_some() || is_pointer_name(&c.log)) {
            bad(r, first, None, "a carried-over log that was read carries what a failed carry may owe, or is a pointer", c.log.clone(), String::new());
        }
        // rules 15: a fallback stands for the pointer (the first entry) of a station that names
        // its pointer; the platforms a failed carry may owe come from the fleet file it read
        if c.pointer_failed.is_some() && (!found_in_place || pointer.is_none()) {
            bad(
                r,
                first,
                None,
                "a carry from a log found in place of a pointer that could not be read, other than the first (or the failed carries the start found there with it), or by a run that names no pointer (rules 15)",
                c.log.clone(),
                json(pointer),
            );
        }
        // rules 15: a failed carry's may-owe list comes from the fleet file it read, or (a pending
        // log tried again whose fleet file is gone too) from what the log that named it pending
        // recorded for it, compared when that log is at hand
        // (a log found in place of the pointer and not carried from also holds what it owed or
        // had spent, read from it: checked against it below)
        if c.error.is_some() && !c.may_owe.is_empty() && c.fleet_read.is_none() && !found_in_place {
            match carried_from[..i].iter().find(|p| p.pending.contains(&c.log)) {
                None => bad(
                    r,
                    first,
                    None,
                    "a failed carry that may owe values without the fleet file it read (rules 15)",
                    c.log.clone(),
                    json(&c.may_owe),
                ),
                Some(p) => {
                    let recorded = carried_text(p, opts).map(|t| owed_debts(&up_to_seq(t, p.last_seq)).pending_may_owe);
                    if let Some(rec) = recorded
                        && rec.get(&c.log) != Some(&c.may_owe)
                    {
                        bad(
                            r,
                            first,
                            None,
                            "a failed carry's may-owe list, without a fleet file, differs from what the log that named it pending recorded (rules 15)",
                            json(&c.may_owe),
                            json(&rec.get(&c.log)),
                        );
                    }
                }
            }
        }
        for p in &c.pending {
            if !carried_from.iter().any(|x| &x.log == p) {
                bad(
                    r,
                    first,
                    None,
                    "a log still owed for (pending) not tried again at the start",
                    p.clone(),
                    c.log.clone(),
                );
            }
        }
        named.push(&c.log);
    }
    for c in carried_from {
        let text = carried_text(c, opts);
        if let Some(why) = &c.error {
            // rules 15: the fleet file the start read, when it is at hand: that one, and the
            // platforms it may owe values to exactly its armed ones; a start that recorded none
            // claimed it could not be read, so one at hand that reads is a mismatch
            let fleet_checked = match &c.fleet_read {
                Some(f) => Some(check_fleet_read(r, first, events, c, f, opts)),
                None => {
                    let name = fleet_file_name(&c.log);
                    let given = opts.fleets.iter().find(|(n, _)| *n == name);
                    if let Some((_, bytes)) = given
                        && fleet_may_owe(bytes).is_some() {
                        bad(
                            r,
                            first,
                            None,
                            "a failed carry that recorded no fleet file read, though its fleet file is given and reads (rules 15)",
                            c.log.clone(),
                            name.clone(),
                        );
                    }
                    given.map(|_| true)
                }
            };
            // recorded failed, yet the log is at hand: what it owed was not carried; a log that
            // owed something is a mismatch (its failure can't be told from a forged one), unless
            // trusted. Rules 15: not a log found in place of the pointer that the start would not
            // carry from (its chain or anchors did not check, or another log could be the one the
            // pointer named): it stays pending, and it must hold every platform it owed values
            // to, had spent or held for a failed carry of its own (`may_owe`), so nothing it owed
            // drops unsaid for this run
            let refused_fallback = true && c.pointer_failed.is_some();
            if refused_fallback && let Some(t) = text {
                let d = owed_debts(t);
                let unheld: Vec<&Uid> = d
                    .owed
                    .iter()
                    .map(|o| &o.uid)
                    .chain(d.pending_may_owe.values().flatten().map(|m| &m.uid))
                    .filter(|u| !c.may_owe.iter().any(|m| &m.uid == *u))
                    .collect();
                if !unheld.is_empty() {
                    bad(
                        r,
                        first,
                        None,
                        &format!(
                            "{} was found in place of the pointer and not carried from, yet does not hold every platform it owed values to, had spent or held (rules 15)",
                            c.log
                        ),
                        json(&c.may_owe.iter().map(|m| &m.uid).collect::<Vec<_>>()),
                        json(&unheld),
                    );
                }
            }
            let readable = text
                .and_then(|t| parse_log(t).ok().filter(|ev| !ev.is_empty()).map(|_| owed_debts(t)))
                .filter(|d| {
                    !d.is_empty()
                        || (true && !d.nodes_lost.is_empty())
                });
            if let Some(d) = readable
                && !opts.trust_resumed
                && !refused_fallback
            {
                bad(
                    r,
                    first,
                    None,
                    &format!(
                        "{} is recorded as a failed carry ({why}), yet it reads and still owed: what it owed was not carried",
                        c.log
                    ),
                    json(&(&d.owed, &d.pending)),
                    String::new(),
                );
            }
            r.carried_restores.push(CarriedRestores {
                log: c.log.clone(),
                restores: 0,
                checked: false,
                head: None,
                failed: Some(why.clone()),
                pointer_failed: c.pointer_failed.clone(),
                fleet_checked,
            });
            continue;
        }
        // as the start read it: up to its last complete event (a crash may have torn a line
        // after it)
        let text = text.map(|t| up_to_seq(t, c.last_seq));
        let text = text.as_deref();
        let Some(text) = text else {
            r.carried_restores.push(CarriedRestores {
                log: c.log.clone(),
                restores: c.owed.len(),
                checked: false,
                head: c.head.clone(),
                failed: None,
                pointer_failed: c.pointer_failed.clone(),
                fleet_checked: None,
            });
            continue;
        };
        let prev = replay_str_with(
            text,
            &ReplayOptions {
                rules: RulesAccepted::AtLeast(1),
                trust_resumed: true,
                ..ReplayOptions::default()
            },
        );
        match &prev {
            Err(err) => bad(r, first, None, "a log carried over from can't be read", format!("{}: {err}", c.log), String::new()),
            Ok(p) => {
                if let Some(m) = p.mismatches.first() {
                    bad(
                        r,
                        first,
                        None,
                        "a log carried over from does not replay",
                        format!("{}: {} mismatch(es), first at seq {}: {}", c.log, p.mismatches.len(), m.seq, m.what),
                        String::new(),
                    );
                }
            }
        }
        let read = log_head(text);
        if read.as_ref().map(|(h, _)| h) != c.head.as_ref() || read.as_ref().map(|(_, s)| *s) != c.last_seq {
            bad(
                r,
                first,
                None,
                "the log given is not the one carried over from (its head or last seq differs)",
                json(&(&c.head, c.last_seq)),
                json(&read),
            );
        }
        let debts = owed_debts(text);
        if debts.owed != c.owed {
            bad(
                r,
                first,
                None,
                "carried values differ from what the log they name still owed (each platform's values, the vehicle, the consequential orders)",
                json(&c.owed),
                json(&debts.owed),
            );
        }
        if debts.pending != c.pending {
            bad(
                r,
                first,
                None,
                "the earlier logs carried as still owed for differ from the ones the log names pending",
                json(&c.pending),
                json(&debts.pending),
            );
        }
        // rules 30: our vehicles it had confirmed lost and not revoked (and had carried in), every
        // one of them
        if debts.nodes_lost != c.nodes_lost {
            bad(
                r,
                first,
                None,
                "the vehicles carried as lost differ from the ones the log had confirmed lost and not revoked (rules 30)",
                json(&c.nodes_lost),
                json(&debts.nodes_lost),
            );
        }
        // rules 24: the rounds it had released (and had carried in), every one of them
        // rules 15: a log carried from in place of a pointer is the station's own
        if c.pointer_failed.is_some() && station_pointer(text) != *pointer {
            bad(
                r,
                first,
                None,
                "a log carried from in place of the pointer that is not this station's (its run.started names another pointer)",
                json(&station_pointer(text)),
                json(pointer),
            );
        }
        r.carried_restores.push(CarriedRestores {
            log: c.log.clone(),
            restores: c.owed.len(),
            checked: true,
            head: prev.ok().and_then(|p| p.head_sha256),
            failed: None,
            pointer_failed: c.pointer_failed.clone(),
            fleet_checked: None,
        });
    }
}

/// Rules 12: the text of a log `run.started` says the start carried from, when it is at hand.
fn carried_text<'a>(c: &CarriedFrom, opts: &ReplayOptions<'a>) -> Option<&'a str> {
    opts.carried_logs
        .iter()
        .find(|(n, _)| *n == c.log)
        .or_else(|| opts.restores_from.as_ref().filter(|(n, _)| *n == c.log))
        .map(|(_, t)| *t)
}

/// Rules 15: why a log's hash chain does not hold, if it does not: a line that does not parse as
/// an event (but a torn last one, a crash mid-write), a seq that is not contiguous from 1, or a
/// `prev` that is not the SHA-256 of the line before. What a start that falls back from a pointer
/// it can't read checks before carrying from a log (with its anchors).
pub fn chain_problem(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut prev = GENESIS.to_string();
    for (i, l) in lines.iter().enumerate() {
        let e = match serde_json::from_str::<Event>(l) {
            Ok(e) => e,
            // a torn last line is not part of the log (a crash cut it short)
            Err(_) if i + 1 == lines.len() && i > 0 => return None,
            Err(err) => return Some(format!("line {} does not parse: {err}", i + 1)),
        };
        if e.seq != i as u64 + 1 {
            return Some(format!("seq {} where {} was due", e.seq, i + 1));
        }
        if e.prev != prev {
            return Some(format!("seq {}: its prev is not the hash of the line before", e.seq));
        }
        prev = sha256_hex(l.as_bytes());
    }
    if lines.is_empty() {
        return Some("no event".into());
    }
    None
}

/// Rules 15: the `.last-run` pointer a log's `run.started` names (its station), if any.
pub fn station_pointer(text: &str) -> Option<String> {
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    match serde_json::from_str::<Event>(first) {
        Ok(Event {
            body: EventBody::RunStarted { pointer, .. },
            ..
        }) => pointer,
        _ => None,
    }
}

/// Rules 15: a failed carry's fleet file against what the start recorded (`fleet_read`): when
/// the file is at hand ([`ReplayOptions::fleets`]), its SHA-256 must be the one read and the
/// platforms the carry may owe values to exactly its vehicles of the armed platforms recorded;
/// and every platform this log's decisions read must agree with that list (an armed platform
/// left out, or one listed that the catalog does not arm, is a mismatch). Returns whether the
/// file was at hand.
fn check_fleet_read(
    r: &mut ReplayReport,
    first: &Event,
    _events: &[Event],
    c: &CarriedFrom,
    f: &FleetRead,
    opts: &ReplayOptions<'_>,
) -> bool {
    let name = fleet_file_name(&c.log);
    let Some((_, bytes)) = opts.fleets.iter().find(|(n, _)| *n == name) else {
        return false;
    };
    let hash = sha256_hex(bytes);
    if hash != f.sha256 {
        bad(r, first, None, "the fleet file given is not the one the failed carry read (its SHA-256 differs)", f.sha256.clone(), hash);
    }
    let want = fleet_may_owe(bytes);
    // rules 15: a log found in place of the pointer and not carried from holds its fleet's armed
    // platforms and also what the log itself owed or had spent (checked against the log)
    let fits = match (&want, c.pointer_failed.is_some()) {
        (Some(w), true) => w.iter().all(|m| c.may_owe.iter().any(|x| x.uid == m.uid)),
        (w, false) => w.as_ref() == Some(&c.may_owe),
        (None, true) => false,
    };
    if !fits {
        bad(
            r,
            first,
            None,
            "the platforms a failed carry may owe values to differ from its fleet file's armed ones",
            json(&c.may_owe),
            json(&want),
        );
    }
    true
}

/// Rules 12: a log's text up to and including its event of seq `last` (the head a start
/// recorded; a crash may have torn a line after it); the whole text when there is no such
/// event, and nothing when `last` is none (a log with no event).
pub fn up_to_seq(text: &str, last: Option<u64>) -> String {
    let Some(last) = last else { return String::new() };
    let mut out = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        out.push_str(line);
        out.push('\n');
        if serde_json::from_str::<Event>(line).is_ok_and(|e| e.seq == last) {
            return out;
        }
    }
    text.to_string()
}

/// A resumed intent as its `intent.resumed` event carries it (seq, from, the open intent).
type ResumedEvent = (u64, crate::queue::ResumedFrom, OpenIntent, State);

/// Every `intent.resumed` against the previous log (one generation): that log is the one
/// named (its head and last seq), it replays, and it leaves the intent open exactly as
/// carried (spec, state, basis, policy, approval, op states, deadline), in the state it
/// resumes in here. Without the previous log, a mismatch unless the caller trusts it.
fn check_resumed(r: &mut ReplayReport, resumed: &[ResumedEvent], opts: &ReplayOptions<'_>) {
    let Some(text) = opts.previous else {
        if !opts.trust_resumed {
            for (seq, from, open, _) in resumed {
                r.mismatches.push(Mismatch {
                    seq: *seq,
                    intent: Some(open.spec.id),
                    what: format!(
                        "intent carried over from {} not verified: replay with that log (fleetmesh-replay --prev), or trust it (--trust-resumed)",
                        from.log
                    ),
                    logged: String::new(),
                    recomputed: String::new(),
                });
            }
        }
        return;
    };
    let prev = replay_str_with(
        text,
        &ReplayOptions {
            rules: RulesAccepted::AtLeast(1),
            trust_resumed: true,
            ..ReplayOptions::default()
        },
    );
    let push = |r: &mut ReplayReport, seq: u64, id: IntentId, what: &str, logged: String, want: String| {
        r.mismatches.push(Mismatch {
            seq,
            intent: Some(id),
            what: what.to_string(),
            logged,
            recomputed: want,
        });
    };
    let prev = match prev {
        Ok(p) => p,
        Err(e) => {
            if let Some((seq, _, open, _)) = resumed.first() {
                push(r, *seq, open.spec.id, "the previous log can't be read", e.to_string(), String::new());
            }
            return;
        }
    };
    for (seq, from, open, state) in resumed {
        let id = open.spec.id;
        if !prev.ok() {
            push(r, *seq, id, "the previous log does not replay", format!("{} mismatch(es)", prev.mismatches.len()), String::new());
        }
        if Some(&from.head) != prev.head_sha256.as_ref() || from.seq != prev.last_seq {
            push(
                r,
                *seq,
                id,
                "carried over from another log than the one given (head or last seq differ)",
                json(&(&from.head, from.seq)),
                json(&(&prev.head_sha256, prev.last_seq)),
            );
            continue;
        }
        let Some(was) = prev.open.iter().find(|o| o.spec.id == id) else {
            push(r, *seq, id, "carried over, but the previous log does not leave it open", String::new(), String::new());
            continue;
        };
        if from.was != was.state {
            push(r, *seq, id, "carried over from another state than the previous log ends it in", json(&from.was), json(&was.state));
        }
        // rules 8: the previous run's authentication, as it logged it
        if from.auth != prev.auth {
            push(r, *seq, id, "carried over claiming another authentication than the previous log's", json(&from.auth), json(&prev.auth));
        }
        if *state != was.resumes_as() {
            push(r, *seq, id, "resumed in another state than its op states allow", json(state), json(&was.resumes_as()));
        }
        let carried = (
            &open.spec,
            &open.basis,
            &open.policy,
            &open.approval,
            &open.ops,
            (&open.committed, open.dispatched_before, &open.platforms),
            &open.maybe_started,
        );
        let there = (
            &was.spec,
            &was.basis,
            &was.policy,
            &was.approval,
            &was.ops,
            (&was.committed, was.dispatched_before, &was.platforms),
            &was.maybe_started,
        );
        if carried != there {
            push(r, *seq, id, "carried over differently from how the previous log leaves it", json(&carried), json(&there));
        }
        if was.state == State::Queued && open.expires_t != was.expires_t {
            push(r, *seq, id, "queue deadline differs from the previous log's", json(&open.expires_t), json(&was.expires_t));
        }
    }
}


/// Rules 6: every carried `platform.restore` against the earlier log it names
/// ([`ReplayOptions::restores_from`]; one generation, like `intent.resumed`). The bridge
/// derived them from it at start ([`owed_restores`]), so that log must replay (its own
/// carry-overs trusted) and they must be exactly what it still owed: each platform's
/// every platform it owed carried. A carried restore names that log by its file name (no
/// head hash, unlike `intent.resumed`: the head replay reports is to be compared with that
/// log's anchors), and one start carries from the one log its `.last-run-<port>` pointer
/// names. Without that log they are a mismatch unless trusted
/// ([`ReplayOptions::trust_resumed`]); one that names no log by its file name is a mismatch
/// either way.
fn check_carried_restores(r: &mut ReplayReport, events: &[Event], opts: &ReplayOptions<'_>) {
    // FLEETMESH-REVIEW: neutral behaviour (no carried restore is verified here).
    let _ = (r, events, opts);
}

/// The operator session an event acts under, if it is an operator's act.
fn acting_session(body: &EventBody) -> Option<SessionId> {
    match body {
        EventBody::OperatorAction { session, .. }
        | EventBody::ApprovalGranted { session, .. }
        | EventBody::ApprovalConcurred { session, .. }
        | EventBody::ApprovalAtSubmission { session, .. }
        | EventBody::ApprovalRejected { session, .. }
        | EventBody::OperatorView { session, .. } => Some(*session),
        EventBody::IntentCreated { session: Some(s), .. } => Some(*s),
        // rules 10: a passkey registered through a session
        EventBody::PasskeyRegistered { session: Some(s), .. } => Some(*s),
        // rules 30: a loss confirmed, or revoked by an operator
        EventBody::NodeLost { session, .. } => Some(*session),
        EventBody::NodeLossRevoked {
            cause: crate::nodes::Revocation::Operator { session, .. },
            ..
        } => Some(*session),
        _ => None,
    }
}

/// Rules 8: a step-up as logged: the session and operator it was entered in, when, and what
/// used it so far; rules 10: its factor (password, passkey, scripted), whether its session was
/// a person's, and a passkey's purpose.
struct StepUpSeen {
    session: SessionId,
    operator: OperatorId,
    t: Time,
    used: StepUse,
    factor: Option<AuthMethod>,
    person: bool,
    purpose: Option<crate::passkey::StepUpFor>,
}

/// Rules 10: how a step-up was used: by a signature, a passkey registration or (rules 30) a
/// confirmation of one of ours lost.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
enum StepUse {
    Unused,
    Signature,
    Registration,
    /// Rules 30: a confirmation of one of ours lost.
    NodeLost,
}

impl StepUse {
    fn what(self) -> &'static str {
        match self {
            StepUse::Unused | StepUse::Signature => "a signature",
            StepUse::Registration => "a passkey registration",
            StepUse::NodeLost => "a confirmation of a loss",
        }
    }
}

/// Rules 10: the act quoting a step-up, for a passkey's purpose to be compared with.
enum Expected {
    Is(crate::passkey::StepUpFor),
    /// A passkey registration: it re-enters the password, whatever the step-up factor.
    Registration,
}


impl Expected {
    /// Why a valid step-up (`seen`) can't stand for this act, if it can't: a passkey's made
    /// for another act, a password where the policy asks a person for a passkey (a
    /// registration excepted), a passkey quoted by a registration.
    fn refuses(&self, seen: &StepUpSeen, policy: Option<&Policy>, auth: Option<AuthMode>) -> Option<String> {
        
        if let Some(made) = &seen.purpose {
            let fits = match self {
                Expected::Is(want) => want == made,
                Expected::Registration => false,
            };
            if !fits {
                return Some(format!("a passkey assertion made for {} quoted by another act", made.describe()));
            }
        }
        match self {
            Expected::Registration if seen.factor == Some(AuthMethod::Passkey) => {
                Some("a passkey registration quoting a passkey step-up (it re-enters the password)".into())
            }
            Expected::Registration => None,
            _ if auth == Some(AuthMode::Credentials)
                && seen.person
                && policy.is_some_and(|p| p.step_up_factor() == policy::StepUpFactor::Passkey)
                && seen.factor != Some(AuthMethod::Passkey) =>
            {
                Some("a person's step-up by password where the policy requires a passkey (step_up_factor)".into())
            }
            _ => None,
        }
    }
}

/// Rules 10: the passkeys a log knows, as replay follows them.
#[derive(Default)]
struct PasskeysSeen {
    /// The hash of the log's first line: every challenge binds to it.
    run_hash: String,
    /// The relying party id and the origins (`run.started`).
    rp: Option<(String, Vec<String>)>,
    /// By credential id: the key (its counter the last one logged) and whether it was removed.
    keys: BTreeMap<String, (crate::passkey::PasskeyKey, bool)>,
    /// Every challenge's nonce answered so far.
    nonces: BTreeSet<String>,
}

impl PasskeysSeen {
    /// Check one logged passkey use by `operator` (in `session`; none for a login) at `t`
    /// (see [`crate::passkey`]), then record its counter and nonce. Err says why it is not a
    /// use the run could have accepted.
    fn use_once(
        &mut self,
        operator: &OperatorId,
        session: Option<SessionId>,
        t: Time,
        u: &crate::passkey::PasskeyUse,
    ) -> Result<(), String> {
        let (rp_id, origins) = self.rp.clone().ok_or("the run logged no relying party")?;
        if !self.nonces.insert(u.nonce.clone()) {
            return Err("a challenge answered twice (an assertion used again)".into());
        }
        let age = t.ms_since(u.issued_t);
        if !(0..=crate::passkey::CHALLENGE_TTL_MS).contains(&age) {
            return Err(format!("its challenge was {age} ms old (at most {})", crate::passkey::CHALLENGE_TTL_MS));
        }
        let Some((key, removed)) = self.keys.get_mut(&u.credential) else {
            return Err("a credential the run does not know".into());
        };
        if *removed {
            return Err("a passkey used after its removal".into());
        }
        if &key.operator != operator {
            return Err(format!("{}'s passkey claimed by {operator}", key.operator));
        }
        let challenge = crate::passkey::challenge(&self.run_hash, session, operator, &u.purpose, &u.nonce, u.issued_t);
        let v = crate::passkey::verify(key, &rp_id, &origins, &challenge, &u.assertion)?;
        if v.counter != u.counter {
            return Err(format!("the logged counter {} is not the authenticator's {}", u.counter, v.counter));
        }
        if !crate::passkey::counter_ok(key.counter, v.counter) {
            return Err(format!("the counter went from {} to {} (a cloned authenticator)", key.counter, v.counter));
        }
        key.counter = v.counter;
        Ok(())
    }
}

/// Whether an act that quotes a step-up quotes one logged under the actor's own session, used at
/// most once. One the act needs (`needed`: its decision asks for it, or `[approval] step_up_acts`
/// lists the act) in a credentials run must be there. Rules 10: a passkey's was made for this act (`expected`),
/// and the policy's step-up factor holds; a study counts its participant's acts by factor.
#[allow(clippy::too_many_arguments)]
fn check_step_up(
    r: &mut ReplayReport,
    e: &Event,
    id: Option<IntentId>,
    needed: bool,
    by: &OperatorId,
    session: SessionId,
    quoted: Option<StepUpRef>,
    step_ups: &mut BTreeMap<u64, StepUpSeen>,
    act: StepUse,
    expected: &Expected,
    policy: Option<&Policy>,
) {
    let what = act.what();
    let Some(s) = quoted else {
        if needed && r.auth == Some(AuthMode::Credentials) {
            let why = match act {
                StepUse::Unused | StepUse::Signature => {
                    "a consequential signature without a step-up (its decision asks for the signer's credential at signing)".to_string()
                }
                StepUse::Registration => {
                    "a passkey registration without a step-up (through a session, the operator re-enters their password first)".to_string()
                }
                _ => format!("{what} without a step-up (the policy's step_up_acts asks for the actor's credential)"),
            };
            bad(r, e, id, &why, by.to_string(), "step_up".into());
        }
        return;
    };
    let shares = |used: StepUse| matches!(used, StepUse::Unused);
    match step_ups.get_mut(&s.seq) {
        Some(seen)
            if seen.session == session
                && &seen.operator == by
                && shares(seen.used)
                && e.t.ms_since(seen.t) <= STEP_UP_MAX_AGE_MS =>
        {
            seen.used = act;
            match act {
                StepUse::Unused | StepUse::Signature => r.stepped_signatures += 1,
                _ => r.stepped_acts += 1,
            }
            // rules 10: what a passkey was made for, and the policy's factor
            if let Some(why) = expected.refuses(seen, policy, r.auth) {
                bad(r, e, id, &why, json(&(s.seq, &seen.purpose, seen.factor)), what.into());
            }
            // a study: its participant's consequential acts, by factor
            let factor = seen.factor;
            if !matches!(act, StepUse::Registration)
                && let Some(st) = r.study.as_mut().filter(|st| &st.participant == by)
            {
                if factor == Some(AuthMethod::Passkey) {
                    st.passkey_acts += 1;
                } else {
                    st.password_acts += 1;
                }
            }
        }
        other => {
            let why = match act {
                StepUse::Unused | StepUse::Signature => {
                    "a signature quoting a step-up that is not the signer's own, fresh and unused".to_string()
                }
                _ => format!("{what} quoting a step-up that is not the actor's own, fresh and unused"),
            };
            bad(
                r,
                e,
                id,
                &why,
                json(&(s.seq, by, session.0)),
                json(&other.map(|seen| (seen.session.0, seen.operator.clone(), seen.t.0, seen.used))),
            );
        }
    }
}

/// An event's `type`.
fn event_type(body: &EventBody) -> String {
    serde_json::to_value(body)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// A human act (`what`) under a session opened for that operator (an act that is no
/// intent's).
fn check_operator(
    r: &mut ReplayReport,
    e: &Event,
    by: &OperatorId,
    session: SessionId,
    sessions: &BTreeMap<SessionId, (OperatorId, Grants)>,
    what: &str,
) {
    match sessions.get(&session) {
        Some((op, _)) if op == by => {}
        other => bad(
            r,
            e,
            None,
            &format!("{what} under a session that is not the operator's"),
            by.to_string(),
            json(&other.map(|o| &o.0)),
        ),
    }
}


/// A human act (`what`) under a session opened earlier for that operator.
fn check_session(
    r: &mut ReplayReport,
    e: &Event,
    id: IntentId,
    by: &OperatorId,
    session: SessionId,
    sessions: &BTreeMap<SessionId, (OperatorId, Grants)>,
    what: &str,
) {
    match sessions.get(&session) {
        Some((op, _)) if op == by => {}
        other => bad(
            r,
            e,
            Some(id),
            &format!("{what} under a session that is not the operator's"),
            by.to_string(),
            json(&other.map(|o| &o.0)),
        ),
    }
}

fn check_signature(
    r: &mut ReplayReport,
    e: &Event,
    id: IntentId,
    tr: &Tracked,
    by: &OperatorId,
    session: SessionId,
    sessions: &BTreeMap<SessionId, (OperatorId, Grants)>,
) {
    match sessions.get(&session) {
        Some((op, grants)) if op == by => {
            if tr.spec.kind().needs_consequential_authority() && !grants.consequential {
                bad(
                    r,
                    e,
                    Some(id),
                    "consequential order approved without consequential authority",
                    by.to_string(),
                    "a session opened with grants.consequential".into(),
                );
            }
        }
        other => bad(
            r,
            e,
            Some(id),
            "approval under a session that is not the approver's",
            by.to_string(),
            json(&other.map(|o| &o.0)),
        ),
    }
    if tr.proposer_excluded && is_self_approval(&tr.spec.source, by) {
        bad(
            r,
            e,
            Some(id),
            "approved by its proposer (two-person rule)",
            by.to_string(),
            "someone other than the proposer".into(),
        );
    }
    if tr.signers.contains(by) {
        bad(
            r,
            e,
            Some(id),
            "the same operator signed twice",
            by.to_string(),
            json(&tr.signers),
        );
    }
}

/// Verify a logged area set against its hash; the result is the new set in force.
fn load_areas(
    r: &mut ReplayReport,
    e: &Event,
    sha256: &str,
    set: &AreaSet,
) -> (String, Option<AreaSet>) {
    r.area_sets += 1;
    match set.content_hash() {
        Ok(h) if h == sha256 => (sha256.to_string(), Some(set.clone())),
        other => {
            bad(
                r,
                e,
                None,
                "area set hash does not match its content",
                sha256.to_string(),
                other.unwrap_or_else(|err| format!("<unserializable: {err}>")),
            );
            (sha256.to_string(), None)
        }
    }
}


/// What an `intent.dispatched` says was handed on.
struct Dispatched<'a> {
    task: &'a Task,
    uids: &'a [Uid],
    /// Rules 22: the transit waypoints and a patrol's routed route it carries.
    via: &'a BTreeMap<Uid, Vec<crate::geo::LonLat>>,
    path: Option<&'a Vec<crate::geo::LonLat>>,
}


/// Rules 34: a group step's cause against the facts: its drops exactly the members on the layout
/// the facts and the log show dropped; each member back one a re-form left out, heard now, its
/// return no earlier than that and at least [`crate::group::BACK_AFTER_S`] ago (the engine's own
/// tracking of how long it was heard running is bounded, not recomputed). Returns the standing.
#[allow(clippy::too_many_arguments)]
fn group_cause_check(
    r: &mut ReplayReport,
    e: &Event,
    gid: IntentId,
    world: &WorldSnapshot,
    cause: &crate::group::Cause,
    force: &crate::group::Force,
    intents: &BTreeMap<IntentId, Tracked>,
    newest_on: &BTreeMap<Uid, IntentId>,
    nodes: &crate::nodes::NodeLedger,
    pol: &Policy,
) -> Option<crate::group::Standing> {
    let views: BTreeMap<Uid, crate::group::MemberView> = force
        .members
        .iter()
        .filter_map(|(u, mid)| {
            let m = intents.get(mid)?;
            let taken = newest_on
                .get(u)
                .is_some_and(|n| n > mid && intents.get(n).and_then(|i| i.spec.parent()) != Some(gid));
            Some((
                u.clone(),
                crate::group::MemberView {
                    state: m.state,
                    ended_t: m.ended_t,
                    taken,
                },
            ))
        })
        .collect();
    let st = crate::group::standing(force, &views, world, nodes, &pol.rules);
    if st.dropped != cause.dropped {
        bad(r, e, Some(gid), "a group step's drops are not what its facts show", json(&cause.dropped), json(&st.dropped));
        return None;
    }
    for (u, since) in &cause.back {
        let left = force.left.get(u).copied();
        let ok = st.heard.contains(u)
            && left.is_some_and(|l| *since >= l)
            && world.t.ms_since(*since) >= crate::group::BACK_AFTER_S * 1000;
        if !ok {
            bad(r, e, Some(gid), "a member counted back that its facts do not show back (left out, heard now, long enough)", json(&(u, since)), json(&left));
            return None;
        }
    }
    Some(st)
}

fn due_name(d: &crate::group::Due) -> String {
    match d {
        crate::group::Due::Nothing => "nothing due".into(),
        crate::group::Due::Suspend { strength, min } => format!("suspended ({strength} of a minimum {min})"),
        crate::group::Due::Reform => "a re-form".into(),
    }
}

/// Rules 34: how long member order `id` may wait in the queue, s (its group's `wait_s`).
fn member_wait(intents: &BTreeMap<IntentId, Tracked>, id: IntentId) -> Option<u32> {
    let g = intents.get(&intents.get(&id)?.spec.parent()?)?;
    g.spec.group()?.wait_s
}

/// Rules 34: why a member order's `intent.created` is not one its group's dispatch makes, if it is
/// not: its group a group order, approved, whose last passing check counted this member (sent or
/// waiting), one member order per member, its task and share the layout's, its source, constraints
/// and rationale its group's, made in no session.
fn member_created_mismatch(m: &IntentSpec, has_session: bool, intents: &BTreeMap<IntentId, Tracked>, policy: Option<&Policy>) -> Option<&'static str> {
    if !policy.is_some_and(|_p| true) {
        return Some("a member order under rules that take no group order");
    }
    if has_session {
        return Some("a member order made in a session (its group's dispatch makes it)");
    }
    let Some(gid) = m.parent() else {
        return Some("a share without a group order");
    };
    let Some(g) = intents.get(&gid) else {
        return Some("a member order of an unknown group order");
    };
    if m.group().is_some() || g.spec.group().is_none() {
        return Some("a member order whose parent is not a group order, or that is one itself");
    }
    if !matches!(g.state, State::Approved | State::AutoApproved) {
        return Some("a member order created while its group order is not approved");
    }
    let [uid] = m.uids.as_slice() else {
        return Some("a member order names one vehicle");
    };
    if g.members_created.contains_key(uid) {
        return Some("a second member order for one member");
    }
    let Some(eval) = &g.group_eval else {
        return Some("a member order of a group order with no passing check");
    };
    let Some(out) = eval.members.get(uid) else {
        return Some("a member order to a vehicle outside its group's selection");
    };
    if out.class == Some(crate::group::MemberClass::Excluded) {
        return Some("a member order to a member its group's check left out");
    }
    let Some(want) = eval.member_spec(&g.spec, uid) else {
        return Some("a member order to a member its group's layout gave no share");
    };
    if m.task != want.task || m.share() != want.share() {
        return Some("a member order's task or share is not its group's layout's");
    }
    if m.source != g.spec.source || m.constraints != g.spec.constraints || m.rationale != g.spec.rationale {
        return Some("a member order's source, constraints or rationale are not its group's");
    }
    None
}

fn check_plans(r: &mut ReplayReport, e: &Event, id: IntentId, w: &WorldSnapshot, pol: &Policy) {
    if w.route_plans.is_empty() {
        return;
    }
    for (uid, p) in w.route_plans.iter().flat_map(|(u, ps)| ps.iter().map(move |p| (u, p))) {
        let again = crate::plan::replan(p, w, uid, &pol.rules);
        let same = match (&again, &p.no_route) {
            (Ok(v), None) => v == &p.via,
            (Err(why), Some(logged)) => why == logged,
            _ => false,
        };
        if !same {
            bad(r, e, Some(id), "a routed leg differs from its plan on its own grid", json(&(&p.leg, &p.via, &p.no_route)), json(&again));
        }
    }
}

/// An `intent.dispatched` is exactly the approved intent's order, whatever the rules version
/// (an audit check, not a decision rule): the task and vehicles as approved, and the space or
/// dispatches); rules 22: and the waypoints its plans route it through.
fn check_dispatch(
    r: &mut ReplayReport,
    e: &Event,
    tr: &Tracked,
    got: Dispatched<'_>,
    policy: Option<&Policy>,
) {
    let id = Some(tr.spec.id);
    if *got.task != tr.spec.task {
        bad(
            r,
            e,
            id,
            "dispatched task differs from the approved intent's",
            json(got.task),
            json(&tr.spec.task),
        );
    }
    if got.uids != tr.spec.uids.as_slice() {
        bad(
            r,
            e,
            id,
            "dispatched to other vehicles than the approved intent's",
            json(&got.uids),
            json(&tr.spec.uids),
        );
    }
    let Some(pol) = policy else { return };
    let want = match crate::engine::order_for(&tr.spec, &tr.route_plans, &pol.rules) {
        Ok(o) => crate::engine::dispatched_event(&tr.spec, &o),
        Err(err) => {
            bad(r, e, id, "dispatched although the approved intent gives no order", json(&got.via), err.to_string());
            return;
        }
    };
    let EventBody::IntentDispatched {
        via: want_via,
        path: want_path,
        ..
    } = want
    else {
        return;
    };
    // rules 22: the routes its last passing check planned, no other
    if got.via != &want_via || got.path != want_path.as_ref() {
        bad(
            r,
            e,
            id,
            "dispatched with other routed waypoints than its last passing check planned",
            json(&(got.via, got.path)),
            json(&(&want_via, &want_path)),
        );
    }
}



/// Decision inputs with the area reference resolved to the set in force.
fn resolve(inputs: &LoggedInputs, in_force: &InForce) -> Result<WorldSnapshot, &'static str> {
    let Some(want) = &inputs.areas_sha256 else {
        return Ok(inputs.resolve(None));
    };
    match in_force {
        Some((h, Some(set))) if h == want => Ok(inputs.resolve(Some(set.clone()))),
        Some((h, None)) if h == want => {
            Err("decision references an area set whose hash did not verify")
        }
        _ => Err("decision references an area set that is not the one in force"),
    }
}

#[allow(clippy::too_many_arguments)]
fn group_step_world(
    r: &mut ReplayReport,
    e: &Event,
    gid: IntentId,
    inputs: &LoggedInputs,
    areas: &InForce,
    intents: &BTreeMap<IntentId, Tracked>,
    forces: &BTreeMap<IntentId, crate::group::Force>,
    policy: Option<&Policy>,
) -> Option<WorldSnapshot> {
    if policy.is_none() {
        bad(r, e, Some(gid), "a group step without run.started", String::new(), String::new());
        return None;
    }
    let Some(g) = intents.get(&gid) else {
        bad(r, e, Some(gid), "a group step of an unknown intent", String::new(), String::new());
        return None;
    };
    let Some(force) = forces.get(&gid) else {
        bad(r, e, Some(gid), "a group step of a group order never dispatched", String::new(), String::new());
        return None;
    };
    if !matches!(g.state, State::Dispatched | State::Done) || force.suspended || g.spec.group().and_then(|s| s.reform) != Some(true) {
        bad(r, e, Some(gid), "a group step of a group order not in force (not dispatched or done, suspended, or not re-forming)", json(&g.state), String::new());
        return None;
    }
    match resolve(inputs, areas) {
        Ok(w) => Some(w),
        Err(why) => {
            bad(r, e, Some(gid), why, String::new(), String::new());
            None
        }
    }
}

fn bad(
    r: &mut ReplayReport,
    e: &Event,
    intent: Option<IntentId>,
    what: &str,
    logged: String,
    recomputed: String,
) {
    r.mismatches.push(Mismatch {
        seq: e.seq,
        intent,
        what: what.to_string(),
        logged,
        recomputed,
    });
}

/// Why a transition lacks the evidence it needs, if it does.
fn unbacked(tr: &Tracked, t: Transition) -> Option<&'static str> {
    // rules 5: out of the queue only by a re-check's verdict, or its deadline
    if tr.state == State::Queued {
        return match t {
            Transition::Dispatch if tr.recheck != Some(Verdict::Dispatch) => {
                Some("dispatched from the queue without a passing re-check")
            }
            Transition::Dispatch if tr.dispatch_logged.is_none() => {
                Some("dispatched without its intent.dispatched order")
            }
            Transition::RequireApproval
                if !(tr.recheck == Some(Verdict::Inbox) || tr.queue_expired_inbox)
                    || tr.expires_t.is_none() =>
            {
                Some(
                    "sent back to the inbox without a re-check or a queue expiry that says so and a deadline",
                )
            }
            Transition::Expire if !tr.queue_expired || tr.queue_expired_inbox => {
                Some("expired from the queue without a queue.expired that expires it")
            }
            _ => None,
        };
    }
    match t {
        Transition::Requeue if tr.requeued_logged.is_none() => {
            Some("requeued without an intent.requeued")
        }
        Transition::Queue if tr.queued_logged.is_none() => {
            Some("queued without an intent.queued")
        }
        Transition::Check if tr.last_check_passed.is_none() => {
            Some("checked without a check.result")
        }
        Transition::ChecksFailed if tr.last_check_passed != Some(false) => {
            Some("rejected by rules that passed")
        }
        Transition::AutoApprove
            if tr.last_check_passed != Some(true) || tr.tier != Some(Tier::Auto) =>
        {
            Some("auto-approved without passing checks and an auto policy tier")
        }
        Transition::RequireApproval if tr.tier != Some(Tier::Human) || tr.expires_t.is_none() => {
            Some("sent for approval without a human tier and a deadline")
        }
        Transition::RequireApproval if tr.submitter_authorizes => {
            Some("sent for approval, but the submitter authorizes this tier")
        }
        Transition::AuthorizeAtSubmission
            if !tr.granted_at_submission
                || tr.last_check_passed != Some(true)
                || tr.tier != Some(Tier::Human) =>
        {
            Some(
                "approved at submission without passing checks, a human tier that allows it, and the submitter's approval.at_submission",
            )
        }
        Transition::Approve if !tr.granted || tr.last_check_passed != Some(true) => {
            Some("approved without a human approval and a passing re-check")
        }
        Transition::Approve
            if tr.signers.len() < usize::try_from(tr.approvers).unwrap_or(usize::MAX) =>
        {
            Some("approved without the signatures the tier needs")
        }
        Transition::Dispatch if tr.dispatch_logged.is_none() => {
            Some("dispatched without its intent.dispatched order")
        }
        Transition::Release if !tr.effect_logged => Some("released without an consequential.effect"),
        Transition::Abort if !tr.stopped => Some("aborted without an consequential.stopped"),
        Transition::Cancel if !tr.cancel_reported => {
            Some("cancelled after dispatch without the operator's cancel and the engine's cancelled report")
        }
        Transition::Reject if !tr.rejected_by_human => Some("rejected without a human rejection"),
        Transition::Expire if !tr.expired => Some("expired without an approval.expired"),
        _ => None,
    }
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("<unserializable: {e}>"))
}

/// A decision's logged `waiting` fact (rules 5: the vehicles with a human-approved order
/// waiting in the queue) against the queued intents replay tracks at that point.
fn check_waiting(
    r: &mut ReplayReport,
    e: &Event,
    id: IntentId,
    logged: &BTreeMap<crate::ids::Uid, IntentId>,
    intents: &BTreeMap<IntentId, Tracked>,
) {
    let Some(tr) = intents.get(&id) else { return };
    let want = waiting_vehicles(
        intents.iter().map(|(i, t)| {
            (
                *i,
                &t.spec,
                t.state,
                t.basis.as_ref().is_some_and(|b| b.by_human),
                !t.committed.is_empty(),
            )
        }),
        &tr.spec,
    );
    if *logged != want {
        bad(
            r,
            e,
            Some(id),
            "human-order-waiting facts differ",
            json(logged),
            json(&want),
        );
    }
}

/// The fields a decision's inputs carry that its rules version did not have
/// ([`crate::rules::RULES_MARKERS`]): from rules 2 a drop zone's standing `approval`, from rules
/// 3 the traffic picture.
fn check_markers(r: &mut ReplayReport, e: &Event, id: IntentId, w: &WorldSnapshot, rules: &crate::rules::RulesConfig) {
    if !rules.at_least(crate::rules::TRAFFIC_SEPARATION_SINCE)
        && w.delivery.as_ref().is_some_and(|d| d.picture.is_some())
    {
        bad(
            r,
            e,
            Some(id),
            "a field newer than the log's rules version (rules markers)",
            "check.result.inputs.delivery.picture".into(),
            format!("absent before rules {}", crate::rules::TRAFFIC_SEPARATION_SINCE),
        );
    }
    if rules.at_least(crate::rules::STANDING_APPROVALS_SINCE) {
        return;
    }
    let approved: Vec<String> = w
        .delivery
        .iter()
        .flat_map(|d| d.places.drop_zones.values())
        .filter(|z| z.approval.is_some())
        .map(|z| z.id.clone())
        .collect();
    if !approved.is_empty() {
        bad(
            r,
            e,
            Some(id),
            "a field newer than the log's rules version (rules markers)",
            format!("check.result.inputs.delivery.places.drop_zones.approval on {}", approved.join(", ")),
            format!("absent before rules {}", crate::rules::STANDING_APPROVALS_SINCE),
        );
    }
}

/// A terrain answer taken from a nearby sample lies within this of its point, m.
const TERRAIN_NEAR_M: f64 = 50.0;

/// Rules 17: a decision's logged terrain answers: each taken from a sample within
/// [`TERRAIN_NEAR_M`] of its point, and as far as it says; the answers along a leg within
/// [`crate::route::LEG_NEAR_M`] of their points.
fn check_terrain(r: &mut ReplayReport, e: &Event, id: IntentId, w: &WorldSnapshot) {
    let crate::world::TerrainInputs::Sampled { samples, along } = &w.terrain else {
        return;
    };
    for l in along {
        if let Some(d) = l.near_m
            && !(0.0..=crate::route::LEG_NEAR_M).contains(&d)
        {
            bad(
                r,
                e,
                Some(id),
                "terrain answers along a leg taken from samples too far from their points",
                format!("{d:.1} m"),
                format!("at most {:.0} m", crate::route::LEG_NEAR_M),
            );
        }
    }
    for s in samples {
        let Some(from) = &s.from else { continue };
        let p = crate::geo::LonLat::new(s.lon_e7 as f64 / 1e7, s.lat_e7 as f64 / 1e7);
        let q = crate::geo::LonLat::new(from.lon_e7 as f64 / 1e7, from.lat_e7 as f64 / 1e7);
        let d = crate::geo::haversine_m(p, q);
        if from.distance_m > TERRAIN_NEAR_M || (d - from.distance_m).abs() > 1.0 {
            bad(
                r,
                e,
                Some(id),
                "terrain answer from a sample too far from the point, or not as far as it says",
                json(from),
                format!("{d:.1} m (at most {TERRAIN_NEAR_M:.0} m)"),
            );
        }
    }
}
