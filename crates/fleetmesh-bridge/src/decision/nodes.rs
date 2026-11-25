//! Rules 30 on the bridge (docs/NODE-LOSS.md sections 2, 4 and 5, bd murmur-zb1d.5): one of ours
//! silent, presumed lost, confirmed lost. The engine keeps the standing (`fleetmesh_decide::nodes`);
//! this module runs its step on the tick, serves the review, takes a person's confirmation or
//! revocation and carries out what the bridge owes them.
//!
//! - **The tick.** [`Decision::step_nodes`] hands the engine our vehicles' facts now
//!   (`Engine::step_nodes`: a loss whose vehicle is heard again is revoked, a presumption raised
//!   or cleared). A loss gone since the last look that no operator revoked was revoked by a
//!   report: the inbox says so (`node_reported_again`, high) and the lost-link values its
//!   confirmation closed are owed back again, put back when it answers.
//! - **The review.** `GET /api/review` (and the inbox's `review`) carries `node_losses`: the
//!   vehicles the engine presumes lost and the silent ones an operator flagged by hand, each with
//!   its evidence and times, what confirming it would end, and why the engine would refuse it
//!   now; and `nodes_lost`, the losses in force. Display only.
//! - **The acts.** `POST /api/review/nodes/{uid}/lost` (an operator with consequential authority,
//!   decision D1: 403 without it, 409 when the rules refuse, both before a credential is asked
//!   for, then the step-up a credentials run asks for), `.../revoke` (any operator, with a note:
//!   it only restricts), `.../flag` and `.../dismiss` (any operator: a silent vehicle put on the
//!   review list by hand, or taken off it; logged as `operator.action`, deciding nothing). An agent
//!   has no route here (403) and no MCP tool for any of them.
//!   (stage 2, `Engine::silent_regions`), taken on the tick with its step and kept until the next
//!   (read-only, never logged): on the review's items, `GET /api/nodes` (the map's SILENT regions)
//!   and agents' `standing`. A vehicle confirmed lost has none: it is out of every keep-out.
//! - **What a confirmation ends.** Its orders' ops on the vehicle stop (the engine ended them
//!   `lost`; an order no vehicle is left on ended with them), the restore of its owed values stops
//!   (closed), and its `lost_link`, `restore_pending` and `may_be_running` alerts close behind a
//!   `node_lost` alert for information.
//!
//! Before rules 30 none of it runs: the routes answer 404 and nothing is logged, shown or kept.

use std::collections::{BTreeMap, BTreeSet};

use fleetmesh_decide::{
    Clock, Evidence, LostMark, State, Task, Time, Uid as DUid, VehicleFacts,
     WorldSnapshot,
};
use serde_json::{Value, json};

use super::alerts::{Alert, Severity};
use super::snapshot::{bridge_uid, duid};
use super::{Core, Decision, DecisionError, engine_err};
use super::Actor;
use crate::vehicle::{Uid, VehicleCell};

/// How long the inbox says that a vehicle confirmed lost reported again, s.
pub(super) const REPORTED_AGAIN_ALERT_S: f64 = 300.0;

/// A loss revoked by a report from the vehicle: when the bridge saw it (wall s), the loss, and
/// why the vehicle under the uid now may not be the one confirmed lost (revocation checks no
/// identity: any report under the uid brings the protection back).
pub(super) struct ReportedAgain {
    t: f64,
    mark: LostMark,
    other: Option<String>,
}

/// The bridge's side of rules 30 (display and follow-ups; the engine's ledger decides).
#[derive(Default)]
pub(super) struct NodeView {
    /// the losses in force at the last look: one gone since that no operator revoked was revoked
    /// by a report
    seen: BTreeMap<DUid, LostMark>,
    reported_again: BTreeMap<DUid, ReportedAgain>,
    /// the note of each loss confirmed through this bridge (the log has it; display only)
    notes: BTreeMap<DUid, String>,
    /// where each silent vehicle not confirmed lost can be, as of the last tick (display only)
    regions: BTreeMap<DUid, fleetmesh_decide::silent::Region>,
    /// silent vehicles an operator put on the review list by hand (decision D2): a flag that
    /// decides nothing, dropped when the vehicle is heard again or confirmed lost
    pub(super) flags: BTreeMap<DUid, NodeFlag>,
}

