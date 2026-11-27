//! The decision layer in the bridge (Phase C): every action, from the console, the API or
//! (Phase F) an agent, goes intent -> checks -> policy -> (auto | human) -> dispatch.
//!
//! **Who the human is.** A human tier needs a human decision. An operator's own request is
//! one: when the tier has `submitter_authorizes = true` (the default in `sim/policy.toml`),
//! the engine approves it at submission (`approval.at_submission`) and it is dispatched at
//! once, with [`Outcome::authorization`] saying which tier the operator authorized. With
//! `submitter_authorizes = false` it waits in the inbox (HTTP 202). An agent's proposal in a
//! human tier always waits in the inbox for an operator.
//!
//! The bridge owns one [`fleetmesh_decide::Engine`] (policy `sim/policy.toml`, catalog
//! `sim/platforms.toml`, areas `sim/areas/`, audit log `logs/audit/<run-id>.jsonl`, the real
//! clock) behind one lock. This module is the only code that holds it.
//!
//! **One path to the vehicles.** [`crate::tasks::TaskManager::dispatch`] takes only an
//! [`AuthorizedOrder`]. Its fields are private to this module and it has no public
//! constructor, so the only way to make one is [`Decision`]'s dispatch step, which first
//! gets a `DispatchOrder` from `Engine::dispatch` (legal only for an intent in
//! `auto_approved` or `approved`, and logged as `intent.dispatched`) and builds the order
//! from it. (A test-only constructor exists under `#[cfg(test)]` for the task layer's own
//! tests.)
//!
//! **Operators** (rules 8; `crate::auth`). Every request acts for an [`Actor`]. In a
//! credentials run (the default) that is an operator who logged in with their own password:
//! each login opens one engine session ([`Decision::open_login`], `session.opened` with
//! `auth: password`), its end (logout, expiry, a credential removed or changed) closes it
//! ([`Decision::close_login`], `session.closed`), and a consequential signature (every
//! ([`crate::auth::StepUpChecked`], then `Engine::step_up`). The roster is the credential
//! file's operators when the run starts. In a dev run (`FLEETMESH_AUTH=dev`, logged as such)
//! requests name their operator in the `X-FleetMesh-Operator` header against
//! `FLEETMESH_OPERATORS` and `FLEETMESH_CONSEQUENTIAL_OPERATORS` (who must be operators too, or the bridge
//! does not start), each operator gets one session opened on first use, and the legacy
//! operator. Only `crate::auth` and this module open engine sessions. The agent path (Phase
//! F's MCP server, `crate::mcp`) is [`agent`]: it gets an `AgentPort` per agent, never a
//! session.
//!
//! **The loop** ([`Decision::run`], once a second): expire overdue approvals, write vehicle
//! events (mode changes, failsafes, STATUSTEXT) to the audit log, void waiting intents the
//! world invalidated, report each dispatched intent's per-vehicle op states and finish it
//! when every op has ended, then push the inbox to WebSocket clients if it changed.
//!
//! operators with consequential authority (`FLEETMESH_CONSEQUENTIAL_OPERATORS`; never its proposer),
//! stores the picture they approved, and every second the loop lets it hold that picture,
//! release the effect or stop it with its recorded fallback; `POST
//! /api/intents/{id}/abort` lets any operator stop it before the effect.
//!
//! **The queue** (rules 5, [`queue`]). An approved order for a vehicle the link can't carry
//! it to now is held, not sent into a dead link; each tick it is offered again on the world
//! then, re-checked before it goes, and it survives a bridge restart.
//!
//! **Voiding.** A waiting intent (pending or approved, not yet dispatched) is voided when a
//! target vehicle goes silent for longer than its freshness limit, `rules.max_fact_age_s` or
//! `rules.fact_age_intervals` expected report intervals of its link if larger (60 s for an
//! acoustic AUV reporting every 20 s; its facts could no
//! longer support the decision), or when another intent is dispatched to one of its vehicles
//! (superseded: approving it later would silently undo the newer order).

pub mod agent;
pub(crate) mod alerts;
mod civil;
mod failsafe;
pub mod groups;
mod message;
/// Rules 30: one of ours silent, presumed lost, confirmed lost (docs/NODE-LOSS.md).
pub mod nodes;
mod lost_link_read;
pub mod queue;
pub mod snapshot;
pub mod delivery_rt;
pub mod fleet;
pub(crate) mod terrain;
pub mod traffic;
pub mod triage;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use fleetmesh_decide::{
    AnchorConfig, Anchors, AreaSet, AuditSink, AuthMethod, AuthMode, Catalog, Clock, Constraints,
    Engine, EngineError, Intent, IntentId, JsonlFile, LonLat, OperatorId, OperatorSession,
    OperatorView, Policy, Proposal, Rationale,  SessionId,  State,
    Task, TaskOutcome, Time, Uid as DUid, rules,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

use self::snapshot::{bridge_uid, duid};
pub use self::terrain::Terrain;
use crate::clock;
use crate::tasks::{
    LngLat, OpState, OrderError, SiteParam, TaskOrder, TaskParams, TaskRequest, Tasking, TaskingSnapshot,
};
use crate::vehicle::{Uid, VehicleCell};
use crate::world::World;

/// An orbit request without a radius gets this (as the task layer always flew it).
pub const DEFAULT_ORBIT_RADIUS_M: f64 = 200.0;
/// The WebSocket inbox message lists this many recent intents.
const PUSH_INTENTS: usize = 50;

/// The window of the inbox's `decided` count, ms (the operator-load readout: decisions in the last
/// ten minutes).
const DECIDED_WINDOW_MS: i64 = 600_000;

/// How many times a person decided this intent since `from`: a move from pending approval to
/// approved or rejected. A rejection by the checks, an expiry and a voiding are not decisions.
fn decisions_since(history: &[fleetmesh_decide::intent::StateChange], from: Time) -> usize {
    history
        .windows(2)
        .filter(|w| w[0].state == State::PendingApproval && matches!(w[1].state, State::Approved | State::Rejected) && w[1].t.0 >= from.0)
        .count()
}

/// Who a request acts for: an operator and, in a credentials run, the login session it came
/// in on (`crate::auth` makes those, after checking the password; the session must still be
/// open). An operator named by the request itself ([`Actor::asserted`]) acts only in a dev
/// run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    operator: OperatorId,
    login: Option<SessionId>,
}

impl Actor {
    /// An operator the request names (dev mode, tests); refused in a credentials run.
    pub fn asserted(operator: OperatorId) -> Actor {
        Actor {
            operator,
            login: None,
        }
    }

    /// An operator on their login session (`crate::auth` only, for a session
    /// [`Decision::open_login`] opened).
    pub(crate) fn logged_in(operator: OperatorId, session: SessionId) -> Actor {
        Actor {
            operator,
            login: Some(session),
        }
    }

    pub fn operator(&self) -> &OperatorId {
        &self.operator
    }

    /// The login session (credentials run), if any.
    pub fn login(&self) -> Option<SessionId> {
        self.login
    }
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.operator.fmt(f)
    }
}

/// An order the engine released for an approved (or auto-approved) intent: the only thing
/// [`crate::tasks::TaskManager::dispatch`] accepts. Made only in this module, from the
/// `DispatchOrder` that `Engine::dispatch` returns.
pub struct AuthorizedOrder {
    intent: IntentId,
    order: TaskOrder,
}

impl AuthorizedOrder {
    /// For the task layer: the intent's number and the order to run.
    pub(crate) fn into_parts(self) -> (u64, TaskOrder) {
        (self.intent.0, self.order)
    }

    /// Task-layer tests only (no engine in front); intent 0.
    #[cfg(test)]
    pub(crate) fn unchecked(order: TaskOrder) -> AuthorizedOrder {
        AuthorizedOrder {
            intent: IntentId(0),
            order,
        }
    }
}

/// `POST /api/intents`: the console's task request plus the intent fields.
#[derive(Debug, Clone, Deserialize)]
pub struct IntentRequest {
    pub task: String,
    /// Vehicle tasks: the vehicles. Collect and cover: none.
    #[serde(default)]
    pub uids: Vec<Uid>,
    #[serde(flatten)]
    pub params: TaskParams,
    #[serde(default)]
    pub constraints: Constraints,
    #[serde(default)]
    pub rationale: Option<String>,
    /// Rules 34: a group order's spec (its spread, minimum, layer base, wait, whether it re-forms;
    /// absent: the engine's defaults). From rules 34 an order of an ordinary kind to more than one
    /// vehicle is a group order whether it gives one or not.
    #[serde(default)]
    pub group: Option<fleetmesh_decide::group::GroupSpec>,
}

impl From<TaskRequest> for IntentRequest {
    fn from(r: TaskRequest) -> Self {
        IntentRequest {
            task: r.task,
            uids: r.uids,
            params: r.params,
            constraints: Constraints::default(),
            rationale: None,
            group: None,
        }
    }
}

/// An API error: HTTP status, a sentence for the operator, and the intent if one exists.
#[derive(Debug, Clone)]
pub struct DecisionError {
    pub status: u16,
    pub detail: String,
    pub intent: Option<Value>,
}

impl DecisionError {
    pub(crate) fn new(status: u16, detail: impl Into<String>) -> Self {
        DecisionError {
            status,
            detail: detail.into(),
            intent: None,
        }
    }
}

fn engine_err(e: EngineError) -> DecisionError {
    let status = match &e {
        EngineError::UnknownIntent(_) => 404,
        EngineError::Proposal(_) => 400,
        EngineError::NotOwner { .. } => 403,
        EngineError::Illegal { .. }
        | EngineError::Expired(_)
        | EngineError::ViewNotPending { .. }
        | EngineError::ChecksFailedAtApproval(_)
        | EngineError::NotDispatchable { .. }
        | EngineError::AlreadySigned { .. }
        // rules 30: a confirmation or revocation of a loss the engine refused
        | EngineError::NodeLoss(_) => 409,
        EngineError::ProposerCannotApprove { .. }
        | EngineError::NotOnRoster { .. } => 403,
        // rules 8: the signer must (re-)enter their credential
        EngineError::NotAuthenticated { .. } | EngineError::StepUpRequired { .. } => 401,
        _ => 500,
    };
    DecisionError::new(status, e.to_string())
}

/// What a decision call produced.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub id: IntentId,
    pub state: State,
    pub intent: Value,
    /// Set once the intent was dispatched.
    pub tasking: Option<TaskingSnapshot>,
    /// The submitting operator's request was itself the human decision a human tier asks
    /// for: `{by, at_submission, tier, reason, why}` (see [`authorization_json`]).
    pub authorization: Option<Value>,
    /// Collect and cover, once dispatched: the simulated run (see [`space::SpaceRun::json`]).
    pub space: Option<Value>,
}

impl Outcome {
    /// Handed on: to the vehicle task layer, or to the space executor.
    pub fn dispatched(&self) -> bool {
        self.tasking.is_some() || self.space.is_some()
    }
}

/// Where things are, from the environment (see `main.rs`).
pub struct Settings {
    pub policy: PathBuf,
    pub catalog: PathBuf,
    pub areas: PathBuf,
    pub audit: PathBuf,
    /// The run's operators: the credential file's (credentials run, set by `crate::auth`), or
    /// `FLEETMESH_OPERATORS` (dev run).
    pub operators: Vec<OperatorId>,
    /// Those whose sessions carry consequential authority: the credential file's flag, or
    /// `FLEETMESH_CONSEQUENTIAL_OPERATORS` (dev run).
    pub consequential_operators: Vec<OperatorId>,
    /// Rules 12, a credentials run: those whose credential is harness-held (`scripted`) when
    /// the run starts, fixed for the run on the roster (`main` sets it from the file).
    pub scripted: Vec<OperatorId>,
    /// How operators are authenticated (`FLEETMESH_AUTH`: credentials unless `dev`).
    pub auth: AuthMode,
    pub tidewater: Option<String>,
    /// A baked land/water grid for the domain rule when there is no tidewater
    /// (`FLEETMESH_LAND_WATER_GRID`; world v3's H1, the headless simulator's worlds): see
    /// [`terrain`]. Not read with a tidewater URL.
    pub land_water: Option<PathBuf>,
    /// Run-time anchors of the audit log (`logs/anchors/<run-id>.anchors`, or
    /// `FLEETMESH_ANCHOR_DIR`; every `FLEETMESH_ANCHOR_EVERY` events, 100, and one for the decisions
    /// of each tick's batch, at its end: [`Decision::tick`]; `FLEETMESH_ANCHOR_CMD` gets each
    /// one). `FLEETMESH_ANCHORS=0` turns them off.
    pub anchors: Option<AnchorConfig>,
    /// Rules 6: the file naming this station's last audit log (`logs/audit/.last-run-<http
    /// port>`, kept across clean stops too): lost-link values it still owed back are carried
    /// over at start.
    pub last_run: Option<PathBuf>,
    /// Where the previous run's open intents come from (`FLEETMESH_RESUME`, see [`queue`]).
    pub resume: queue::ResumeFrom,
    /// The pointer to this run's log and the intents it leaves open
    /// (`logs/audit/.resume-<FLEETMESH_HTTP_PORT>.json`; none with `FLEETMESH_RESUME=0`).
    pub pointer: Option<PathBuf>,
    /// Rules 10 (`main` sets them in a credentials run): the relying party and the operators'
    /// passkeys from the credential file, and the study the run is (`FLEETMESH_STUDY`).
    pub passkeys: Option<fleetmesh_decide::PasskeyRoster>,
    pub study: Option<fleetmesh_decide::Study>,
}

