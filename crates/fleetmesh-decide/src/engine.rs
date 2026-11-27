//! The engine: owns the intents, applies rules and policy, enforces the state machine, and
//! writes every step to the audit log before taking it.

use std::collections::{BTreeMap, BTreeSet};

use crate::areas::AreaSet;
use crate::audit::{AuditSink, CheckPhase, Event, EventBody, LoggedAreas, OperatorView};
use crate::authority::{
    AuthMethod, AuthMode, ConsequentialAuthority, Grants, HumanAuthority, OperatorSession, Roster,
    STEP_UP_MAX_AGE_MS, StepUp, StepUpRef,
};
use crate::hash::{GENESIS, sha256_hex};
use crate::ids::{AgentId, IntentId, OperatorId, SessionId, Uid};
use crate::intent::{
    Approval, IllegalTransition, Intent, IntentSpec, Proposal, ProposalError, Rationale, Source,
    State, Task, TaskKind, Transition, next_state,
};
use crate::policy::{self, Policy, PolicyDecision, Tier};
use crate::queue::{
    self as queue, Basis, CarriedApproval, OpenIntent, QueueState, Reach, RecheckRecord,
    ResumedFrom, Verdict,
};
use crate::rules::{self, CheckResult, Outcome};
use serde::Serialize;
use crate::terrain::{NoTerrain, TerrainOracle};
use crate::time::{Clock, Time};
use crate::world::{LoggedInputs, WorldSnapshot};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("only a triage.* event is recorded as triage")]
    NotTriage,
    /// Rules 8: a play refused (older rules, no `[plays]`, outside it, not in force).
    #[error("{0}")]
    Play(String),
    #[error("unknown intent {0}")]
    UnknownIntent(IntentId),
    #[error(transparent)]
    Proposal(#[from] ProposalError),
    #[error("intent {id}: {err}")]
    Illegal {
        id: IntentId,
        err: IllegalTransition,
    },
    #[error("intent {0} expired before it was approved")]
    Expired(IntentId),
    #[error("intent {id} is {state:?}: an open or decide view is only taken while it waits for a decision")]
    ViewNotPending { id: IntentId, state: State },
    #[error("intent {0} failed its checks against the current world at approval and was voided")]
    ChecksFailedAtApproval(IntentId),
    #[error("agent {agent} can't withdraw intent {id}: not its own")]
    NotOwner { agent: AgentId, id: IntentId },
    #[error("audit log write failed, nothing changed: {0}")]
    Audit(#[from] std::io::Error),
    #[error("audit event failed to serialize: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("intent {id} can't be dispatched: {why}")]
    NotDispatchable { id: IntentId, why: String },
    #[error("intent {id}: {by} proposed it and may not approve it (two-person rule)")]
    ProposerCannotApprove { id: IntentId, by: OperatorId },
    #[error("intent {id}: {by} has already signed it; another operator must")]
    AlreadySigned { id: IntentId, by: OperatorId },
    /// The roster a run was started with is not one a bridge could be configured with.
    #[error("operator roster: {0}")]
    Roster(String),
    /// A session for an operator the run's roster does not list, or with a grant it does not
    /// give them: nothing was logged.
    #[error("operator {operator}: {why}")]
    NotOnRoster { operator: OperatorId, why: String },
    /// Rules 8: a session or step-up the run's authentication does not allow (an asserted
    /// session in a credentials run, a step-up in a dev run, a step-up for another session
    /// or too old): nothing was logged.
    #[error("operator {operator}: {why}")]
    NotAuthenticated { operator: OperatorId, why: String },
    /// Rules 8, a credentials run: the decision marks this approval consequential, and the
    /// signer did not re-enter their credential for it (logged as `approval.refused`).
    #[error("intent {id}: {by} must re-enter their credential to sign this (step-up)")]
    StepUpRequired { id: IntentId, by: OperatorId },
    /// Rules 12: what a start carried over can't go into `run.started` (a log not named by its
    /// bare file name, rules before 12): the engine did not start.
    #[error("carry-over: {0}")]
    Carry(String),
    /// Rules 30: a confirmation or revocation of a loss the engine can't take (not silent since
    /// 30): nothing was logged.
    #[error("node loss: {0}")]
    NodeLoss(String),
    /// Rules 33: a policy the run's decision rules can't decide with as written (scenario
    /// settings other than the standing values under rules that don't read them,
    /// [`crate::Policy::fits_rules`]): the engine did not start.
    #[error("{0}")]
    Policy(String),
}

/// Rules 12: what a start carries over from earlier runs' logs ([`Engine::start`]): the entries of
/// `run.started`'s `carried_from`, and (rules 15) the station's `.last-run` pointer, by its file
/// name, logged as `run.started`'s `pointer`; and what else the start records of the bridge it
/// runs in. A list of entries alone is a carry without a pointer (a test).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StartCarry {
    pub pointer: Option<String>,
    pub from: Vec<crate::replay::CarriedFrom>,
    /// Rules 33: the files the policy was built from ([`crate::Policy::load_layers`]), logged as
    /// `run.started`'s `policy_layers` (each path never absolute, [`crate::Layer::for_log`]);
    /// none for a policy not read from a file (a test).
    pub policy_layers: Vec<crate::policy::Layer>,
}

impl From<Vec<crate::replay::CarriedFrom>> for StartCarry {
    fn from(from: Vec<crate::replay::CarriedFrom>) -> Self {
        StartCarry {
            from,
            ..StartCarry::default()
        }
    }
}

/// How old a prepared space answer may be when the check that uses it runs, ms.
pub use crate::rules::PREPARED_SPACE_MAX_AGE_MS;





/// Who reports an op state: the bridge (the task layer, the space executor), the bridge
/// ahead of a vehicle start (rules 7), or the engine itself (rules 11: a space task's cancel
/// and re-check).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpReport {
    Bridge,
    Ahead,
    Engine,
}

/// What a step over the open intents did: the intents whose step failed (an audit write, say),
/// each on its own.
#[derive(Debug, Default)]
pub struct StepOutcome {
    pub errors: Vec<(IntentId, EngineError)>,
}

/// Rules 5: the orders a human approved that wait on `spec`'s vehicles, other than `spec`
/// itself, by vehicle (the lowest id when there are several): queued, or dispatched with no
/// vehicle having started them yet (an agent's order there would replace them before they
/// run). The engine logs this with each decision (`waiting`); replay recomputes it from its
/// own intents. `intents`: (id, spec, state, approved by a human, some vehicle started it).
pub(crate) fn waiting_vehicles<'a>(
    intents: impl Iterator<Item = (IntentId, &'a IntentSpec, State, bool, bool)>,
    spec: &IntentSpec,
) -> BTreeMap<Uid, IntentId> {
    let mut out: BTreeMap<Uid, IntentId> = BTreeMap::new();
    for (id, s, state, by_human, started) in intents {
        let waits = match state {
            State::Queued => true,
            State::Dispatched => !started,
            _ => false,
        };
        if id == spec.id || !waits || !by_human {
            continue;
        }
        for u in spec.uids.iter().filter(|u| s.uids.contains(u)) {
            out.entry(u.clone()).or_insert(id);
        }
    }
    out
}


/// What the task layer should execute for an approved intent.
#[derive(Clone, Debug, PartialEq)]
pub struct DispatchOrder {
    pub intent: IntentId,
    pub task: Task,
    pub uids: Vec<Uid>,
    /// Rules 22: each vehicle's transit waypoints, the route a plan laid around the wrong surface
    /// or a no-go area (flown before the task; a patrol's own routed legs are in its route).
    pub via: BTreeMap<Uid, Vec<crate::geo::LonLat>>,
    /// Rules 34, a member order: its share of the group's task (the altitude it flies, where it
    /// enters the ring or the route), from its spec.
    pub share: Option<Box<crate::group::Share>>,
}

/// What [`Engine::dispatch_order`] hands the bridge: vehicle work for the task manager, or
/// a simulated satellite request (collect) or service watch (cover).
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    Vehicles(DispatchOrder),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskOutcome {
    Done,
    Failed { detail: String },
}

/// What [`Engine::dispatch_or_queue`] did with an approved intent.
#[derive(Clone, Debug, PartialEq)]
pub enum Routed {
    /// Every vehicle could be reached: handed on now.
    Dispatched(Order),
    /// Some vehicle can't be reached now: held (`intent.queued`) until `expires_t`.
    Queued { expires_t: Time, reach: Vec<Reach> },
}

/// What [`Engine::dequeue`] did with a queued intent.
#[derive(Clone, Debug, PartialEq)]
pub enum Dequeued {
    /// A vehicle still can't be reached: nothing logged, still queued.
    Waiting(Vec<Reach>),
    /// Re-checked and dispatched: execute the order.
    Dispatched(Order),
    /// Re-checked: a human must decide again; it waits in the inbox.
    Inbox,
    /// Re-checked: a check failed on the world now; voided.
    Voided,
    /// Its deadline passed first; expired.
    Expired,
}

pub struct Engine {
    policy: Policy,
    clock: Box<dyn Clock + Send>,
    sink: Box<dyn AuditSink + Send>,
    seq: u64,
    last_t: Time,
    /// SHA-256 of the last line written: the next event's `prev`.
    head: String,
    /// The area set last logged (in `run.started` or `areas.loaded`) and its hash.
    areas: Option<LoggedAreas>,
    next_intent: u64,
    next_session: u64,
    intents: BTreeMap<IntentId, Intent>,
    /// Rules 8: the plays a person adopted that have not ended, by id, and the next play's id.
    plays: BTreeMap<u64, crate::plays::Play>,
    next_play: u64,
    /// When each drop zone was last confirmed by a person (an approved `confirm_drop_zone`, or an
    /// approved `deliver` held for `DZ_UNCONFIRMED`): the `confirmed` input of the next decision.
    dz_confirmed: BTreeMap<String, Time>,
    /// Who may open an operator session, and with consequential authority (`run.started`).
    roster: Roster,
    /// Rules 8: how this run's operators are authenticated (the roster's logged `auth`);
    /// None before rules 8.
    auth: Option<AuthMode>,
    /// Rules 8: sessions ended (`session.closed`): nothing more is logged under them.
    closed: BTreeSet<SessionId>,
    last_dispatched: Option<IntentId>,
    /// Per vehicle, the intent dispatched to it last: an older one's order there was replaced.
    newest_on: BTreeMap<Uid, IntentId>,
    /// Rules 5: when each vehicle was last logged rebooting (`vehicle.rebooted`).
    rebooted: BTreeMap<Uid, Time>,
    /// Rules 6: the platforms' own lost-link values owed back, as the log has them (the
    /// ledger replay keeps): rules 7 holds a mission for a vehicle still owed them.
    restores: crate::replay::Restores,
    /// Rules 7: the intent queued last: like the one dispatched last, it may supersede an older
    /// queued order.
    last_queued: Option<IntentId>,
    /// Rules 7: the order whose dispatch or queueing is the step in progress, and the seq of its
    /// first event.
    step: Option<(IntentId, u64)>,
    /// Rules 10: the SHA-256 of the `run.started` line, which every passkey challenge binds
    /// to (an assertion from another run does not verify here).
    run_hash: String,
    /// Rules 10: the passkeys this run knows (its roster's, and those registered since), by
    /// credential id.
    passkeys: BTreeMap<String, KeySeen>,
    /// Rules 10: the nonce of every passkey use logged: each challenge is answered once.
    used_nonces: BTreeSet<String>,
    /// Rules 30: our vehicles' standing (the presumptions and the losses in force), stepped with every event written,
    /// as replay steps it with every event read ([`crate::nodes`]).
    nodes: crate::nodes::NodeLedger,
    /// Rules 34: each group order dispatched, as it stays in force (its layout, its member orders,
    /// its re-forms; [`crate::group::Force`]), and since when the engine has heard each member a
    /// re-form left out (its own tracking: a re-form logs the times, which replay bounds).
    forces: BTreeMap<IntentId, crate::group::Force>,
    heard: BTreeMap<IntentId, BTreeMap<Uid, Time>>,
}