/// A silent vehicle an operator flagged for review, saying why.
pub(super) struct NodeFlag {
    by: fleetmesh_decide::OperatorId,
    note: String,
    t: f64,
    since: Option<Time>,
    heard: Option<Time>,
}

/// Milliseconds of the engine's clock as the review's seconds.
fn secs(t: Time) -> f64 {
    t.0 as f64 / 1000.0
}

fn evidence_json(e: &Evidence) -> Value {
    json!({ "seq": e.seq, "t": secs(e.t), "detail": e.detail })
}

/// The vehicles a confirmation would end orders on, as the engine would at that moment: the
/// intents naming it that wait (voided), and the dispatched orders it runs (their op on it ended).
fn consequences(core: &Core, uid: &DUid) -> (Vec<u64>, Vec<u64>) {
    let names = |_task: &Task, uids: &[DUid]| uids.contains(uid);
    let mut voids = Vec::new();
    let mut ends = Vec::new();
    for i in core.engine.intents() {
        let s = i.spec();
        if names(&s.task, &s.uids)
            && matches!(i.state(), State::PendingApproval | State::AutoApproved | State::Approved | State::Queued)
        {
            voids.push(i.id().0);
        } else if i.state() == State::Dispatched
            && true
            && s.uids.contains(uid)
            && !i.ops().get(uid).is_some_and(|o| fleetmesh_decide::queue::ended_op(o))
        {
            ends.push(i.id().0);
        }
    }
    (voids, ends)
}

impl Decision {
    /// One vehicle's facts now, as a snapshot gives them: what the review reads for the handful of
    /// vehicles in it (a whole snapshot of a large fleet is not free).
    fn vehicle_now(&self, cell: &VehicleCell) -> VehicleFacts {
        let now = crate::clock::mono();
        let interval = cell.link_snapshot(now).expected_interval_ms;
        cell.read(|v| super::snapshot::vehicle_facts(v, self.facts_catalog(), now, interval, None))
    }

    /// The region silent vehicle `uid` can be in (docs/NODE-LOSS.md section 3: its order's corridor,
    /// its failsafe's way home, its drift, its top speed's reach), as of the last tick: null for one reporting, never placed, or
    /// confirmed lost.
    fn silent_region(core: &Core, uid: &DUid) -> Value {
        core.nodes.regions.get(uid).map_or(Value::Null, |r| json!(r))
    }

    // ---- the tick -------------------------------------------------------------------------

    /// Rules 30, on the tick before its decisions: our vehicles' standing on their facts now
    /// (`Engine::step_nodes`), then what the bridge owes a loss revoked by a report, and the hand
    /// flags of vehicles heard again or confirmed lost dropped.
    pub(super) fn step_nodes(&self, core: &mut Core) {
        // our vehicles alone (the engine's step reads nothing else), each with its own failsafe
        let world = super::snapshot::build(&self.world, self.facts_catalog(), &self.areas, self.clock.now(), true);
        let step = core.engine.step_nodes(&world);
        // where each silent vehicle can be now (a confirmed loss is out of every keep-out: shown
        // LOST, with no region)
        let regions = core.engine.silent_regions(&world, world.t);
        core.nodes.regions = regions.into_iter().filter(|(u, _)| core.engine.node_lost(u).is_none()).collect();
        match step {
            Ok(step) => {
                for u in &step.presumed {
                    println!("vehicle {u} presumed lost: for an operator to confirm");
                }
                for u in &step.cleared {
                    println!("vehicle {u} heard again: no longer presumed lost");
                }
            }
            Err(e) => eprintln!("decision: step_nodes: {e}"),
        }
        self.follow_revocations(core);
        let cfg = &self.rules;
        let Core { engine, nodes, .. } = core;
        nodes.flags.retain(|u, f| {
            engine.node_lost(u).is_none()
                && world
                    .vehicles
                    .get(u)
                    .is_some_and(|v| fleetmesh_decide::nodes::heard_since(v, f.since, f.heard, world.t, cfg).is_none())
        });
    }