impl Settings {
    pub fn from_env(root: &Path, catalog: &Path) -> Settings {
        let var = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
        let run = var("FLEETMESH_RUN_ID").unwrap_or_else(|| {
            let id = civil::compact_utc(clock::utc() as i64);
            format!("{id}-{}", std::process::id())
        });
        let operators: Vec<OperatorId> = var("FLEETMESH_OPERATORS")
            .unwrap_or_else(|| "op-1".into())
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(OperatorId::new)
            .collect();
        let consequential_operators: Vec<OperatorId> = var("FLEETMESH_CONSEQUENTIAL_OPERATORS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(OperatorId::new)
            .collect();
        Settings {
            policy: var("FLEETMESH_POLICY")
                .map_or_else(|| root.join("sim/policy.toml"), PathBuf::from),
            catalog: catalog.to_path_buf(),
            areas: var("FLEETMESH_AREAS").map_or_else(|| root.join("sim/areas"), PathBuf::from),
            audit: root.join("logs/audit").join(format!("{run}.jsonl")),
            operators,
            consequential_operators,
            scripted: Vec::new(),
            // `main` sets it, and in a credentials run the roster, from `crate::auth::Settings`
            // (a dev run unless FLEETMESH_AUTH=credentials)
            auth: AuthMode::Dev,
            tidewater: var("FLEETMESH_TIDEWATER_URL"),
            land_water: var("FLEETMESH_LAND_WATER_GRID").map(PathBuf::from),
            last_run: Some(root.join("logs/audit").join(format!(
                ".last-run-{}",
                var("FLEETMESH_HTTP_PORT").and_then(|p| p.parse::<u16>().ok()).unwrap_or(8765)
            ))),
            anchors: (var("FLEETMESH_ANCHORS").as_deref() != Some("0")).then(|| AnchorConfig {
                path: var("FLEETMESH_ANCHOR_DIR")
                    .map_or_else(|| root.join("logs/anchors"), PathBuf::from)
                    .join(format!("{run}.anchors")),
                every: var("FLEETMESH_ANCHOR_EVERY")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(100),
                cmd: var("FLEETMESH_ANCHOR_CMD"),
            }),
            resume: match var("FLEETMESH_RESUME").as_deref() {
                Some("0") => queue::ResumeFrom::Off,
                Some("1") | None => queue::ResumeFrom::Pointer {
                    window_s: var("FLEETMESH_RESUME_WINDOW_S")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(queue::RESUME_WINDOW_S),
                },
                Some(path) => queue::ResumeFrom::Log(PathBuf::from(path)),
            },
            pointer: (var("FLEETMESH_RESUME").as_deref() != Some("0")).then(|| {
                let port = var("FLEETMESH_HTTP_PORT").unwrap_or_else(|| "8765".into());
                root.join("logs/audit").join(format!(".resume-{}.json", port.trim()))
            }),
            passkeys: None,
            study: None,
        }
    }
}

/// A clock both the engine and the snapshot builder read.
#[derive(Clone)]
struct SharedClock(Arc<dyn Clock + Send + Sync>);

impl Clock for SharedClock {
    fn now(&self) -> Time {
        self.0.now()
    }
}

/// A dispatched intent the loop follows until its ops end.
struct Running {
    tasking: Arc<Tasking>,
    /// op id -> (state, not moving) last written to the log
    reported: HashMap<u64, (OpState, bool)>,
    /// rules 30: its ops on vehicles confirmed lost (the engine ended them `lost`): never
    /// reported again, and no part of how the order ended
    lost: HashSet<u64>,
}

struct Core {
    engine: Engine,
    /// Dev run: one session per asserted operator, opened on first use.
    sessions: BTreeMap<OperatorId, OperatorSession>,
    /// Credentials run: one session per login, by its engine session id.
    logins: BTreeMap<SessionId, OperatorSession>,
    running: BTreeMap<IntentId, Running>,
    /// every tasking an intent produced
    taskings: BTreeMap<IntentId, Arc<Tasking>>,
    /// `operator.view` budget per operator: (tokens, monotonic seconds when last filled)
    view_budget: BTreeMap<OperatorId, (f64, f64)>,
    /// resumed intents a vehicle had started, waiting for their vehicles to connect
    follow: BTreeMap<IntentId, queue::PendingFollow>,
    /// the open intents last written to the pointer file, and when (monotonic s)
    pointer_written: Option<(Vec<u64>, f64)>,
    /// the fleet identity last written beside the log, and when it was last looked at
    fleet_written: Option<queue::FleetIdentity>,
    fleet_looked: Option<f64>,
    /// rules 30: the bridge's side of our vehicles' standing (hand flags, notices; display)
    nodes: nodes::NodeView,
}

/// `operator.view` events an operator may log: a burst of this many, refilled at
/// [`VIEW_RATE_PER_S`] (a console posts a handful per card; more is not instrumentation).
const VIEW_BURST: f64 = 40.0;
const VIEW_RATE_PER_S: f64 = 10.0;

pub struct Decision {
    world: Arc<World>,
    clock: SharedClock,
    catalog: Catalog,
    areas: AreaSet,
    operators: Vec<OperatorId>,
    consequential_operators: Vec<OperatorId>,
    /// How operators are authenticated (logged in `run.started`).
    auth: AuthMode,
    terrain: Terrain,
    rules: fleetmesh_decide::RulesConfig,
    /// The hubs and drop zones of this world (`sim/hubs.toml`, `FLEETMESH_HUBS`).
    places: fleetmesh_decide::delivery::Places,
    /// What the delivery rules read that the bridge does not measure itself: parcels, weather,
    /// drop-zone observations and link predictions, fed by dispatch, a scenario or a test.
    feed: Mutex<DeliveryFeed>,
    /// The delivery runtime: the scenario's parcels, the dispatcher's jobs, the navigation monitor
    /// ([`delivery_rt`]).
    runtime: Mutex<delivery_rt::Runtime>,
    /// The company's picture of the traffic the fleet does not control ([`traffic`], rules 3).
    traffic: traffic::Picture,
    /// The watch settings this run decides under (`Policy::effective_watch`; see
    /// docs/SCENARIO-SETTINGS.md): the policy's `[watch]` from rules 33, else the legacy values
    /// with the scenario's area. MCP `get_policy` serves them.
    audit_path: PathBuf,
    /// The audit log's run-time anchors, when on (set once by [`Decision::load`]).
    anchors: std::sync::OnceLock<Anchors>,
    core: Mutex<Core>,
    updates: watch::Sender<Arc<String>>,
    /// Vehicle events the loop logged, for agents (`agent::RecentEvents`).
    recent: agent::RecentEvents,
    /// Rules 7: the AI watch officer's triage of the Inbox ([`triage`]).
    triage: Mutex<triage::Triage>,
    /// Operators' rejection notes, for agents to read with the intent (display only).
    rejection_notes: agent::RejectionNotes,
    /// `[queue]` of the policy (rules 5).
    queue_cfg: fleetmesh_decide::QueueConfig,
    /// The pointer file this run keeps ([`queue`]), if any (set once by [`Decision::load`]).
    pointer: std::sync::OnceLock<PathBuf>,
    /// The fleet identity file beside the audit log (set once by [`Decision::load`]).
    fleet_file: std::sync::OnceLock<PathBuf>,
    /// Itself, for the task layer's write-ahead of a vehicle start (rules 7: the op logs it
    /// through the decision lock before sending it).
    me: std::sync::Weak<Decision>,
    /// The decisions message's intents as last written ([`message`]; display only).
    intent_texts: Mutex<message::IntentTexts>,
    /// A tick's call for the decisions message, which [`Decision::run`]'s publisher answers
    /// beside the tick (bd murmur-jvdc: the console's message never holds the tick up).
    publish_due: tokio::sync::Notify,
}

/// Everything [`Decision::from_parts`] needs (tests pass a memory sink and a manual clock).
pub struct Parts {
    pub policy: Policy,
    pub catalog: Catalog,
    pub areas: AreaSet,
    pub sink: Box<dyn AuditSink + Send>,
    pub audit_path: PathBuf,
    pub clock: Arc<dyn Clock + Send + Sync>,
    pub terrain: Terrain,
    pub operators: Vec<OperatorId>,
    pub consequential_operators: Vec<OperatorId>,
    /// Rules 12, a credentials run: the operators whose credential is harness-held at the
    /// start (logged on the roster; empty: none).
    pub scripted: Vec<OperatorId>,
    pub auth: AuthMode,
    /// Rules 10, a credentials run: the relying party and the operators' passkeys when the run
    /// starts (logged on the roster), and the study it is, if one.
    pub passkeys: Option<fleetmesh_decide::PasskeyRoster>,
    pub study: Option<fleetmesh_decide::Study>,
    /// Rules 12: what earlier runs' logs still owed, as the start read them
    /// (`consequential::read_carry`): logged in `run.started` and owed back from there.
    pub carried_from: Vec<fleetmesh_decide::replay::CarriedFrom>,
    /// Rules 15: the station's `.last-run` pointer, by its file name (`run.started`'s `pointer`):
    /// a later start whose pointer can't be read falls back to this station's newest log.
    pub pointer: Option<String>,
    /// Rules 33: the files the policy was built from (`Policy::load_layers`), logged in
    /// `run.started` (`policy_layers`); empty for a policy not read from a file (a test).
    pub policy_layers: Vec<fleetmesh_decide::Layer>,
}

/// The decision rules a run is asked to decide under (`FLEETMESH_RULES`): none (unset or empty) for
/// the default, [`rules::RULES_VERSION`]; the default or the rules being built
/// ([`rules::RULES_NEWEST`], docs/NODE-LOSS.md section 8) by number; anything else is an error that
/// stops the bridge.
pub(crate) fn rules_asked(v: Option<&str>) -> Result<Option<u32>, String> {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    match v.parse::<u32>() {
        Ok(n) if n == rules::RULES_VERSION || n == rules::RULES_NEWEST => Ok(Some(n)),
        _ => Err(format!(
            "FLEETMESH_RULES={v}: this build decides under rules {} (the default) or {} only",
            rules::RULES_VERSION,
            rules::RULES_NEWEST
        )),
    }
}

/// Remove `path` if it is an empty file (a start that failed before writing to it).
fn remove_if_empty(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0) {
        let _ = std::fs::remove_file(path);
    }
}