/// Rules 10: a passkey as the engine follows it: whose, its public key, the last counter
/// logged for it, and whether it was removed.
#[derive(Clone, Debug)]
struct KeySeen {
    key: crate::passkey::PasskeyKey,
    removed: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("seq", &self.seq)
            .field("intents", &self.intents.len())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Start a run: logs `run.started` with the full policy and the operator roster (only
    /// operators on it can open a session, with consequential authority only those on its
    /// consequential list). The area set is logged (as `areas.loaded`) the first time a
    /// decision reads one; [`Engine::with_areas`] logs it in `run.started` instead.
    pub fn new(
        policy: Policy,
        roster: Roster,
        clock: Box<dyn Clock + Send>,
        sink: Box<dyn AuditSink + Send>,
    ) -> Result<Self, EngineError> {
        Self::with_areas(policy, None, roster, clock, sink)
    }

    /// Start a run with the area set known up front: `run.started` carries the policy, the
    /// roster and the area set with its content hash. Decisions reference the hash; if a
    /// snapshot later brings a different set, it is logged once as `areas.loaded`.
    pub fn with_areas(
        policy: Policy,
        areas: Option<AreaSet>,
        roster: Roster,
        clock: Box<dyn Clock + Send>,
        sink: Box<dyn AuditSink + Send>,
    ) -> Result<Self, EngineError> {
        Self::start(policy, areas, roster, Vec::new(), clock, sink)
    }

    /// [`Engine::with_areas`] for a bridge that carried over what earlier runs' logs still owed
    /// (rules 12, [`crate::rules::REVIEW_FOLLOWUPS_SINCE`]): `carried_from` goes into
    /// `run.started` (the log `.last-run-<port>` named first, then any log whose carry had
    /// failed, each with its head and what it owed), in that one line, and seeds the ledger of
    /// the platforms' own lost-link values owed back. Refused before rules 12 (those carry with
    /// `platform.restore` events, [`Engine::record_platform_restore`]) and for an entry that
    /// does not name a log by its bare file name.
    pub fn start(
        policy: Policy,
        areas: Option<AreaSet>,
        roster: Roster,
        carry: impl Into<StartCarry>,
        clock: Box<dyn Clock + Send>,
        sink: Box<dyn AuditSink + Send>,
    ) -> Result<Self, EngineError> {
        let StartCarry {
            pointer,
            from: carried_from,
            policy_layers,
        } = carry.into();
        // the files the policy was built from are logged
        let policy_layers: Vec<crate::policy::Layer> = {
            policy_layers.iter().map(crate::policy::Layer::for_log).collect()
        };
        // rules 15: the station's pointer, the spent platforms carried, a failed carry's fleet
        // file and a fallback from a pointer that could not be read (no build of older rules
        // logged them)
        let pointer = pointer.filter(|_| true);
        // rules 24: the rounds earlier runs released (no build of older rules logged them)
        // rules 30: our vehicles earlier runs confirmed lost (no build of older rules logged one)
        if let Some(p) = &pointer
            && !crate::replay::is_pointer_name(p)
        {
            return Err(EngineError::Carry(format!("the station's pointer is a .last-run file name, not {p:?}")));
        }
        if !carried_from.is_empty() && false {
            return Err(EngineError::Carry(
                "these decision rules carry earlier runs' debts with platform.restore events, not in run.started".into(),
            ));
        }
        if let Some(c) = carried_from.iter().find(|c| !crate::replay::is_log_file_name(&c.log)) {
            return Err(EngineError::Carry(format!(
                "a carried-over log is named by its bare file name, not {:?}",
                c.log
            )));
        }
        roster.validate().map_err(EngineError::Roster)?;
        // rules 8: the roster says how its operators are authenticated (dev when the caller
        // said nothing: names asserted, never evidence about people); older rules never
        // logged it
        let mut roster = roster;
        roster.auth = (true)
            .then(|| roster.auth.unwrap_or(AuthMode::Dev));
        // rules 10: passkeys and a study, only from rules 10 (older rules never logged them),
        // checked against the policy (a study requires passkey step-ups unless overridden)
        roster
            .validate_passkeys(policy.step_up_factor())
            .map_err(EngineError::Roster)?;
        // rules 12: a credentials run fixes its scripted (harness-held) operators at the start,
        // an empty list when none is; never before rules 12, never in a dev run
        if roster.auth == Some(AuthMode::Credentials) {
            roster.scripted.get_or_insert_with(Vec::new);
        } else {
            roster.scripted = None;
        }
        // what the rules read from other sections, without what older rules don't have (rules
        // logged one); `fleetmesh-policy show` prints the same
        let policy = policy.as_logged();
        let areas = match areas {
            Some(a) => Some(LoggedAreas {
                sha256: a.content_hash()?,
                areas: a,
            }),
            None => None,
        };
        // rules 12: what earlier runs still owed, owed back from here on (rules 15: and the
        // platforms they had spent stay spent)
        let mut e = Engine {
            policy,
            clock,
            sink,
            seq: 0,
            last_t: Time(i64::MIN),
            head: GENESIS.to_string(),
            areas: None,
            next_intent: 1,
            next_session: 1,
            intents: BTreeMap::new(),
            plays: BTreeMap::new(),
            next_play: 1,
            dz_confirmed: BTreeMap::new(),
            auth: roster.auth,
            roster: roster.clone(),
            closed: BTreeSet::new(),
            last_dispatched: None,
            newest_on: BTreeMap::new(),
            rebooted: BTreeMap::new(),
            restores: crate::replay::Restores::default(),
            last_queued: None,
            step: None,
            run_hash: String::new(),
            passkeys: BTreeMap::new(),
            used_nonces: BTreeSet::new(),
            nodes: crate::nodes::NodeLedger::default(),
            forces: BTreeMap::new(),
            heard: BTreeMap::new(),
        };
        let policy = e.policy.clone();
        let keys = roster.passkeys.as_ref().map(|p| p.keys.clone()).unwrap_or_default();
        e.emit(EventBody::RunStarted {
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            rules_version: Some(policy.rules.rules_version),
            policy_schema: Some(policy.rules.policy_schema()),
            build: Some(crate::audit::BuildInfo::current()),
            roster: Some(roster),
            policy,
            areas: areas.clone(),
            carried_from: carried_from.clone(),
            pointer,
            policy_layers,
        })?;
        // rules 12: what earlier runs still owed, owed back from here on (rules 15: and the
        // platforms they had spent stay spent)
        e.restores.carry(&carried_from);
        e.areas = areas;
        e.run_hash = e.head.clone();
        for k in keys {
            e.passkeys.insert(k.id.clone(), KeySeen { key: k, removed: false });
        }
        Ok(e)
    }

    /// Rules 10: the SHA-256 of this run's `run.started` line (every passkey challenge binds
    /// to it).
    pub fn run_hash(&self) -> &str {
        &self.run_hash
    }

    /// Rules 10: the run's relying party and the passkeys it knows now (its roster's and those
    /// registered since, not removed), public parts only.
    pub fn passkeys(&self) -> Option<crate::passkey::PasskeyRoster> {
        let p = self.roster.passkeys.as_ref()?;
        Some(crate::passkey::PasskeyRoster {
            rp_id: p.rp_id.clone(),
            origins: p.origins.clone(),
            keys: self.passkeys.values().filter(|k| !k.removed).map(|k| k.key.clone()).collect(),
        })
    }

    /// Rules 10: the study this run is, if it is one.
    pub fn study(&self) -> Option<&crate::passkey::Study> {
        self.roster.study.as_deref()
    }

    /// SHA-256 of the last event written: the head of the log's hash chain. Publishing it
    /// somewhere the log writer can't rewrite (future work) is what turns the chain from
    /// tamper-evident into evidence against a full rewrite or a truncated tail.
    pub fn head_hash(&self) -> &str {
        &self.head
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }




    pub fn intent(&self, id: IntentId) -> Option<&Intent> {
        self.intents.get(&id)
    }

    pub fn intents(&self) -> impl Iterator<Item = &Intent> {
        self.intents.values()
    }

    /// When each drop zone was confirmed by a person (an approved `confirm_drop_zone` or held
    /// `deliver`): what the next decision reads as `confirmed` (read-only; the dispatcher asks
    /// whether a zone needs a person's approval before it sends parcels there).
    pub fn drop_zone_confirmations(&self) -> &BTreeMap<String, Time> {
        &self.dz_confirmed
    }

    /// Pending approvals, soonest expiry first.
    pub fn inbox(&self) -> Vec<&Intent> {
        let mut v: Vec<_> = self
            .intents
            .values()
            .filter(|i| i.state() == State::PendingApproval)
            .collect();
        v.sort_by_key(|i| (i.expires_t(), i.id()));
        v
    }

    // ---- operator side --------------------------------------------------------------

    /// The one place a [`HumanAuthority`] comes from. The bridge calls this from its
    /// authenticated operator endpoint only. Logged as `operator.action session.opened`.
    pub fn open_operator_session(
        &mut self,
        operator: OperatorId,
    ) -> Result<OperatorSession, EngineError> {
        self.open_operator_session_with(operator, Grants::default())
    }

    /// session carries an [`ConsequentialAuthority`] (the only thing that approves an
    /// `operator.action session.opened`), so replay knows who could approve what.
    ///
    /// Rules 8: the session is logged as opened on an asserted name (`auth: asserted`), so
    /// only a dev run (the roster's `auth`) opens one this way; a credentials run refuses
    /// it ([`EngineError::NotAuthenticated`]) and opens sessions with
    /// [`Engine::open_authenticated_session`] after the bridge checked the password.
    pub fn open_operator_session_with(
        &mut self,
        operator: OperatorId,
        grants: Grants,
    ) -> Result<OperatorSession, EngineError> {
        self.open_session(operator, grants, AuthMethod::Asserted, None)
    }

    /// Rules 8: open a session for an operator whose identity the bridge established by
    /// `method` (a credentials run: `Password`, a person's credential checked at login, or
    /// `Scripted`, a harness-held one checked the same way but never a person's; a dev run:
    /// `Asserted`). Logged as `operator.action session.opened` with
    /// `auth`. The only way into a session in a credentials run; refused when the method is
    /// not the run's ([`EngineError::NotAuthenticated`], nothing logged).
    pub fn open_authenticated_session(
        &mut self,
        operator: OperatorId,
        grants: Grants,
        method: AuthMethod,
    ) -> Result<OperatorSession, EngineError> {
        if self.auth.is_none() {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: "these decision rules record no authentication: open the session with open_operator_session_with".into(),
            });
        }
        if method == AuthMethod::Passkey {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: "a passkey login carries its assertion: open_passkey_session".into(),
            });
        }
        self.open_session(operator, grants, method, None)
    }

    /// Rules 10: open a session for an operator who signed in with a passkey: `proof` is the
    /// assertion the bridge checked, made for a login ([`crate::StepUpFor::Login`]) on a
    /// challenge [`Engine::passkey_challenge`] issued. The engine verifies it again against the
    /// public key the run logged (the credential must be the operator's, its counter must
    /// count up, the challenge unanswered and fresh) and logs it with the session
    /// (`session.opened`, `auth: passkey`, `passkey`). Refused, nothing logged, otherwise, and
    /// when the policy's `login_factor` is `password`.
    pub fn open_passkey_session(
        &mut self,
        operator: OperatorId,
        grants: Grants,
        proof: crate::passkey::PasskeyUse,
    ) -> Result<OperatorSession, EngineError> {
        if proof.purpose != crate::passkey::StepUpFor::Login {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: format!("the passkey assertion was made for {}, not a login", proof.purpose.describe()),
            });
        }
        self.check_passkey_use(&operator, None, &proof)
            .map_err(|why| EngineError::NotAuthenticated { operator: operator.clone(), why })?;
        self.open_session(operator, grants, AuthMethod::Passkey, Some(proof))
    }

    fn open_session(
        &mut self,
        operator: OperatorId,
        grants: Grants,
        method: AuthMethod,
        proof: Option<crate::passkey::PasskeyUse>,
    ) -> Result<OperatorSession, EngineError> {
        if let Some(why) = self.roster.refuses(&operator, grants) {
            return Err(EngineError::NotOnRoster {
                operator,
                why: why.into(),
            });
        }
        if let Some(mode) = self.auth
            && !mode.admits(method)
        {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: format!(
                    "a {} run opens sessions only on {} identities",
                    mode.as_str(),
                    match mode {
                        AuthMode::Credentials => "password-checked, passkey-checked or scripted",
                        AuthMode::Dev => "asserted",
                    }
                ),
            });
        }
        if method == AuthMethod::Passkey && false {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: "these decision rules have no passkeys".into(),
            });
        }
        // rules 12: a scripted credential opens scripted sessions, a person's a person's, as
        // the roster fixed them at the run's start
        if let Some(why) = self.roster.method_refused(&operator, method) {
            return Err(EngineError::NotAuthenticated { operator, why });
        }
        // rules 10: how a person may sign in (`[approval] login_factor`)
        if self.auth == Some(AuthMode::Credentials) && !self.policy.login_factor().admits(method) {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: format!(
                    "this run's policy signs people in by {} only ([approval] login_factor)",
                    self.policy.login_factor().as_str()
                ),
            });
        }
        let session = SessionId(self.next_session);
        let counter = proof.as_ref().map(|p| (p.credential.clone(), p.counter, p.nonce.clone()));
        self.emit(EventBody::OperatorAction {
            operator: operator.clone(),
            session,
            action: "session.opened".into(),
            detail: if grants.consequential {
                "consequential authority".into()
            } else {
                String::new()
            },
            grants: (grants != Grants::default()).then_some(grants),
            auth: self.auth.map(|_| method),
            step_up: None,
            passkey: proof.map(Box::new),
        })?;
        if let Some((id, n, nonce)) = counter {
            self.passkey_used(&id, n, nonce);
        }
        self.next_session += 1;
        Ok(OperatorSession::mint(operator, session, grants, self.auth.map(|_| method)))
    }

    // ---- passkeys (rules 10) ---------------------------------------------------------

    /// Rules 10: the challenge a passkey use for `purpose` by `operator` signs (in `session`;
    /// none for a login), with the bridge's random `nonce`, issued now. Not logged: the use
    /// logs it, and an unanswered challenge leaves no trace; but its time is a floor for every
    /// later event's (a host clock that steps back can't log the use before its challenge).
    /// Refused outside a credentials run of rules 10 that logged a relying party, for an
    /// operator not on the roster, or a session that is not theirs or has ended.
    pub fn passkey_challenge(
        &mut self,
        operator: &OperatorId,
        session: Option<&OperatorSession>,
        purpose: crate::passkey::StepUpFor,
        nonce: String,
    ) -> Result<crate::passkey::Issued, EngineError> {
        let refuse = |why: &str| EngineError::NotAuthenticated { operator: operator.clone(), why: why.into() };
        if self.auth != Some(AuthMode::Credentials) || self.roster.passkeys.is_none() {
            return Err(refuse("this run takes no passkey (a credentials run of rules 10 does)"));
        }
        if self.roster.refuses(operator, Grants::default()).is_some() {
            return Err(refuse("not on the run's roster"));
        }
        let sid = match session {
            Some(s) if s.operator() != operator => return Err(refuse("not this operator's session")),
            Some(s) if self.closed.contains(&s.authority().session()) => return Err(refuse("the session has ended")),
            Some(s) => Some(s.authority().session()),
            None => None,
        };
        if (purpose == crate::passkey::StepUpFor::Login) != sid.is_none() {
            return Err(refuse("a login's challenge has no session; every other one has its session"));
        }
        let issued_t = self.now();
        self.last_t = issued_t;
        let challenge = crate::passkey::challenge(&self.run_hash, sid, operator, &purpose, &nonce, issued_t);
        Ok(crate::passkey::Issued {
            purpose,
            nonce,
            issued_t,
            challenge,
        })
    }

    /// picture token `token` and the SHA-256 of that token (its plan, the intent it is for);
    /// any other intent's names the intent.
    pub fn sign_purpose(&self, id: IntentId, _token: Option<u64>) -> Result<crate::passkey::StepUpFor, EngineError> {
        self.get(id)?;
        Ok(crate::passkey::StepUpFor::Sign { intent: id, token: None, picture_sha256: None })
    }

    /// Why a passkey use by `operator` (in `session`; none for a login) can't be logged, if it
    /// can't: see [`crate::passkey`]. Ok with the counter it reported.
    fn check_passkey_use(
        &self,
        operator: &OperatorId,
        session: Option<SessionId>,
        u: &crate::passkey::PasskeyUse,
    ) -> Result<u32, String> {
        if self.auth != Some(AuthMode::Credentials) {
            return Err("only a credentials run of rules 10 takes a passkey".into());
        }
        let Some(rp) = &self.roster.passkeys else {
            return Err("this run logged no relying party".into());
        };
        if self.used_nonces.contains(&u.nonce) {
            return Err("this challenge was answered already (a passkey assertion is used once)".into());
        }
        let age = self.now().ms_since(u.issued_t);
        if !(0..=crate::passkey::CHALLENGE_TTL_MS).contains(&age) {
            return Err(format!(
                "the challenge is {age} ms old (at most {} ms): ask for a new one",
                crate::passkey::CHALLENGE_TTL_MS
            ));
        }
        let Some(seen) = self.passkeys.get(&u.credential) else {
            return Err("not a passkey this run knows".into());
        };
        if seen.removed {
            return Err("this passkey was removed".into());
        }
        if &seen.key.operator != operator {
            return Err("another operator's passkey".into());
        }
        let challenge = crate::passkey::challenge(&self.run_hash, session, operator, &u.purpose, &u.nonce, u.issued_t);
        let v = crate::passkey::verify(&seen.key, &rp.rp_id, &rp.origins, &challenge, &u.assertion)?;
        if v.counter != u.counter {
            return Err(format!("the counter given ({}) is not the authenticator's ({})", u.counter, v.counter));
        }
        if !crate::passkey::counter_ok(seen.key.counter, v.counter) {
            return Err(format!(
                "the authenticator's counter went from {} to {}: it may have been cloned, refused",
                seen.key.counter, v.counter
            ));
        }
        Ok(v.counter)
    }

    /// A passkey use was logged: its counter and nonce.
    fn passkey_used(&mut self, credential: &str, counter: u32, nonce: String) {
        if let Some(k) = self.passkeys.get_mut(credential) {
            k.key.counter = counter;
        }
        self.used_nonces.insert(nonce);
    }

    /// Rules 10, a credentials run: the operator of `session` stepped up with a passkey: the
    /// bridge checked the assertion (made on a challenge [`Engine::passkey_challenge`] issued
    /// for this session and act), the engine verifies it again against the run's logged
    /// public key and logs it (`operator.action step_up`, `auth: passkey`, `passkey`: the
    /// assertion and what it is for). The [`StepUp`] returned stands only for that act (the act
    /// compares its own purpose), once, within [`STEP_UP_MAX_AGE_MS`]. Refused, nothing
    /// logged, for a scripted session (a harness's credential holds no passkey), another
    /// operator's credential, a counter that does not count up, a challenge answered before or
    /// older than [`crate::CHALLENGE_TTL_MS`], or a login's assertion.
    pub fn step_up_passkey(
        &mut self,
        session: &OperatorSession,
        proof: crate::passkey::PasskeyUse,
    ) -> Result<StepUp, EngineError> {
        let operator = session.operator().clone();
        let id = session.authority().session();
        let refuse = |why: String| EngineError::NotAuthenticated { operator: operator.clone(), why };
        if self.closed.contains(&id) {
            return Err(refuse(format!("session {} has ended", id.0)));
        }
        if !session.method().is_some_and(AuthMethod::is_person) {
            return Err(refuse("a passkey is a person's: this session is not (scripted or asserted)".into()));
        }
        if proof.purpose == crate::passkey::StepUpFor::Login {
            return Err(refuse("a login's assertion is not a step-up".into()));
        }
        let counter = self.check_passkey_use(&operator, Some(id), &proof).map_err(refuse)?;
        let purpose = proof.purpose.clone();
        let nonce = proof.nonce.clone();
        let credential = proof.credential.clone();
        let t = self.emit(EventBody::OperatorAction {
            operator: operator.clone(),
            session: id,
            action: "step_up".into(),
            detail: format!("passkey for {}", purpose.describe()),
            grants: None,
            auth: Some(AuthMethod::Passkey),
            step_up: None,
            passkey: Some(Box::new(proof)),
        })?;
        self.passkey_used(&credential, counter, nonce);
        Ok(StepUp::mint(operator, id, self.seq, t, AuthMethod::Passkey, session.method(), Some(purpose)))
    }

    /// Rules 10, a credentials run: a passkey was registered for `key.operator` while the run
    /// is up (`passkey.registered`, public parts only): through `by`, the operator's own
    /// session, quoting the step-up they just made (their credential re-entered: a session
    /// left open does not add a credential to someone's account), or (`by` None) through a
    /// one-time invitation the bridge checked. Usable from here on. Refused, nothing logged,
    /// for a credential id the run knows already, a key that is not a P-256 point, an operator
    /// not on the roster or not the session's, or a stale or missing step-up.
    pub fn record_passkey_registered(
        &mut self,
        key: crate::passkey::PasskeyKey,
        by: Option<(&OperatorSession, StepUp)>,
    ) -> Result<(), EngineError> {
        let operator = key.operator.clone();
        let refuse = |why: String| EngineError::NotAuthenticated { operator: operator.clone(), why };
        if self.auth != Some(AuthMode::Credentials) || self.roster.passkeys.is_none() {
            return Err(refuse("only a credentials run of rules 10 takes a passkey".into()));
        }
        if self.roster.refuses(&operator, Grants::default()).is_some() {
            return Err(refuse("not on the run's roster".into()));
        }
        if self.passkeys.contains_key(&key.id) {
            return Err(refuse("this credential is registered already".into()));
        }
        crate::passkey::check_key(&key.public_key).map_err(refuse)?;
        let (session, step_up) = match by {
            Some((s, step)) => {
                if s.operator() != &operator {
                    return Err(refuse("a passkey is registered for the session's own operator".into()));
                }
                let sid = s.authority().session();
                if self.closed.contains(&sid) {
                    return Err(refuse(format!("session {} has ended", sid.0)));
                }
                if !s.method().is_some_and(AuthMethod::is_person) {
                    return Err(refuse("a passkey is a person's: this session is not (scripted or asserted)".into()));
                }
                if step.factor() != AuthMethod::Password {
                    return Err(refuse("a registration re-enters the password (a passkey assertion is made for another act)".into()));
                }
                let r = self.fresh_step(&operator, sid, &step).map_err(refuse)?;
                (Some(sid), Some(r))
            }
            None => (None, None),
        };
        self.emit(EventBody::PasskeyRegistered {
            key: key.clone(),
            via: if session.is_some() { "session".into() } else { "invitation".into() },
            session,
            step_up,
        })?;
        self.passkeys.insert(key.id.clone(), KeySeen { key, removed: false });
        Ok(())
    }

    /// Rules 10: a passkey was removed from the credential file while the run is up
    /// (`passkey.removed`); nothing may use it from here on. Nothing happens for one the run
    /// does not know or already removed.
    pub fn record_passkey_removed(&mut self, operator: &OperatorId, credential: &str, reason: &str) -> Result<(), EngineError> {
        match self.passkeys.get(credential) {
            Some(k) if !k.removed && &k.key.operator == operator => {}
            _ => return Ok(()),
        }
        self.emit(EventBody::PasskeyRemoved {
            operator: operator.clone(),
            credential: credential.to_string(),
            reason: reason.to_string(),
        })?;
        if let Some(k) = self.passkeys.get_mut(credential) {
            k.removed = true;
        }
        Ok(())
    }

    /// How this run authenticates its operators (rules 8; None before).
    pub fn auth_mode(&self) -> Option<AuthMode> {
        self.auth
    }

    /// Rules 8: end an operator session (logout, expiry, a credential removed or changed:
    /// `reason`), logged as `operator.action session.closed`. Consumes the session and the
    /// authorities it holds; picture tokens issued to it are dropped. Nothing may be logged
    /// under it afterwards (replay checks). Before rules 8 sessions are not closed: the
    /// session is dropped and nothing is logged.
    pub fn close_operator_session(
        &mut self,
        session: OperatorSession,
        reason: &str,
    ) -> Result<(), EngineError> {
        let id = session.authority().session();
        if self.auth.is_none() || self.closed.contains(&id) {
            return Ok(());
        }
        self.emit(EventBody::OperatorAction {
            operator: session.operator().clone(),
            session: id,
            action: "session.closed".into(),
            detail: reason.to_string(),
            grants: None,
            auth: None,
            step_up: None,
            passkey: None,
        })?;
        self.closed.insert(id);
        Ok(())
    }

    /// Rules 8, a credentials run: the operator of `session` re-entered their credential just
    /// now, and the bridge checked it (its step-up endpoint is the only caller, right before
    /// the signature it is for). Logged as `operator.action step_up` (`auth: password`) under
    /// the session; the [`StepUp`] returned is what a consequential signature quotes
    /// [`STEP_UP_MAX_AGE_MS`]. Refused (nothing logged) in a run that does not authenticate
    /// with credentials: a step-up there would prove nothing.
    pub fn step_up(&mut self, session: &OperatorSession) -> Result<StepUp, EngineError> {
        let operator = session.operator().clone();
        let id = session.authority().session();
        if self.auth != Some(AuthMode::Credentials) {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: "a step-up re-enters a credential: only a credentials run has one".into(),
            });
        }
        if self.closed.contains(&id) {
            return Err(EngineError::NotAuthenticated {
                operator,
                why: format!("session {} has ended", id.0),
            });
        }
        // the password re-entered (a person's, whether their session opened on a password or,
        // rules 10, a passkey), or a machine-held credential (the harness's scripted operators)
        let factor = match session.method() {
            Some(AuthMethod::Scripted) => AuthMethod::Scripted,
            _ => AuthMethod::Password,
        };
        let t = self.emit(EventBody::OperatorAction {
            operator: operator.clone(),
            session: id,
            action: "step_up".into(),
            detail: "credential re-entered to sign".into(),
            grants: None,
            auth: Some(factor),
            step_up: None,
            passkey: None,
        })?;
        Ok(StepUp::mint(operator, id, self.seq, t, factor, session.method(), None))
    }

    /// Why `step` can't stand for an act by `by` in `session` now (another session's or
    /// operator's, older than [`STEP_UP_MAX_AGE_MS`]; rules 10: a passkey's made for another
    /// act than `expected`, or a password where the policy asks a person for a passkey), else
    /// its reference.
    fn valid_step(
        &self,
        by: &OperatorId,
        session: SessionId,
        step: &StepUp,
        expected: Option<&crate::passkey::StepUpFor>,
    ) -> Result<StepUpRef, String> {
        let r = self.fresh_step(by, session, step)?;
        if let Some(made) = step.purpose()
            && Some(made) != expected
        {
            return Err(format!(
                "the passkey assertion was made for {}, not for {}",
                made.describe(),
                expected.map_or_else(|| "this act".to_string(), crate::passkey::StepUpFor::describe)
            ));
        }
        if let Some(why) = self.factor_refusal(step) {
            return Err(why);
        }
        Ok(r)
    }

    /// Why `step` can't stand for an act by `by` in `session` now (another session's or
    /// operator's, or older than [`STEP_UP_MAX_AGE_MS`]), else its reference.
    fn fresh_step(&self, by: &OperatorId, session: SessionId, step: &StepUp) -> Result<StepUpRef, String> {
        let now = self.now();
        if step.session() != session || step.operator() != by {
            return Err(format!("the step-up was made in session {} by {}, not this signer's", step.session().0, step.operator()));
        }
        if now.ms_since(step.t()) > STEP_UP_MAX_AGE_MS {
            return Err(format!("the step-up is {} ms old (at most {STEP_UP_MAX_AGE_MS})", now.ms_since(step.t())));
        }
        Ok(StepUpRef { seq: step.seq() })
    }

    /// Rules 10: why a person's step-up does not meet the policy's `step_up_factor`, if it
    /// doesn't (a scripted credential is a harness's: it steps up as it is held).
    fn factor_refusal(&self, step: &StepUp) -> Option<String> {
        let person = step.session_method().is_some_and(AuthMethod::is_person);
        (self.auth == Some(AuthMode::Credentials)
            && person
            && self.policy.step_up_factor() == crate::policy::StepUpFactor::Passkey
            && step.factor() != AuthMethod::Passkey)
            .then(|| {
                "this run's policy asks for your passkey at this step-up, not your password ([approval] step_up_factor = \"passkey\")".to_string()
            })
    }

    /// Rules 8, a credentials run: a login for `operator` (on the roster) was refused; logged
    /// as `auth.refused` with why and, when it locked the id, for how long. Never the
    /// password. Refused for an operator not on the roster (nothing logged: made-up ids would
    /// flood the log) and outside a credentials run.
    pub fn record_login_refused(
        &mut self,
        operator: &OperatorId,
        reason: &str,
        locked_s: Option<u64>,
    ) -> Result<(), EngineError> {
        if self.auth != Some(AuthMode::Credentials) {
            return Err(EngineError::NotAuthenticated {
                operator: operator.clone(),
                why: "only a credentials run has logins".into(),
            });
        }
        if self.roster.refuses(operator, Grants::default()).is_some() {
            return Err(EngineError::NotOnRoster {
                operator: operator.clone(),
                why: "not on the run's roster".into(),
            });
        }
        self.emit(EventBody::AuthRefused {
            operator: operator.clone(),
            reason: reason.to_string(),
            locked_s,
        })?;
        Ok(())
    }

    /// Whether a signature on `id` by `by` in `session` may go ahead as far as step-up goes,
    /// and the reference it records. Refused (logged as `approval.refused`) when the decision
    /// asks for a step-up in a credentials run and none was given; a step-up for another
    /// session, or too old, is refused too.
    fn check_step_up(
        &mut self,
        id: IntentId,
        by: &OperatorId,
        session: SessionId,
        step: Option<StepUp>,
        expected: &crate::passkey::StepUpFor,
    ) -> Result<Option<StepUpRef>, EngineError> {
        let needed = self.auth == Some(AuthMode::Credentials)
            && self.get(id)?.policy().is_some_and(|p| p.step_up);
        let Some(s) = step else {
            if needed {
                self.refuse(by, session, id, "step-up required: the signer must re-enter their credential to sign this")?;
                return Err(EngineError::StepUpRequired { id, by: by.clone() });
            }
            return Ok(None);
        };
        match self.valid_step(by, session, &s, Some(expected)) {
            Ok(r) => Ok(Some(r)),
            Err(why) => {
                self.refuse(by, session, id, &why)?;
                Err(EngineError::NotAuthenticated { operator: by.clone(), why })
            }
        }
    }

    /// A human's intent (also what `POST /api/tasks` becomes). If its checks pass and policy
    /// routes it to a human tier that lets the submitter authorize
    /// ([`crate::PolicyDecision::submitter_authorizes`]), the submission is the human
    /// decision: it goes straight to `approved` (`approval.at_submission`, approval
    /// `{by: this operator, at_submission: true, self_approved: true}`). Otherwise it waits
    /// in the inbox like any other.
    pub fn submit(
        &mut self,
        by: &HumanAuthority,
        proposal: Proposal,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<&Intent, EngineError> {
        let source = Source::Human {
            id: by.operator().clone(),
        };
        let id = self.submit_inner(source, Some(by), proposal, world, terrain)?;
        self.get(id)
    }

    /// Rules 12: the earlier logs whose carry failed, as the ledger has them, with why and the
    /// armed platforms their fleet named (a mission for one waits until the log is read or
    /// written off): for the inbox, what an operator could write off.
    pub fn pending_carries(&self) -> Vec<(String, String, Vec<Uid>)> {
        self.restores.pending()
    }


    /// Approve a pending intent. Re-runs the checks against the current world first: an
    /// approval of a picture that no longer holds is void.
    ///
    /// When the tier needs more than one approver (`approvers = N`), each call is one
    /// signature: the first N-1 are logged as `approval.concurred` and leave the intent
    /// pending; the Nth is `approval.granted` and approves it. Each signer must be a
    /// different operator, and where the tier excludes the proposer
    ///
    /// it is made with [`Engine::approve_acknowledging`].
    pub fn approve(
        &mut self,
        id: IntentId,
        by: &HumanAuthority,
        note: Option<String>,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<&Intent, EngineError> {
        self.approve_inner(id, by, note, false, None, world, terrain)
    }

    /// [`Engine::approve`], acknowledging the older orders it supersedes on the same
    /// vehicles (logged as `supersedes` on the approval).
    pub fn approve_acknowledging(
        &mut self,
        id: IntentId,
        by: &HumanAuthority,
        note: Option<String>,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<&Intent, EngineError> {
        self.approve_inner(id, by, note, true, None, world, terrain)
    }

    /// Rules 8: [`Engine::approve`] (with `acknowledge` as in
    /// [`Engine::approve_acknowledging`]) quoting a step-up, the approver's credential
    /// re-entered just now ([`Engine::step_up`]). A decision marked `step_up` needs one in a
    /// credentials run; the signature records it (`step_up: {seq}`).
    #[allow(clippy::too_many_arguments)]
    pub fn approve_stepped(
        &mut self,
        id: IntentId,
        by: &HumanAuthority,
        step: StepUp,
        note: Option<String>,
        acknowledge: bool,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<&Intent, EngineError> {
        self.approve_inner(id, by, note, acknowledge, Some(step), world, terrain)
    }

    #[allow(clippy::too_many_arguments)]
    fn approve_inner(
        &mut self,
        id: IntentId,
        by: &HumanAuthority,
        note: Option<String>,
        acknowledge: bool,
        step: Option<StepUp>,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<&Intent, EngineError> {
        self.sign(id, by.operator(), by.session(), note, acknowledge, step, world, terrain)
    }

    /// Log a refused approval attempt (nothing else changes).
    fn refuse(
        &mut self,
        by: &OperatorId,
        session: SessionId,
        id: IntentId,
        why: &str,
    ) -> Result<(), EngineError> {
        self.emit(EventBody::OperatorAction {
            operator: by.clone(),
            session,
            action: "approval.refused".into(),
            detail: format!("{id}: {why}"),
            grants: None,
            auth: None,
            step_up: None,
            passkey: None,
        })?;
        Ok(())
    }

    /// One signature on a pending intent, the common part of [`Engine::approve`] and its
    /// variants.
    #[allow(clippy::too_many_arguments)]
    fn sign(
        &mut self,
        id: IntentId,
        by: &OperatorId,
        session: SessionId,
        note: Option<String>,
        acknowledge: bool,
        step: Option<StepUp>,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<&Intent, EngineError> {
        let intent = self.get(id)?;
        next_state(intent.state(), Transition::Approve)
            .map_err(|err| EngineError::Illegal { id, err })?;
        let spec = intent.spec().clone();
        let expires = intent.expires_t();
        let (of, proposer_excluded) = intent
            .policy()
            .map_or((1, false), |p| (p.approvers.max(1), p.proposer_excluded));
        let signed_before = intent.concurrences().iter().any(|a| a.by == *by);
        let now = self.now();
        if expires.is_some_and(|e| now >= e) {
            self.expire(id)?;
            return Err(EngineError::Expired(id));
        }
        let self_approved = is_self_approval(&spec.source, by);
        if proposer_excluded && self_approved {
            self.refuse(by, session, id, "the proposer may not approve it (two-person rule)")?;
            return Err(EngineError::ProposerCannotApprove { id, by: by.clone() });
        }
        if signed_before {
            self.refuse(by, session, id, "already signed: another operator must")?;
            return Err(EngineError::AlreadySigned { id, by: by.clone() });
        }
        // rules 8: a consequential approval quotes the approver's step-up (rules 10: a
        // passkey's, made for a signature on this intent)
        let expected = crate::passkey::StepUpFor::Sign { intent: id, token: None, picture_sha256: None };
        let step_up = self.check_step_up(id, by, session, step, &expected)?;
        let (inputs, _, results, ok) = self.run_checks(&spec, CheckPhase::Approve, world, terrain)?;
        if !ok {
            let reason = format!(
                "checks failed at approval: {}",
                summarize_failures(&results)
            );
            self.transition(id, Transition::Void, reason)?;
            return Err(EngineError::ChecksFailedAtApproval(id));
        }
        // acknowledgement has nothing to acknowledge
        let _ = acknowledge;
        let n = u32::try_from(self.get(id)?.concurrences().len()).unwrap_or(u32::MAX) + 1;
        if n < of {
            let t = self.emit(EventBody::ApprovalConcurred {
                intent: id,
                by: by.clone(),
                session,
                n,
                of,
                note: note.clone(),
                supersedes: Vec::new(),
                checked_t: None,
                step_up,
            })?;
            self.get_mut(id)?.push_concurrence(Approval {
                by: by.clone(),
                session,
                t,
                self_approved,
                at_submission: false,
                note,
                step_up: step_up.is_some(),
            });
            return self.get(id);
        }
        let t = self.emit(EventBody::ApprovalGranted {
            intent: id,
            by: by.clone(),
            session,
            self_approved,
            note: note.clone(),
            supersedes: Vec::new(),
            checked_t: None,
            step_up,
        })?;
        self.get_mut(id)?.set_approval(Approval {
            by: by.clone(),
            session,
            t,
            self_approved,
            at_submission: false,
            note,
            step_up: step_up.is_some(),
        });
        let signers = if of > 1 {
            format!(" (signature {n} of {of})")
        } else {
            String::new()
        };
        let reason = if self_approved {
            format!("approved by {by} (self-approved: also the proposer){signers}")
        } else {
            format!("approved by {by}{signers}")
        };
        let t = self.transition_at(id, Transition::Approve, reason)?;
        // what this approval rests on: its re-check, the decision that asked for it
        let policy = self.get(id)?.policy().cloned();
        self.get_mut(id)?.set_basis(Basis {
            t,
            by_human: true,
            checks: results,
            policy,
            in_play: inputs.in_play.clone(),
        });
        self.get(id)
    }

    pub fn reject(
        &mut self,
        id: IntentId,
        by: &HumanAuthority,
        note: Option<String>,
    ) -> Result<&Intent, EngineError> {
        next_state(self.get(id)?.state(), Transition::Reject)
            .map_err(|err| EngineError::Illegal { id, err })?;
        self.emit(EventBody::ApprovalRejected {
            intent: id,
            by: by.operator().clone(),
            session: by.session(),
            note,
        })?;
        self.transition(
            id,
            Transition::Reject,
            format!("rejected by {}", by.operator()),
        )?;
        self.get(id)
    }


    /// An operator withdraws an intent before dispatch (pending, approved or queued:
    /// `voided`), or, rules 11 ([`rules::SPACE_FOLLOWUPS_SINCE`]), a dispatched collect or
    /// cover: the engine then reports what the cancel means for the order under its
    /// resource (`op.state cancelled`, [`space::cancel_detail`]: a tasking request
    /// withdrawn before its pass; a pass already under way, which cannot be recalled, its
    /// product no longer awaited; a watch ended) and the intent is `aborted` (transition
    /// order supersedes the one, an abort stops the other), and an agent withdraws nothing
    /// after dispatch.
    pub fn cancel(
        &mut self,
        id: IntentId,
        by: &HumanAuthority,
        note: &str,
    ) -> Result<&Intent, EngineError> {
        let intent = self.get(id)?;
        let after_dispatch = intent.state() == State::Dispatched
            && false
            && true;
        let transition = if after_dispatch {
            Transition::Cancel
        } else {
            Transition::Void
        };
        next_state(intent.state(), transition).map_err(|err| EngineError::Illegal { id, err })?;
        self.emit(EventBody::OperatorAction {
            operator: by.operator().clone(),
            session: by.session(),
            action: "intent.cancel".into(),
            detail: format!("{id}: {note}"),
            grants: None,
            auth: None,
            step_up: None,
            passkey: None,
        })?;
        self.transition(
            id,
            transition,
            format!("cancelled by {}: {note}", by.operator()),
        )?;
        self.get(id)
    }

    /// Log something an operator did that isn't an intent decision (acknowledge, snooze).
    /// Rules 8: the session's own events (`session.opened`, `session.closed`, `step_up`) are
    /// the engine's to log, refused here in a credentials run.
    pub fn record_operator_action(
        &mut self,
        by: &HumanAuthority,
        action: &str,
        detail: &str,
    ) -> Result<(), EngineError> {
        if self.auth.is_some() && matches!(action, "session.opened" | "session.closed" | "step_up") {
            return Err(EngineError::NotAuthenticated {
                operator: by.operator().clone(),
                why: format!("{action} is logged by the session itself"),
            });
        }
        self.emit(EventBody::OperatorAction {
            operator: by.operator().clone(),
            session: by.session(),
            action: action.into(),
            detail: detail.into(),
            grants: None,
            auth: None,
            step_up: None,
            passkey: None,
        })?;
        Ok(())
    }

    /// Log what an operator's console showed for an intent (`operator.view`, Phase G):
    /// the card opened, closed, or a decision button was clicked, with what was visible.
    /// Decides nothing; replay makes no decision from it (it checks the session is the
    /// operator's). The intent must exist, the view be well-formed
    /// ([`OperatorView::validate`]), and an `open` or `decide` view is taken only while the
    /// intent waits for a decision (`close` any time: a card can go away after its intent
    /// ended), so a late decide view cannot follow the decision it describes.
    pub fn record_operator_view(
        &mut self,
        by: &HumanAuthority,
        intent: IntentId,
        view: OperatorView,
    ) -> Result<(), EngineError> {
        let state = self.get(intent)?.state();
        view.validate()
            .map_err(|m| EngineError::Proposal(ProposalError::BadParams(m)))?;
        if view.event != "close" && state != State::PendingApproval {
            return Err(EngineError::ViewNotPending { id: intent, state });
        }
        self.emit(EventBody::OperatorView {
            operator: by.operator().clone(),
            session: by.session(),
            intent,
            view,
        })?;
        Ok(())
    }




    /// Rules 5: the human-approved orders waiting in the queue on `spec`'s vehicles, other
    /// than `spec` itself: a fact the engine logs with each decision (`waiting`).
    fn waiting_for(&self, spec: &IntentSpec) -> BTreeMap<Uid, IntentId> {
        waiting_vehicles(
            self.intents.values().map(|i| {
                (
                    i.id(),
                    i.spec(),
                    i.state(),
                    i.basis().is_some_and(|b| b.by_human),
                    !i.committed().is_empty(),
                )
            }),
            spec,
        )
    }


    /// Rules 5: a newer order still standing for one of the intent's vehicles, and which
    /// vehicle (see [`Engine::dequeue`]): an older order is neither sent nor followed over it.
    pub fn newer_order(&self, id: IntentId) -> Option<(IntentId, Uid)> {
        self.newer_on(self.intents.get(&id)?.spec())
    }

    /// Rules 5: a newer order still standing for one of `spec`'s vehicles (approved, queued,
    /// dispatched, or waiting for a human's approval after a human submitted it), and which
    /// vehicle. An older queued order never goes over it: the newest order stands.
    fn newer_on(&self, spec: &IntentSpec) -> Option<(IntentId, Uid)> {
        self.intents.values().find_map(|i| {
            let standing = match i.state() {
                State::Approved | State::AutoApproved | State::Queued | State::Dispatched => true,
                State::PendingApproval => !i.spec().source.is_agent(),
                _ => false,
            };
            if i.id() <= spec.id || false || !standing {
                return None;
            }
            let u = spec.uids.iter().find(|u| i.spec().uids.contains(u))?;
            Some((i.id(), u.clone()))
        })
    }



    /// Rules 30: our vehicles confirmed lost that a decision on `w` reads (its `nodes_lost` fact,
    /// from the ledger, never the caller's): each in force on a vehicle of `w` not heard since on
    /// `w`'s facts (one heard is revoked before the decision is logged, [`Self::log_inputs_at`]),
    /// a carried one with the vehicle under the uid now. Every decision's inputs are filled so
    /// (they all pass through [`Self::identify`]); nothing before rules 30.
    fn fill_nodes(&self, w: &mut WorldSnapshot) {
        w.nodes_lost = {
            self.nodes.lost_for(&w.vehicles, w.t, &self.policy.rules)
        };
    }

    /// The inputs a decision on `spec` would read on `world` now, filled as the engine fills
    /// not asked for.
    pub fn dry_run_inputs(&self, spec: &IntentSpec, world: &WorldSnapshot, terrain: &dyn TerrainOracle) -> WorldSnapshot {
        let world = self.with_fields(spec, world);
        let mut inputs = world.for_intent_with(spec, terrain, &self.policy.rules);
        self.fill_facts(&mut inputs);
        self.fill_engine_facts(spec, &mut inputs);
        inputs
    }

    /// What the engine fills into a decision's inputs from its own ledger, after the world's part:
    /// our vehicles confirmed lost. Every decision's checks read them so, and so does every dry
    /// decision (a preview sees the same losses).
    fn fill_facts(&self, w: &mut WorldSnapshot) {
        self.fill_nodes(w);
    }

    /// The facts a queue reach judgement reads: the intent's vehicles of `world`, with the
    /// nodes lost filled in.
    fn reach_inputs(&self, spec: &IntentSpec, world: &WorldSnapshot) -> WorldSnapshot {
        let mut inputs = world.subset_for(spec);
        self.fill_facts(&mut inputs);
        inputs
    }

    /// For the console (not a decision, not logged): whether each of a queued intent's
    /// vehicles can be reached for it on `world`, and why not, as its dequeue would judge it.
    pub fn reach_now(&self, id: IntentId, world: &WorldSnapshot) -> Option<Vec<Reach>> {
        let spec = self.intents.get(&id)?.spec();
        Some(queue::reach(
            spec,
            &self.reach_inputs(spec, world),
            &self.policy.queue(),
            &self.policy.rules,
        ))
    }

    fn fill_engine_facts(&self, spec: &IntentSpec, inputs: &mut WorldSnapshot) {
        inputs.waiting = self.waiting_for(spec);
        self.fill_delivery(spec, inputs);
        // rules 8: the plays in force when it is decided
        if self.policy.rules.at_least(crate::rules::PLAYS_SINCE) {
            inputs.plays = crate::plays::in_force(&self.plays.values().cloned().collect::<Vec<_>>(), inputs.t);
        }
    }

    /// Rules 8 ([`crate::rules::PLAYS_SINCE`], docs/design/incidents.md): a person adopts a play, a
    /// standing decision for a pattern at a hub, in force from now for `for_s` at most. Refused
    /// under older rules, without `[plays]` in the policy, or outside it.
    pub fn adopt_play(&mut self, by: &HumanAuthority, req: crate::plays::PlayRequest) -> Result<crate::plays::Play, EngineError> {
        if !self.policy.rules.at_least(crate::rules::PLAYS_SINCE) {
            return Err(EngineError::Play(format!("these decision rules take no play (rules {})", crate::rules::PLAYS_SINCE)));
        }
        let Some(pp) = self.policy.plays.clone() else {
            return Err(EngineError::Play("this run's policy has no [plays]: no play is adopted".into()));
        };
        if let Some(why) = pp.refuse(&req) {
            return Err(EngineError::Play(why));
        }
        let now = self.now();
        let play = crate::plays::Play {
            id: self.next_play,
            kind: req.kind,
            hub: req.hub,
            codes: req.codes,
            limits: req.limits,
            except_priority: req.except_priority,
            until_t: now.plus_ms(i64::from(req.for_s) * 1000),
            lifts: req.lifts,
            by: by.operator().clone(),
        };
        self.emit_at(now, EventBody::PlayAdopted { play: play.clone(), session: by.session() })?;
        self.next_play += 1;
        self.plays.insert(play.id, play.clone());
        Ok(play)
    }

    /// Rules 8: a play ends: lifted by a person (`by`), or its end condition cleared (`by` none:
    /// the bridge, which watches it).
    pub fn end_play(&mut self, id: u64, why: &str, by: Option<&HumanAuthority>) -> Result<(), EngineError> {
        if !self.plays.contains_key(&id) {
            return Err(EngineError::Play(format!("play {id} is not in force")));
        }
        self.emit(EventBody::PlayEnded { play: id, why: why.to_string(), by: by.map(|b| b.operator().clone()), session: by.map(HumanAuthority::session) })?;
        self.plays.remove(&id);
        Ok(())
    }

    /// Rules 8: end every play past its `until_t` (logged at the time it ran out). Call
    /// periodically, beside [`Engine::expire_due`].
    pub fn expire_plays(&mut self) -> Result<Vec<u64>, EngineError> {
        let now = self.now();
        let due: Vec<u64> = self.plays.values().filter(|p| now >= p.until_t).map(|p| p.id).collect();
        for id in &due {
            self.emit(EventBody::PlayEnded { play: *id, why: "its time ran out".into(), by: None, session: None })?;
            self.plays.remove(id);
        }
        Ok(due)
    }

    /// Rules 8: the plays in force.
    pub fn plays(&self) -> Vec<crate::plays::Play> {
        crate::plays::in_force(&self.plays.values().cloned().collect::<Vec<_>>(), self.now())
    }

    /// What the delivery rules read of the engine's own orders (delivery-orders.md section 6):
    /// when each drop zone was confirmed, the order an `abort` names, the order a `reroute`
    /// replaces, the `deliver` a `confirm_drop_zone` is for. Logged by value with the decision.
    fn fill_delivery(&self, spec: &IntentSpec, inputs: &mut WorldSnapshot) {
        use crate::delivery::{RerouteBase, TargetFact};
        let vehicles = inputs.vehicles.clone();
        let Some(di) = inputs.delivery.as_mut() else { return };
        di.confirmed = self.dz_confirmed.clone();
        di.targets.clear();
        di.reroute_base.clear();
        di.waiting_deliver = None;
        match &spec.task {
            Task::Abort { target, .. } => {
                if let Some(i) = self.intents.get(target) {
                    let (parcel, drop_zone) = match &i.spec().task {
                        Task::Deliver { parcel, drop_zone, .. } => (Some(parcel.clone()), Some(drop_zone.clone())),
                        _ => (None, None),
                    };
                    let released = parcel.as_ref().is_some_and(|p| {
                        di.parcels.get(p).is_some_and(|f| f.state == crate::delivery::ParcelState::Released)
                    }) || i.ops().values().any(|o| o == "released");
                    di.targets.insert(
                        target.to_string(),
                        TargetFact {
                            kind: i.spec().kind(),
                            state: i.state(),
                            uids: i.spec().uids.clone(),
                            parcel,
                            drop_zone,
                            released,
                        },
                    );
                }
            }
            Task::Reroute { .. } => {
                for uid in &spec.uids {
                    let running = self.intents.values().find(|i| {
                        i.state() == State::Dispatched
                            && i.spec().uids.contains(uid)
                            && matches!(i.spec().kind(), TaskKind::Deliver | TaskKind::Pickup | TaskKind::ReturnToHub)
                            && !i.ops().get(uid).is_some_and(|o| queue::ended_op(o))
                    });
                    if let Some(i) = running {
                        let mut pts: Vec<crate::geo::LonLat> =
                            vehicles.get(uid).and_then(|v| v.position.as_ref()).map(|p| p.value).into_iter().collect();
                        if let Some(p) = i.spec().task.path() {
                            pts.extend(p.points);
                        }
                        let route_m = pts.windows(2).map(|w| crate::geo::haversine_m(w[0], w[1])).sum();
                        di.reroute_base.insert(uid.clone(), RerouteBase { intent: i.id(), kind: i.spec().kind(), route_m });
                    }
                }
            }
            Task::ConfirmDropZone { drop_zone, parcel, .. } => {
                di.waiting_deliver = self
                    .intents
                    .values()
                    .find(|i| {
                        matches!(
                            i.state(),
                            State::PendingApproval | State::Approved | State::AutoApproved | State::Queued | State::Dispatched
                        ) && matches!(&i.spec().task, Task::Deliver { parcel: p, drop_zone: z, .. } if p == parcel && z == drop_zone)
                    })
                    .map(Intent::id);
            }
            _ => {}
        }
    }

    /// A person's confirmation of a drop zone, from an approval: an approved `confirm_drop_zone`,
    /// or an approved `deliver` that was held for `DZ_UNCONFIRMED` (its approval counts as the
    /// confirmation, delivery-orders.md 5.2). Valid for the policy's `confirm_valid_s` under rules
    /// 1; from rules 2 ([`rules::STANDING_APPROVALS_SINCE`]) it approves the zone as a delivery
    /// point, and so does an approved `deliver` held for `DZ_NOT_APPROVED`, from then on.
    fn note_confirmation(&mut self, id: IntentId, at: Time) {
        let Some(i) = self.intents.get(&id) else { return };
        let zone = match &i.spec().task {
            Task::ConfirmDropZone { drop_zone, .. } => Some(drop_zone.clone()),
            Task::Deliver { drop_zone, .. }
                if i.checks().iter().any(|c| c.codes.iter().any(|k| k == "DZ_UNCONFIRMED" || k == "DZ_NOT_APPROVED")) =>
            {
                Some(drop_zone.clone())
            }
            _ => None,
        };
        if let Some(z) = zone {
            self.dz_confirmed.insert(z, at);
        }
    }

    /// A `confirm_drop_zone` that was rejected or expired ends the `deliver` that waited on it
    /// (delivery-orders.md 5.9): voided, the dispatcher asked for a replacement. An expiry is
    /// logged as "nobody decided".
    fn end_waiting_deliver(&mut self, id: IntentId, why: &str) -> Result<(), EngineError> {
        let Some(Task::ConfirmDropZone { drop_zone, parcel, .. }) = self.intents.get(&id).map(|i| i.spec().task.clone()) else {
            return Ok(());
        };
        let waiting: Vec<IntentId> = self
            .intents
            .values()
            .filter(|i| {
                matches!(i.state(), State::PendingApproval | State::Approved | State::AutoApproved | State::Queued)
                    && matches!(&i.spec().task, Task::Deliver { parcel: p, drop_zone: z, .. } if *p == parcel && *z == drop_zone)
            })
            .map(Intent::id)
            .collect();
        for w in waiting {
            self.void(w, &format!("confirm_drop_zone {id} {why}: the delivery is voided"))?;
        }
        Ok(())
    }

    /// Rules 30: for each of our silent vehicles in `world` but `platform`, the orders this
    /// engine's record says may be running on it ([`crate::silent::in_force`]).
    fn silent_orders(&self, platform: Option<&Uid>, world: &WorldSnapshot) -> BTreeMap<Uid, crate::silent::InForce> {
        let silent = crate::silent::silent_uids(world, &self.policy.rules, platform);
        if silent.is_empty() {
            return BTreeMap::new();
        }
        let views: Vec<crate::silent::IntentView> = self
            .intents
            .values()
            .filter(|i| i.spec().uids.iter().any(|u| silent.contains(u)))
            .map(|i| crate::silent::IntentView::of(i, &self.policy.rules))
            .collect();
        crate::silent::in_force(world, &self.policy.rules, platform, &views, &self.nodes)
    }

    /// Rules 30: the region each silent vehicle of ours can be in, as the friendly keep-out would
    /// count it now ([`crate::silent::region`] on the orders this engine's
    /// record says may be running on it). Read-only and never logged: for a display, and for the
    /// headless check of the regions against the truth.
    pub fn silent_regions(&self, world: &WorldSnapshot, at: Time) -> BTreeMap<Uid, crate::silent::Region> {
        let mut w = world.clone();
        w.silent_orders = self.silent_orders(None, world);
        w.silent_orders
            .keys()
            .filter_map(|u| {
                let v = w.vehicles.get(u)?;
                Some((u.clone(), crate::silent::region(u, v, &w, &self.policy.rules, at, false)?))
            })
            .collect()
    }

    /// Rules 27: `world` with our live fields a decision on `spec` reads in it (the ledger's,
    /// never the caller's), so that the route plans and the geofence read them: the world itself
    /// when they are the same (no field: no copy).
    fn with_fields<'w>(&self, _spec: &IntentSpec, world: &'w WorldSnapshot) -> std::borrow::Cow<'w, WorldSnapshot> {
        let w = world.clone();
        std::borrow::Cow::Owned(w)
    }




    // ---- agent side -----------------------------------------------------------------

    /// The agent-facing surface: propose, withdraw its own, read. No path to approval.
    pub fn agent_port(&mut self, agent: AgentId) -> AgentPort<'_> {
        AgentPort {
            engine: self,
            agent,
        }
    }

    // ---- system side ----------------------------------------------------------------

    /// The bridge voids an intent whose basis no longer holds (vehicle lost, superseded).
    /// Anyone can stop an action; only a human can authorize one.
    pub fn void(&mut self, id: IntentId, reason: &str) -> Result<&Intent, EngineError> {
        let note = self.running_note(id);
        self.transition(id, Transition::Void, format!("system: {reason}{note}"))?;
        self.get(id)
    }

    /// Rules 5: what a vehicle may still be doing with an intent that ends or goes to a human
    /// unconfirmed: running it (its start went out and nothing confirmed it), or holding it
    /// (it was sent before its link was lost). Empty otherwise.
    fn running_note(&self, id: IntentId) -> String {
        let Some(i) = self.intents.get(&id) else {
            return String::new();
        };
        if !i.maybe_started().is_empty() {
            let who: Vec<String> = i.maybe_started().iter().map(ToString::to_string).collect();
            return format!(
                " (it may be running on vehicle(s) {}: its start went out before the link was lost, and nothing confirmed it since; look at the vehicle)",
                who.join(", ")
            );
        }
        if i.dispatched_before() && i.state() == State::Queued {
            return " (it was sent before its link was lost: the vehicle may hold it; look at the vehicle)".into();
        }
        String::new()
    }

    /// Expire every pending approval past its deadline. Call periodically.
    pub fn expire_due(&mut self) -> Result<Vec<IntentId>, EngineError> {
        let now = self.now();
        let due: Vec<IntentId> = self
            .intents
            .values()
            .filter(|i| {
                i.state() == State::PendingApproval && i.expires_t().is_some_and(|e| now >= e)
            })
            .map(Intent::id)
            .collect();
        for id in &due {
            self.expire(*id)?;
        }
        // rules 5: queued intents whose vehicles were not reachable before their deadline
        let queued: Vec<IntentId> = self
            .intents
            .values()
            .filter(|i| {
                i.state() == State::Queued && i.queue().is_some_and(|q| now >= q.expires_t)
            })
            .map(Intent::id)
            .collect();
        for id in &queued {
            self.expire_queued(*id)?;
        }
        Ok(due.into_iter().chain(queued).collect())
    }

    /// Hand an approved (or auto-approved) vehicle intent to the task layer. A collect or
    /// cover is refused ([`EngineError::SpaceIntent`], nothing changes): it has no vehicles,
    /// so use [`Engine::dispatch_order`]. Rules 5, a kind the queue holds: only if the link
    /// reaches every vehicle on `world` ([`queue::reach`], logged with the dispatch); else
    /// refused, nothing logged ([`Engine::dispatch_or_queue`] queues it instead).
    pub fn dispatch(
        &mut self,
        id: IntentId,
        world: &WorldSnapshot,
    ) -> Result<DispatchOrder, EngineError> {
        let spec = self.get(id)?.spec().clone();
        let order = if self.reach_needed(&spec) {
            next_state(self.get(id)?.state(), Transition::Dispatch)
                .map_err(|err| EngineError::Illegal { id, err })?;
            self.refuse_queued(id)?;
            let cfg = self.policy.queue();
            let inputs = self.reach_inputs(&spec, world);
            let reach = queue::reach(&spec, &inputs, &cfg, &self.policy.rules);
            if !queue::all_reachable(&reach) {
                return Err(EngineError::NotDispatchable {
                    id,
                    why: format!(
                        "the link can't reach its vehicles now ({}): queue it (dispatch_or_queue)",
                        queue::unreachable_summary(&reach)
                    ),
                });
            }
            let logged = self.log_inputs(&inputs)?;
            self.dispatch_inner(id, Some((logged, reach)))?
        } else {
            self.dispatch_order(id)?
        };
        match order {
            Order::Vehicles(o) => Ok(o),
        }
    }

    /// Rules 5: whether dispatching `spec` straight from its approval needs the link's reach
    /// logged with it (a kind the policy's queue holds).
    fn reach_needed(&self, spec: &IntentSpec) -> bool {
        true && self.policy.queue().queues(spec.kind())
    }

    /// Rules 34: a group order goes out only as its member orders ([`Engine::dispatch_group`]).
    fn refuse_group(&self, id: IntentId) -> Result<(), EngineError> {
        if crate::group::is_group(self.get(id)?.spec(), &self.policy.rules) {
            return Err(EngineError::NotDispatchable {
                id,
                why: "a group order goes out as its member orders (dispatch_group)".into(),
            });
        }
        Ok(())
    }

    /// Rules 34: how long a member order of a group may wait in the queue, s (its group's
    /// `wait_s`; none: the queue's own wait).
    fn member_wait(&self, spec: &IntentSpec) -> Option<u32> {
        let g = self.intents.get(&spec.parent()?)?;
        g.spec().group()?.wait_s
    }

    /// A queued intent leaves the queue only through [`Engine::dequeue`] (its re-check).
    fn refuse_queued(&self, id: IntentId) -> Result<(), EngineError> {
        if self.get(id)?.state() == State::Queued {
            return Err(EngineError::NotDispatchable {
                id,
                why: "it is queued: it leaves the queue through its re-check (dequeue)".into(),
            });
        }
        Ok(())
    }

    /// Hand any approved (or auto-approved) intent on: a [`DispatchOrder`] for vehicle
    /// tasks, a [`CollectOrder`] (the chosen option, as the last check found it) or a
    /// [`CoverOrder`] for space tasks. Logged as `intent.dispatched` (with `space` for
    /// collect and cover). Rules 5: a vehicle task the queue holds goes through
    /// [`Engine::dispatch`] or [`Engine::dispatch_or_queue`] (the link's reach is logged with
    /// it), and a queued intent through [`Engine::dequeue`]: refused here.
    pub fn dispatch_order(&mut self, id: IntentId) -> Result<Order, EngineError> {
        self.refuse_group(id)?;
        let spec = self.get(id)?.spec().clone();
        if self.reach_needed(&spec) {
            return Err(EngineError::NotDispatchable {
                id,
                why: "under rules 5 a vehicle task is dispatched with its vehicles' reach (dispatch, dispatch_or_queue)".into(),
            });
        }
        self.refuse_queued(id)?;
        self.dispatch_inner(id, None)
    }

    /// The dispatch itself: `intent.dispatched` (with the reach that allowed it, when one was
    /// judged), then the transition.
    fn dispatch_inner(
        &mut self,
        id: IntentId,
        reached: Option<(LoggedInputs, Vec<Reach>)>,
    ) -> Result<Order, EngineError> {
        let intent = self.get(id)?;
        next_state(intent.state(), Transition::Dispatch)
            .map_err(|err| EngineError::Illegal { id, err })?;
        let intent = self.get(id)?;
        let order = order_for(intent.spec(), intent.route_plans(), &self.policy.rules)?;
        let reason = "dispatched to the task layer";
        let uids = intent.spec().uids.clone();
        let t = self.now().max(self.last_t);
        let mut event = dispatched_event(intent.spec(), &order);
        if let (
            EventBody::IntentDispatched {
                inputs: logged,
                reach: logged_reach,
                ..
            },
            Some((inputs, reach)),
        ) = (&mut event, reached)
        {
            *logged = Some(inputs);
            *logged_reach = Some(reach);
        }
        self.emit_at(t, event)?;
        self.transition(id, Transition::Dispatch, reason.into())?;
        self.get_mut(id)?.mark_dispatched();
        self.last_dispatched = Some(id);
        self.step = Some((id, self.seq));
        for u in uids {
            self.newest_on.insert(u, id);
        }
        Ok(order)
    }

    /// Rules 34 ([`crate::rules::GROUP_ORDER_SINCE`]): send an approved (or auto-approved) group
    /// order out as its member orders (docs/SWARMS.md section 2.2): for each member its last
    /// passing check counted (sent or waiting), in uid order, a one-vehicle intent of its own task
    /// and share, its `parent` the group (`intent.created`), approved under the group's approval
    /// (the transition `under`, its basis the group's decision); then `group.dispatched` and the
    /// group's own dispatch; then each member order dispatched or queued on its own, as any
    /// approved order is ([`Engine::dispatch_or_queue`]: a waiting member waits in the queue until
    /// order and what became of it.
    pub fn dispatch_group(&mut self, id: IntentId, world: &WorldSnapshot) -> Result<Vec<(IntentId, Routed)>, EngineError> {
        let intent = self.get(id)?;
        let spec = intent.spec().clone();
        if !crate::group::is_group(&spec, &self.policy.rules) {
            return Err(EngineError::NotDispatchable {
                id,
                why: "not a group order".into(),
            });
        }
        if !matches!(intent.state(), State::Approved | State::AutoApproved) {
            let err = next_state(intent.state(), Transition::Dispatch).err().unwrap_or(IllegalTransition {
                from: intent.state(),
                transition: Transition::Dispatch,
            });
            return Err(EngineError::Illegal { id, err });
        }
        let Some(eval) = intent.group_eval().cloned() else {
            return Err(EngineError::NotDispatchable {
                id,
                why: "no passing check laid its members out".into(),
            });
        };
        let basis = intent.basis().cloned().unwrap_or_else(|| no_basis(self.now()));
        let decision = intent.policy().cloned();
        let who = match intent.approval() {
            Some(a) if a.at_submission => format!("authorized at submission by {}", a.by),
            Some(a) => format!("approved by {}", a.by),
            None => format!("auto-approved by the policy's {} tier", decision.as_ref().map_or("", |d| d.rule.as_str())),
        };
        let plans = intent.route_plans().clone();
        let mut members: BTreeMap<Uid, IntentId> = BTreeMap::new();
        for uid in eval.counted() {
            let Some(mut ms) = eval.member_spec(&spec, &uid) else { continue };
            let mid = IntentId(self.next_intent);
            ms.id = mid;
            ms.created_t = self.now();
            self.emit(EventBody::IntentCreated {
                intent: ms.clone(),
                session: None,
            })?;
            self.next_intent += 1;
            let mut member = Intent::new(ms.clone());
            // rules 22: the way its group's check planned for its share
            if let Some(p) = plans.get(&uid) {
                member.set_route_plans(BTreeMap::from([(uid.clone(), p.clone())]));
            }
            self.intents.insert(mid, member);
            let waits = eval.members.get(&uid).and_then(|m| m.class) == Some(crate::group::MemberClass::Waiting);
            self.transition(
                mid,
                Transition::Under,
                format!(
                    "approved under group order {id} ({who}){}",
                    if waits { "; it waits for fresh facts" } else { "" }
                ),
            )?;
            self.get_mut(mid)?.set_basis(crate::group::member_basis(&basis, decision.as_ref()));
            members.insert(uid, mid);
        }
        let excluded = spec.uids.len() - members.len();
        let dispatched_t = self.emit(EventBody::GroupDispatched {
            intent: id,
            members: members.clone(),
        })?;
        self.transition(
            id,
            Transition::Dispatch,
            format!("dispatched as {} member order(s); {excluded} left out by its check", members.len()),
        )?;
        self.get_mut(id)?.set_members(members.clone());
        self.get_mut(id)?.mark_dispatched();
        // rules 34, stage 4: in force on its layout, for re-forms
        self.forces.insert(id, crate::group::Force::new(eval.layout.clone(), members.clone(), dispatched_t));
        let mut out = Vec::new();
        for mid in members.values() {
            out.push((*mid, self.dispatch_or_queue(*mid, world)?));
        }
        Ok(out)
    }

    /// Rules 5: dispatch an approved (or auto-approved) vehicle intent if the link can carry
    /// it to every one of its vehicles now ([`queue::reach`] on `world`), else hold it in the
    /// queue (`intent.queued`, with the vehicle facts that judgement read and its deadline,
    /// rules before 5 are dispatched as [`Engine::dispatch_order`] does.
    pub fn dispatch_or_queue(
        &mut self,
        id: IntentId,
        world: &WorldSnapshot,
    ) -> Result<Routed, EngineError> {
        self.refuse_group(id)?;
        let intent = self.get(id)?;
        let state = intent.state();
        if !matches!(state, State::Approved | State::AutoApproved) {
            let err = next_state(state, Transition::Queue)
                .err()
                .unwrap_or(IllegalTransition {
                    from: state,
                    transition: Transition::Queue,
                });
            return Err(EngineError::Illegal { id, err });
        }
        let spec = intent.spec().clone();
        let cfg = self.policy.queue();
        if !self.reach_needed(&spec) {
            return Ok(Routed::Dispatched(self.dispatch_order(id)?));
        }
        let inputs = self.reach_inputs(&spec, world);
        let reach = queue::reach(&spec, &inputs, &cfg, &self.policy.rules);
        let logged = self.log_inputs(&inputs)?;
        if queue::all_reachable(&reach) {
            return Ok(Routed::Dispatched(
                self.dispatch_inner(id, Some((logged, reach)))?,
            ));
        }
        // one instant for the deadline and the event, so replay recomputes it from e.t
        let now = self.now();
        let basis = self
            .get(id)?
            .basis()
            .cloned()
            .unwrap_or_else(|| no_basis(now));
        // rules 34: a member order waits at most its group's wait, and never goes to the inbox
        let (expires_t, inbox) = cfg.bound_for(&spec, self.member_wait(&spec), cfg.wait_end(spec.kind(), now), &basis, now);
        self.emit_at(
            now,
            EventBody::IntentQueued {
                intent: id,
                inputs: logged,
                reach: reach.clone(),
                expires_t,
            },
        )?;
        self.get_mut(id)?.set_queue(Some(QueueState {
            t: now,
            expires_t,
            inbox,
            reach: reach.clone(),
            recheck: None,
        }));
        let why = queue::unreachable_summary(&reach);
        self.transition(
            id,
            Transition::Queue,
            format!(
                "queued for at most {} s: {why}",
                expires_t.ms_since(now) / 1000
            ),
        )?;
        self.last_queued = Some(id);
        self.step = Some((id, self.seq));
        Ok(Routed::Queued { expires_t, reach })
    }

    /// Rules 5: try a queued intent again. Past its deadline it expires. While some vehicle
    /// still can't be reached it stays queued and nothing is logged. Otherwise its checks run
    /// on the world now (`check.result`, phase `dequeue`), the policy decides on the same
    /// inputs, and the verdict ([`queue::assess`] against what the approval rested on) is
    /// logged as `queue.recheck`: dispatch the approved order, back to the inbox (a human
    /// must decide again), or void.
    pub fn dequeue(
        &mut self,
        id: IntentId,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<Dequeued, EngineError> {
        let intent = self.get(id)?;
        if intent.state() != State::Queued {
            let err = next_state(intent.state(), Transition::Dispatch)
                .err()
                .unwrap_or(IllegalTransition {
                    from: intent.state(),
                    transition: Transition::Dispatch,
                });
            return Err(EngineError::Illegal { id, err });
        }
        let spec = intent.spec().clone();
        let deadline = intent.queue().map(|q| q.expires_t);
        if deadline.is_some_and(|d| self.now() >= d) {
            return self.expire_queued(id);
        }
        // never an older order over a newer one for the same vehicle: the newest stands
        if let Some((newer, uid)) = self.newer_on(&spec) {
            let note = self.running_note(id);
            self.transition(
                id,
                Transition::Void,
                format!(
                    "system: superseded: {newer} is a newer order for vehicle {uid} (the newest order stands){note}"
                ),
            )?;
            return Ok(Dequeued::Voided);
        }
        let cfg = self.policy.queue();
        let reach = queue::reach(&spec, &self.reach_inputs(&spec, world), &cfg, &self.policy.rules);
        if !queue::all_reachable(&reach) {
            return Ok(Dequeued::Waiting(reach));
        }
        let (inputs, _, results, passed) =
            self.run_checks(&spec, CheckPhase::Dequeue, world, terrain)?;
        // the re-check is judged before the deadline, or not at all
        let now = self.now();
        if deadline.is_some_and(|d| now >= d) {
            return self.expire_queued(id);
        }
        // rules 29: a handed-over order is the previous watch's: the policy decides nothing,
        // unless a check could not be verified and a person must judge it (who: the policy's)
        let handed_over = self.get(id)?.handed_over().is_some();
        let ask = passed && (!handed_over || queue::handover_unverified(&results));
        let decision = ask.then(|| policy::decide(&spec, &inputs, &self.policy));
        // without a basis (none was recorded), a human decides: nothing to compare with
        let intent = self.get(id)?;
        let carried = intent.basis_carried();
        let basis = intent
            .basis()
            .cloned()
            .unwrap_or_else(|| no_basis(inputs.t));
        let since = queue::Since {
            basis: &basis,
            carried,
            rebooted: &self.rebooted,
            handed_over,
        };
        let recheck = queue::assess(&spec, &since, &results, passed, decision.as_ref(), &inputs);
        let t = self.emit_at(now, EventBody::QueueRecheck {
            intent: id,
            reach,
            decision: decision.clone(),
            verdict: recheck.verdict,
            changes: recheck.changes.clone(),
        })?;
        if let Some(q) = self.get_mut(id)?.queue_mut() {
            q.recheck = Some(RecheckRecord {
                t,
                verdict: recheck.verdict,
                changes: recheck.changes.clone(),
            });
        }
        match (recheck.verdict, decision) {
            // the order goes out at once, right after the re-check that allows it
            (Verdict::Dispatch, _) => Ok(Dequeued::Dispatched(self.dispatch_inner(id, None)?)),
            (Verdict::Inbox, Some(decision)) => {
                self.back_to_inbox(
                    id,
                    Some(decision),
                    format!(
                        "back to the inbox from the queue: {}",
                        recheck.changes.join("; ")
                    ),
                )?;
                Ok(Dequeued::Inbox)
            }
            _ => {
                let note = self.running_note(id);
                self.transition(
                    id,
                    Transition::Void,
                    format!(
                        "re-check failed at dequeue: {}{note}",
                        recheck.changes.join("; ")
                    ),
                )?;
                Ok(Dequeued::Voided)
            }
        }
    }

    /// From the queue back to the inbox: a new decision is needed (`decision`, a re-check's;
    /// none: the one it has), so the old approval and its signatures are dropped, and it
    /// waits for a human under the policy's approval deadline (`approval.requested`).
    fn back_to_inbox(
        &mut self,
        id: IntentId,
        decision: Option<PolicyDecision>,
        reason: String,
    ) -> Result<(), EngineError> {
        let task = self.get(id)?.spec().task.clone();
        let reason = format!("{reason}{}", self.running_note(id));
        let intent = self.get_mut(id)?;
        if let Some(d) = decision {
            intent.set_policy(d);
        }
        intent.clear_approval();
        // the human's next approval decides, starting it again included
        intent.clear_maybe_started();
        let now = self.now();
        let expires_t = policy::approval_deadline(&self.policy, &task, now);
        self.emit_at(now, EventBody::ApprovalRequested { intent: id, expires_t })?;
        self.get_mut(id)?.set_expires(expires_t);
        self.transition(id, Transition::RequireApproval, reason)?;
        Ok(())
    }

    /// A queued intent's deadline passed: `queue.expired`, then expired; or, for a human's
    /// approval held as long as `[queue] human_expire_s` allows, back to the inbox.
    fn expire_queued(&mut self, id: IntentId) -> Result<Dequeued, EngineError> {
        let intent = self.get(id)?;
        let (expires_t, since, why, inbox) = intent.queue().map_or(
            (Time(0), Time(0), String::new(), false),
            |q| (q.expires_t, q.t, queue::unreachable_summary(&q.reach), q.inbox),
        );
        let tr = if inbox {
            Transition::RequireApproval
        } else {
            Transition::Expire
        };
        next_state(intent.state(), tr).map_err(|err| EngineError::Illegal { id, err })?;
        let waited = expires_t.ms_since(since) / 1000;
        let reason = if inbox {
            // rules 7: and why it was queued (the reach `intent.queued` logged; what holds it
            // now is the queue's live view, not logged while it waits)
            let held = if !why.is_empty() {
                format!(" (queued because its vehicles could not be reached: {why})")
            } else {
                String::new()
            };
            format!(
                "a human's approval waits in the queue at most {} s: back to the inbox for a new decision{held}",
                self.policy.queue().human_expire_s()
            )
        } else if why.is_empty() {
            format!(
                "not dispatched within {waited} s: its vehicles could not be reached{}",
                self.running_note(id)
            )
        } else {
            format!(
                "not dispatched within {waited} s: its vehicles could not be reached ({why}){}",
                self.running_note(id)
            )
        };
        self.emit(EventBody::QueueExpired {
            intent: id,
            expires_t,
            reason: reason.clone(),
            inbox,
        })?;
        if inbox {
            self.back_to_inbox(id, None, reason)?;
            return Ok(Dequeued::Inbox);
        }
        self.transition(id, Transition::Expire, reason)?;
        Ok(Dequeued::Expired)
    }

    /// Rules 5: a dispatched intent whose link was lost to every one of its vehicles before
    /// any of them confirmed it (each vehicle's last op state `requeued`, or `unconfirmed`
    /// when its start had gone out; none ever committed) goes back to the queue
    /// (`intent.requeued`): re-checked on the world before it is sent again (then asking each
    /// vehicle what it holds first, and only following it where it may have started), until
    /// its kind's wait from the approval at the latest. When a delivery that only confirmed
    /// an order that may have started found a vehicle not running it (`unconfirmed` there),
    /// it is not started again on its own: it goes back to a human at once (`intent.requeued`
    /// with its deadline now, then `queue.expired` to the inbox). Refused, nothing logged,
    /// otherwise.
    pub fn requeue(&mut self, id: IntentId) -> Result<Time, EngineError> {
        let intent = self.get(id)?;
        let refuse = |why: &str| EngineError::NotDispatchable {
            id,
            why: why.to_string(),
        };
        next_state(intent.state(), Transition::Requeue)
            .map_err(|err| EngineError::Illegal { id, err })?;
        let spec = intent.spec().clone();
        if !self.reach_needed(&spec) {
            return Err(refuse("only a vehicle task the queue holds is requeued (rules 5)"));
        }
        if !intent.committed().is_empty() {
            return Err(refuse("a vehicle had started it: it is followed, never sent again"));
        }
        let lost: Vec<String> = spec
            .uids
            .iter()
            .filter(|u| {
                matches!(
                    intent.ops().get(*u).map(String::as_str),
                    Some(queue::REQUEUED_OP | queue::UNCONFIRMED_OP)
                )
            })
            .map(|u| u.to_string())
            .collect();
        if lost.len() != spec.uids.len() {
            return Err(refuse(
                "requeued only when every vehicle's op ended with its link lost before it was confirmed",
            ));
        }
        // a delivery that only confirmed it, to a vehicle that does not run it
        let not_running: Vec<String> = queue::confirm_failed(intent.confirming(), intent.ops())
            .iter()
            .map(|u| {
                let detail = intent.op_details().get(u).cloned().unwrap_or_default();
                format!("vehicle {u}: {detail}")
            })
            .collect();
        let cfg = self.policy.queue();
        let now = self.now();
        let basis = intent.basis().cloned().unwrap_or_else(|| no_basis(now));
        let wait = self.member_wait(&spec);
        // rules 34 (bd murmur-4jlq): a member order whose delivery found its vehicle not running it
        // is re-checked in the queue under its group's approval, which covers its share, never
        // sent to a person on its own
        let member = spec.parent().is_some();
        let (expires_t, inbox) = if not_running.is_empty() || member {
            cfg.bound_for(&spec, wait, cfg.wait_end(spec.kind(), basis.t), &basis, now)
        } else {
            (now, true)
        };
        self.emit_at(
            now,
            EventBody::IntentRequeued {
                intent: id,
                expires_t,
                inbox,
            },
        )?;
        self.get_mut(id)?.set_queue(Some(QueueState {
            t: now,
            expires_t,
            inbox,
            reach: Vec::new(),
            recheck: None,
        }));
        if member && !not_running.is_empty() {
            // it does not run there: its next delivery, once its re-check passes, sends it whole
            self.get_mut(id)?.clear_maybe_started();
            self.transition(
                id,
                Transition::Requeue,
                format!(
                    "back to the queue under its group's approval: it may have started before its link was lost, and does not run now ({}); re-checked before it is sent again (for {} s at most)",
                    not_running.join("; "),
                    expires_t.ms_since(now) / 1000
                ),
            )?;
            return Ok(expires_t);
        }
        if !not_running.is_empty() {
            self.transition(
                id,
                Transition::Requeue,
                "back to the queue, on its way to a human: it may have started, and does not run now".into(),
            )?;
            let reason = format!(
                "it may have started before its link was lost, and does not run now: not started again on its own; a human decides ({})",
                not_running.join("; ")
            );
            self.emit(EventBody::QueueExpired {
                intent: id,
                expires_t,
                reason: reason.clone(),
                inbox: true,
            })?;
            // it is not running there: the reason says so, not "may be running"
            self.get_mut(id)?.clear_maybe_started();
            self.back_to_inbox(id, None, reason)?;
            return Ok(expires_t);
        }
        let started: Vec<String> = spec
            .uids
            .iter()
            .filter(|u| intent_ops_unconfirmed(self.get(id).ok(), u))
            .map(ToString::to_string)
            .collect();
        let how = if started.is_empty() {
            "before the order started".to_string()
        } else {
            format!(
                "after its start went out to {} (it may be running: its next delivery only follows it there)",
                started.join(", ")
            )
        };
        self.transition(
            id,
            Transition::Requeue,
            format!(
                "back to the queue: the link to vehicle(s) {} was lost {how}; re-checked before it is sent again (for {} s at most)",
                lost.join(", "),
                expires_t.ms_since(now) / 1000
            ),
        )?;
        Ok(expires_t)
    }

    /// Rules 5: carry an intent the previous run left open into this one (`intent.resumed`),
    /// keeping its id. It continues queued (re-checked on the world before it goes, delivered
    /// once) unless some vehicle had already started it, when it continues dispatched: the
    /// bridge follows it there and never sends it again. Ids after the previous run's are
    /// reserved with [`Engine::reserve_ids`].
    pub fn resume(
        &mut self,
        open: &OpenIntent,
        from: ResumedFrom,
    ) -> Result<IntentId, EngineError> {
        let id = open.spec.id;
        let refuse = |why: &str| EngineError::NotDispatchable {
            id,
            why: why.to_string(),
        };
        if self.intents.contains_key(&id) {
            return Err(refuse("an intent with that id already exists in this run"));
        }
        if !queue::queueable(open.spec.kind()) {
            return Err(refuse("only vehicle tasks are carried over"));
        }
        // rules 8: only between runs that authenticated their operators alike (an approval on
        // asserted ids never stands in a credentials run, nor the reverse)
        let mut from = from;
        if self.auth.is_some() {
            let (was, now) = (
                crate::authority::effective_mode(from.auth),
                crate::authority::effective_mode(self.auth),
            );
            if was != now {
                return Err(refuse(&format!(
                    "carried from a {} run into a {} run: intents are carried only between runs that authenticate operators alike",
                    was.as_str(),
                    now.as_str()
                )));
            }
        } else {
            from.auth = None;
        }
        let state = open.resumes_as();
        let cfg = self.policy.queue();
        // the deadline it carries: its own if it was queued, else its kind's wait from the
        // approval (bounded below for a human's approval, as at any queueing)
        let expires_t = (state == State::Queued).then(|| {
            open.expires_t
                .unwrap_or_else(|| cfg.wait_end(open.spec.kind(), open.basis.t))
        });
        let t = self.emit(EventBody::IntentResumed {
            intent: open.spec.clone(),
            from: from.clone(),
            state,
            basis: open.basis.clone(),
            policy: open.policy.clone(),
            approval: open.approval.clone(),
            expires_t,
            ops: open.ops.clone(),
            committed: open.committed.clone(),
            dispatched_before: open.dispatched_before,
            platforms: open.platforms.clone(),
            maybe_started: open.maybe_started.clone(),
        })?;
        let mut intent = Intent::new(open.spec.clone());
        intent.set_checks(open.basis.checks.clone());
        if let Some(p) = &open.policy {
            intent.set_policy(p.clone());
        }
        if let Some(a) = &open.approval {
            intent.set_approval(carried_approval(a));
        }
        intent.set_basis(open.basis.clone());
        intent.set_carried(
            open.committed.iter().cloned().collect(),
            open.dispatched_before,
            true,
            open.maybe_started.iter().cloned().collect(),
        );
        intent.set_resumed(crate::intent::Resumed {
            from: from.clone(),
            ops: open.ops.clone(),
        });
        if let Some(by_kind) = expires_t {
            let (expires_t, inbox) = cfg.bound(by_kind, &open.basis, t);
            intent.set_queue(Some(QueueState {
                t,
                expires_t,
                inbox,
                reach: Vec::new(),
                recheck: None,
            }));
        }
        let reason = match state {
            State::Dispatched => format!(
                "resumed after a restart from {} ({}): started on {}; followed, never sent again",
                from.log,
                state_name(from.was),
                open.committed()
                    .iter()
                    .map(Uid::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => format!(
                "resumed after a restart from {} ({}): queued until it can be re-checked and sent",
                from.log,
                state_name(from.was)
            ),
        };
        intent.resume_in(state, t, reason);
        if state == State::Dispatched {
            for u in &open.spec.uids {
                self.newest_on.insert(u.clone(), id);
            }
        }
        self.intents.insert(id, intent);
        self.next_intent = self.next_intent.max(id.0 + 1);
        Ok(id)
    }

    /// Rules 29 ([`rules::HANDOVER_SINCE`]): carry in an order the fleet file hands over at the
    /// start (`intent.handed_over`): one vehicle's starting task, the previous watch's order, its
    /// vehicle already on that task's station. It enters queued, with the previous watch's
    /// approval as its basis (nothing for a person or the policy of this run to approve again),
    /// until `[queue] expire_s` from now whatever its kind; [`Engine::dequeue`] re-checks it on
    /// the world when its vehicle can be reached and dispatches it if its checks pass, voids it
    /// if one fails, and sends it to a person if one could not be verified. An orbit, a patrol or a hold for one vehicle only, and only at the start:
    /// refused once the run has any other intent (one created, or carried over a restart).
    pub fn hand_over(
        &mut self,
        proposal: Proposal,
        from: queue::HandedOverFrom,
    ) -> Result<IntentId, EngineError> {
        let id = IntentId(self.next_intent);
        let refuse = |why: String| EngineError::NotDispatchable { id, why };
        if !matches!(proposal.task.kind(), TaskKind::Orbit | TaskKind::Patrol | TaskKind::Hold) {
            return Err(refuse(format!(
                "a starting task is an orbit, a patrol or a hold, not {}",
                proposal.task.kind().as_str()
            )));
        }
        if proposal.uids.len() != 1 {
            return Err(refuse("a starting task is one vehicle's".into()));
        }
        proposal.validate()?;
        if let Some(other) = self.intents.values().find(|i| i.handed_over().is_none()) {
            return Err(refuse(format!(
                "handed over only at the start, before any other order ({} is one)",
                other.id()
            )));
        }
        if let Some(other) = self.intents.values().find(|i| i.spec().uids == proposal.uids) {
            return Err(refuse(format!(
                "vehicle {} has a starting task already ({})",
                proposal.uids[0],
                other.id()
            )));
        }
        let t = self.now().max(self.last_t);
        let spec = IntentSpec {
            id,
            created_t: t,
            source: Source::Handover {
                id: from.fleet.clone(),
            },
            task: self.normalized(proposal.task, t),
            uids: proposal.uids,
            constraints: proposal.constraints,
            rationale: proposal.rationale,
            grouping: Default::default(),
        };
        let expires_t = t.plus_ms(i64::from(self.policy.queue().expire_s) * 1000);
        self.emit_at(t, EventBody::IntentHandedOver {
            intent: spec.clone(),
            from: from.clone(),
            expires_t,
        })?;
        self.next_intent += 1;
        let mut intent = Intent::new(spec);
        intent.set_basis(handover_basis(t));
        intent.set_handed_over(from.clone());
        intent.set_queue(Some(QueueState {
            t,
            expires_t,
            inbox: false,
            reach: Vec::new(),
            recheck: None,
        }));
        intent.resume_in(
            State::Queued,
            t,
            format!(
                "handed over at the start by the fleet file {} (sha256 {}): the previous watch's order, re-checked on the world before it is sent",
                from.fleet,
                from.sha256.get(..12).unwrap_or(&from.sha256)
            ),
        );
        self.intents.insert(id, intent);
        Ok(id)
    }

    /// New intents get ids from `next` on (after a restart: past every id of the previous
    /// run, so an id names one intent across the carried-over logs).
    pub fn reserve_ids(&mut self, next: u64) {
        self.next_intent = self.next_intent.max(next);
    }

    /// Rules 5: a vehicle rebooted (its time since boot went back, or its boot instant
    /// jumped later than `booted_before_t` by more than [`queue::BOOT_TOLERANCE_MS`]): logged
    /// as `vehicle.rebooted` with both readings and the new boot instant. Refused, nothing
    /// logged, when it shows neither.
    pub fn record_reboot(
        &mut self,
        uid: &Uid,
        boot_ms_before: u64,
        boot_ms: u64,
        booted_t: Time,
        booted_before_t: Option<Time>,
    ) -> Result<(), EngineError> {
        if let Some(why) = queue::reboot_mismatch(boot_ms_before, boot_ms, booted_t, booted_before_t)
        {
            return Err(EngineError::NotDispatchable {
                id: IntentId(0),
                why: format!("vehicle {uid}: {why}"),
            });
        }
        let t = self.emit(EventBody::VehicleRebooted {
            uid: uid.clone(),
            boot_ms_before,
            boot_ms,
            booted_t,
            booted_before_t,
        })?;
        self.rebooted.insert(uid.clone(), t);
        Ok(())
    }

    /// Report a dispatched intent's progress on one of its own vehicles (for a collect or
    /// cover, under the resource its order names: `norad:<id>`, `satcom:<class>`,
    /// `gnss:<class>`). Refused ([`EngineError::NotItsOrder`], nothing logged) for an intent
    /// that was never dispatched, a vehicle or resource it was not dispatched to, a vehicle or
    /// report without a stop that released that fallback: replay rejects each of these.
    pub fn record_op_state(
        &mut self,
        id: IntentId,
        uid: &Uid,
        state: &str,
        detail: &str,
    ) -> Result<(), EngineError> {
        self.op_state(id, uid, state, detail, OpReport::Bridge, None)
    }

    /// Rules 7 ([`rules::QUEUE_FOLLOWUPS_SINCE`]): a vehicle task's start, logged ahead of
    /// sending it (`op.state` starting, `ahead`): the bridge calls this before the mission
    /// mode, the hold or rtl mode, or new items to a vehicle already flying a mission go out,
    /// and sends nothing if it fails. A bridge that stops right after still finds in its log
    /// that the order may have started, so a carry-over never starts it again on its own.
    /// Refused, nothing logged, under rules before 7, for anything but a vehicle task, and
    /// where [`Engine::record_op_state`] refuses.
    pub fn record_op_state_ahead(&mut self, id: IntentId, uid: &Uid, detail: &str) -> Result<(), EngineError> {
        self.op_state(id, uid, queue::STARTING_OP, detail, OpReport::Ahead, None)
    }

    /// An op state, from `by`; logged at `at` when given (a report whose detail was
    /// computed for that instant), else now.
    fn op_state(
        &mut self,
        id: IntentId,
        uid: &Uid,
        state: &str,
        detail: &str,
        by: OpReport,
        at: Option<Time>,
    ) -> Result<(), EngineError> {
        let ahead = by == OpReport::Ahead;
        let intent = self.get(id)?;
        // rules 11: a space task's cancel and re-check are reported by the engine itself,
        // bound to the operator's cancel or the check before them; the bridge is refused them
        let dispatched = intent.history().iter().any(|h| h.state == State::Dispatched);
        let replaced = self.newest_on.get(uid).is_some_and(|n| *n != id);
        let start = StartAhead {
            ahead,
            required: true,
            started: intent.started_ahead().contains(uid),
        };
        if let Some(why) = op_state_mismatch(intent.spec(), intent.state(), dispatched, replaced, uid, state, start) {
            return Err(EngineError::NotDispatchable { id, why: why.into() });
        }
        // rules 7: what replay rejects in every log, the engine refuses too: an op whose start
        // went out on this delivery may be running, so it ends `unconfirmed`, never `requeued`
        if start.required && state == queue::REQUEUED_OP && intent.start_went_out().contains(uid) {
            return Err(EngineError::NotDispatchable {
                id,
                why: "op requeued after its start went out (it may be running: it ends unconfirmed)".into(),
            });
        }
        let at = at.unwrap_or_else(|| self.now());
        self.emit_at(
            at,
            EventBody::OpState {
                intent: id,
                uid: uid.clone(),
                state: state.into(),
                detail: detail.into(),
                ahead,
            },
        )?;
        self.get_mut(id)?
            .record_op(uid, state, detail, queue::committed_op(state), ahead);
        Ok(())
    }

    pub fn finish(&mut self, id: IntentId, outcome: TaskOutcome) -> Result<&Intent, EngineError> {
        let (tr, reason) = match outcome {
            TaskOutcome::Done => (Transition::Complete, "done".to_string()),
            TaskOutcome::Failed { detail } => (Transition::Fail, detail),
        };
        self.transition(id, tr, reason)?;
        self.get(id)
    }


    /// Log a dispatch plan with the input it was made from (replay re-runs the assigner).
    pub fn record_dispatch_plan(
        &mut self,
        tick: u64,
        input: crate::dispatch::AssignInput,
        plan: crate::dispatch::Plan,
    ) -> Result<(), EngineError> {
        self.emit(EventBody::DispatchPlan { tick, input: Box::new(input), plan })?;
        Ok(())
    }

    /// Log a fact about a delivery's parts that no decision carries (a parcel's state, a look at a
    /// drop zone, a hub's packs, the navigation monitor's sight of a bad fix).
    pub fn record_delivery_event(
        &mut self,
        kind: &str,
        uid: Option<&Uid>,
        parcel: Option<&str>,
        detail: &str,
        data: serde_json::Value,
    ) -> Result<(), EngineError> {
        self.emit(EventBody::DeliveryEvent {
            kind: kind.into(),
            uid: uid.cloned(),
            parcel: parcel.map(str::to_string),
            detail: detail.into(),
            data,
        })?;
        Ok(())
    }

    /// Rules 7: a step of the watch officer's triage, as the bridge took it
    /// (docs/design/watch-officer.md): a `triage.*` event only, anything else refused. The bridge
    /// keeps the triage itself; the engine's decisions do not read it.
    pub fn record_triage(&mut self, body: EventBody) -> Result<(), EngineError> {
        if !matches!(
            body,
            EventBody::TriageWatch { .. }
                | EventBody::TriageRouted { .. }
                | EventBody::TriageVerdict { .. }
                | EventBody::TriageReleased { .. }
                | EventBody::TriageClosed { .. }
        ) {
            return Err(EngineError::NotTriage);
        }
        self.emit(body)?;
        Ok(())
    }

    pub fn record_vehicle_event(
        &mut self,
        uid: &Uid,
        kind: &str,
        text: &str,
    ) -> Result<(), EngineError> {
        self.emit(EventBody::VehicleEvent {
            uid: uid.clone(),
            kind: kind.into(),
            text: text.into(),
        })?;
        Ok(())
    }

    // ---- internals ------------------------------------------------------------------

    fn get(&self, id: IntentId) -> Result<&Intent, EngineError> {
        self.intents.get(&id).ok_or(EngineError::UnknownIntent(id))
    }

    fn get_mut(&mut self, id: IntentId) -> Result<&mut Intent, EngineError> {
        self.intents
            .get_mut(&id)
            .ok_or(EngineError::UnknownIntent(id))
    }

    /// Clock time, never earlier than the last logged event.
    fn now(&self) -> Time {
        self.clock.now().max(self.last_t)
    }

    fn emit(&mut self, body: EventBody) -> Result<Time, EngineError> {
        let now = self.now();
        self.emit_at(now, body)
    }

    fn emit_at(&mut self, t: Time, body: EventBody) -> Result<Time, EngineError> {
        self.emit_keeping(t, body).map(|(t, _)| t)
    }

    /// [`Self::emit_at`], handing the body back once it is written.
    fn emit_keeping(&mut self, t: Time, body: EventBody) -> Result<(Time, EventBody), EngineError> {
        let t = t.max(self.last_t);
        let event = Event {
            seq: self.seq + 1,
            t,
            prev: self.head.clone(),
            body,
        };
        let line = serde_json::to_string(&event)?;
        self.sink.append(&line)?;
        self.seq += 1;
        self.last_t = t;
        self.head = sha256_hex(line.as_bytes());
        // rules 30: our vehicles' standing follows the log (replay steps it the same way)
        self.nodes.step(event.seq, t, &event.body);
        Ok((t, event.body))
    }

    /// The decision inputs as logged: the area set and any space answer replaced by their
    /// hashes. Logs the answer as `space.query` and the set as `areas.loaded` first, each only
    /// if this log does not hold it yet.
    fn log_inputs(&mut self, inputs: &WorldSnapshot) -> Result<LoggedInputs, EngineError> {
        self.log_inputs_at(inputs, None)
    }

    /// [`Self::log_inputs`], its space answer and area set logged at `at` when given (else at the
    /// clock).
    fn log_inputs_at(&mut self, inputs: &WorldSnapshot, at: Option<Time>) -> Result<LoggedInputs, EngineError> {
        // rules 30: a loss whose vehicle these facts show heard again is revoked before the
        // decision that reads the report is logged
        self.revoke_heard(&inputs.vehicles, inputs.t, None)?;
        // rules 25: no build of older rules logged a vehicle's damage report (and none of their
        // decisions reads it); rules 30: nor its lost-link failsafe, nor our vehicles confirmed lost
        let drop_damage = false && inputs.vehicles.values().any(|v| v.damage.is_some());
        let drop_loss = false
            && (!inputs.nodes_lost.is_empty() || inputs.vehicles.values().any(|v| v.lost_link.is_some()));
        // rules 31: nor a camera's reaches by size class
        let drop_class = false;
        // rules 32: nor a vehicle's sensors' reach now
        let drop_reach = false && inputs.vehicles.values().any(|v| v.reach_now.is_some());
        let stripped;
        let inputs = if drop_damage || drop_loss || drop_class || drop_reach {
            let mut w = inputs.clone();
            for v in w.vehicles.values_mut() {
                if drop_damage {
                    v.damage = None;
                }
                if drop_loss {
                    v.lost_link = None;
                }
                if drop_reach {
                    v.reach_now = None;
                }
            }
            if drop_loss {
                w.nodes_lost.clear();
            }
            stripped = w;
            &stripped
        } else {
            inputs
        };
        let areas_sha256 = match &inputs.areas {
            None => None,
            Some(set) => Some(self.log_areas(set, at)?),
        };
        Ok(LoggedInputs::from_world(inputs, areas_sha256))
    }

    /// A vehicle's facts as an event logs them, with its sensors' reach now left out (no such
    /// event reads it).
    fn vehicle_as_logged(&self, v: Option<&crate::world::VehicleFacts>) -> Box<crate::world::VehicleFacts> {
        let mut v = v.cloned().unwrap_or_default();
        v.reach_now = None;
        Box::new(v)
    }

    /// The hash a decision references `set` by, the set logged as `areas.loaded` first (at `at`,
    /// else at the clock) when it is not the one in force.
    fn log_areas(&mut self, set: &crate::areas::AreaSet, at: Option<Time>) -> Result<String, EngineError> {
        if let Some(cur) = &self.areas
            && cur.areas == *set
        {
            return Ok(cur.sha256.clone());
        }
        let sha256 = set.content_hash()?;
        let t = at.unwrap_or_else(|| self.now());
        self.emit_at(
            t,
            EventBody::AreasLoaded {
                sha256: sha256.clone(),
                areas: set.clone(),
            },
        )?;
        self.areas = Some(LoggedAreas {
            sha256: sha256.clone(),
            areas: set.clone(),
        });
        Ok(sha256)
    }

    fn transition(
        &mut self,
        id: IntentId,
        transition: Transition,
        reason: String,
    ) -> Result<State, EngineError> {
        let from = self.get(id)?.state();
        let to = next_state(from, transition).map_err(|err| EngineError::Illegal { id, err })?;
        let t = self.emit(EventBody::IntentState {
            intent: id,
            transition,
            from,
            to,
            reason: reason.clone(),
        })?;
        let state = self
            .get_mut(id)?
            .apply(transition, t, reason)
            .map_err(|err| EngineError::Illegal { id, err })?;
        // a person's confirmation of a drop zone stands from its approval; one rejected or
        // expired takes the delivery that waited on it with it
        if to == State::Approved {
            self.note_confirmation(id, t);
        }
        if matches!(to, State::Rejected | State::Expired) {
            let why = if to == State::Expired { "expired: nobody decided" } else { "was rejected" };
            self.end_waiting_deliver(id, why)?;
        }
        // rules 34: a member order's end may end its group
        if to.is_terminal()
            && let Some(parent) = self.get(id)?.spec().parent()
        {
            self.settle_group(parent)?;
        }
        Ok(state)
    }

    /// Rules 34 ([`crate::rules::GROUP_ORDER_SINCE`], docs/SWARMS.md section 4.3): step every
    /// group order in force on `world` (the bridge's picture now), once a tick. Each one's members
    /// as the facts show them ([`crate::group::standing`]): a member on its layout drops when its
    /// member order ends other than done, it is presumed or confirmed lost, or it stays silent
    /// [`crate::group::DROP_AFTER_S`] past its stale limit (drift alone never drops one); one a
    /// re-form left out is back once heard [`crate::group::BACK_AFTER_S`] running. Below its
    /// minimum once settled the group is suspended (`group.suspended`, and it re-forms no more);
    /// on material, settled drops and returns, at most once a minute, it re-forms inside its
    /// envelope and tier (`group.reformed`): only the gaps' neighbours move, a member on a link
    /// passes every vehicle rule (a member back whose share fails is left out for good) and raises
    /// nothing its group's approval does not cover; else nothing moves (`group.reform_refused`).
    /// Each moved or returning member gets a member order under the group's approval, dispatched
    /// or queued on its own (returned for the bridge to run). Nothing before rules 34.
    pub fn step_groups(&mut self, world: &WorldSnapshot, terrain: &dyn TerrainOracle) -> Result<GroupStep, EngineError> {
        let mut out = GroupStep::default();
        let gids: Vec<IntentId> = self.forces.keys().copied().collect();
        for gid in gids {
            let Some(g) = self.intents.get(&gid) else { continue };
            if !matches!(g.state(), State::Dispatched | State::Done) {
                continue;
            }
            let spec = g.spec().clone();
            let Some(gs) = spec.group().cloned() else { continue };
            let Some(force) = self.forces.get(&gid).cloned() else { continue };
            if force.suspended || gs.reform != Some(true) {
                continue;
            }
            let views = self.member_views(gid, &force);
            let st = crate::group::standing(&force, &views, world, &self.nodes, &self.policy.rules);
            if st.on_share.is_empty() && st.dropped.is_empty() && st.heard.is_empty() {
                continue;
            }
            // the engine's own tracking of each member left out that is heard again, since when
            let heard = self.heard.entry(gid).or_default();
            heard.retain(|u, _| st.heard.contains(u));
            for u in &st.heard {
                heard.entry(u.clone()).or_insert(world.t);
            }
            let back: BTreeMap<Uid, Time> = heard
                .iter()
                .filter(|(_, t)| world.t.ms_since(**t) >= crate::group::BACK_AFTER_S * 1000)
                .map(|(u, t)| (u.clone(), *t))
                .collect();
            let cause = crate::group::Cause {
                dropped: st.dropped.clone(),
                back,
            };
            match crate::group::due(&force, &gs, &cause, st.on_share.len(), st.taken.len(), spec.uids.len(), world.t) {
                crate::group::Due::Nothing => {}
                crate::group::Due::Suspend { strength, min } => {
                    let inputs = self.group_inputs(&spec, world);
                    let logged = self.log_inputs(&inputs)?;
                    self.emit(EventBody::GroupSuspended {
                        intent: gid,
                        inputs: logged,
                        cause,
                        strength,
                        min,
                    })?;
                    if let Some(f) = self.forces.get_mut(&gid) {
                        f.suspended = true;
                        f.last_attempt = Some(world.t);
                    }
                    out.suspended.push(gid);
                }
                crate::group::Due::Reform => self.reform_group(gid, &spec, &gs, &force, &st, cause, world, terrain, &mut out)?,
            }
        }
        Ok(out)
    }

    /// Rules 34: the facts a group step reads: every vehicle of its selection (and what the engine
    /// fills for them: waiting orders, the vehicles confirmed lost).
    fn group_inputs(&self, spec: &IntentSpec, world: &WorldSnapshot) -> WorldSnapshot {
        let mut inputs = world.for_intent_with(spec, &NoTerrain, &self.policy.rules);
        self.fill_engine_facts(spec, &mut inputs);
        inputs.terrain = crate::world::TerrainInputs::NotNeeded;
        inputs.route_plans.clear();
        inputs
    }

    /// Rules 34: each member order in force of a group, as its step reads it.
    fn member_views(&self, gid: IntentId, force: &crate::group::Force) -> BTreeMap<Uid, crate::group::MemberView> {
        force
            .members
            .iter()
            .filter_map(|(u, mid)| {
                let m = self.intents.get(mid)?;
                let ended_t = if m.state().is_terminal() { m.history().last().map(|h| h.t) } else { None };
                let taken = self
                    .newest_on
                    .get(u)
                    .is_some_and(|n| n > mid && self.intents.get(n).and_then(|i| i.spec().parent()) != Some(gid));
                Some((
                    u.clone(),
                    crate::group::MemberView {
                        state: m.state(),
                        ended_t,
                        taken,
                    },
                ))
            })
            .collect()
    }

    /// Rules 34: a group order dispatched, as it stays in force (its layout, member orders and
    /// re-forms).
    pub fn force(&self, gid: IntentId) -> Option<&crate::group::Force> {
        self.forces.get(&gid)
    }

    /// Rules 34: every group order dispatched, as each stays in force.
    pub fn forces(&self) -> &BTreeMap<IntentId, crate::group::Force> {
        &self.forces
    }

    /// Rules 34: a group order in force's members as the facts at `world` show them, and what its
    /// step would act on now (its drops, and its members back by the engine's own tracking as of
    /// its last step); None for a group order not in force. Read-only: the console's view of a
    /// group's strength, and the bridge's prefetch of a re-form's shares.
    pub fn group_standing(&self, gid: IntentId, world: &WorldSnapshot) -> Option<(crate::group::Standing, crate::group::Cause)> {
        let g = self.intents.get(&gid)?;
        if !matches!(g.state(), State::Dispatched | State::Done) {
            return None;
        }
        let force = self.forces.get(&gid)?;
        let views = self.member_views(gid, force);
        let st = crate::group::standing(force, &views, world, &self.nodes, &self.policy.rules);
        let heard = self.heard.get(&gid);
        let back = st
            .heard
            .iter()
            .filter_map(|u| {
                let t = *heard?.get(u)?;
                (world.t.ms_since(t) >= crate::group::BACK_AFTER_S * 1000).then(|| (u.clone(), t))
            })
            .collect();
        let cause = crate::group::Cause {
            dropped: st.dropped.clone(),
            back,
        };
        Some((st, cause))
    }

    /// Rules 34: each group order a re-form is due for on `world` (its spec, its force, its
    /// members on their shares and its cause), for the bridge to lay out outside its lock and
    /// fetch the terrain along the shares it would move members to; the tick's step decides.
    pub fn reforms_due(&self, world: &WorldSnapshot) -> Vec<(IntentSpec, crate::group::Force, BTreeSet<Uid>, crate::group::Cause)> {
        self.forces
            .iter()
            .filter_map(|(gid, force)| {
                let spec = self.intents.get(gid)?.spec().clone();
                let gs = spec.group()?.clone();
                let (st, cause) = self.group_standing(*gid, world)?;
                let due = crate::group::due(force, &gs, &cause, st.on_share.len(), st.taken.len(), spec.uids.len(), world.t);
                (due == crate::group::Due::Reform).then(|| (spec, force.clone(), st.on_share, cause))
            })
            .collect()
    }

    /// Rules 34: who approved a group order, for its member orders' `under`.
    fn group_approved_by(intent: &Intent) -> String {
        match intent.approval() {
            Some(a) if a.at_submission => format!("authorized at submission by {}", a.by),
            Some(a) => format!("approved by {}", a.by),
            None => format!(
                "auto-approved by the policy's {} tier",
                intent.policy().map_or("", |d| d.rule.as_str())
            ),
        }
    }

    /// Rules 34: one re-form due ([`Engine::step_groups`]).
    #[allow(clippy::too_many_arguments)]
    fn reform_group(
        &mut self,
        gid: IntentId,
        spec: &IntentSpec,
        gs: &crate::group::GroupSpec,
        force: &crate::group::Force,
        st: &crate::group::Standing,
        cause: crate::group::Cause,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
        out: &mut GroupStep,
    ) -> Result<(), EngineError> {
        let mut inputs = self.group_inputs(spec, world);
        let decision = self.get(gid)?.policy().cloned();
        let plan = crate::group::plan_reform(spec, gs, force, &st.on_share, &cause, &mut inputs, Some((world, terrain)), &self.policy, decision.as_ref());
        let refuse = |e: &mut Engine, inputs: &WorldSnapshot, cause: crate::group::Cause, why: String| -> Result<(), EngineError> {
            let logged = e.log_inputs(inputs)?;
            let uids = cause.uids();
            let why_logged = why.clone();
            e.emit(EventBody::GroupReformRefused {
                intent: gid,
                inputs: logged,
                cause,
                why,
            })?;
            if let Some(f) = e.forces.get_mut(&gid) {
                f.last_attempt = Some(inputs.t);
                f.refused = Some(uids);
                f.refused_why = Some(why_logged);
            }
            Ok(())
        };
        let plan = match plan {
            Err(why) => {
                out.refused.push((gid, why.clone()));
                return refuse(self, &inputs, cause, why);
            }
            Ok(p) => p,
        };
        let seq = force.seq + 1;
        let moved: BTreeMap<Uid, IntentId> = plan
            .specs
            .iter()
            .enumerate()
            .filter_map(|(k, ms)| Some((ms.uids.first()?.clone(), IntentId(self.next_intent + k as u64))))
            .collect();
        let logged = self.log_inputs(&inputs)?;
        self.emit(EventBody::GroupReformed {
            intent: gid,
            reform: seq,
            inputs: logged,
            cause: cause.clone(),
            layout: Box::new(plan.reform.layout.clone()),
            moved: moved.clone(),
            left_out: plan.left_out.clone(),
            results: plan.results.clone(),
            largest_gap_m: plan.reform.largest_gap_m,
            even_spacing_m: plan.reform.even_spacing_m,
        })?;
        let group = self.get(gid)?;
        let who = Self::group_approved_by(group);
        let basis = group.basis().cloned().unwrap_or_else(|| no_basis(self.now()));
        let decision = group.policy().cloned();
        for ms in &plan.specs {
            let Some(uid) = ms.uids.first().cloned() else { continue };
            let Some(mid) = moved.get(&uid).copied() else { continue };
            let mut ms = ms.clone();
            ms.id = mid;
            ms.created_t = self.now();
            self.emit(EventBody::IntentCreated {
                intent: ms.clone(),
                session: None,
            })?;
            self.next_intent = self.next_intent.max(mid.0 + 1);
            let mut member = Intent::new(ms);
            if let Some(p) = inputs.route_plans.get(&uid) {
                member.set_route_plans(BTreeMap::from([(uid.clone(), p.clone())]));
            }
            self.intents.insert(mid, member);
            let why = if cause.back.contains_key(&uid) { "back" } else { "moved" };
            self.transition(mid, Transition::Under, format!("approved under group order {gid} ({who}); its re-form {seq}: {why}"))?;
            self.get_mut(mid)?.set_basis(crate::group::member_basis(&basis, decision.as_ref()));
        }
        // the group in force on its new layout and member orders
        if let Some(f) = self.forces.get_mut(&gid) {
            f.layout = plan.reform.layout.clone();
            f.seq = seq;
            f.last_attempt = Some(inputs.t);
            f.refused = None;
            f.refused_why = None;
            for (u, m) in &moved {
                f.members.insert(u.clone(), *m);
                f.left.remove(u);
            }
            for u in plan.left_out.keys() {
                f.out.insert(u.clone());
            }
            for u in cause.dropped.keys() {
                f.left.insert(u.clone(), inputs.t);
            }
        }
        if let Some(h) = self.heard.get_mut(&gid) {
            for u in moved.keys().chain(plan.left_out.keys()) {
                h.remove(u);
            }
        }
        let mut members = self.get(gid)?.members().clone();
        members.extend(moved.clone());
        self.get_mut(gid)?.set_members(members);
        let mut routed = Vec::new();
        for mid in moved.values() {
            routed.push((*mid, self.dispatch_or_queue(*mid, world)?));
        }
        out.reformed.push((gid, routed));
        Ok(())
    }

    /// Rules 34: a dispatched group order ends once every member order it sent has ended: done
    /// if at least its minimum reached their shares (a member a newer order took counts neither
    /// way, the minimum less one for each), else failed, the reason counting each end
    /// ([`crate::group::outcome`]).
    fn settle_group(&mut self, gid: IntentId) -> Result<(), EngineError> {
        let Some(g) = self.intents.get(&gid) else { return Ok(()) };
        if g.state() != State::Dispatched || g.members().is_empty() {
            return Ok(());
        }
        let min = g.spec().group().map_or(1, crate::group::GroupSpec::min_members);
        let mut ends = Vec::new();
        for (uid, mid) in g.members() {
            let Some(m) = self.intents.get(mid) else { return Ok(()) };
            if !m.state().is_terminal() {
                return Ok(());
            }
            let taken = self.newest_on.get(uid).is_some_and(|n| n > mid);
            ends.push((m.state(), taken));
        }
        let (done, reason) = crate::group::outcome(min, &ends);
        self.transition(gid, if done { Transition::Complete } else { Transition::Fail }, reason)?;
        Ok(())
    }

    /// [`Self::transition`], returning the time the state change was logged at.
    fn transition_at(
        &mut self,
        id: IntentId,
        transition: Transition,
        reason: String,
    ) -> Result<Time, EngineError> {
        self.transition(id, transition, reason)?;
        Ok(self.last_t)
    }

    fn expire(&mut self, id: IntentId) -> Result<(), EngineError> {
        let expires_t = self.get(id)?.expires_t().unwrap_or(Time(0));
        next_state(self.get(id)?.state(), Transition::Expire)
            .map_err(|err| EngineError::Illegal { id, err })?;
        self.emit(EventBody::ApprovalExpired {
            intent: id,
            expires_t,
        })?;
        self.transition(
            id,
            Transition::Expire,
            "no decision before the deadline".into(),
        )?;
        Ok(())
    }

    fn run_checks(
        &mut self,
        spec: &IntentSpec,
        phase: CheckPhase,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<(WorldSnapshot, LoggedInputs, Vec<CheckResult>, bool), EngineError> {
        let world = self.with_fields(spec, world);
        // rules 34: a group order's terrain and routes are its members' shares', sampled once the
        // layout is known (below), never the group's own task's
        let group = crate::group::is_group(spec, &self.policy.rules);
        let mut inputs = if group {
            world.for_intent_with(spec, &crate::terrain::NoTerrain, &self.policy.rules)
        } else {
            world.for_intent_with(spec, terrain, &self.policy.rules)
        };
        self.fill_facts(&mut inputs);
        self.fill_engine_facts(spec, &mut inputs);
        if group {
            inputs.terrain = crate::world::TerrainInputs::NotNeeded;
            inputs.route_plans.clear();
            for ms in crate::group::member_specs(spec, &inputs, &self.policy.rules) {
                let mw = world.for_intent_with(&ms, terrain, &self.policy.rules);
                inputs.terrain.merge(mw.terrain);
                inputs.route_plans.extend(mw.route_plans);
            }
        }
        let eval = group.then(|| crate::group::evaluate(spec, &inputs, &self.policy.rules));
        let results = match &eval {
            Some(e) => e.results.clone(),
            None => rules::evaluate(spec, &inputs, &self.policy.rules),
        };
        let passed = rules::passed(&results, spec.kind());
        let logged = self.log_inputs_at(&inputs, None)?;
        let t = self.now();
        self.emit_at(
            t,
            EventBody::CheckResult {
                intent: spec.id,
                phase,
                inputs: logged.clone(),
                results: results.clone(),
                passed,
                layout: eval.as_ref().map(|e| Box::new(e.layout.clone())),
            },
        )?;
        let intent = self.get_mut(spec.id)?;
        intent.set_checks(results.clone());
        // rules 34: what a group order is dispatched with: its passing check's layout and classes
        if group {
            intent.set_group_eval(eval.filter(|_| passed));
        }
        // rules 22: what it is dispatched with: the routed legs its passing check planned
        {
            let plans = if passed { inputs.route_plans.clone() } else { BTreeMap::new() };
            self.get_mut(spec.id)?.set_route_plans(plans);
        }
        Ok((inputs, logged, results, passed))
    }



    /// A task as logged: given as is.
    fn normalized(&self, task: Task, _created_t: Time) -> Task {
        task
    }


    /// Create, check and route an intent. `submitter` is the submitting human's authority on
    /// the human paths and `None` on the agent paths: an at-submission approval needs it (and
    /// needs it to be the intent's own source), so an agent's proposal can never get one.
    fn submit_inner(
        &mut self,
        source: Source,
        submitter: Option<&HumanAuthority>,
        proposal: Proposal,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<IntentId, EngineError> {
        proposal.validate()?;
        // rules 34: no build of older rules took a group order's spec
        if proposal.group.is_some() && false {
            return Err(EngineError::Proposal(ProposalError::BadParams(
                "these decision rules take no group order (rules 34)".into(),
            )));
        }
        // rules 27: no build of older rules took a lay order
        // rules 34: no build of older rules took a transit
        if proposal.task.kind() == TaskKind::Transit && false {
            return Err(EngineError::Proposal(ProposalError::BadParams(
                "these decision rules take no transit (rules 34)".into(),
            )));
        }
        let grouping = self.grouping(&proposal)?;
        let id = IntentId(self.next_intent);
        let created_t = self.now();
        let spec = IntentSpec {
            id,
            created_t,
            source,
            task: self.normalized(proposal.task, created_t),
            uids: proposal.uids,
            constraints: proposal.constraints,
            rationale: proposal.rationale,
            grouping,
        };
        self.emit(EventBody::IntentCreated {
            intent: spec.clone(),
            // rules 8: a human's intent is made in a live session of theirs
            session: if self.auth.is_some() { submitter.map(HumanAuthority::session) } else { None },
        })?;
        self.next_intent += 1;
        self.intents.insert(id, Intent::new(spec.clone()));
        let (inputs, logged, results, ok) =
            self.run_checks(&spec, CheckPhase::Submit, world, terrain)?;
        self.transition(id, Transition::Check, "rules evaluated".into())?;
        if !ok {
            self.transition(id, Transition::ChecksFailed, summarize_failures(&results))?;
            return Ok(id);
        }

        // Policy decides on the same inputs the rules saw (one terrain query, one picture).
        let decision = policy::decide(&spec, &inputs, &self.policy);
        self.emit(EventBody::PolicyDecision {
            intent: id,
            inputs: logged,
            decision: decision.clone(),
        })?;
        let tier = decision.tier;
        let reason = format!("policy {}: {}", decision.rule, decision.reason);
        // The submission is the human decision only for the submitter's own intent, with
        // the submitter's authority in hand (never on an agent path, where it is None).
        let at_submission = (decision.submitter_authorizes && tier == Tier::Human)
            .then_some(submitter)
            .flatten()
            .filter(|by| is_self_approval(&spec.source, by.operator()));
        let rule = decision.rule.clone();
        // what an approval here rests on (auto, or the submitter's own): these checks and
        // this decision, on these inputs
        let basis = Basis {
            t: inputs.t,
            by_human: tier == Tier::Human && at_submission.is_some(),
            checks: results.clone(),
            policy: Some(decision.clone()),
            in_play: inputs.in_play.clone(),
        };
        self.get_mut(id)?.set_policy(decision);
        match (tier, at_submission) {
            (Tier::Auto, _) => {
                let t = self.transition_at(id, Transition::AutoApprove, reason)?;
                self.get_mut(id)?.set_basis(Basis { t, ..basis });
            }
            (Tier::Human, Some(by)) => {
                let t = self.emit(EventBody::ApprovalAtSubmission {
                    intent: id,
                    by: by.operator().clone(),
                    session: by.session(),
                    self_approved: true,
                    rule,
                })?;
                self.get_mut(id)?.set_approval(Approval {
                    by: by.operator().clone(),
                    session: by.session(),
                    t,
                    self_approved: true,
                    at_submission: true,
                    note: None,
                    step_up: false,
                });
                let t = self.transition_at(
                    id,
                    Transition::AuthorizeAtSubmission,
                    format!("{reason}; authorized at submission by {}", by.operator()),
                )?;
                self.get_mut(id)?.set_basis(Basis { t, ..basis });
            }
            (Tier::Human, None) => {
                // one instant for the deadline and the event, so replay recomputes it from e.t
                let now = self.now();
                let expires_t = policy::approval_deadline(&self.policy, &spec.task, now);
                self.emit_at(now, EventBody::ApprovalRequested { intent: id, expires_t })?;
                self.get_mut(id)?.set_expires(expires_t);
                self.transition(id, Transition::RequireApproval, reason)?;
            }
        }
        Ok(id)
    }
}

/// The approving operator is the human who proposed the intent.
pub(crate) fn is_self_approval(source: &Source, approver: &OperatorId) -> bool {
    matches!(source, Source::Human { id } if id == approver)
}


/// The order a dispatch hands the bridge for `spec`: the approved task, as the routes its last
/// passing check planned carry it. [`Engine::dispatch_order`] logs it as `intent.dispatched`;
/// replay recomputes it from the same logged facts and requires the logged order to be exactly it.
pub(crate) fn order_for(
    spec: &IntentSpec,
    plans: &BTreeMap<Uid, Vec<crate::plan::PlannedLeg>>,
    _rules: &crate::rules::RulesConfig,
) -> Result<Order, EngineError> {
    let id = spec.id;
    // rules 22: the routes its last passing check planned (none before rules 22)
    let (via, path) = crate::plan::dispatch_routes(spec, plans);
    Ok(Order::Vehicles(DispatchOrder {
        intent: id,
        task: match (path, &spec.task) {
            // rules 34: a transit's route routed as it is, open
            (Some(route), Task::Transit { .. }) => Task::Transit { route },
            (Some(route), _) => Task::Patrol { route },
            (None, _) => spec.task.clone(),
        },
        uids: spec.uids.clone(),
        via,
        share: spec.share().cloned().map(Box::new),
    }))
}

/// The `intent.dispatched` event for `order`: the approved task and vehicles, and the routes
/// the order carries.
pub(crate) fn dispatched_event(spec: &IntentSpec, order: &Order) -> EventBody {
    // rules 22: the routes it carries (each vehicle's transit waypoints; a patrol's routed route)
    let (via, path) = match order {
        Order::Vehicles(o) => (
            o.via.clone(),
            match (&o.task, &spec.task) {
                (Task::Patrol { route }, Task::Patrol { route: asked }) if route != asked => Some(route.clone()),
                (Task::Transit { route }, Task::Transit { route: asked }) if route != asked => Some(route.clone()),
                _ => None,
            },
        ),
    };
    EventBody::IntentDispatched {
        intent: spec.id,
        task: spec.task.clone(),
        uids: spec.uids.clone(),
        via,
        path,
        inputs: None,
        reach: None,
        share: spec.share().cloned(),
    }
}

/// Why an op state report is not one the intent may have (replay rejects it in every log and the
/// engine refuses it): never dispatched, a vehicle the order was not sent to, a start not logged
/// ahead of sending it, a report after the task ended, or an order a newer dispatch replaced.
pub(crate) fn op_state_mismatch(
    spec: &IntentSpec,
    state: State,
    dispatched: bool,
    replaced: bool,
    uid: &Uid,
    op: &str,
    start: StartAhead,
) -> Option<&'static str> {
    if !dispatched {
        return Some("op state for an intent that was never dispatched");
    }
    if op_state_owner(spec, uid).is_err() {
        return Some("op state for a vehicle or resource the intent was not dispatched to");
    }
    let vehicle_task = queue::vehicle_task(spec.kind());
    if start.ahead && (!vehicle_task || op != queue::STARTING_OP) {
        return Some("op state logged ahead that is not a vehicle task's start");
    }
    if start.required && vehicle_task {
        if op == queue::STARTING_OP && !start.ahead {
            return Some("a vehicle start not logged ahead of sending it");
        }
        if queue::committed_op(op) && !start.started {
            return Some("a vehicle took an order whose start was not logged ahead of it on this delivery");
        }
    }
    if state != State::Dispatched {
        // a vehicle task reports while it runs; its finish is final
        return Some("op state for a task that already ended");
    }
    if replaced && !TERMINAL_OPS.contains(&op) {
        return Some("op state for an order a newer dispatch replaced on that vehicle");
    }
    if op == "superseded" && !replaced {
        return Some("op state superseded with no newer order on that vehicle");
    }
    None
}

/// Whether `uid` is one a dispatched intent reports op states under: a vehicle task's or an
/// (`satcom:<class>`, `gnss:<class>`). `Err` lists the ones it does report under.
pub(crate) fn op_state_owner(spec: &IntentSpec, uid: &Uid) -> Result<(), String> {
    let own = spec.uids.clone();
    if own.contains(uid) {
        Ok(())
    } else {
        Err(own.iter().map(Uid::as_str).collect::<Vec<_>>().join(", "))
    }
}

/// Rules 30 ([`crate::rules::NODE_LOSS_SINCE`]): what [`Engine::step_nodes`] did on one tick: our
/// vehicles presumed lost, the presumptions cleared and the losses revoked (each heard again), by
/// uid.
/// What [`Engine::step_groups`] did this step: each group re-formed with its new member orders
/// and what became of each (for the bridge to run, as a group's dispatch), each refused (and
/// why), each suspended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupStep {
    pub reformed: Vec<(IntentId, Vec<(IntentId, Routed)>)>,
    pub refused: Vec<(IntentId, String)>,
    pub suspended: Vec<IntentId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeStep {
    pub presumed: Vec<Uid>,
    pub cleared: Vec<Uid>,
    pub revoked: Vec<Uid>,
}

/// Rules 30: what a confirmation of a loss ended ([`Engine::confirm_lost`]): the intents naming
/// dispatched orders whose op on it ended `lost`, and of those the ones it ended (no vehicle left
/// on them); the lost-link values owed back to it, closed (`restore.closed` lost). The bridge
/// stops its restore jobs for it and closes its alerts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LossOutcome {
    pub voided: Vec<IntentId>,
    pub ended: Vec<IntentId>,
    pub finished: Vec<IntentId>,
    pub closed: Option<BTreeMap<String, f64>>,
}

impl Engine {
    /// Rules 30 ([`crate::rules::NODE_LOSS_SINCE`], docs/NODE-LOSS.md section 2): one step of our
    /// vehicles' standing on `world` (the bridge's picture now), on its tick beside
    /// [`Engine::step_lays`]: a loss whose vehicle is heard again is revoked (`node.loss_revoked`,
    /// cause heard; the values its confirmation closed owed back again); a presumption whose
    /// vehicle is heard again is cleared (`node.presumption_cleared`); a silent vehicle with logged
    /// (`node.presumed`, a flag for a person's review that decides nothing). Silence alone presumes
    /// nothing. Nothing before rules 30.
    pub fn step_nodes(&mut self, world: &WorldSnapshot) -> Result<NodeStep, EngineError> {
        let mut out = NodeStep::default();
        let now = self.now();
        out.revoked = self.revoke_heard(&world.vehicles, world.t, Some(now))?;
        let cfg = self.policy.rules.clone();
        for (uid, v) in &world.vehicles {
            if self.nodes.lost_mark(uid).is_some() {
                continue;
            }
            if let Some(p) = self.nodes.presumption(uid) {
                if let Some(detail) = crate::nodes::heard_since(v, p.since, p.heard, world.t, &cfg) {
                    let since = p.since;
                    self.emit(EventBody::NodePresumptionCleared {
                        uid: uid.clone(),
                        since,
                        world_t: world.t,
                        vehicle: self.vehicle_as_logged(Some(v)),
                        detail,
                    })?;
                    out.cleared.push(uid.clone());
                }
                continue;
            }
            if !crate::nodes::silent(v, world.t, &cfg) {
                continue;
            }
            let evidence = self.nodes.evidence(uid, v);
            if evidence.is_empty() {
                continue;
            }
            self.emit(EventBody::NodePresumed {
                uid: uid.clone(),
                since: v.position.as_ref().map(|p| p.t),
                evidence,
                world_t: world.t,
                vehicle: self.vehicle_as_logged(Some(v)),
            })?;
            out.presumed.push(uid.clone());
        }
        Ok(out)
    }

    /// Rules 30: revoke every loss in force whose vehicle `vehicles` (facts read at `t`) shows
    /// heard again (`node.loss_revoked`, cause heard, with its facts), logged at `at` or else at
    /// the log's time (before the decision that reads the report, so no event's time moves): its
    /// protection is back, and the values its confirmation closed are owed back again. The uids
    /// revoked. Nothing before rules 30.
    fn revoke_heard(
        &mut self,
        vehicles: &BTreeMap<Uid, crate::world::VehicleFacts>,
        t: Time,
        at: Option<Time>,
    ) -> Result<Vec<Uid>, EngineError> {
        let heard: Vec<(crate::nodes::LostMark, String)> = self
            .nodes
            .marks()
            .into_iter()
            .filter_map(|m| {
                let v = vehicles.get(&m.uid)?;
                let why = crate::nodes::heard_since(v, m.since, m.heard, t, &self.policy.rules)?;
                Some((m, why))
            })
            .collect();
        let mut out = Vec::new();
        for (m, detail) in heard {
            let body = EventBody::NodeLossRevoked {
                uid: m.uid.clone(),
                since: m.since,
                from: m.from.clone(),
                cause: crate::nodes::Revocation::Heard {
                    world_t: t,
                    vehicle: self.vehicle_as_logged(vehicles.get(&m.uid)),
                    detail,
                },
            };
            let at = at.unwrap_or(self.last_t);
            self.emit_at(at, body.clone())?;
            self.restores.step_body(&body);
            out.push(m.uid);
        }
        Ok(out)
    }

    /// Rules 30: why `uid` can't be confirmed lost now, on `world` (its facts and time) with
    /// `note`, if it can't: these rules have no loss; it is lost already; it is not silent since
    /// exactly its report at `since` (the one the review showed: [`crate::nodes::lost_since_report`],
    /// platform's own lost-link action); nothing presumes it lost and there is no note saying why
    /// (decision D2). None: [`Engine::confirm_lost`] would take it (what the bridge asks before it
    /// asks for a credential).
    pub fn confirm_lost_refusal(&self, uid: &Uid, since: Option<Time>, note: &str, world: &WorldSnapshot) -> Option<String> {
        if let Some(m) = self.nodes.lost_mark(uid) {
            return Some(format!("it is {} already", m.node().describe()));
        }
        if let Err(why) = crate::nodes::lost_since_report(world.vehicles.get(uid), since, world.t, &self.policy.rules) {
            return Some(why);
        }
        if note.trim().is_empty() && self.nodes.presumption(uid).is_none_or(|p| p.since != since) {
            return Some("nothing presumes it lost: say why it is (a note: a crew's report, a picture from outside)".into());
        }
        None
    }

    /// Rules 30: what a passkey step-up for confirming `uid` lost on its silence since `since` is
    /// for (its challenge binds both).
    pub fn confirm_lost_purpose(&self, uid: &Uid, since: Option<Time>) -> crate::passkey::StepUpFor {
        crate::passkey::StepUpFor::NodeLost { uid: uid.clone(), since }
    }

    /// Rules 30 (docs/NODE-LOSS.md sections 2 and 4, decisions D1 and D2): an operator with
    /// consequential authority confirms one of ours lost, on the silence they saw (since its report
    /// at `since`), under their own session: with the evidence of the presumption in force, or
    /// none with a note saying why. Refused, nothing logged, as [`Engine::confirm_lost_refusal`]
    /// says, and without the step-up a credentials run asks for (password or passkey per the
    /// policy's `step_up_factor`; a passkey's made for this act, [`Engine::confirm_lost_purpose`]):
    /// it removes a friendly-fire protection. Logged as `node.lost` with the vehicle's facts and
    /// the bridge's identity of it (a later run's carry holds only that vehicle); then its
    /// consequences, in this order: every intent naming it that waits (pending, approved, queued)
    /// is voided, every dispatched order it runs has its op ended `lost` (the order ends when no
    /// vehicle is left on it; an identify's look ends with it), and the lost-link values owed back
    /// to it close (`restore.closed` lost). From then on it leaves the friendly keep-outs, the line
    /// loss. An agent can't ([`AgentPort`] has no such method).
    pub fn confirm_lost(
        &mut self,
        by: &ConsequentialAuthority,
        uid: Uid,
        since: Option<Time>,
        note: &str,
        world: &WorldSnapshot,
        step: Option<StepUp>,
    ) -> Result<LossOutcome, EngineError> {
        if let Some(why) = self.confirm_lost_refusal(&uid, since, note, world) {
            return Err(EngineError::NodeLoss(format!("vehicle {uid}: {why}")));
        }
        let purpose = self.confirm_lost_purpose(&uid, since);
        let step_up = match step {
            None if self.auth == Some(AuthMode::Credentials) => {
                return Err(EngineError::NotAuthenticated {
                    operator: by.operator().clone(),
                    why: "confirming one of ours lost removes a friendly-fire protection: re-enter your credential (a step-up)".into(),
                });
            }
            None => None,
            Some(s) => Some(
                self.valid_step(by.operator(), by.session(), &s, Some(&purpose))
                    .map_err(|why| EngineError::NotAuthenticated {
                        operator: by.operator().clone(),
                        why,
                    })?,
            ),
        };
        let evidence = self
            .nodes
            .presumption(&uid)
            .filter(|p| p.since == since)
            .map(|p| p.evidence.clone())
            .unwrap_or_default();
        self.emit(EventBody::NodeLost {
            uid: uid.clone(),
            since,
            by: by.operator().clone(),
            session: by.session(),
            note: note.trim().to_string(),
            evidence,
            world_t: world.t,
            vehicle: self.vehicle_as_logged(world.vehicles.get(&uid)),
            step_up,
        })?;
        self.end_for_loss(&uid, by.operator(), by.session())
    }

    /// Rules 30: what a confirmed loss of `uid` ends, right after its `node.lost`
    /// ([`Engine::confirm_lost`], [`LossOutcome`]).
    fn end_for_loss(&mut self, uid: &Uid, operator: &OperatorId, _session: SessionId) -> Result<LossOutcome, EngineError> {
        let mut out = LossOutcome::default();
        let reason = format!("vehicle {uid} lost: {} by {operator}", crate::nodes::LOST_REASON);
        let names = |spec: &IntentSpec| spec.uids.contains(uid);
        let waiting: Vec<(IntentId, bool)> = self
            .intents
            .values()
            .filter(|i| {
                names(i.spec())
                    && matches!(i.state(), State::PendingApproval | State::AutoApproved | State::Approved | State::Queued)
            })
            .map(|i| (i.id(), false && i.state() == State::Approved))
            .collect();
        for (id, approved_consequential) in waiting {
            if approved_consequential {
            } else {
                self.void(id, &reason)?;
            }
            out.voided.push(id);
        }
        let running: Vec<IntentId> = self
            .intents
            .values()
            .filter(|i| {
                i.state() == State::Dispatched
                    && true
                    && i.spec().uids.contains(uid)
                    && !i.ops().get(uid).is_some_and(|o| queue::ended_op(o))
            })
            .map(Intent::id)
            .collect();
        for id in running {
            self.op_state(id, uid, crate::nodes::LOST_OP, &reason, OpReport::Engine, None)?;
            out.ended.push(id);
            let i = self.get(id)?;
            let left = i
                .spec()
                .uids
                .iter()
                .any(|u| self.nodes.lost_mark(u).is_none() && !i.ops().get(u).is_some_and(|o| queue::ended_op(o)));
            if !left {
                self.transition(id, Transition::Fail, format!("system: {reason}"))?;
                out.finished.push(id);
            }
        }
        Ok(out)
    }

    /// Rules 30: any operator revokes the loss of `uid` in force (this run's or carried over),
    /// under their own session, saying why (it only restricts: the vehicle counts again in every
    /// keep-out at once and takes orders, and the values its confirmation closed are owed back
    /// again). Logged as `node.loss_revoked`, cause operator. Refused, nothing logged, before rules
    /// 30, without a note, and for a vehicle not lost. An agent can't ([`AgentPort`] has no such
    /// method).
    pub fn revoke_lost(&mut self, by: &HumanAuthority, uid: &Uid, note: &str) -> Result<(), EngineError> {
        let note = note.trim();
        if note.is_empty() {
            return Err(EngineError::NodeLoss("say why the loss is revoked (a note)".into()));
        }
        let Some(m) = self.nodes.lost_mark(uid).cloned() else {
            return Err(EngineError::NodeLoss(format!("vehicle {uid} is not confirmed lost")));
        };
        let body = EventBody::NodeLossRevoked {
            uid: uid.clone(),
            since: m.since,
            from: m.from,
            cause: crate::nodes::Revocation::Operator {
                operator: by.operator().clone(),
                session: by.session(),
                note: note.to_string(),
            },
        };
        self.emit(body.clone())?;
        self.restores.step_body(&body);
        Ok(())
    }

    /// Rules 30: the loss in force on `uid` (this run's, or carried over), if any: for the
    /// bridge's review list and alerts.
    pub fn node_lost(&self, uid: &Uid) -> Option<&crate::nodes::LostMark> {
        self.nodes.lost_mark(uid)
    }

    /// Rules 30: every loss in force (this run's, `from` none, and those carried over).
    pub fn nodes_lost(&self) -> Vec<crate::nodes::LostMark> {
        self.nodes.marks()
    }

    /// Rules 30: every presumption of loss in force, by uid (what the review lists for a person).
    pub fn nodes_presumed(&self) -> &BTreeMap<Uid, crate::nodes::Presumption> {
        self.nodes.presumptions()
    }
}




/// The op states the task layer ends an op with: after a newer order took the vehicle, an
/// older one's op can only be reported in one of these (it was replaced, or had ended).
const TERMINAL_OPS: [&str; 4] = ["done", "failed", "superseded", crate::nodes::LOST_OP];

/// Rules 7: what an op state report says about a vehicle start logged ahead of sending it,
/// and what its intent's delivery needs ([`op_state_mismatch`]).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StartAhead {
    /// The report is a start logged ahead of sending it.
    pub ahead: bool,
    /// The log's rules write every vehicle start ahead (rules 7 on).
    pub required: bool,
    /// This vehicle's start (or follow) on the current delivery was logged ahead.
    pub started: bool,
}


/// Whether `uid`'s last op on the intent ended `unconfirmed` (its start went out).
fn intent_ops_unconfirmed(intent: Option<&Intent>, uid: &Uid) -> bool {
    intent.is_some_and(|i| i.ops().get(uid).map(String::as_str) == Some(queue::UNCONFIRMED_OP))
}

/// A basis that forces a human: no checks to compare with (every result is a change), by a
/// human. Used for a queued intent with no recorded basis (never written by this engine).
pub(crate) fn no_basis(t: Time) -> Basis {
    Basis {
        t,
        by_human: true,
        checks: Vec::new(),
        policy: None,
        in_play: BTreeMap::new(),
    }
}

/// Rules 29: the basis of a handed-over intent: the previous watch's approval, which this run
/// does not see (no checks, no policy decision of its own): its dequeue judges the checks alone
/// ([`queue::Since::handed_over`]).
pub(crate) fn handover_basis(t: Time) -> Basis {
    Basis {
        t,
        by_human: false,
        checks: Vec::new(),
        policy: None,
        in_play: BTreeMap::new(),
    }
}

/// The approval a resumed intent carries, as the engine records approvals.
fn carried_approval(a: &CarriedApproval) -> Approval {
    Approval {
        by: a.by.clone(),
        session: a.session,
        t: a.t,
        self_approved: a.self_approved,
        at_submission: a.at_submission,
        note: None,
        step_up: false,
    }
}


fn summarize_failures(results: &[CheckResult]) -> String {
    results
        .iter()
        .filter(|r| r.outcome == Outcome::Fail)
        .map(|r| match &r.uid {
            Some(u) => format!("{} {u}: {}", r.rule.as_str(), r.detail),
            None => format!("{}: {}", r.rule.as_str(), r.detail),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

impl Engine {
    /// Rules 34: an order of an ordinary kind to more than one vehicle is a group order, its spec
    /// as logged with the defaults filled; none for one vehicle.
    fn grouping(&self, proposal: &Proposal) -> Result<crate::group::Grouping, EngineError> {
        let n = proposal.uids.len();
        let kind = proposal.task.kind();
        if !(true && n > 1 && true) {
            return Ok(crate::group::Grouping::default());
        }
        let g = proposal.group.clone().unwrap_or_default();
        if let Some(why) = g.invalid(kind, n) {
            return Err(EngineError::Proposal(ProposalError::BadParams(why)));
        }
        Ok(crate::group::Grouping {
            group: Some(g.normalized(kind, n, self.policy.group().min_share)),
            ..Default::default()
        })
    }

    /// A dry run for an operator: the checks and the policy decision a vehicle task from
    /// `operator` would get right now. Nothing is created and nothing is logged: it decides
    /// `approved` where the submission itself would be the approval (a human tier the
    /// submitter authorizes), so a caller that is not a person (a situation's runner placing
    /// opening orders under a scripted id) sees that before it submits and can send such an
    /// order to a person instead. Vehicle tasks only.
    pub fn dry_run_for_operator(
        &self,
        operator: &OperatorId,
        proposal: Proposal,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<Preview, EngineError> {
        proposal.validate()?;
        if proposal.task.kind() == TaskKind::Transit && false {
            return Err(EngineError::Proposal(ProposalError::BadParams(
                "these decision rules take no transit (rules 34)".into(),
            )));
        }
        let created_t = self.now();
        let grouping = self.grouping(&proposal)?;
        let spec = IntentSpec {
            id: IntentId(0),
            created_t,
            source: Source::Human { id: operator.clone() },
            task: self.normalized(proposal.task, created_t),
            uids: proposal.uids,
            constraints: proposal.constraints,
            rationale: proposal.rationale,
            grouping,
        };
        let (inputs, checks, eval) = self.dry_run_checks(&spec, world, terrain);
        let passed = rules::passed(&checks, spec.kind());
        let policy = passed.then(|| policy::decide(&spec, &inputs, &self.policy));
        let would_reach = match &policy {
            None => State::Rejected,
            Some(d) if d.tier == Tier::Auto => State::AutoApproved,
            Some(d) if d.submitter_authorizes => State::Approved,
            Some(_) => State::PendingApproval,
        };
        let group = eval.map(|e| GroupPreview::of(&spec, e));
        Ok(Preview {
            kind: spec.kind(),
            uids: spec.uids,
            checks,
            passed,
            policy,
            would_reach,
            group,
        })
    }

    /// What a dry run checks of `spec`, and on what: the inputs its decision reads, its checks,
    /// and for a group order (rules 34) its check's evaluation, laid out and checked member by
    /// member as its submission would be, each member's terrain sampled along its own share. An
    /// operator's dry run and an agent's preview share it, so neither judges an order to many all
    /// or nothing where its submission would not. `space`: a collect's or cover's answer, asked
    /// for the preview (none for a vehicle task).
    fn dry_run_checks(
        &self,
        spec: &IntentSpec,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> (WorldSnapshot, Vec<CheckResult>, Option<crate::group::GroupEval>) {
        let cfg = &self.policy.rules;
        let group = crate::group::is_group(spec, cfg);
        let mut inputs = if group {
            world.for_intent_with(spec, &crate::terrain::NoTerrain, cfg)
        } else {
            world.for_intent_with(spec, terrain, cfg)
        };
        self.fill_facts(&mut inputs);
        self.fill_engine_facts(spec, &mut inputs);
        if group {
            inputs.terrain = crate::world::TerrainInputs::NotNeeded;
            inputs.route_plans.clear();
            for _ms in crate::group::member_specs(spec, &inputs, cfg) {
            }
        }
        let eval = group.then(|| crate::group::evaluate(spec, &inputs, cfg));
        let checks = match &eval {
            Some(e) => e.results.clone(),
            None => rules::evaluate(spec, &inputs, cfg),
        };
        (inputs, checks, eval)
    }
}

/// Rules 34: what a dry run of a group order shows of it (the operator's order form draws it; an
/// agent's preview reads its bottom line): its spec as it would be logged, the layout its check
/// made (each member's share: slot, layer, altitude, point) and each member's own task as its
/// member order would fly it. Each member's class (sent, waiting, excluded) and why is on its
/// failing checks; the minimum on `spec`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroupPreview {
    pub spec: crate::group::GroupSpec,
    pub layout: crate::group::Layout,
    pub tasks: BTreeMap<Uid, Task>,
}

impl GroupPreview {
    fn of(spec: &IntentSpec, e: crate::group::GroupEval) -> Box<GroupPreview> {
        Box::new(GroupPreview {
            spec: spec.group().cloned().unwrap_or_default(),
            layout: e.layout,
            tasks: e.members.into_iter().filter_map(|(u, m)| Some((u, m.task?))).collect(),
        })
    }
}

/// What a dry run of a proposal would decide ([`AgentPort::preview`]): the checks, the
/// policy decision and the state the intent would reach if proposed now. Nothing is
/// created; the engine logs the preview as `agent.action intent.preview`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preview {
    pub kind: TaskKind,
    pub uids: Vec<Uid>,
    pub checks: Vec<CheckResult>,
    pub passed: bool,
    /// Absent when the checks fail (policy never runs on a rejected intent).
    pub policy: Option<PolicyDecision>,
    /// `rejected`, `auto_approved` or `pending_approval`: never `approved` for an agent's
    /// proposal, because it waits for a human in a human tier. An operator's dry run
    /// ([`Engine::dry_run_for_operator`]) reaches `approved` where the submission itself
    /// would be the approval.
    pub would_reach: State,
    /// Rules 34, a dry run of a group order (an operator's or an agent's): its layout and its
    /// members' tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<Box<GroupPreview>>,
}

/// What agent code is given (Phase F's MCP server holds one per agent). It can propose
/// (with a mandatory [`Rationale`]), preview a proposal (the checks and policy without an
/// intent), withdraw its own intents, note what it did, and read. It has no method that
/// approves, rejects on a human's behalf, or yields an operator session, and its engine
/// reference is private.
///
/// ```compile_fail,E0599
/// use fleetmesh_decide::{AgentPort, IntentId};
/// fn agent_code(port: &mut AgentPort<'_>) {
///     port.approve(IntentId(1)); // no such method
/// }
/// ```
///
/// An agent also cannot reach through to the engine to open an operator session or approve with
/// its own identity:
///
/// ```compile_fail,E0616
/// use fleetmesh_decide::{AgentPort, OperatorId};
/// fn agent_code(port: &mut AgentPort<'_>) {
///     // reaching through to the engine to open an operator session: field is private
///     let _ = port.engine.open_operator_session(OperatorId::new("me"));
/// }
/// ```
///
/// ```compile_fail,E0308
/// use fleetmesh_decide::{AgentId, Engine, IntentId, NoTerrain, WorldSnapshot};
/// fn agent_code(engine: &mut Engine, me: &AgentId, world: &WorldSnapshot) {
///     // an agent identity is not an authority
///     let _ = engine.approve(IntentId(1), me, None, world, &NoTerrain);
/// }
/// ```
pub struct AgentPort<'a> {
    engine: &'a mut Engine,
    agent: AgentId,
}

impl AgentPort<'_> {
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// Logged as `agent.action intent.propose` (the kind, the vehicles and the rationale)
    /// before the intent's own events.
    pub fn propose(
        &mut self,
        mut proposal: Proposal,
        rationale: Rationale,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<&Intent, EngineError> {
        proposal.validate()?;
        self.engine.emit(EventBody::AgentAction {
            agent: self.agent.clone(),
            action: "intent.propose".into(),
            detail: format!(
                "{}: {}",
                describe(&proposal.task, &proposal.uids),
                rationale.as_str()
            ),
        })?;
        proposal.rationale = Some(rationale);
        let source = Source::Agent {
            id: self.agent.clone(),
        };
        let id = self
            .engine
            .submit_inner(source, None, proposal, world, terrain)?;
        self.engine.get(id)
    }

    /// A dry run: the checks and the policy decision a proposal would get right now,
    /// without creating an intent (nothing to withdraw, nothing in the inbox). The same
    /// rules and policy run on the same world view the real submission would read, so the
    /// answer is what [`AgentPort::propose`] would do a moment later, as long as the world
    /// holds still. Logged as `agent.action intent.preview`; the decision inputs are not
    /// logged (there is no decision to replay).
    pub fn preview(
        &mut self,
        proposal: Proposal,
        world: &WorldSnapshot,
        terrain: &dyn TerrainOracle,
    ) -> Result<Preview, EngineError> {
        proposal.validate()?;
        // rules 34: refused as its submission would be under older rules
        if proposal.group.is_some() && false {
            return Err(EngineError::Proposal(ProposalError::BadParams(
                "these decision rules take no group order (rules 34)".into(),
            )));
        }
        if proposal.task.kind() == TaskKind::Transit && false {
            return Err(EngineError::Proposal(ProposalError::BadParams(
                "these decision rules take no transit (rules 34)".into(),
            )));
        }
        let created_t = self.engine.now();
        // rules 34: an order to many is a group order, laid out and checked member by member as
        // its proposal would be (none under older rules)
        let grouping = self.engine.grouping(&proposal)?;
        let spec = IntentSpec {
            id: IntentId(0),
            created_t,
            source: Source::Agent {
                id: self.agent.clone(),
            },
            task: self.engine.normalized(proposal.task, created_t),
            uids: proposal.uids,
            constraints: proposal.constraints,
            rationale: proposal.rationale,
            grouping,
        };
        // what the proposal's decision would read, lost contacts included (rules 16)
        let (inputs, checks, eval) = self.engine.dry_run_checks(&spec, world, terrain);
        let passed = rules::passed(&checks, spec.kind());
        let policy = passed.then(|| policy::decide(&spec, &inputs, &self.engine.policy));
        let would_reach = match &policy {
            None => State::Rejected,
            Some(d) if d.tier == Tier::Auto => State::AutoApproved,
            Some(_) => State::PendingApproval,
        };
        let verdict = match &policy {
            None => format!("checks fail ({})", summarize_failures(&checks)),
            Some(d) => format!("checks pass; policy {}: {}", d.rule, d.reason),
        };
        self.engine.emit(EventBody::AgentAction {
            agent: self.agent.clone(),
            action: "intent.preview".into(),
            detail: format!(
                "{}: {verdict}; would reach {}",
                describe(&spec.task, &spec.uids),
                state_name(would_reach)
            ),
        })?;
        let group = eval.map(|e| GroupPreview::of(&spec, e));
        Ok(Preview {
            kind: spec.kind(),
            uids: spec.uids,
            checks,
            passed,
            policy,
            would_reach,
            group,
        })
    }

    /// Log something this agent did that isn't a proposal (connected, read the inbox).
    /// Logged as `agent.action <action>`, like [`Engine::record_operator_action`] for
    /// operators.
    pub fn record_action(&mut self, action: &str, detail: &str) -> Result<(), EngineError> {
        self.engine.emit(EventBody::AgentAction {
            agent: self.agent.clone(),
            action: action.into(),
            detail: detail.into(),
        })?;
        Ok(())
    }

    /// Withdraw one of this agent's own intents before dispatch.
    pub fn withdraw(&mut self, id: IntentId, reason: &str) -> Result<&Intent, EngineError> {
        let mine = matches!(&self.engine.get(id)?.spec().source, Source::Agent { id: a } if *a == self.agent);
        if !mine {
            return Err(EngineError::NotOwner {
                agent: self.agent.clone(),
                id,
            });
        }
        next_state(self.engine.get(id)?.state(), Transition::Void)
            .map_err(|err| EngineError::Illegal { id, err })?;
        self.engine.emit(EventBody::AgentAction {
            agent: self.agent.clone(),
            action: "intent.withdraw".into(),
            detail: format!("{id}: {reason}"),
        })?;
        self.engine.transition(
            id,
            Transition::Void,
            format!("withdrawn by agent {}: {reason}", self.agent),
        )?;
        self.engine.get(id)
    }


    pub fn intent(&self, id: IntentId) -> Option<&Intent> {
        self.engine.intent(id)
    }

    /// Every intent, any source, in id order (read-only).
    pub fn intents(&self) -> impl Iterator<Item = &Intent> {
        self.engine.intents()
    }

    pub fn inbox(&self) -> Vec<&Intent> {
        self.engine.inbox()
    }
}

/// `orbit on [sky-1, sky-2]`, for agent action details.
fn describe(task: &Task, uids: &[Uid]) -> String {
    let uids: Vec<&str> = uids.iter().map(Uid::as_str).collect();
    format!("{} on [{}]", task.kind().as_str(), uids.join(", "))
}

/// The state's name as logged (`pending_approval`).
fn state_name(state: State) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{state:?}"))
}