    /// A loss in force at the last look and gone now, that no operator revoked, was revoked by a
    /// report from the vehicle (`node.loss_revoked`, cause heard: on the tick's step, or before a
    /// decision that read the report): the inbox says so, and the values its confirmation closed
    /// are owed back again (restored when it answers, as after an un-spend).
    fn follow_revocations(&self, core: &mut Core) {
        let now_marks: BTreeMap<DUid, LostMark> = core.engine.nodes_lost().into_iter().map(|m| (m.uid.clone(), m)).collect();
        let gone: Vec<LostMark> = core
            .nodes
            .seen
            .iter()
            .filter(|(u, _)| !now_marks.contains_key(*u))
            .map(|(_, m)| m.clone())
            .collect();
        let wall = crate::clock::wall();
        for m in gone {
            if bridge_uid(&m.uid).is_none() {
                continue;
            }
            println!(
                "vehicle {} reported again after it was confirmed lost by {}: the loss is revoked, it takes orders again",
                m.uid, m.by
            );
            core.nodes.notes.remove(&m.uid);
            core.nodes.reported_again.insert(m.uid.clone(), ReportedAgain { t: wall, mark: m, other: None });
        }
        // confirmed lost again: its notice of the last report goes
        for u in now_marks.keys() {
            core.nodes.reported_again.remove(u);
        }
        core.nodes.reported_again.retain(|_, r| wall - r.t < REPORTED_AGAIN_ALERT_S);
        core.nodes.seen = now_marks;
    }

    // ---- the acts -------------------------------------------------------------------------

    /// Rules 30: what a passkey step-up for confirming `uid` lost on its silence since `since`
    /// is for (`/api/auth/step_up {"act": "node_lost", "uid": n, "since": ms}`).
    pub fn node_lost_purpose(&self, uid: Uid, since: Option<Time>) -> Result<fleetmesh_decide::StepUpFor, DecisionError> {
        Ok(self.lock().engine.confirm_lost_purpose(&duid(uid), since))
    }

    /// Whether `by` may confirm `uid` lost on its silence since `since` with `note`, as a dry
    /// check that logs nothing: 403 without the authority the roster gives, 409 where the engine
    /// would refuse it (no note where nothing presumes it lost). The API answers these before it
    /// asks for a credential, so a refused confirmation never takes a password.
    pub fn node_lost_check(&self, by: &Actor, uid: Uid, since: Option<Time>, note: &str) -> Result<(), DecisionError> {
        let mut guard = self.lock();
        let core = &mut *guard;
        let world = self.snapshot();
        let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
        if session.consequential_authority().is_none() {
            return Err(no_authority(by));
        }
        match core.engine.confirm_lost_refusal(&duid(uid), since, note, &world) {
            Some(why) => Err(DecisionError::new(409, format!("vehicle {uid}: {why}"))),
            None => Ok(()),
        }
    }