impl Decision {
    /// Load policy, catalog and areas, open the audit log and start the engine. Any failure
    /// stops the bridge: without the decision layer nothing may reach a vehicle.
    pub fn load(world: Arc<World>, s: &Settings) -> Result<Arc<Decision>, String> {
        let (mut policy, policy_layers) = Policy::load_layers(&s.policy).map_err(|e| e.to_string())?;
        // the decision rules being built, only when asked for by number
        if let Some(n) = rules_asked(std::env::var("FLEETMESH_RULES").ok().as_deref())? {
            if n != policy.rules.rules_version {
                println!("rules: deciding under decision rules {n} (FLEETMESH_RULES), not yet the default {}", rules::RULES_VERSION);
            }
            policy.rules.rules_version = n;
        }
        // rules 10: a study requires its participant's passkey at every step-up, whatever the
        // policy file says, unless the explicit override allows a password (logged; never
        // evidence)
        if let Some(st) = &s.study {
            match &st.password_step_up {
                None if policy.approval.step_up_factor != Some(fleetmesh_decide::StepUpFactor::Passkey) => {
                    println!("study: step-ups by passkey ([approval] step_up_factor = \"passkey\" for this run)");
                    policy.approval.step_up_factor = Some(fleetmesh_decide::StepUpFactor::Passkey);
                }
                None => {}
                Some(why) => println!(
                    "study: PASSWORD STEP-UPS ALLOWED by FLEETMESH_STUDY_PASSWORD_STEP_UP ({why}): logged, and this run is not evidence about people"
                ),
            }
        }
        let catalog = Catalog::load(&s.catalog).map_err(|e| e.to_string())?;
        // the bridge's clock, never the wall clock itself: the facts are stamped on it, and a
        // wall clock that steps must not move the log's times or the ages decided on
        // (murmur-dup)
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(clock::BridgeClock);
        let areas = AreaSet::load_dir(&s.areas, clock.now()).map_err(|e| e.to_string())?;
        if let Some(dir) = s.audit.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        if s.operators.is_empty() {
            return Err(match s.auth {
                AuthMode::Dev => "FLEETMESH_OPERATORS lists no operator".into(),
                AuthMode::Credentials => "the credential file holds no operator".into(),
            });
        }
        // rules 12: what earlier runs' logs still owe, read before this run's log is opened
        // (a reused run id appends to the same file) and recorded in its first line; a
        // pointer or a log that can't be read is a failed carry there, never a failed start
        let _followups = true;
        // rules 15: the station's pointer, named in run.started
        let pointer = s
            .last_run
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned())
            .filter(|_| true);
        // FLEETMESH-REVIEW: neutral behaviour (no earlier run's debts are carried); decide what a start carries.
        let carried_from = Vec::new();
        let file = JsonlFile::open(&s.audit).map_err(|e| format!("{}: {e}", s.audit.display()))?;
        // a start that fails from here on leaves no empty log or anchor file behind (a log
        // reused by its run id, already written, stays)
        let undo = |e: String| -> String {
            remove_if_empty(&s.audit);
            if let Some(cfg) = &s.anchors {
                remove_if_empty(&cfg.path);
            }
            e
        };
        // anchored from its first event on: an anchor file that can't be opened stops the
        // bridge like an audit log that can't
        let anchors = match &s.anchors {
            Some(cfg) => Some(
                // timed on the log's own clock
                Anchors::open_with(cfg, &s.audit, Box::new(clock::BridgeClock))
                    .map_err(|e| undo(format!("anchors {}: {e}", cfg.path.display())))?
                    .with_utc(Box::new(clock::UtcClock)),
            ),
            None => None,
        };
        let sink: Box<dyn AuditSink + Send> = match &anchors {
            Some(a) => Box::new(a.sink(file)),
            None => Box::new(file),
        };
        let d = Decision::from_parts(
            world,
            Parts {
                policy,
                catalog,
                areas,
                sink,
                audit_path: s.audit.clone(),
                clock,
                terrain: Terrain::with_grid(s.tidewater.as_deref(), s.land_water.as_deref()),
                operators: s.operators.clone(),
                consequential_operators: s.consequential_operators.clone(),
                scripted: s.scripted.clone(),
                auth: s.auth,
                passkeys: s.passkeys.clone(),
                study: s.study.clone(),
                carried_from,
                pointer,
                policy_layers,
            },
        )
        .map_err(undo)?;
        if let Some(a) = anchors {
            let _ = d.anchors.set(a);
        }
        if let Some(p) = &s.pointer {
            let _ = d.pointer.set(p.clone());
        }
        let _ = d.fleet_file.set(queue::fleet_file(&s.audit));
        // the previous run's open intents (queued, approved, under way), carried over
        // the previous run's open intents (queued, approved, under way), carried over
        let previous = queue::previous_log(&s.resume, s.pointer.as_deref(), &s.audit);
        if let Some(prev) = &previous {
            let name = prev
                .file_name()
                .map_or_else(|| prev.display().to_string(), |n| n.to_string_lossy().into_owned());
            // its anchors, where this run keeps its own (`<dir>/<log stem>.anchors`), and the
            // fleet it had (`<log>.fleet.json`): without either, nothing is carried unless
            // FLEETMESH_RESUME_UNVERIFIED=1 says to go without
            let anchors = s.anchors.as_ref().and_then(|cfg| {
                let stem = prev.file_stem()?.to_string_lossy().into_owned();
                let file = cfg.path.parent()?.join(format!("{stem}.anchors"));
                std::fs::read_to_string(file).ok()
            });
            let fleet = std::fs::read(queue::fleet_file(prev))
                .ok()
                .and_then(|b| serde_json::from_slice::<queue::FleetIdentity>(&b).ok());
            let unverified = std::env::var("FLEETMESH_RESUME_UNVERIFIED").as_deref() == Ok("1");
            let missing: Vec<&str> = [
                anchors.is_none().then_some("its anchor file"),
                fleet.is_none().then_some("its fleet identity"),
            ]
            .into_iter()
            .flatten()
            .collect();
            let check = queue::CarryCheck { anchors, fleet };
            let run = if missing.is_empty() || unverified {
                if !missing.is_empty() {
                    eprintln!(
                        "decision: {name}: carried over without {} (FLEETMESH_RESUME_UNVERIFIED=1)",
                        missing.join(" or ")
                    );
                }
                std::fs::read_to_string(prev)
                    .map_err(|e| format!("{}: {e}", prev.display()))
                    .and_then(|text| d.carry_over(&name, &text, &check))
            } else {
                Err(format!(
                    "{name}: {} can't be read, so it can't be checked: nothing carried over (FLEETMESH_RESUME_UNVERIFIED=1 carries it over unchecked)",
                    missing.join(" and ")
                ))
            };
            match run {
                Ok(ids) if ids.is_empty() => {}
                Ok(ids) => println!(
                    "decision: carried {} open intent(s) over from {name}: {}",
                    ids.len(),
                    ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                ),
                Err(e) => eprintln!("decision: nothing carried over: {e}"),
            }
        }
        // rules 29: the fleet's starting tasks, the previous watch's orders, handed over at the
        // start; none when this run carries on from an earlier log (its vehicles are that
        // run's, past their start)
        if let Some(prev) = &previous {
            println!(
                "decision: this run carries on from {}: the fleet's starting tasks are not handed over",
                prev.display()
            );
        } else {
            let t0 = std::time::Instant::now();
            match d.hand_over_starts() {
                Ok(0) => {}
                Ok(n) => println!(
                    "decision: handed over {n} starting task(s) from the fleet file in {:.0} ms (queued until each vehicle can be reached)",
                    t0.elapsed().as_secs_f64() * 1000.0
                ),
                Err(e) => eprintln!("decision: {e}"),
            }
        }
        // rules 6: lost-link values the last run on this port still owed back (rules 12: the
        // engine took them in run.started; their lanes, the pointer to this log)
        // the start's batch (what was carried over, what is owed back): anchored before the
        // first tick
        d.commit_anchors();
        Ok(d)
    }

    /// The end of a batch (bd murmur-fqid): one anchor for every decision logged since the
    /// last anchor, one fsync for all of them ([`Anchors::commit`]). The tick commits at its
    /// end, under the lock; what is logged between ticks (an operator's or an agent's request,
    /// the task layer's write-ahead of a vehicle start, the queue) is in the next tick's
    /// batch; the start commits its own, and the shutdown anchor covers the last.
    fn commit_anchors(&self) {
        if let Some(a) = self.anchors.get()
            && let Err(e) = a.commit()
        {
            // the events are logged: the anchor stays due, and the next commit tries again
            eprintln!("anchor {}: {e}", a.path().display());
        }
    }

    /// The anchor file, when the log is anchored.
    pub fn anchor_path(&self) -> Option<PathBuf> {
        self.anchors.get().map(Anchors::path)
    }

    /// A clean shutdown: under the decision lock (no event being written, none after),
    /// anchor the log's head and close it behind the anchor, let the anchor command finish,
    /// and exit with the lock still held.
    pub fn shutdown_and_exit(&self) -> ! {
        let mut core = self.lock();
        // rules 8: every login's session ends with the run, on the record
        let logins = std::mem::take(&mut core.logins);
        for (_, s) in logins {
            if let Err(e) = core.engine.close_operator_session(s, "bridge stopped") {
                eprintln!("decision: closing a session at shutdown: {e}");
            }
        }
        if let Some(a) = self.anchors.get() {
            match a.close() {
                Ok(_) => a.flush_hook(),
                Err(e) => eprintln!("anchor {}: {e}", a.path().display()),
            }
        }
        // a clean stop leaves nothing to carry over: the next run starts afresh (only a
        // crash, or `FLEETMESH_RESUME=<log>`, carries open intents into a new run)
        if let Some(p) = self.pointer.get() {
            queue::drop_pointer(p, &self.audit_path);
        }
        std::process::exit(0)
    }

    pub fn from_parts(world: Arc<World>, p: Parts) -> Result<Arc<Decision>, String> {
        let clock = SharedClock(p.clock);
        let rules = p.policy.rules.clone();
        let places = load_places(&world)?;
        let scenario = Decision::load_scenario(&world.cfg.root)?;
        let queue_cfg = p.policy.queue();
        // the operators this run admits, logged in run.started: every consequential operator
        // must be an operator too, or the bridge does not start
        let roster = fleetmesh_decide::Roster::new(p.operators.clone(), p.consequential_operators.clone())
            .map_err(|e| match p.auth {
                AuthMode::Dev => format!(
                    "{e} (FLEETMESH_OPERATORS: {:?}, FLEETMESH_CONSEQUENTIAL_OPERATORS: {:?})",
                    p.operators, p.consequential_operators
                ),
                AuthMode::Credentials => format!("{e} (the credential file's operators)"),
            })?
            .with_auth(p.auth);
        // rules 10: the relying party and the passkeys the run starts with, and a study
        let roster = match p.passkeys {
            Some(k) => roster.with_passkeys(k),
            None => roster,
        };
        let roster = match p.study {
            Some(s) => roster.with_study(s),
            None => roster,
        };
        // rules 12: the scripted operators, fixed at the start (a credentials run's)
        let roster = match p.auth {
            AuthMode::Credentials => roster.with_scripted(p.scripted.clone()),
            AuthMode::Dev => roster,
        };
        let engine = Engine::start(
            p.policy,
            Some(p.areas.clone()),
            roster,
            fleetmesh_decide::StartCarry {
                pointer: p.pointer,
                from: p.carried_from,
                policy_layers: p.policy_layers,
            },
            Box::new(clock.clone()),
            p.sink,
        )
        .map_err(|e| e.to_string())?;
        let world_root = world.cfg.root.clone();
        let d = Arc::new_cyclic(|me| Decision {
            me: me.clone(),
            world,
            clock,
            // rules 34: each platform's group-order figures on its facts (a decision's fact from
            // rules 34 only; older runs' facts as before)
            catalog: { p.catalog.formed() },
            areas: p.areas,
            operators: p.operators,
            consequential_operators: p.consequential_operators,
            auth: p.auth,
            terrain: p.terrain,
            rules,
            places,
            feed: Mutex::new(DeliveryFeed::default()),
            runtime: Mutex::new(delivery_rt::Runtime::new(scenario)),
            traffic: traffic::Picture::new(traffic::Settings::from_env(&world_root)),
            audit_path: p.audit_path,
            anchors: std::sync::OnceLock::new(),
            core: Mutex::new(Core {
                engine,
                sessions: BTreeMap::new(),
                logins: BTreeMap::new(),
                running: BTreeMap::new(),
                taskings: BTreeMap::new(),
                view_budget: BTreeMap::new(),
                follow: BTreeMap::new(),
                pointer_written: None,
                fleet_written: None,
                fleet_looked: None,
                nodes: nodes::NodeView::default(),
            }),
            updates: watch::channel(Arc::new(String::new())).0,
            recent: agent::RecentEvents::default(),
            triage: Mutex::new(triage::Triage::default()),
            rejection_notes: agent::RejectionNotes::default(),
            queue_cfg,
            pointer: std::sync::OnceLock::new(),
            fleet_file: std::sync::OnceLock::new(),
            intent_texts: Mutex::new(message::IntentTexts::default()),
            publish_due: tokio::sync::Notify::new(),
        });
        d.publish();
        Ok(d)
    }


    pub fn audit_path(&self) -> &Path {
        &self.audit_path
    }

    pub fn terrain_configured(&self) -> bool {
        self.terrain.configured()
    }

    /// The baked land/water grid the domain rule reads, when it reads one (no tidewater).
    pub fn terrain_grid(&self) -> Option<String> {
        self.terrain.grid_source()
    }


    fn lock(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }


    /// The operators and who of them has consequential authority (for the console), how they
    /// are authenticated (`credentials` or `dev`), and the acts besides signatures that
    /// re-enter the credential in this run (`[approval] step_up_acts`; none in a dev run).
    pub fn operators_json(&self) -> Value {
        json!({
            "operators": self.all_operators(),
            "consequential": self.consequential_operators,
            "auth": self.auth.as_str(),
        })
    }

    /// Credentials run: a refused login for `op` (on the roster), logged as `auth.refused`
    /// with why and the lock it set. Nothing secret; a failure to log is reported, not fatal.
    pub fn record_login_refused(&self, op: &OperatorId, reason: &str, locked_s: Option<u64>) {
        if let Err(e) = self.lock().engine.record_login_refused(op, reason, locked_s) {
            eprintln!("decision: logging a refused login for {op}: {e}");
        }
    }

    /// Whether `op` is on the run's roster, and with consequential authority.
    pub fn roster_entry(&self, op: &OperatorId) -> Option<bool> {
        self.operators
            .contains(op)
            .then(|| self.consequential_operators.contains(op))
    }

    /// Credentials run: open the engine session for a login whose password `crate::auth`
    /// just checked (`session.opened`, `auth: password`; `auth: scripted` when the credential
    /// is harness-held), with consequential authority if `consequential` (the credential holds it now)
    /// and the roster gave it at start.
    pub fn open_login(&self, op: &OperatorId, consequential: bool, scripted: bool) -> Result<SessionId, DecisionError> {
        if self.auth != AuthMode::Credentials {
            return Err(DecisionError::new(409, "this run asserts operator ids (FLEETMESH_AUTH=dev): there is no login"));
        }
        let grants = fleetmesh_decide::Grants {
            consequential: consequential && self.consequential_operators.contains(op),
        };
        let out = {
            let mut core = self.lock();
            let s = core
                .engine
                .open_authenticated_session(
                    op.clone(),
                    grants,
                    if scripted { AuthMethod::Scripted } else { AuthMethod::Password },
                )
                .map_err(engine_err)?;
            let id = s.authority().session();
            core.logins.insert(id, s);
            id
        };
        Ok(out)
    }

    /// Credentials run: end a login's engine session (`session.closed`, with why: logout,
    /// expiry, a credential removed or changed). Its authorities and picture tokens go with
    /// it. Nothing happens for a session already ended.
    pub fn close_login(&self, session: SessionId, reason: &str) {
        let mut core = self.lock();
        if let Some(s) = core.logins.remove(&session)
            && let Err(e) = core.engine.close_operator_session(s, reason)
        {
            eprintln!("decision: closing session {}: {e}", session.0);
        }
    }

    /// Credentials run: whether approving intent `id` re-enters the approver's credential
    /// (its decision says `step_up`, as any tier marked so does).
    pub fn needs_step_up(&self, id: u64) -> bool {
        self.auth == AuthMode::Credentials
            && self
                .lock()
                .engine
                .intent(IntentId(id))
                .and_then(Intent::policy)
                .is_some_and(|p| p.step_up)
    }

    /// Rules 8: the step-up `crate::auth` checked for `by`'s login, logged under `session`
    /// (`operator.action step_up`) for the act about to quote it (rules 10: a passkey's with
    /// its assertion, which the engine verifies again). None when none was checked.
    fn mint_step(
        engine: &mut Engine,
        session: &OperatorSession,
        by: &Actor,
        step: Option<crate::auth::StepUpChecked>,
    ) -> Result<Option<fleetmesh_decide::StepUp>, DecisionError> {
        match step {
            Some(c) if c.session() == by.login() && c.operator() == by.operator() => match c.into_factor() {
                crate::auth::StepFactor::Password => Ok(Some(engine.step_up(session).map_err(engine_err)?)),
                crate::auth::StepFactor::Passkey(proof) => {
                    Ok(Some(engine.step_up_passkey(session, *proof).map_err(engine_err)?))
                }
            },
            Some(_) => Err(DecisionError::new(401, "the credential re-entered is not this session's")),
            None => Ok(None),
        }
    }

    // ---- passkeys (rules 10) ----------------------------------------------------------------

    /// The run's relying party and the passkeys the engine knows now (its roster's and those
    /// registered since, not removed); None when the run takes no passkey.
    pub fn passkey_roster(&self) -> Option<fleetmesh_decide::PasskeyRoster> {
        self.lock().engine.passkeys()
    }

    /// The study this run is, if it is one.
    pub fn study(&self) -> Option<fleetmesh_decide::Study> {
        self.lock().engine.study().cloned()
    }

    /// What `by`'s step-up must be: a passkey where the policy asks people for one and `by` is
    /// a person (signed in by password or passkey); a password otherwise (a dev run has none).
    pub fn step_up_factor_for(&self, by: &Actor) -> fleetmesh_decide::StepUpFactor {
        let core = self.lock();
        let person = by
            .login
            .and_then(|id| core.logins.get(&id))
            .and_then(OperatorSession::method)
            .is_some_and(AuthMethod::is_person);
        if self.auth == AuthMode::Credentials && person {
            core.engine.policy().step_up_factor()
        } else {
            fleetmesh_decide::StepUpFactor::Password
        }
    }

    /// The policy's step-up factor and login factor (for the console).
    pub fn factors(&self) -> (fleetmesh_decide::StepUpFactor, fleetmesh_decide::LoginFactor) {
        let core = self.lock();
        (core.engine.policy().step_up_factor(), core.engine.policy().login_factor())
    }

    /// A passkey challenge for `purpose` by `op` (in `session`, a login of theirs; none for a
    /// login), with the bridge's random `nonce`.
    pub fn passkey_challenge(
        &self,
        op: &OperatorId,
        session: Option<SessionId>,
        purpose: fleetmesh_decide::StepUpFor,
        nonce: String,
    ) -> Result<fleetmesh_decide::passkey::Issued, DecisionError> {
        let mut guard = self.lock();
        let core = &mut *guard;
        let s = match session {
            Some(id) => Some(
                core.logins
                    .get(&id)
                    .ok_or_else(|| DecisionError::new(401, "your session has ended: sign in again"))?,
            ),
            None => None,
        };
        core.engine.passkey_challenge(op, s, purpose, nonce).map_err(engine_err)
    }

    pub fn sign_purpose(&self, id: u64, token: Option<u64>) -> Result<fleetmesh_decide::StepUpFor, DecisionError> {
        self.lock().engine.sign_purpose(IntentId(id), token).map_err(engine_err)
    }

    /// Credentials run, rules 10: open the engine session for a login whose passkey
    /// `crate::auth` just checked (`session.opened`, `auth: passkey`, with the assertion, which
    /// the engine verifies again).
    pub fn open_passkey_login(
        &self,
        op: &OperatorId,
        consequential: bool,
        proof: fleetmesh_decide::PasskeyUse,
    ) -> Result<SessionId, DecisionError> {
        if self.auth != AuthMode::Credentials {
            return Err(DecisionError::new(409, "this run asserts operator ids (FLEETMESH_AUTH=dev): there is no login"));
        }
        let grants = fleetmesh_decide::Grants {
            consequential: consequential && self.consequential_operators.contains(op),
        };
        let mut core = self.lock();
        let s = core.engine.open_passkey_session(op.clone(), grants, proof).map_err(engine_err)?;
        let id = s.authority().session();
        core.logins.insert(id, s);
        Ok(id)
    }

    /// Rules 10: a passkey registered for `key.operator` (`passkey.registered`): through `by`'s
    /// session with the password they re-entered for it (`step`), or (None) an invitation.
    pub fn record_passkey_registered(
        &self,
        key: fleetmesh_decide::PasskeyKey,
        by: Option<(&Actor, crate::auth::StepUpChecked)>,
    ) -> Result<(), DecisionError> {
        let mut guard = self.lock();
        let core = &mut *guard;
        match by {
            None => core.engine.record_passkey_registered(key, None).map_err(engine_err),
            Some((actor, step)) => {
                let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, actor, &self.consequential_operators, self.auth)?;
                let step = Self::mint_step(&mut core.engine, session, actor, Some(step))?
                    .ok_or_else(|| DecisionError::new(401, "the password was not re-entered"))?;
                core.engine
                    .record_passkey_registered(key, Some((session, step)))
                    .map_err(engine_err)
            }
        }
    }

    /// Rules 10: a passkey removed from the credential file while the run is up
    /// (`passkey.removed`). A failure to log is reported, not fatal (nothing uses it: the
    /// engine and the bridge both refuse it from here on).
    pub fn record_passkey_removed(&self, op: &OperatorId, credential: &str, reason: &str) {
        if let Err(e) = self.lock().engine.record_passkey_removed(op, credential, reason) {
            eprintln!("decision: logging a removed passkey of {op}: {e}");
        }
    }


    fn all_operators(&self) -> Vec<OperatorId> {
        let mut v = self.operators.clone();
        for o in &self.consequential_operators {
            if !v.contains(o) {
                v.push(o.clone());
            }
        }
        v
    }

    /// The policy's battery reserve, percent (`[rules] reserve_pct`): the swarms' low battery.
    pub fn reserve_pct(&self) -> f64 {
        self.rules.reserve_pct
    }

    fn fleet(&self) -> BTreeMap<Uid, Arc<VehicleCell>> {
        self.world
            .vehicles()
            .into_iter()
            .map(|v| (v.uid, v))
            .collect()
    }

    // ---- operators ------------------------------------------------------------------------

    /// Dev run: the operator a request names. `header` is `X-FleetMesh-Operator`; `legacy` lets
    /// the first listed operator. A credentials run never names its operator this way
    /// (`crate::auth` resolves the login instead): 401.
    pub fn operator(
        &self,
        header: Option<&str>,
        legacy: bool,
    ) -> Result<OperatorId, DecisionError> {
        if self.auth == AuthMode::Credentials {
            return Err(DecisionError::new(
                401,
                "sign in: this bridge authenticates operators with their own password (FLEETMESH_AUTH=dev asserts ids by header instead)",
            ));
        }
        match header.map(str::trim).filter(|s| !s.is_empty()) {
            Some(h) => self
                .all_operators()
                .into_iter()
                .find(|o| o.as_str() == h)
                .ok_or_else(|| {
                    DecisionError::new(
                        403,
                        format!("operator {h:?} is not in FLEETMESH_OPERATORS or FLEETMESH_CONSEQUENTIAL_OPERATORS"),
                    )
                }),
            None if legacy => self
                .operators
                .first()
                .cloned()
                .ok_or_else(|| DecisionError::new(500, "no operator configured")),
            None => Err(DecisionError::new(
                401,
                "set the X-FleetMesh-Operator header to an operator id (FLEETMESH_OPERATORS; a dev run: \
                 the id is asserted, not authenticated)",
            )),
        }
    }

    /// The session an actor acts in: their login's (credentials run; 401 once it has ended),
    /// authority if the operator is in `FLEETMESH_CONSEQUENTIAL_OPERATORS`.
    fn session<'a>(
        engine: &mut Engine,
        sessions: &'a mut BTreeMap<OperatorId, OperatorSession>,
        logins: &'a BTreeMap<SessionId, OperatorSession>,
        by: &Actor,
        consequential_operators: &[OperatorId],
        auth: AuthMode,
    ) -> Result<&'a OperatorSession, DecisionError> {
        if let Some(id) = by.login {
            return logins
                .get(&id)
                .filter(|s| s.operator() == &by.operator)
                .ok_or_else(|| DecisionError::new(401, "your session has ended: sign in again"));
        }
        if auth == AuthMode::Credentials {
            return Err(DecisionError::new(
                401,
                "sign in: this bridge authenticates operators with their own password",
            ));
        }
        let op = &by.operator;
        if !sessions.contains_key(op) {
            let grants = fleetmesh_decide::Grants {
                consequential: consequential_operators.contains(op),
            };
            let s = engine
                .open_operator_session_with(op.clone(), grants)
                .map_err(engine_err)?;
            sessions.insert(op.clone(), s);
        }
        sessions
            .get(op)
            .ok_or_else(|| DecisionError::new(500, "operator session missing"))
    }

    // ---- requests -------------------------------------------------------------------------

    /// A human's intent: `POST /api/intents`, and the legacy `POST /api/tasks`. Malformed
    /// requests (unknown task or vehicle, missing parameters) are refused before an intent
    /// exists, as before. Auto-approved intents are dispatched at once.
    pub async fn submit(
        &self,
        by: &Actor,
        req: IntentRequest,
    ) -> Result<Outcome, DecisionError> {
        self.submit_stepped(by, req, None).await
    }

    /// [`Self::submit`] with the step-up a credentials run asks for.
    pub async fn submit_stepped(
        &self,
        by: &Actor,
        req: IntentRequest,
        step: Option<crate::auth::StepUpChecked>,
    ) -> Result<Outcome, DecisionError> {
        let _ = step;
        self.check_vehicle_task(&req)?;
        let proposal = proposal(&req, &self.places)?;
        self.prefetch_grouped(&proposal.task, &proposal.uids, proposal.group.as_ref()).await;
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let world = self.snapshot();
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            let id = core
                .engine
                .submit(
                    session.authority(),
                    proposal,
                    &world,
                    &self.terrain.oracle(),
                )
                .map_err(engine_err)?
                .id();
            self.after_decision(core, id)
        };
        self.publish();
        out
    }

    /// `POST /api/intents/preview`: an operator's dry run of a vehicle task, the checks and the
    /// policy decision it would get now; nothing is created and nothing is logged
    /// ([`fleetmesh_decide::Engine::dry_run_for_operator`]). `would_reach` is `approved` (and
    /// `submission_authorizes` true) where the operator's own submission would be the approval:
    /// a situation's runner asks before a scripted id submits an opening order, and sends such
    /// an order to a person instead.
    pub async fn operator_dry_run(&self, by: &Actor, req: IntentRequest) -> Result<Value, DecisionError> {
        if !(matches!(req.task.as_str(), "orbit" | "patrol" | "hold" | "rtl" | "transit") || is_delivery_task(&req.task)) {
            return Err(DecisionError::new(
                422,
                "an operator's dry run takes a vehicle task (orbit, patrol, transit, hold, rtl, or a delivery order)",
            ));
        }
        self.check_vehicle_task(&req)?;
        let proposal = proposal(&req, &self.places)?;
        self.prefetch_grouped(&proposal.task, &proposal.uids, proposal.group.as_ref()).await;
        let (task, uids) = (proposal.task.clone(), proposal.uids.clone());
        let preview = {
            let guard = self.lock();
            let world = self.snapshot();
            guard
                .engine
                .dry_run_for_operator(by.operator(), proposal, &world, &self.terrain.oracle())
                .map_err(engine_err)?
        };
        let mut v = serde_json::to_value(&preview).unwrap_or(Value::Null);
        v["operator"] = json!(by.operator().as_str());
        v["submission_authorizes"] = json!(preview.would_reach == State::Approved);
        v["note"] = json!("a dry run: nothing was created or logged");
        // rules 22: the legs the planner routed (or could not), for the map
        v["routes"] = routes_json(&self.planned_routes(&task, &uids));
        // information only, after the decision and outside the lock (docs/LINK-COVERAGE.md)
        if !uids.is_empty() {
            v["link_coverage"] = self.link_coverage(&task, &uids).await;
        }
        Ok(v)
    }

    /// The predicted link along each vehicle's route for an order (docs/LINK-COVERAGE.md,
    /// `crate::coverage`): the transit, task path and return the engine's route geometry lays
    /// out (`fleetmesh_decide::route::route_for`), each predicted by the link emulator's own
    /// geometry. Information only: read outside the decision lock, never logged, never a check.
    pub async fn link_coverage(&self, task: &Task, uids: &[DUid]) -> Value {
        let spec = Self::prefetch_spec(self.clock.now(), task, uids);
        let mut world = self.snapshot().subset_for(&spec);
        // rules 22: along the legs as the planner routes them
        world.route_plans = self.planned_routes(task, uids);
        let routes = uids
            .iter()
            .map(|u| crate::coverage::VehicleRoute {
                uid: bridge_uid(u),
                route: match world.vehicles.get(u) {
                    Some(v) => fleetmesh_decide::route::route_for(&spec, v, &world, &self.rules),
                    None => Err(format!("no vehicle with uid {u}")),
                },
            })
            .collect();
        // the failsafe's chance is judged over the standing time on station
        let on_station_s = fleetmesh_linkemu::coverage::ON_STATION_S;
        self.world.coverage().order_answer(&self.world, routes, on_station_s).await
    }

    /// [`Self::link_coverage`] for a request as the dry run takes it (a vehicle task).
    pub async fn link_coverage_for(&self, req: &IntentRequest) -> Result<Value, DecisionError> {
        let p = proposal(req, &self.places)?;
        if p.uids.is_empty() {
            return Err(DecisionError::new(422, "name the vehicles (uids, or uid)"));
        }
        Ok(self.link_coverage(&p.task, &p.uids).await)
    }

    /// A vehicle task's request, as the task layer would take it (known vehicles, a known
    /// kind and its parameters), or why not.
    fn check_vehicle_task(&self, req: &IntentRequest) -> Result<(), DecisionError> {
        if is_delivery_task(&req.task) {
            return self.check_vehicles(&req.uids);
        }
        let order = TaskRequest {
            task: req.task.clone(),
            uids: req.uids.clone(),
            params: req.params.clone(),
        };
        match TaskOrder::new(order, &self.fleet()) {
            Ok(_) => Ok(()),
            Err(e @ OrderError::UnknownVehicles(_)) => Err(DecisionError::new(404, e.to_string())),
            Err(e) => Err(DecisionError::new(400, e.to_string())),
        }
    }

    fn check_vehicles(&self, uids: &[Uid]) -> Result<(), DecisionError> {
        let fleet = self.fleet();
        let missing: Vec<String> = uids
            .iter()
            .filter(|u| !fleet.contains_key(u))
            .map(ToString::to_string)
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(DecisionError::new(
                404,
                format!("unknown vehicles [{}]", missing.join(", ")),
            ))
        }
    }

    /// Approve a pending intent (re-checked against the current world; voided if the
    /// picture no longer holds), then dispatch it. Tests' shorthand for
    /// [`Self::approve_with`] with no token and nothing acknowledged.
    #[cfg(test)]
    pub async fn approve(
        &self,
        by: &Actor,
        id: u64,
        note: Option<String>,
    ) -> Result<Outcome, DecisionError> {
        self.approve_with(by, id, note, None, false).await
    }

    /// [`Self::approve`] with what the signature needs: the picture
    /// token its signer was shown (`token`, from [`Self::issue_picture`]); the engine refuses
    /// it when the token expired, is another session's, or its plan no longer
    /// needs `acknowledge` (the engine refuses it otherwise, and logs what was acknowledged).
    #[cfg(test)]
    pub async fn approve_with(
        &self,
        by: &Actor,
        id: u64,
        note: Option<String>,
        token: Option<u64>,
        acknowledge: bool,
    ) -> Result<Outcome, DecisionError> {
        self.approve_stepped(by, id, note, token, acknowledge, None).await
    }

    /// [`Self::approve_with`] after a step-up: `step` says `crate::auth` just checked the
    /// approver's password again, for this login. The engine logs the step-up under the
    /// session (`operator.action step_up`) and the signature quotes it; a consequential
    /// credentials run (401).
    pub async fn approve_stepped(
        &self,
        by: &Actor,
        id: u64,
        note: Option<String>,
        _token: Option<u64>,
        acknowledge: bool,
        step: Option<crate::auth::StepUpChecked>,
    ) -> Result<Outcome, DecisionError> {
        let id = IntentId(id);
        let (task, uids, group) = {
            let core = self.lock();
            let i = core
                .engine
                .intent(id)
                .ok_or_else(|| DecisionError::new(404, format!("unknown intent {id}")))?;
            (i.spec().task.clone(), i.spec().uids.clone(), i.spec().group().cloned())
        };
        self.prefetch_grouped(&task, &uids, group.as_ref()).await;
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let world = self.snapshot();
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            // rules 8: the password was just checked again for this login: log the step-up
            // under its session, for this signature to quote
            let step = Self::mint_step(&mut core.engine, session, by, step)?;
            let oracle = self.terrain.oracle();
            let r = if let Some(s) = step {
                core.engine
                    .approve_stepped(id, session.authority(), s, note, acknowledge, &world, &oracle)
                    .map(|_| ())
            } else if acknowledge {
                core.engine
                    .approve_acknowledging(id, session.authority(), note, &world, &oracle)
                    .map(|_| ())
            } else {
                core.engine
                    .approve(id, session.authority(), note, &world, &oracle)
                    .map(|_| ())
            };
            match r {
                Ok(()) => self.after_decision(core, id),
                Err(e) => Err(self.with_intent(core, id, engine_err(e))),
            }
        };
        self.publish();
        out
    }

    pub fn reject(
        &self,
        by: &Actor,
        id: u64,
        note: Option<String>,
    ) -> Result<Outcome, DecisionError> {
        let id = IntentId(id);
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            let kept = note.clone();
            match core.engine.reject(id, session.authority(), note) {
                Ok(_) => {
                    // display only, for agents: the engine keeps no note on the intent
                    self.remember_rejection(id.0, by.operator(), kept.as_deref());
                    Ok(self.outcome(core, id))
                }
                Err(e) => Err(self.with_intent(core, id, engine_err(e))),
            }
        };
        self.publish();
        out
    }

    /// Withdraw an intent before dispatch, or (rules 11) a dispatched collect or cover:
    /// the engine reports what the cancel means for the order and aborts it; the space
    /// executor stops following it.
    pub fn cancel(
        &self,
        by: &Actor,
        id: u64,
        note: Option<String>,
    ) -> Result<Outcome, DecisionError> {
        let id = IntentId(id);
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            let note = note.unwrap_or_else(|| "cancelled".into());
            match core.engine.cancel(id, session.authority(), &note) {
                Ok(_i) => {
                    // rules 11: a dispatched space task: the engine's report, for the run's card
                    Ok(self.outcome(core, id))
                }
                Err(e) => Err(self.with_intent(core, id, engine_err(e))),
            }
        };
        self.publish();
        out
    }

    /// Log something an operator did outside the intent flow (an acknowledgement, say) as
    /// `operator.action`, under their session.
    pub fn record_action(
        &self,
        by: &Actor,
        action: &str,
        detail: &str,
    ) -> Result<(), DecisionError> {
        let mut guard = self.lock();
        let core = &mut *guard;
        let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
        core.engine
            .record_operator_action(session.authority(), action, detail)
            .map_err(engine_err)
    }

    /// Log what an operator's console showed for a pending intent (`operator.view`, Phase
    /// G): `POST /api/operator/view`. Unknown intent 404, malformed view 422, an `open` or
    /// `decide` view of an intent no longer waiting for a decision 409, more than
    /// [`VIEW_BURST`] views at once (refilled at [`VIEW_RATE_PER_S`] a second) 429.
    pub fn record_view(
        &self,
        by: &Actor,
        id: u64,
        view: OperatorView,
    ) -> Result<(), DecisionError> {
        let mut guard = self.lock();
        let core = &mut *guard;
        let mono = clock::mono();
        let budget = core.view_budget.entry(by.operator().clone()).or_insert((VIEW_BURST, mono));
        budget.0 = (budget.0 + (mono - budget.1).max(0.0) * VIEW_RATE_PER_S).min(VIEW_BURST);
        budget.1 = mono;
        if budget.0 < 1.0 {
            return Err(DecisionError::new(
                429,
                "too many view events: this is instrumentation, a few per card",
            ));
        }
        budget.0 -= 1.0;
        let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
        core.engine
            .record_operator_view(session.authority(), IntentId(id), view)
            .map_err(|e| match e {
                EngineError::Proposal(p) => DecisionError::new(422, p.to_string()),
                e => engine_err(e),
            })
    }

    fn with_intent(&self, core: &Core, id: IntentId, mut e: DecisionError) -> DecisionError {
        if let Some(i) = core.engine.intent(id) {
            if let Some(last) = i.history().last()
                && !e.detail.contains(&last.reason)
            {
                e.detail = format!("{}: {}", e.detail, last.reason);
            }
            e.intent = serde_json::to_value(i).ok();
        }
        e
    }

    fn outcome(&self, core: &Core, id: IntentId) -> Outcome {
        let intent = core.engine.intent(id);
        Outcome {
            id,
            state: intent.map_or(State::Proposed, Intent::state),
            intent: intent.map_or(Value::Null, |i| intent_json(i, None, self.tol())),
            tasking: core.taskings.get(&id).map(|t| t.snapshot()),
            authorization: intent.and_then(authorization_json),
            space: None,
        }
    }

    /// After submit or approve: dispatch what may run, report the rest as it stands.
    fn after_decision(&self, core: &mut Core, id: IntentId) -> Result<Outcome, DecisionError> {
        let state = core.engine.intent(id).map(Intent::state);
        if matches!(state, Some(State::AutoApproved | State::Approved)) {
            self.dispatch(core, id)?;
        }
        Ok(self.outcome(core, id))
    }

    /// The one place an [`AuthorizedOrder`] is made: from the engine's `DispatchOrder`.
    fn dispatch(&self, core: &mut Core, id: IntentId) -> Result<(), DecisionError> {
        let fleet = self.fleet();
        let spec = core
            .engine
            .intent(id)
            .ok_or_else(|| DecisionError::new(404, format!("unknown intent {id}")))?
            .spec()
            .clone();
        // rules 34: a group order goes out as its member orders, each on its own
        if fleetmesh_decide::group::is_group(&spec, &self.rules) {
            return self.dispatch_group(core, id);
        }
        // a battery swap and a drop-zone confirmation have no vehicle op: the hub and the person
        // do them. The engine marks them dispatched, and they are done (the confirmation stands
        // from its approval, `Engine::note_confirmation`; the hub's swap is instant for now)
        if spec.kind() == fleetmesh_decide::TaskKind::ConfirmDropZone {
            let world = self.snapshot();
            if let fleetmesh_decide::engine::Routed::Dispatched(_) =
                core.engine.dispatch_or_queue(id, &world).map_err(engine_err)?
            {
                core.engine.finish(id, TaskOutcome::Done).map_err(engine_err)?;
            }
            return Ok(());
        }
        // the task layer must accept it before the engine marks it dispatched
        let task = spec.task.clone();
        if let Err(e) = task_request(&task, &spec.uids)
            .and_then(|r| TaskOrder::new(r, &fleet).map_err(|e| e.to_string()))
        {
            core.engine
                .void(id, &format!("the task layer can't take it: {e}"))
                .map_err(engine_err)?;
            return Ok(());
        }
        // rules 5: dispatched when the link can carry it to every vehicle, else queued
        self.dispatch_vehicles(core, id)
    }

    /// Older orders still waiting for these vehicles are superseded (voided), and an
    /// that cause: the newest order stands. A newer order is never voided by an older one's
    /// dispatch (rules 5: an older queued order that comes out of the queue after it).
    fn supersede(&self, core: &mut Core, id: IntentId, uids: &[DUid]) {
        let took: BTreeSet<&DUid> = uids.iter().collect();
        let stale: Vec<(IntentId, DUid)> = core
            .engine
            .intents()
            .filter(|i| {
                i.id() < id
                    && matches!(
                        i.state(),
                        State::PendingApproval | State::Approved | State::Queued
                    )
            })
            .filter_map(|i| {
                let u = i.spec().uids.iter().find(|u| took.contains(u))?;
                Some((i.id(), u.clone()))
            })
            .collect();
        for (other, uid) in stale {
            let why = format!("superseded: {id} was dispatched to vehicle {uid}");
            if let Err(e) = core.engine.void(other, &why) {
                eprintln!("decision: void {other}: {e}");
            }
        }
    }

    // ---- world --------------------------------------------------------------------------

    /// The catalog the decision facts come from: our sensors' `range_m` alone.
    fn facts_catalog(&self) -> &Catalog {
        &self.catalog
    }

    fn snapshot(&self) -> fleetmesh_decide::WorldSnapshot {
        let mut w = snapshot::build(
            &self.world,
            self.facts_catalog(),
            &self.areas,
            self.clock.now(),
            // rules 30: each vehicle's own lost-link failsafe
            true,
        );
        w.delivery = Some(self.delivery_inputs());
        w
    }

    /// What the delivery rules read besides the vehicles' facts: the world's places, the bridge's
    /// own reading of each vehicle's navigation fix and of the other traffic, and what the feed
    /// gives (parcels, weather, observations, link predictions).
    fn delivery_inputs(&self) -> fleetmesh_decide::delivery::DeliveryInputs {
        let feed = self.feed.lock().unwrap_or_else(|e| e.into_inner());
        let mut di = fleetmesh_decide::delivery::DeliveryInputs {
            places: self.places.clone(),
            parcels: feed.parcels.clone(),
            weather: feed.weather.clone(),
            observations: feed.observations.clone(),
            link: feed.link.clone(),
            ..Default::default()
        };
        for (id, n) in &feed.hub_packs {
            if let Some(h) = di.places.hubs.get_mut(id) {
                h.packs = *n;
            }
        }
        snapshot::delivery_vehicle_inputs(&self.world, self.facts_catalog(), &mut di);
        // rules 5: the fleet's own vehicles' routes still ahead of them, for the separation rule
        if self.rules.at_least(fleetmesh_decide::HUB_SEQUENCING_SINCE) {
            for (uid, path) in &feed.routes {
                if let Some(t) = di.traffic.get_mut(uid) {
                    t.route = delivery_rt::remaining_route(t.position.value, path).into_iter().skip(1).collect();
                }
            }
        }
        // rules 3: the picture of other traffic, when this run has one
        if self.rules.at_least(fleetmesh_decide::TRAFFIC_SEPARATION_SINCE)
            && self.traffic.active()
            && let Some(b) = self.traffic.built()
        {
            di.picture = Some(b.picture.clone());
        }
        for (uid, (state, good_for)) in &feed.nav_hysteresis {
            if let Some(n) = di.nav.get_mut(uid) {
                n.prev_state = Some(*state);
                n.good_for_s = Some(*good_for);
            }
        }
        drop(feed);
        di
    }

    /// The delivery feed, to be filled by dispatch (parcels), a weather source, a scenario or a
    /// test.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn feed(&self) -> std::sync::MutexGuard<'_, DeliveryFeed> {
        self.feed.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The hubs and drop zones of this world.
    pub fn places(&self) -> &fleetmesh_decide::delivery::Places {
        &self.places
    }

    /// Rules 30 (docs/NODE-LOSS.md section 3): where each of our silent vehicles can be now, as
    /// a decision reads, changed first by `adjust` (the headless check of the regions against the
    /// truth fills in each vehicle's own lost-link failsafe there, which this bridge does not read
    /// yet). Read-only and never logged; empty under older rules. Only the headless check reads it
    /// so far (the console's SILENT region, docs/NODE-LOSS.md stage 5, is to).
    #[cfg_attr(not(fleetmesh_headless), allow(dead_code))]
    pub fn silent_regions(
        &self,
        adjust: impl FnOnce(&mut fleetmesh_decide::WorldSnapshot),
    ) -> BTreeMap<DUid, fleetmesh_decide::silent::Region> {
        let mut w = self.snapshot();
        adjust(&mut w);
        let at = w.t;
        self.lock().engine.silent_regions(&w, at)
    }

    /// Rules 17: while a safety command the domain rule let go with a warning (an rtl whose way
    /// home crosses land, `CheckResult::warning`) runs, an inbox alert says where, so a person
    /// can send a safer order. It clears when the vehicle's op ends or another order replaces it.
    fn route_alerts(
        &self,
        core: &Core,
        ops: &[crate::tasks::OpSnapshot],
        fleet: &BTreeMap<Uid, Arc<VehicleCell>>,
        now_wall: f64,
    ) -> Vec<alerts::Alert> {
        // each intent's domain and geofence checks that carry a warning, by vehicle, its first
        // for each: looked up for each op, not searched for among every check of a hundred
        // vehicles' order (bd murmur-jvdc)
        let mut warned: BTreeMap<u64, BTreeMap<DUid, (rules::RuleId, String)>> = BTreeMap::new();
        ops.iter()
            .filter(|op| !op.state.is_terminal())
            .filter_map(|op| {
                let by_uid = warned.entry(op.intent_id).or_insert_with(|| {
                    let mut by_uid = BTreeMap::new();
                    if let Some(i) = core.engine.intent(IntentId(op.intent_id)) {
                        for c in i.checks() {
                            if matches!(c.rule, rules::RuleId::Domain | rules::RuleId::Geofence)
                                && let (Some(u), Some(w)) = (&c.uid, &c.warning)
                            {
                                by_uid.entry(u.clone()).or_insert((c.rule, w.clone()));
                            }
                        }
                    }
                    by_uid
                });
                if by_uid.is_empty() {
                    return None;
                }
                let (rule, warning) = by_uid.get(&duid(op.uid)).cloned()?;
                Some(alerts::Alert {
                    id: if rule == rules::RuleId::Geofence { format!("route:{}:minefield", op.id) } else { format!("route:{}", op.id) },
                    kind: if rule == rules::RuleId::Geofence { "path_crosses_minefield" } else { "path_crosses_land" },
                    severity: alerts::Severity::High,
                    uid: op.uid,
                    callsign: fleet.get(&op.uid).and_then(|c| alerts::callsign(c)),
                    detail: warning,
                    since_t: op.history.first().map_or(now_wall, |h| h.t),
                    op_id: Some(op.id),
                    intent_id: Some(op.intent_id),
                    issue: None,
                    track_id: None,
                    track_ids: Vec::new(),
                })
            })
            .collect()
    }

    /// [`Self::prefetch`] of an order with its group spec (rules 34: its spread, layer base and
    /// lanes lay its members out; none: the kind's defaults).
    async fn prefetch_grouped(&self, task: &Task, uids: &[DUid], group: Option<&fleetmesh_decide::group::GroupSpec>) {
        // nothing to fetch with no tidewater (the baked grid answers under the lock as it is)
        if !self.terrain.fetches() {
            return;
        }
        // only when some target must keep to water or land (no world snapshot otherwise)
        if !self.needs_surface(uids) {
            return;
        }
        let world = self.snapshot();
        // rules 34: a group order's terrain is its members' shares' (each member's own way to its
        // slot, ring or lane), laid out on the world now as its check will lay it out
        if let Some(specs) = self.group_member_specs(task, uids, &world, group) {
            for ms in specs {
                self.prefetch_in(&world, &ms.task, &ms.uids).await;
            }
            return;
        }
        self.prefetch_in(&world, task, uids).await;
    }

    /// [`Self::prefetch`] of a vehicle task on a world snapshot already taken (the queued
    /// intents' prefetch takes one for all of them).
    async fn prefetch_in(&self, world: &fleetmesh_decide::WorldSnapshot, task: &Task, uids: &[DUid]) {
        if !self.needs_surface(uids) {
            return;
        }
        let spec = Self::prefetch_spec(self.clock.now(), task, uids);
        let world = world.subset_for(&spec);
        let view = spec.clone();
        if let Some(path) = view.task.path() {
            self.terrain.prefetch(&path.points).await;
        }
        // rules 17: the points along each leg the domain rule judges, from where they are now
        // (a vehicle that moves before the decision finds each answered from a sample near it)
        {
            let points = fleetmesh_decide::route::along_points(&view, &world, &self.rules);
            if !points.is_empty() {
                self.terrain.prefetch(&points).await;
            }
        }
        // rules 22: the cells of the planner's grid for each leg it will route, then the points
        // along the routed legs, so the engine plans and judges them from the cache under its lock
        {
            let grid = fleetmesh_decide::plan::grid_points(&view, &world, &self.terrain.oracle(), &self.rules);
            if !grid.is_empty() {
                self.terrain.prefetch(&grid).await;
            }
            let mut routed = world.clone();
            routed.route_plans = fleetmesh_decide::plan::plans_for(&view, &world, &self.terrain.oracle(), &self.rules);
            if !routed.route_plans.is_empty() {
                let points = fleetmesh_decide::route::along_points(&view, &routed, &self.rules);
                if !points.is_empty() {
                    self.terrain.prefetch(&points).await;
                }
            }
        }
    }

    /// Rules 22: the routes the planner lays out for an order on the world now (read-only, from
    /// the terrain cache a prefetch filled): what the dry run and the map show.
    pub(super) fn planned_routes(&self, task: &Task, uids: &[DUid]) -> std::collections::BTreeMap<DUid, Vec<fleetmesh_decide::plan::PlannedLeg>> {
        let spec = Self::prefetch_spec(self.clock.now(), task, uids);
        let world = self.snapshot().subset_for(&spec);
        // rules 27: round our live fields, as the decision plans
        let view = spec.clone();
        fleetmesh_decide::plan::plans_for(&view, &world, &self.terrain.oracle(), &self.rules)
    }

    /// Whether some of these vehicles must keep to water or land (the catalog's domain).
    fn needs_surface(&self, uids: &[DUid]) -> bool {
        let fleet = self.fleet();
        uids.iter().filter_map(bridge_uid).any(|u| {
            fleet
                .get(&u)
                .and_then(|c| {
                    c.read(|v| {
                        v.identity
                            .as_ref()
                            .and_then(|i| i.str("platform"))
                            .map(str::to_string)
                    })
                })
                .and_then(|p| self.catalog.get(&p).cloned())
                .is_some_and(|p| rules::required_surface(p.domain, p.autopilot).is_some())
        })
    }

    /// The spec a prefetch reads the world for (no intent is made).
    fn prefetch_spec(now: fleetmesh_decide::Time, task: &Task, uids: &[DUid]) -> fleetmesh_decide::IntentSpec {
        fleetmesh_decide::IntentSpec {
            id: IntentId(0),
            created_t: now,
            source: fleetmesh_decide::Source::Human {
                id: OperatorId::new("prefetch"),
            },
            task: task.clone(),
            uids: uids.to_vec(),
            constraints: Constraints::default(),
            rationale: None,
            grouping: Default::default(),
        }
    }

    // ---- the loop -----------------------------------------------------------------------

    /// The decision loop. Terrain is fetched by its own task beside it, so a slow or failing
    /// terrain service never delays the tick.
    pub async fn run(self: Arc<Self>) {
        let prefetcher = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                // rules 34: the shares a re-form due would move members to, laid out outside the
                // lock, so the tick's step judges them from the cache
                prefetcher.prefetch_groups().await;
                // queued intents' waypoints: their dequeue re-check reads them from the cache (one
                // world snapshot for all of them: rules 29 queues a fleet's thousands at the start)
                if prefetcher.terrain.fetches() && !prefetcher.terrain.backing_off() {
                    let queued = prefetcher.queued_paths();
                    if !queued.is_empty() {
                        let world = prefetcher.snapshot();
                        for (task, uids) in queued {
                            prefetcher.prefetch_in(&world, &task, &uids).await;
                        }
                    }
                }
            }
        });
        // the delivery runtime: feeds, follows, dispatches (it proposes as agents, never approves)
        let delivery = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                delivery.delivery_step().await;
            }
        });
        // the console's decisions message, built after each tick but beside it: at Saturation's
        // size it took longer than the tick's own work (bd murmur-jvdc)
        let publisher = self.clone();
        tokio::spawn(async move {
            loop {
                publisher.publish_due.notified().await;
                publisher.publish();
            }
        });
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            crate::perf::time("decision_tick", || self.step());
            self.publish_due.notify_one();
        }
    }

    /// The decision loop's step ([`Self::step`]), then the decisions message for the console
    /// ([`Self::publish`]), for the tests; the loop itself ([`Self::run`]) publishes beside its
    /// steps.
    #[cfg(test)]
    pub fn tick(&self) {
        self.step();
        self.publish();
    }

    /// The decision loop's step, once a second, under the decision lock; it ends with the
    /// batch's anchor ([`Self::commit_anchors`]: every decision logged since the last tick).
    fn step(&self) {
        {
            let mut guard = self.lock();
            let core = &mut *guard;
            // orders whose time ran out are expired first, so no decision of this tick reads them
            if let Err(e) = core.engine.expire_due() {
                eprintln!("decision: expire: {e}");
            }
            crate::perf::time("decision.log_reboots", || self.log_reboots(core));
            crate::perf::time("decision.log_vehicle_events", || self.log_vehicle_events(core));
            // rules 30: our vehicles' standing (a loss a report revoked, a presumption raised or
            // cleared)
            crate::perf::time("decision.step_nodes", || self.step_nodes(core));
            crate::perf::time("decision.void_lost", || self.void_lost(core));
            crate::perf::time("decision.follow_queue", || self.follow_queue(core));
            crate::perf::time("decision.follow_resumed", || self.follow_resumed(core));
            // rules 27: the mines the layers reached are laid before their ops' ends finish them
            crate::perf::time("decision.follow_ops", || self.follow_ops(core));
            // rules 34: the groups in force (drops, re-forms, suspensions), once their members'
            // ends are logged (timed only then: an older run's /api/stats reads as before)
            crate::perf::time("decision.step_groups", || self.step_groups(core));
            crate::perf::time("decision.write_pointer", || self.write_pointer(core));
            crate::perf::time("decision.write_fleet", || self.write_fleet(core));
            // the batch's anchor: this tick's decisions and those logged since the last tick
            crate::perf::time("decision.commit_anchors", || self.commit_anchors());
        }
        // rules 7: the watch officer's triage of what the Inbox now holds (outside the lock: the
        // alerts read it themselves)
        crate::perf::time("decision.step_triage", || self.step_triage());
        // rules 30: each vehicle's own lost-link failsafe read from it (outside the lock: it
        // only asks the vehicles)
        crate::perf::time("decision.read_lost_link", || lost_link_read::read_lost_link(&self.world));
    }

    fn log_vehicle_events(&self, core: &mut Core) {
        let mut failsafes: Vec<(DUid, String)> = Vec::new();
        let mut left: Vec<(DUid, String)> = Vec::new();
        for cell in self.world.vehicles() {
            let (events, mission_mode) = cell.write(|v| {
                let events: Vec<_> = v.events.drain(..).collect();
                (events, v.kind.and_then(crate::tasks::caps).map(|c| c.auto))
            });
            for e in &events {
                if let Err(err) = core
                    .engine
                    .record_vehicle_event(&duid(cell.uid), e.kind, &e.text)
                {
                    eprintln!("decision: vehicle event: {err}");
                }
                // a failsafe that took control from the mission (a mode switch, a disarm, a
                // landing: failsafe.rs); one that says nothing of what it did is left to
                // the mode change below
                if e.kind == "failsafe" && failsafe::failsafe_took_over(&e.text) {
                    failsafes.push((duid(cell.uid), e.text.clone()));
                }
                // a mode change away from the mission mode: the task layer stops watching the
                // mode once the platform is on station (an orbit keeps orbiting)
                if e.kind == "mode"
                    && let Some(auto) = mission_mode
                    && let Some((from, to)) = e.text.split_once(" -> ")
                    && from.trim() == auto
                    && to.trim() != auto
                {
                    left.push((duid(cell.uid), e.text.clone()));
                }
            }
            self.remember_events(&cell, &events);
        }
    }

    /// Void waiting intents whose vehicle has gone silent (its facts are past the policy's
    /// freshness limit, link-aware: `max_fact_age_s` or `fact_age_intervals` report intervals,
    /// so the picture the decision rests on is gone).
    fn void_lost(&self, core: &mut Core) {
        let fleet = self.fleet();
        let now = clock::wall();
        let mono = clock::mono();
        let lost: Vec<(IntentId, String)> = core
            .engine
            .intents()
            .filter(|i| matches!(i.state(), State::PendingApproval | State::Approved))
            .filter_map(|i| {
                i.spec().uids.iter().find_map(|u| {
                    let cell = fleet.get(&bridge_uid(u)?)?;
                    let seen = cell.read(|v| v.seen.heartbeat);
                    let interval = cell.link_snapshot(mono).expected_interval_ms;
                    let (limit_ms, _) = self.rules.fact_age_limit_ms(Some(interval));
                    match seen {
                        Some(t) if now - t <= limit_ms as f64 / 1000.0 => None,
                        Some(t) => Some((
                            i.id(),
                            format!("vehicle {u} lost: no heartbeat for {:.0} s", now - t),
                        )),
                        None => Some((i.id(), format!("vehicle {u} lost: no heartbeat"))),
                    }
                })
            })
            .collect();
        for (id, why) in lost {
            if let Err(e) = core.engine.void(id, &why) {
                eprintln!("decision: void {id}: {e}");
            }
        }
    }

    /// Log each op's state changes (and NOT MOVING) as `op.state`; when all its ops have
    /// ended, finish the intent: done if every vehicle got there, else failed with why; or
    /// (rules 5), when every op ended `requeued` (the link lost before any vehicle started
    /// it), send it back to the queue.
    fn follow_ops(&self, core: &mut Core) {
        let ahead = self.writes_ahead();
        let Core {
            engine, running, ..
        } = core;
        let mut ended = Vec::new();
        let mut requeued = Vec::new();
        for (id, run) in running.iter_mut() {
            let snap = run.tasking.snapshot();
            for op in &snap.ops {
                let key = (op.state, op.detail.starts_with("NOT MOVING"));
                // rules 30: an op on a vehicle confirmed lost was ended by the engine
                if run.reported.get(&op.id) == Some(&key) || run.lost.contains(&op.id) {
                    continue;
                }
                // rules 7: a start is logged by the op itself, ahead of sending it
                // (`write_ahead`), never after it here
                if ahead && op.state == OpState::Starting {
                    continue;
                }
                let state = serde_json::to_value(op.state)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                if let Err(e) = engine.record_op_state(*id, &duid(op.uid), &state, &op.detail) {
                    eprintln!("decision: op state: {e}");
                    continue;
                }
                run.reported.insert(op.id, key);
            }
            if snap.ops.iter().all(|o| o.state.is_terminal()) {
                if !snap.ops.is_empty()
                    && snap
                        .ops
                        .iter()
                        .all(|o| matches!(o.state, OpState::Requeued | OpState::Unconfirmed))
                {
                    requeued.push(*id);
                    continue;
                }
                let bad: Vec<String> = snap
                    .ops
                    .iter()
                    .filter(|o| o.state != OpState::Done && !run.lost.contains(&o.id))
                    .map(|o| {
                        let s = serde_json::to_value(o.state).unwrap_or(Value::Null);
                        format!(
                            "vehicle {}: {} ({})",
                            o.uid,
                            s.as_str().unwrap_or("?"),
                            o.detail
                        )
                    })
                    .collect();
                let outcome = if bad.is_empty() {
                    TaskOutcome::Done
                } else {
                    TaskOutcome::Failed {
                        detail: bad.join("; "),
                    }
                };
                ended.push((*id, outcome));
            }
        }
        // back to the queue: re-checked on the world when the link is back, then delivered
        // asking the vehicle what it holds first; if the engine refuses (a vehicle had
        // started it), the order failed
        for id in requeued {
            match engine.requeue(id) {
                Ok(_) => {
                    running.remove(&id);
                }
                Err(e) => {
                    eprintln!("decision: requeue {id}: {e}");
                    let detail = format!("the link was lost before the order started: {e}");
                    match engine.finish(id, TaskOutcome::Failed { detail }) {
                        Ok(_) => {
                            running.remove(&id);
                        }
                        Err(e) => eprintln!("decision: finish {id}: {e}"),
                    }
                }
            }
        }
        for (id, outcome) in ended {
            match engine.finish(id, outcome) {
                Ok(_) => {
                    running.remove(&id);
                }
                Err(e) => eprintln!("decision: finish {id}: {e}"),
            }
        }
    }

    /// Rules 7: whether vehicle starts are logged ahead of sending them.
    fn writes_ahead(&self) -> bool {
        true
    }

    /// Rules 7: the task layer's write-ahead of a vehicle start, for an order to the vehicles
    /// (none under earlier rules): an op logs its start through it, and the start goes out
    /// only once it is in the audit log.
    pub(super) fn write_ahead_hook(&self) -> Option<crate::tasks::WriteAhead> {
        if !self.writes_ahead() {
            return None;
        }
        let me = self.me.clone();
        Some(Arc::new(move |op: &crate::tasks::OpCell, detail: &str| {
            match me.upgrade() {
                Some(d) => d.write_ahead(op.intent_id, op.id, op.uid, detail),
                None => Err("the decision layer is gone".into()),
            }
        }))
    }

    /// Rules 7: `op.state` starting for op `op` (intent `intent`, vehicle `uid`), logged ahead
    /// of the start it records (`Engine::record_op_state_ahead`), under the decision lock: it
    /// waits for any decision in progress, so it comes after the dispatch that sent the order.
    /// `Err`: not logged (the order ended or a newer one took the vehicle meanwhile, or the
    /// log refused it), and the op sends nothing.
    fn write_ahead(&self, intent: u64, op: u64, uid: Uid, detail: &str) -> Result<(), String> {
        let mut guard = self.lock();
        let Core {
            engine, running, ..
        } = &mut *guard;
        let id = IntentId(intent);
        let Some(run) = running.get_mut(&id) else {
            return Err(format!("{id} is no longer followed"));
        };
        if run.reported.get(&op).is_some_and(|(s, _)| *s == OpState::Starting) {
            return Ok(());
        }
        engine
            .record_op_state_ahead(id, &duid(uid), detail)
            .map_err(|e| e.to_string())?;
        run.reported.insert(op, (OpState::Starting, false));
        Ok(())
    }

    // ---- reading ------------------------------------------------------------------------

    /// `GET /api/intents`: newest first.
    pub fn intents_json(&self) -> Value {
        let core = self.lock();
        let all: Vec<&Intent> = core.engine.intents().collect();
        let intents: Vec<Value> = all
            .into_iter()
            .rev()
            .map(|i| {
                let mut v = intent_json(i, core.taskings.get(&i.id()).map(|t| t.id), self.tol());
                if core.follow.contains_key(&i.id()) {
                    v["follow_pending"] = json!(true);
                }
                v
            })
            .collect();
        json!({
            "intents": intents,
            "audit": {
                "path": self.audit_path,
                "head": core.engine.head_hash(),
                "anchors": self.anchor_path(),
            },
        })
    }

    /// `GET /api/intents/{id}`: the intent (with a collect's or cover's whole space answer)
    /// and, once dispatched, its tasking with every op or its space run.
    /// The collect match tolerance for compact space answers: none without a space segment.
    fn tol(&self) -> Option<u32> {
        None
    }

    pub fn intent_json(&self, id: u64) -> Option<Value> {
        let core = self.lock();
        let id = IntentId(id);
        let i = core.engine.intent(id)?;
        let tasking = core.taskings.get(&id).map(|t| t.snapshot());
        Some(json!({ "intent": intent_json(i, None, None), "tasking": tasking }))
    }

    /// The inbox's alerts now, most severe first, then oldest first (the inbox and the
    /// agents' change feed read the same list).
    pub fn current_alerts(&self) -> Vec<alerts::Alert> {
        // each vehicle's latest op and the fleet, read once for every kind of alert below (bd
        // murmur-jvdc: Saturation's 3,600 ops were copied three times a message)
        let ops = self.world.tasks.current_ops();
        let cells = self.world.vehicles();
        let fleet: BTreeMap<Uid, Arc<VehicleCell>> = cells.iter().map(|c| (c.uid, c.clone())).collect();
        let mut alerts = alerts::alerts(&cells, &ops, clock::wall());
        {
            let core = self.lock();
            // rules 17: an rtl that went as ordered though its way home crosses land
            alerts.extend(self.route_alerts(&core, &ops, &fleet, clock::wall()));
            // rules 30: a vehicle confirmed lost has its lost link and an order that may be
            // running closed behind a notice of the loss; one that reported again after it, an
            // alert
            let lost = self.lost_uids(&core);
            if !lost.is_empty() {
                alerts.retain(|a| {
                    !(lost.contains(&a.uid) && matches!(a.kind, "lost_link" | "may_be_running" | "restore_pending"))
                });
            }
            alerts.extend(self.node_alerts(&core, clock::wall()));
        }
        // the navigation monitor acted for a vehicle whose navigation is untrusted
        alerts.extend(self.nav_alerts());
        // the issues automation raised (a drop zone that stayed not clear, a zone nobody approved);
        // a delivery op that failed on its drop-time look is said by its issue, not again as a
        // failed task
        alerts.retain(|a| !(a.kind == "task_failed" && a.detail.starts_with("drop zone not clear")));
        // an op that failed on a dispatcher's order it is about to try again (an upload or a
        // command with no answer): automation handles it, and tells the person if it cannot
        alerts.retain(|a| !(matches!(a.kind, "task_failed" | "ack_timeout") && self.dispatcher_retrying(a.uid, a.intent_id)));
        alerts.extend(self.issue_alerts());
        alerts.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.since_t.total_cmp(&b.since_t)));
        alerts
    }

    /// `GET /api/inbox`: pending approvals (soonest expiry first, with why each needs a
    /// human) and alerts (most severe first), as the person sees them (rules 7: without what the
    /// watch officer holds or handled).
    pub fn inbox_json(&self) -> Value {
        self.inbox_json_inner()
    }

    /// MCP `get_inbox`: every item, each with where it stands in the officer's triage (rules 7).
    pub fn inbox_json_for_agents(&self) -> Value {
        self.inbox_view(true)
    }

    /// `GET /api/triage` (rules 7): every item the watch officer triaged this run.
    pub fn triage_json(&self) -> Value {
        let officer = self.lock().engine.policy().officer.clone();
        self.triage.lock().unwrap_or_else(|e| e.into_inner()).json(officer.as_ref())
    }

    /// An agent made an MCP call: the watch officer's presence (rules 7).
    pub fn agent_seen(&self, agent: &fleetmesh_decide::AgentId) {
        let officer = self.lock().engine.policy().officer.as_ref().map(|o| o.agent.clone());
        if officer.as_deref() == Some(agent.as_str()) {
            self.triage.lock().unwrap_or_else(|e| e.into_inner()).seen(clock::wall());
        }
    }

    /// The watch officer's verdict on an item it holds (rules 7, docs/design/watch-officer.md
    /// section 4): logged, or refused with why.
    pub fn triage_verdict(&self, by: &fleetmesh_decide::AgentId, item: &str, v: triage::VerdictIn) -> Result<Value, String> {
        let verdict = v.verdict;
        let mut core = self.lock();
        let Some(officer) = core.engine.policy().officer.clone() else {
            return Err("this run's policy has no [officer]: nothing is triaged".into());
        };
        let now = self.clock.now();
        let ev = {
            let mut t = self.triage.lock().unwrap_or_else(|e| e.into_inner());
            t.verdict(&officer, by, item, v, now)?
        };
        core.engine.record_triage(ev).map_err(|e| e.to_string())?;
        drop(core);
        self.publish();
        Ok(json!({ "item": item, "verdict": verdict }))
    }

    /// One tick of the officer's triage (rules 7): presence, new items routed, deadlines, items
    /// that left; each step logged.
    fn step_triage(&self) {
        let officer = self.lock().engine.policy().officer.clone();
        let Some(officer) = officer else { return };
        let alerts = self.current_alerts();
        let mut core = self.lock();
        let now = self.clock.now();
        let held: Vec<_> = core.engine.inbox().into_iter().map(triage::held_item).collect();
        let alert_items: Vec<_> = alerts.iter().map(triage::alert_item).collect();
        // why an order the officer held is no longer waiting: its state now
        let left: BTreeMap<String, String> = core
            .engine
            .intents()
            .filter(|i| !matches!(i.state(), fleetmesh_decide::State::PendingApproval))
            .map(|i| (triage::intent_key(i.id()), format!("{:?}", i.state()).to_lowercase()))
            .collect();
        let events = {
            let mut t = self.triage.lock().unwrap_or_else(|e| e.into_inner());
            t.step(&officer, now, clock::wall(), triage::Items { held, alerts: alert_items, left })
        };
        for ev in events {
            if let Err(e) = core.engine.record_triage(ev) {
                eprintln!("decision: triage log: {e}");
            }
        }
    }

    /// [`Self::inbox_json`]; the decisions message takes it too (bd murmur-jvdc).
    fn inbox_json_inner(&self) -> Value {
        self.inbox_view(false)
    }

    /// The Inbox for the person (`officer` false: what the watch officer holds or handled left
    /// out, an escalated item with its recommendation) or for an agent (`officer` true: every
    /// item, each with where it stands in triage) (rules 7, [`triage`]).
    fn inbox_view(&self, officer: bool) -> Value {
        let triage = self.triage.lock().unwrap_or_else(|e| e.into_inner());
        let pending: Vec<Value> = {
            let core = self.lock();
            core.engine
                .inbox()
                .into_iter()
                .filter_map(|i| {
                    let (shown, note) = triage.view(&triage::intent_key(i.id()));
                    if !shown && !officer {
                        return None;
                    }
                    let mut v = intent_json(i, None, self.tol());
                    v["why"] = json!(why_human(i));
                    if let Some(n) = note {
                        v["triage"] = n;
                    } else if !shown {
                        v["triage"] = json!({ "stage": "with_officer" });
                    }
                    if officer && v["triage"].is_object() {
                        v["triage"]["item"] = json!(triage::intent_key(i.id()));
                    }
                    Some(v)
                })
                .collect()
        };
        let mut alerts = self.current_alerts();
        let mut alert_notes: BTreeMap<String, Value> = BTreeMap::new();
        alerts.retain(|a| {
            let key = triage::alert_key(&a.id);
            let (shown, note) = triage.view(&key);
            let note = note.or_else(|| (!shown).then(|| json!({ "stage": "with_officer" })));
            if let Some(mut n) = note {
                if officer {
                    n["item"] = json!(key);
                }
                alert_notes.insert(a.id.clone(), n);
            }
            shown || officer
        });
        let triage_counts = triage.counts();
        drop(triage);
        // rules 5: orders held for vehicles the link can't carry them to now
        let queued = {
            let core = self.lock();
            self.queued_json(&core)
        };
// rules 30: our vehicles presumed lost or flagged by hand (confirm), and those lost
        let review = {
            let core = self.lock();
            let mut review = json!({ "counts": { "node_losses": 0 } });
            if let Some((losses, lost)) = self.node_review(&core) {
                review["counts"]["node_losses"] = json!(losses.len());
                review["node_losses"] = json!(losses);
                review["nodes_lost"] = json!(lost);
            }
            review
        };
        let n_review = review["counts"]["node_losses"].as_u64().unwrap_or(0);
        let n_alerts = alerts.len();
        // the operator's load (docs/design/attention-cost.md section 4.1): the decisions a person
        // made in the last ten minutes, counted from the intents' own histories
        let decided_10m = {
            let core = self.lock();
            let from = Time(self.clock.now().0 - DECIDED_WINDOW_MS);
            core.engine.intents().map(|i| decisions_since(i.history(), from)).sum::<usize>()
        };
        // swarms (murmur-z07i): many members' alerts of one kind fold into one card, a view
        // only (each member's alert stays, marked `folded_into`); a manifest without swarms
        // leaves the inbox as it was
        let groups = self.group_views();
        let (mut alerts, swarm_alerts) = match crate::swarms::inbox_view(&self.world, &alerts, &groups, true) {
            Some((list, folds)) => (json!(list), Some(folds)),
            None => (json!(alerts), None),
        };
        if let Some(list) = alerts.as_array_mut() {
            for a in list {
                if let Some(n) = a["id"].as_str().and_then(|id| alert_notes.get(id)) {
                    a["triage"] = n.clone();
                }
            }
        }
        let mut v = json!({
            "pending": pending,
            "alerts": alerts,
            "queued": queued,
            "review": review,
            "counts": {
                "pending": pending.len(),
                "alerts": n_alerts,
                "queued": queued.len(),
                "review": n_review,
                "decided_10m": decided_10m,
                "triage": triage_counts,
            },
        });
        if let Some(folds) = swarm_alerts {
            v["counts"]["swarm_alerts"] = json!(folds.len());
            v["swarm_alerts"] = json!(folds);
        }
        v
    }

    /// The WebSocket message (`type: "decisions"`): the inbox and the latest intents.
    fn message(&self) -> String {
        // its parts timed in /api/stats `loops` (bd murmur-jvdc)
        let inbox = crate::perf::time("decisions_message.inbox", || self.inbox_json_inner());
        let intents_start = std::time::Instant::now();
        let intents: Vec<Arc<str>> = {
            let core = self.lock();
            let mut texts = self.intent_texts.lock().unwrap_or_else(|e| e.into_inner());
            let all: Vec<&Intent> = core.engine.intents().collect();
            let shown: Vec<&Intent> = all.into_iter().rev().take(PUSH_INTENTS).collect();
            let out = shown
                .iter()
                .map(|i| {
                    let tasking = core.taskings.get(&i.id()).map(|t| t.id);
                    // resumed under way, waiting for its vehicles to connect to be followed
                    let follow = core.follow.contains_key(&i.id());
                    if let Some(text) = texts.get(i, tasking, follow) {
                        // a test build writes it afresh
                        #[cfg(debug_assertions)]
                        {
                            let mut v = intent_json(i, tasking, self.tol());
                            if follow {
                                v["follow_pending"] = json!(true);
                            }
                            assert_eq!(*text, v.to_string(), "{}: the decisions message kept a stale intent", i.id());
                        }
                        return text;
                    }
                    let mut v = intent_json(i, tasking, self.tol());
                    if follow {
                        v["follow_pending"] = json!(true);
                    }
                    let text: Arc<str> = Arc::from(v.to_string());
                    texts.put(i, tasking, follow, text.clone());
                    text
                })
                .collect();
            texts.keep_only(&shown.iter().map(|i| i.id()).collect());
            out
        };
        crate::perf::record("decisions_message.intents", intents_start, intents_start.elapsed());
        // the space runs went with the removed space segment: none
        let space = json!({ "runs": [] });
        // as `json!({"type": "decisions", "inbox", "intents", "space", "operators", "spent"})`
        // writes it, around the intents' kept texts
        message::text(&inbox, &intents, &space, &self.operators_json())
    }

    /// Push the inbox to WebSocket clients if it changed. Built only while anyone watches, as
    /// `push::pusher`'s frames are, and for a newcomer at [`Self::subscribe`] (murmur-4yj: the
    /// headless simulator never has a console).
    pub fn publish(&self) {
        if self.updates.receiver_count() == 0 {
            return;
        }
        let msg = crate::perf::time("decisions_message", || self.message());
        self.updates.send_if_modified(|cur| {
            if **cur == msg {
                false
            } else {
                *cur = Arc::new(msg);
                true
            }
        });
    }

    /// A WebSocket client's view of the decisions: the message as of now first (none is built
    /// while no one watches), then every change.
    pub fn subscribe(&self) -> watch::Receiver<Arc<String>> {
        let rx = self.updates.subscribe();
        self.publish();
        rx
    }
}

/// An intent for lists, the inbox and the WebSocket: a collect's or cover's space answer
/// (thousands of exclusions, possibly) is cut down to [`space::compact`] when
/// `compact_space` gives the match tolerance.
fn intent_json(i: &Intent, tasking_id: Option<u64>, _compact_space: Option<u32>) -> Value {
    let mut v = serde_json::to_value(i).unwrap_or(Value::Null);
    if let Some(t) = tasking_id {
        v["tasking_id"] = json!(t);
    }
    v
}

/// For an intent its submitter authorized at submission: who, which policy tier asked for
/// a human, and why (the inbox's reasons). The console notes it in the command log
/// ("authorized by you (armed platform)").
pub fn authorization_json(i: &Intent) -> Option<Value> {
    let a = i.approval().filter(|a| a.at_submission)?;
    let p = i.policy()?;
    Some(json!({
        "by": a.by,
        "at_submission": true,
        "self_approved": a.self_approved,
        "tier": p.rule,
        "reason": p.reason,
        "why": why_human(i),
    }))
}

/// Why an intent needs a human, for the inbox (and an at-submission authorization).
fn why_human(i: &Intent) -> Vec<String> {
    let mut why = Vec::new();
    if let Some(p) = i.policy() {
        why.push(format!("policy {}: {}", p.rule, p.reason));
        if p.approvers > 1 {
            why.push(format!("{} distinct approvers", p.approvers));
        }
        if p.facts.leaves_approved_area {
            why.push(p.facts.area_detail.clone());
        }
        why.extend(p.facts.unverified.iter().cloned());
    }
    why
}