    /// `POST /api/review/nodes/{uid}/lost`: an operator with the roster's authority confirms one
    /// of ours lost on the silence they saw (since its report at `since`), under their own
    /// session, with the step-up a credentials run asks for, saying why where nothing presumes it
    /// lost. Logged as `node.lost`, then its consequences (the engine's); the bridge stops its
    /// orders' ops on the vehicle.
    pub fn confirm_node_lost(
        &self,
        by: &Actor,
        uid: Uid,
        since: Option<Time>,
        note: &str,
        step: Option<crate::auth::StepUpChecked>,
    ) -> Result<Value, DecisionError> {
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let world = self.snapshot();
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            let Some(authority) = session.consequential_authority() else {
                return Err(no_authority(by));
            };
            // refused before a step-up is logged for it when the rules refuse it anyway
            if let Some(why) = core.engine.confirm_lost_refusal(&duid(uid), since, note, &world) {
                return Err(DecisionError::new(409, format!("vehicle {uid}: {why}")));
            }
            let step = Self::mint_step(&mut core.engine, session, by, step)?;
            let outcome = core
                .engine
                .confirm_lost(authority, duid(uid), since, note, &world, step)
                .map_err(engine_err)?;
            println!(
                "vehicle {uid} confirmed lost by {by}: {} order(s) voided, {} ended",
                outcome.voided.len(),
                outcome.ended.len()
            );
            self.follow_loss(core, uid, &outcome);
            core.nodes.flags.remove(&duid(uid));
            if !note.trim().is_empty() {
                core.nodes.notes.insert(duid(uid), note.trim().to_string());
            }
            if let Some(m) = core.engine.node_lost(&duid(uid)) {
                core.nodes.seen.insert(duid(uid), m.clone());
            }
            core.nodes.reported_again.remove(&duid(uid));
            json!({ "lost": {
                "uid": uid,
                "since": since.map(|t| t.0),
                "voided": outcome.voided.iter().map(|i| i.0).collect::<Vec<_>>(),
                "ended": outcome.ended.iter().map(|i| i.0).collect::<Vec<_>>(),
                "finished": outcome.finished.iter().map(|i| i.0).collect::<Vec<_>>(),
            } })
        };
        self.publish();
        Ok(out)
    }

    /// What the bridge owes a confirmation: each order the engine ended on the vehicle has its op
    /// there stopped (the executor aborted, the op shown superseded, never reported to the engine
    /// again: the engine ended it `lost`), and an order no vehicle is left on is no longer
    /// followed (the engine ended it).
    fn follow_loss(&self, core: &mut Core, uid: Uid, out: &fleetmesh_decide::LossOutcome) {
        let why = format!("vehicle {uid} confirmed lost: {}", fleetmesh_decide::nodes::LOST_REASON);
        for id in &out.ended {
            if let Some(run) = core.running.get_mut(id) {
                for op in run.tasking.ops.iter().filter(|o| o.uid == uid) {
                    op.supersede(why.clone());
                    run.lost.insert(op.id);
                }
            }
        }
        for id in &out.finished {
            core.running.remove(id);
            core.follow.remove(id);
        }
    }

    /// `POST /api/review/nodes/{uid}/revoke`: any operator revokes the loss of `uid` in force,
    /// saying why (it only restricts: the vehicle takes orders again). Logged as
    /// `node.loss_revoked`, cause operator. No step-up.
    pub fn revoke_node_lost(&self, by: &Actor, uid: Uid, note: &str) -> Result<Value, DecisionError> {
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            core.engine
                .revoke_lost(session.authority(), &duid(uid), note)
                .map_err(engine_err)?;
            println!("vehicle {uid}: its loss revoked by {by}");
            // not a report: no notice of one
            core.nodes.seen.remove(&duid(uid));
            core.nodes.notes.remove(&duid(uid));
            json!({ "revoked": { "uid": uid } })
        };
        self.publish();
        Ok(out)
    }

    /// `POST /api/review/nodes/{uid}/flag`: any operator puts a silent vehicle on the review list
    /// by hand, saying why (a crew's report, a picture from outside), for an operator with the
    /// roster's authority to confirm or not. It decides nothing (logged as `operator.action`
    /// `node.flagged`). 409 for a vehicle that is reporting or lost.
    pub fn flag_node(&self, by: &Actor, uid: Uid, note: &str) -> Result<Value, DecisionError> {
        let note = note.trim();
        if note.is_empty() {
            return Err(DecisionError::new(422, "say why you think it lost (a note)"));
        }
        let Some(cell) = self.fleet().get(&uid).cloned() else {
            return Err(DecisionError::new(404, format!("no vehicle {uid}")));
        };
        let facts = self.vehicle_now(&cell);
        let t = self.clock.now();
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            if let Some(m) = core.engine.node_lost(&duid(uid)) {
                return Err(DecisionError::new(409, format!("vehicle {uid} is {} already", m.node().describe())));
            }
            if !fleetmesh_decide::nodes::silent(&facts, t, &self.rules) {
                return Err(DecisionError::new(409, format!("vehicle {uid} is reporting: it is not silent")));
            }
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            core.engine
                .record_operator_action(session.authority(), "node.flagged", &format!("vehicle {uid}: {note}"))
                .map_err(engine_err)?;
            core.nodes.flags.insert(
                duid(uid),
                NodeFlag {
                    by: by.operator().clone(),
                    note: note.to_string(),
                    t: crate::clock::wall(),
                    since: facts.position.as_ref().map(|p| p.t),
                    heard: facts.heard_t(),
                },
            );
            json!({ "flagged": { "uid": uid } })
        };
        self.publish();
        Ok(out)
    }

    /// `POST /api/review/nodes/{uid}/dismiss`: any operator takes a hand flag off the review list
    /// (logged as `operator.action` `node.flag_dismissed`). A presumption is the engine's: it
    /// goes only when the vehicle is heard again or is confirmed lost.
    pub fn dismiss_node_flag(&self, by: &Actor, uid: Uid, note: &str) -> Result<Value, DecisionError> {
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            if !core.nodes.flags.contains_key(&duid(uid)) {
                return Err(DecisionError::new(409, format!("vehicle {uid} is not flagged by hand")));
            }
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            let note = note.trim();
            let detail = if note.is_empty() { format!("vehicle {uid}") } else { format!("vehicle {uid}: {note}") };
            core.engine
                .record_operator_action(session.authority(), "node.flag_dismissed", &detail)
                .map_err(engine_err)?;
            core.nodes.flags.remove(&duid(uid));
            json!({ "dismissed": { "uid": uid } })
        };
        self.publish();
        Ok(out)
    }