/// The engine's proposal for a request.
fn proposal(req: &IntentRequest, places: &fleetmesh_decide::delivery::Places) -> Result<Proposal, DecisionError> {
    let bad = |m: String| DecisionError::new(400, m);
    let p = &req.params;
    let ll = |a: [f64; 2]| LonLat::new(a[0], a[1]);
    let kind = fleetmesh_decide::TaskKind::from_name(&req.task).ok_or_else(|| bad(format!("unknown task {}", req.task)))?;
    use fleetmesh_decide::TaskKind;
    let task = match kind {
        TaskKind::Orbit => Task::Orbit {
            center: ll(p
                .center
                .ok_or_else(|| bad("orbit needs center [lon, lat]".into()))?),
            radius_m: p.radius.unwrap_or(DEFAULT_ORBIT_RADIUS_M),
        },
        TaskKind::Patrol => Task::Patrol {
            route: p.route.iter().flatten().copied().map(ll).collect(),
        },
        // rules 34: along a route once, held at its end (the engine refuses it under older rules)
        TaskKind::Transit => Task::Transit {
            route: p.route.iter().flatten().copied().map(ll).collect(),
        },
        TaskKind::Hold => Task::Hold { max_s: p.max_s },
        TaskKind::Rtl => Task::Rtl {},
        // the delivery orders: a hub or a drop zone named by its id is looked up in the world's
        // places; the engine's order rules check that the id and the point agree
        TaskKind::Pickup => Task::Pickup {
            parcel: p.parcel.clone().ok_or_else(|| bad("pickup needs a parcel".into()))?,
            site: match p.site.as_ref().ok_or_else(|| bad("pickup needs a site (a hub id or [lon, lat])".into()))? {
                SiteParam::Id(id) => {
                    let h = places.hubs.get(id).ok_or_else(|| bad(format!("unknown hub {id}")))?;
                    // rules 5: where at the hub (the vehicle's stand); the engine's order rule
                    // checks it lies within the hub
                    fleetmesh_decide::Place { id: Some(id.clone()), at: p.at.map_or_else(|| h.at(), ll) }
                }
                SiteParam::Point(pt) => fleetmesh_decide::Place { id: None, at: ll(*pt) },
            },
            load_by_s: p.load_by_s,
        },
        TaskKind::Deliver => {
            let zone = p.drop_zone.clone().ok_or_else(|| bad("deliver needs a drop_zone".into()))?;
            let z = places.drop_zones.get(&zone).ok_or_else(|| bad(format!("unknown drop zone {zone}")))?;
            Task::Deliver {
                parcel: p.parcel.clone().ok_or_else(|| bad("deliver needs a parcel".into()))?,
                drop_zone: zone,
                at: z.at(),
                route: p.route.iter().flatten().copied().map(ll).collect(),
                release: parse_enum(p.release.as_deref().unwrap_or("land")).map_err(bad)?,
                window_end: p.window_end.clone(),
            }
        }
        TaskKind::Reroute => Task::Reroute {
            route: p.route.iter().flatten().copied().map(ll).collect(),
            cause: parse_enum(p.cause.as_deref().unwrap_or("operator")).map_err(bad)?,
        },
        TaskKind::ReturnToHub => Task::ReturnToHub {
            hub: match p.hub.as_ref().or(match &p.site {
                Some(SiteParam::Id(i)) => Some(i),
                _ => None,
            }) {
                Some(id) => {
                    let h = places.hubs.get(id).ok_or_else(|| bad(format!("unknown hub {id}")))?;
                    // rules 5: the stand or pad it lands on
                    Some(fleetmesh_decide::Place { id: Some(id.clone()), at: p.at.map_or_else(|| h.at(), ll) })
                }
                None => None,
            },
        },
        TaskKind::LandNow => Task::LandNow {
            site: match &p.site {
                Some(SiteParam::Point(pt)) => Some(ll(*pt)),
                Some(SiteParam::Id(id)) => {
                    Some(places.hubs.get(id).ok_or_else(|| bad(format!("unknown hub {id}")))?.at())
                }
                None => None,
            },
        },
        TaskKind::Abort => Task::Abort {
            target: IntentId(p.target.ok_or_else(|| bad("abort needs a target (the intent id to stop)".into()))?),
            then: match p.then.as_deref() {
                Some(t) => parse_enum(t).map_err(bad)?,
                None => fleetmesh_decide::AbortThen::ReturnToHub,
            },
        },
        TaskKind::SwapBattery => Task::SwapBattery {
            hub: p.hub.clone().ok_or_else(|| bad("swap_battery needs a hub".into()))?,
            pad: p.pad.clone(),
        },
        TaskKind::ConfirmDropZone => Task::ConfirmDropZone {
            drop_zone: p.drop_zone.clone().ok_or_else(|| bad("confirm_drop_zone needs a drop_zone".into()))?,
            parcel: p.parcel.clone().ok_or_else(|| bad("confirm_drop_zone needs a parcel".into()))?,
            evidence: p.evidence.clone().unwrap_or_default(),
        },
    };
    let mut proposal = Proposal::new(task, req.uids.iter().map(|u| duid(*u)).collect());
    proposal.constraints = req.constraints.clone();
    proposal.group = req.group.clone();
    proposal.rationale = req
        .rationale
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(Rationale::new)
        .transpose()
        .map_err(|e| bad(e.to_string()))?;
    proposal.validate().map_err(|e| bad(e.to_string()))?;
    Ok(proposal)
}

/// What the delivery rules read that the bridge does not measure itself. Empty until something
/// feeds it: an input that was never given reaches the rules as unknown, never as a default.
#[derive(Default)]
pub struct DeliveryFeed {
    pub parcels: BTreeMap<String, fleetmesh_decide::delivery::ParcelFact>,
    pub weather: Vec<fleetmesh_decide::delivery::WxFact>,
    pub observations: BTreeMap<String, fleetmesh_decide::delivery::DzObservation>,
    pub link: BTreeMap<DUid, fleetmesh_decide::delivery::LinkAlong>,
    /// Charged packs now at each hub (`swap_battery` takes one; it charges again); a hub not
    /// here has its file's count.
    pub hub_packs: BTreeMap<String, u32>,
    /// Each vehicle's navigation trust as the monitor holds it and how long better conditions
    /// have held (the hysteresis the rules read).
    pub nav_hysteresis: BTreeMap<DUid, (fleetmesh_decide::delivery::NavTrust, f64)>,
    /// Rules 5: each vehicle's dispatched route (a delivery, a return, a reroute, a transit), its
    /// points in travel order, for the separation rule's prediction of the fleet's own vehicles
    /// (the part still ahead of each is cut at each decision).
    pub routes: BTreeMap<DUid, Vec<LonLat>>,
}