/// `GET /api/nodes`: each of ours not reporting (silent, presumed lost, lost) with the region
    /// it can be in (read-only, never logged).
    pub fn nodes_json(&self) -> Result<Value, DecisionError> {
        let core = self.lock();
        let presumed = core.engine.nodes_presumed();
        let mut nodes: BTreeMap<DUid, Value> = BTreeMap::new();
        for (u, r) in &core.nodes.regions {
            let Some(uid) = bridge_uid(u) else { continue };
            let p = presumed.get(u);
            nodes.insert(
                u.clone(),
                json!({
                    "uid": uid,
                    "state": if p.is_some() { "presumed_lost" } else { "silent" },
                    "since_t": secs(r.t0),
                    "silent_s": (r.span_s * 10.0).round() / 10.0,
                    "region": r,
                }),
            );
        }
        for (u, p) in presumed {
            let Some(uid) = bridge_uid(u) else { continue };
            nodes.entry(u.clone()).or_insert_with(|| {
                json!({ "uid": uid, "state": "presumed_lost", "since_t": p.since.map(secs), "region": null })
            });
        }
        for m in core.engine.nodes_lost() {
            let Some(uid) = bridge_uid(&m.uid) else { continue };
            nodes.insert(
                m.uid.clone(),
                json!({ "uid": uid, "state": "lost", "since_t": m.since.map(secs), "by": m.by, "t": secs(m.t), "region": null }),
            );
        }
        Ok(json!({
            "t": secs(self.clock.now()),
            "nodes": nodes.into_values().collect::<Vec<_>>(),
            "note": "where each of ours not reporting can be (as of the last tick); a vehicle confirmed lost is out of every keep-out",
        }))
    }

    // ---- the reads ------------------------------------------------------------------------

    /// Rules 30: the review's node items (`node_losses`, `nodes_lost`), or none before rules 30
    /// (the review then reads as it always did).
    pub(super) fn node_review(&self, core: &Core) -> Option<(Vec<Value>, Vec<Value>)> {
        let fleet = self.fleet();
        let manifest = self.world.manifest();
        let t = self.clock.now();
        let name = |uid: Uid| -> Value {
            fleet
                .get(&uid)
                .and_then(|c| super::alerts::callsign(c))
                .or_else(|| manifest.identities.get(&uid).and_then(|i| i.str("callsign")).map(str::to_string))
                .map_or(Value::Null, Value::String)
        };
        let platform = |uid: Uid| -> Value {
            manifest
                .identities
                .get(&uid)
                .and_then(|i| i.str("platform"))
                .map(str::to_string)
                .or_else(|| fleet.get(&uid).and_then(|c| c.read(|v| v.identity.as_ref()?.str("platform").map(str::to_string))))
                .map_or(Value::Null, Value::String)
        };
        let facts_of = |uid: Uid| fleet.get(&uid).map(|c| self.vehicle_now(c));
        let silent_s = |f: Option<&VehicleFacts>| -> Value {
            f.and_then(|f| f.position.as_ref())
                .map_or(Value::Null, |p| json!((t.ms_since(p.t) as f64 / 100.0).round() / 10.0))
        };
// presumed lost, and flagged by hand
        let presumed = core.engine.nodes_presumed();
        let mut uids: BTreeSet<DUid> = presumed.keys().cloned().collect();
        uids.extend(core.nodes.flags.keys().cloned());
        let losses: Vec<Value> = uids
            .into_iter()
            .filter(|u| core.engine.node_lost(u).is_none())
            .filter_map(|u| {
                let uid = bridge_uid(&u)?;
                let facts = facts_of(uid).unwrap_or_default();
                let since = facts.position.as_ref().map(|p| p.t);
                let p = presumed.get(&u).filter(|p| p.since == since);
                let flag = core.nodes.flags.get(&u);
                let mut evidence: Vec<Value> = p.map(|p| p.evidence.iter().map(evidence_json).collect()).unwrap_or_default();
                if let Some(f) = flag {
                    evidence.push(json!({
                        "kind": "operator",
                        "t": f.t,
                        "detail": format!("flagged by {}, the operator's own: {}", f.by, f.note),
                    }));
                }
                let (voids, ends) = consequences(core, &u);
                let one = WorldSnapshot {
                    vehicles: [(u.clone(), facts.clone())].into(),
                    ..WorldSnapshot::new(t)
                };
                // what the engine would say now (a note given: whether one is needed is its own field)
                let refused = core.engine.confirm_lost_refusal(&u, since, "-", &one);
                let mut item = json!({
                    "kind": "node_loss",
                    "id": format!("N-{uid}"),
                    "uid": uid,
                    "callsign": name(uid),
                    "platform": platform(uid),
                    "state": if p.is_some() { "presumed_lost" } else { "silent" },
                    "since": since.map(|t| t.0),
                    "since_t": since.map(secs),
                    "silent_s": silent_s(Some(&facts)),
                    "last_position": facts.position.as_ref().map(|p| [p.value.lon, p.value.lat]),
                    "presumed_t": p.map(|p| secs(p.t)),
                    "evidence": evidence,
                    "needs_note": p.is_none(),
                    "would_void": voids,
                    "would_end": ends,
                    "would_close": Option::<std::collections::BTreeMap<String, f64>>::None,
                    "refused": refused,
                    "region": Self::silent_region(core, &u),
                    "actions": if flag.is_some() { json!(["confirm", "dismiss"]) } else { json!(["confirm"]) },
                });
                if let Some(f) = flag {
                    item["flagged"] = json!({ "by": f.by, "t": f.t, "note": f.note });
                }
                Some(item)
            })
            .collect();
        let lost: Vec<Value> = core
            .engine
            .nodes_lost()
            .into_iter()
            .filter_map(|m| {
                let uid = bridge_uid(&m.uid)?;
                let facts = facts_of(uid).unwrap_or_default();
                let node = m.node();
                let mut item = json!({
                    "kind": "node_lost",
                    "id": format!("L-{uid}"),
                    "uid": uid,
                    "callsign": name(uid),
                    "platform": platform(uid),
                    "since": m.since.map(|t| t.0),
                    "since_t": m.since.map(secs),
                    "silent_s": silent_s(Some(&facts)),
                    "last_position": facts.position.as_ref().map(|p| [p.value.lon, p.value.lat]),
                    "by": m.by,
                    "t": secs(m.t),
                    "note": core.nodes.notes.get(&m.uid),
                    "closed": m.values,
                    "holds": node.holds(),
                    "actions": ["revoke"],
                });
                if let Some(from) = &m.from {
                    item["from"] = json!(from);
                }
                if let Some(why) = node.other_vehicle() {
                    item["not_held"] = json!(why);
                }
                Some(item)
            })
            .collect();
        Some((losses, lost))
    }

    /// Rules 30: the inbox's alerts of our vehicles' standing: a `node_lost` notice for each loss
    /// in force (information: it asks nothing), and a `node_reported_again` alert for each loss a
    /// report revoked lately. None before rules 30.
    pub(super) fn node_alerts(&self, core: &Core, now_wall: f64) -> Vec<Alert> {
        let fleet = self.fleet();
        let callsign = |uid: Uid| fleet.get(&uid).and_then(|c| super::alerts::callsign(c));
        let mut out: Vec<Alert> = core
            .engine
            .nodes_lost()
            .into_iter()
            .filter_map(|m| {
                let uid = bridge_uid(&m.uid)?;
                let carried = m.from.as_deref().map(|f| format!(" (carried over from {f})")).unwrap_or_default();
                Some(Alert {
                    id: format!("node_lost:{uid}"),
                    kind: "node_lost",
                    severity: Severity::Info,
                    uid,
                    callsign: callsign(uid),
                    detail: format!(
                        "confirmed lost by {}{carried}: it is left out of every keep-out and takes no order; its orders ended and its owed lost-link values closed. Any report from it revokes the loss, and any operator may revoke it in the Review",
                        m.by
                    ),
                    since_t: secs(m.t).min(now_wall),
                    op_id: None,
                    intent_id: None,
                    issue: None,
                    track_id: None,
                    track_ids: Vec::new(),
                })
            })
            .collect();
        out.extend(core.nodes.reported_again.iter().filter_map(|(u, r)| {
            let uid = bridge_uid(u)?;
            let other = r
                .other
                .as_deref()
                .map(|o| format!(" It may not be the vehicle that was lost: {o}."))
                .unwrap_or_default();
            Some(Alert {
                id: format!("node_reported_again:{uid}"),
                kind: "node_reported_again",
                severity: Severity::High,
                uid,
                callsign: callsign(uid),
                detail: format!(
                    "reported again after {} confirmed it lost: the loss is revoked, it counts in every keep-out again and takes orders, and a consequential order whose picture left it out stops.{other} Its own lost-link values, if any were closed, are owed back again",
                    r.mark.by
                ),
                since_t: r.t,
                op_id: None,
                intent_id: None,
                issue: None,
                track_id: None,
                track_ids: Vec::new(),
            })
        }));
        out
    }

    /// Rules 30: the vehicles confirmed lost now (the alerts their loss closes are left out).
    pub(super) fn lost_uids(&self, core: &Core) -> BTreeSet<Uid> {
        core.engine.nodes_lost().iter().filter_map(|m| bridge_uid(&m.uid)).collect()
    }

    /// Rules 30, for agents' reads (MCP `get_fleet`, `get_vehicle`): the losses in force and the
    /// presumptions, by uid; none before rules 30.
    pub fn node_standings(&self) -> Option<NodeStandings> {
        let core = self.lock();
        Some(NodeStandings {
            lost: core.engine.nodes_lost().into_iter().map(|m| (m.uid.clone(), m)).collect(),
            presumed: core.engine.nodes_presumed().iter().map(|(u, p)| (u.clone(), p.evidence.clone())).collect(),
            regions: core.nodes.regions.clone(),
            rules: self.rules.clone(),
        })
    }

}