/// The world's hubs and drop zones: `FLEETMESH_HUBS`, else `sim/hubs.toml` under the root; none
/// when the file does not exist (a world with no places refuses the orders that name one).
fn load_places(world: &World) -> Result<fleetmesh_decide::delivery::Places, String> {
    // a scenario carries its own hubs and drop zones in its dispatch file (beside its parcels)
    if let Ok(dir) = std::env::var("FLEETMESH_SCENARIO_DIR") {
        let dir = PathBuf::from(dir);
        let dir = if dir.is_absolute() { dir } else { world.cfg.root.join(dir) };
        let path = dir.join("dispatch.toml");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut all: toml::Table = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut only = toml::Table::new();
        for k in ["hub", "dropzone"] {
            if let Some(v) = all.remove(k) {
                only.insert(k.into(), v);
            }
        }
        let only = toml::to_string(&only).map_err(|e| e.to_string())?;
        return fleetmesh_decide::delivery::Places::from_toml(&path.display().to_string(), &only).map_err(|e| e.to_string());
    }
    let path = std::env::var("FLEETMESH_HUBS").map_or_else(|_| world.cfg.root.join("sim/hubs.toml"), PathBuf::from);
    if !path.exists() {
        return Ok(fleetmesh_decide::delivery::Places::default());
    }
    fleetmesh_decide::delivery::Places::load(&path).map_err(|e| e.to_string())
}

/// A delivery order the task layer has no kind of its own for (it flies them as its transit, hold
/// and rtl): the decision layer and the engine take its parameters.
pub(crate) fn is_delivery_task(task: &str) -> bool {
    matches!(
        task,
        "pickup" | "deliver" | "reroute" | "return_to_hub" | "land_now" | "abort" | "swap_battery" | "confirm_drop_zone"
    )
}

fn task_kind_name(t: &Task) -> &'static str {
    t.kind().as_str()
}

/// An enum parameter given as its snake_case name.
fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(s.to_string())).map_err(|_| format!("{s:?} is not one of the values this takes"))
}

/// The task layer's request for an engine task (the inverse of [`proposal`]).
fn task_request(task: &Task, uids: &[DUid]) -> Result<TaskRequest, String> {
    let uid = |u: &DUid| bridge_uid(u).ok_or_else(|| format!("vehicle {u} has no bridge uid"));
    let uids = uids.iter().map(uid).collect::<Result<Vec<_>, _>>()?;
    let mut params = TaskParams::default();
    let name = match task {
        Task::Orbit { center, radius_m } => {
            params.center = Some([center.lon, center.lat]);
            params.radius = Some(*radius_m);
            "orbit"
        }
        Task::Patrol { route } => {
            params.route = Some(route.iter().map(|p| [p.lon, p.lat]).collect());
            "patrol"
        }
        // rules 34: flown once, held at its end
        Task::Transit { route } => {
            params.route = Some(route.iter().map(|p| [p.lon, p.lat]).collect());
            "transit"
        }
        Task::Hold { .. } => "hold",
        Task::Rtl {} => "rtl",
        // the delivery orders are flown as the task layer's own: a pickup, a deliver and a
        // reroute as a transit to the end of their route, a return to a hub as a transit to it
        // (an rtl where no hub is named: the vehicle's own way home), a land_now as the
        // platform's safe stop (a hold: the task layer has no land mode yet), an abort as what
        // it does next
        Task::Pickup { site, parcel, .. } => {
            params.route = Some(vec![[site.at.lon, site.at.lat]]);
            params.action = Some("pickup".into());
            params.parcel = Some(parcel.clone());
            "transit"
        }
        Task::Deliver { route, at, parcel, drop_zone, release, .. } => {
            let mut r: Vec<LngLat> = route.iter().map(|p| [p.lon, p.lat]).collect();
            r.push([at.lon, at.lat]);
            params.route = Some(r);
            params.action = Some("deliver".into());
            params.parcel = Some(parcel.clone());
            params.drop_zone = Some(drop_zone.clone());
            params.release = Some(
                serde_json::to_value(release)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
            );
            "transit"
        }
        Task::Reroute { route, .. } => {
            params.route = Some(route.iter().map(|p| [p.lon, p.lat]).collect());
            "transit"
        }
        Task::ReturnToHub { hub: Some(h) } => {
            params.route = Some(vec![[h.at.lon, h.at.lat]]);
            params.action = Some("home".into());
            "transit"
        }
        Task::ReturnToHub { hub: None } => "rtl",
        // the platform's own way down: a land mode, a landing mission, or a stop
        Task::LandNow { .. } => "land",
        Task::Abort { then, .. } => match then {
            fleetmesh_decide::AbortThen::ReturnToHub => "rtl",
            fleetmesh_decide::AbortThen::Hold => "hold",
            fleetmesh_decide::AbortThen::LandNow => "land",
        },
        // the hub swaps the battery (its time, its pack); a person confirms a place
        Task::SwapBattery { hub, .. } => {
            params.hub = Some(hub.clone());
            "swap"
        }
        Task::ConfirmDropZone { .. } => {
            return Err(format!("{} has no vehicle op", task_kind_name(task)));
        }
    };
    Ok(TaskRequest {
        task: name.into(),
        uids,
        params,
    })
}

/// Rules 22: the legs the planner routed, or could not, as the dry run, `preview_intent` and the
/// map read them: by vehicle uid, each leg's ends, what it met, the waypoints it adds, how much
/// longer it is and a sentence (the grid it was planned on stays in the decision's log).
pub(super) fn routes_json(plans: &BTreeMap<DUid, Vec<fleetmesh_decide::plan::PlannedLeg>>) -> Value {
    let mut out = serde_json::Map::new();
    for (u, legs) in plans {
        let legs: Vec<Value> = legs
            .iter()
            .map(|p| {
                json!({
                    "leg": p.leg,
                    "from": p.from,
                    "to": p.to,
                    "met": p.met,
                    "keeps": p.keeps,
                    "via": p.via,
                    "no_route": p.no_route,
                    "added_m": p.added_m.round(),
                    "summary": p.summary(),
                })
            })
            .collect();
        out.insert(u.to_string(), Value::Array(legs));
    }
    Value::Object(out)
}

#[cfg(test)]
mod decided_tests {
    use super::*;
    use fleetmesh_decide::intent::StateChange;

    fn sc(state: State, t: i64) -> StateChange {
        StateChange { state, t: Time(t), reason: String::new() }
    }

    #[test]
    fn only_a_persons_decision_inside_the_window_counts() {
        let h = vec![sc(State::Proposed, 0), sc(State::Checked, 1), sc(State::PendingApproval, 2), sc(State::Approved, 500)];
        assert_eq!(decisions_since(&h, Time(0)), 1);
        assert_eq!(decisions_since(&h, Time(501)), 0, "before the window");
        let rejected_by_checks = vec![sc(State::Proposed, 0), sc(State::Checked, 1), sc(State::Rejected, 2)];
        assert_eq!(decisions_since(&rejected_by_checks, Time(0)), 0);
        let expired = vec![sc(State::Checked, 1), sc(State::PendingApproval, 2), sc(State::Expired, 9)];
        assert_eq!(decisions_since(&expired, Time(0)), 0);
        let no = vec![sc(State::PendingApproval, 2), sc(State::Rejected, 3)];
        assert_eq!(decisions_since(&no, Time(0)), 1);
    }
}