/// Our vehicles' standing as agents read it (rules 30): [`NodeStandings::of`].
pub struct NodeStandings {
    lost: BTreeMap<DUid, LostMark>,
    presumed: BTreeMap<DUid, Vec<Evidence>>,
    regions: BTreeMap<DUid, fleetmesh_decide::silent::Region>,
    rules: fleetmesh_decide::RulesConfig,
}

impl NodeStandings {
    /// A vehicle's standing on its facts at `t`: `lost` (confirmed, by whom, when), `presumed_lost`
    /// (the evidence), `silent` (its position stale); none while it is reporting, so a read of a
    /// reporting fleet is what it always was.
    pub fn of(&self, uid: &DUid, facts: Option<&VehicleFacts>, t: Time) -> Option<Value> {
        let since = facts.and_then(|f| f.position.as_ref()).map(|p| p.t);
        let silent_s = since.map(|s| (t.ms_since(s) as f64 / 100.0).round() / 10.0);
        if let Some(m) = self.lost.get(uid) {
            let mut v = json!({
                "state": "lost",
                "since_t": m.since.map(secs),
                "silent_s": silent_s,
                "by": m.by,
                "t": secs(m.t),
                "note": "confirmed lost by an operator with consequential authority: left out of every keep-out, takes no order; any report from it revokes the loss",
            });
            if let Some(f) = &m.from {
                v["from"] = json!(f);
            }
            return Some(v);
        }
        let region = self.regions.get(uid).map_or(Value::Null, |r| json!(r));
        if let Some(e) = self.presumed.get(uid) {
            return Some(json!({
                "state": "presumed_lost",
                "since_t": since.map(secs),
                "silent_s": silent_s,
                "evidence": e.iter().map(evidence_json).collect::<Vec<_>>(),
                "region": region,
                "note": "presumed lost by the engine on this evidence: a flag that decides nothing; it still counts in every keep-out, in its region, until an operator with consequential authority confirms it lost",
            }));
        }
        let f = facts?;
        fleetmesh_decide::nodes::silent(f, t, &self.rules).then(|| {
            json!({
                "state": "silent",
                "since_t": since.map(secs),
                "silent_s": silent_s,
                "region": region,
                "note": "silent: its position is stale; every consequential order keep-out counts it in its region (its orders' corridors, its failsafe's way home, a drift, its top speed's reach; as of the last tick)",
            })
        })
    }
}

/// 403: this operator lacks the authority the roster gives for confirming a loss.
fn no_authority(by: &Actor) -> DecisionError {
    DecisionError::new(
        403,
        format!("{by} may not confirm a vehicle lost: only an operator the roster gives that authority does"),
    )
}
