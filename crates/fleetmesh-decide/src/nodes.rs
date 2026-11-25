//! Rules 30 ([`crate::rules::NODE_LOSS_SINCE`], bd murmur-zb1d): losing one of our vehicles. From
//! the C2's seat a destroyed vehicle and a cut link look the same at first: silence. A vehicle
//! whose position is stale is **silent** (nothing new has been heard from it); a silent vehicle
//! with evidence is **presumed lost** (`node.presumed`, a flag that decides nothing); one an
//! operator with consequential authority confirmed lost on the silence they saw is **lost**
//! (`node.lost`): it leaves the friendly keep-outs, takes no order, and its orders end. Any
//! report from it revokes the loss (`node.loss_revoked`, cause `heard`) before a decision reads
//! that report; any operator may revoke it too (it only restricts).
//!
//! This module is the ledger the engine and replay both keep of it, built from the log's events
//! alone ([`NodeLedger::step`]), and the judgements on a vehicle's facts both make: whether it is
//! silent ([`silent`]), heard again since a report ([`heard_since`]), and what evidence presumes
//! it lost: damage it reported of itself before going silent (rules 25's fact). Silence alone
//! never presumes anything.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::audit::EventBody;
use crate::ids::{OperatorId, SessionId, Uid};
use crate::rules::RulesConfig;
use crate::time::Time;
use crate::world::{LostNode, VehicleFacts};

/// How far a logged world may be from the event that read it, ms (a presumption or a loss
/// confirmation is made on the world of the moment).
pub const WORLD_SKEW_MS: i64 = 5_000;

/// The op state a dispatched order's op on a vehicle confirmed lost ends in.
pub const LOST_OP: &str = "lost";

/// The reason an intent that names a vehicle confirmed lost is voided (or ends) with.
pub const LOST_REASON: &str = "vehicle confirmed lost";

/// Whether `v` is silent at `t`: it never reported a position, or its last one is stale by the
/// link-aware age limit the checks use ([`RulesConfig::fact_age_limit_ms`], as `rules::fresh`
/// does).
pub fn silent(v: &VehicleFacts, t: Time, cfg: &RulesConfig) -> bool {
    match &v.position {
        None => true,
        Some(p) => t.ms_since(p.t) > cfg.fact_age_limit_ms(v.report_interval_ms).0,
    }
}

/// Why `uid` cannot be confirmed lost at `t`, or Ok: it is one of ours in the world model, its
/// position is not fresh by the link-aware age limit the checks use, and the last one it reported
/// is at `since` (none: it never reported one), the silence the review showed.
pub fn lost_since_report(
    v: Option<&VehicleFacts>,
    since: Option<Time>,
    t: Time,
    cfg: &RulesConfig,
) -> Result<(), String> {
    let Some(v) = v else {
        return Err("not one of ours in the world model".into());
    };
    let last = v.position.as_ref().map(|p| p.t);
    if let Some(at) = last {
        let (max_ms, _) = cfg.fact_age_limit_ms(v.report_interval_ms);
        if t.ms_since(at) <= max_ms {
            return Err("its position is fresh: it is not lost".into());
        }
    }
    if last != since {
        return Err(format!(
            "lost since {}, not {}: confirm what the review shows",
            last.map_or("none".to_string(), |t| t.0.to_string()),
            since.map_or("none".to_string(), |t| t.0.to_string())
        ));
    }
    Ok(())
}

/// Why `v` counts as heard again at `t` since its report at `since` (none: it had reported no
/// position) when it was last heard at `heard` (its heartbeat, position or navigation status:
/// [`VehicleFacts::heard_t`]), if it does: a position newer than `since`, its position fresh, or
/// anything heard after `heard`. Any report under the uid counts, whatever vehicle sends it (fail
/// closed: the protection comes back). None: still silent since then.
pub fn heard_since(v: &VehicleFacts, since: Option<Time>, heard: Option<Time>, t: Time, cfg: &RulesConfig) -> Option<String> {
    if let Some(p) = &v.position {
        if since.is_none_or(|s| p.t > s) {
            return Some(format!("it reported a position at {} ms, after the report at {}", p.t.0, at(since)));
        }
        let (limit, _) = cfg.fact_age_limit_ms(v.report_interval_ms);
        if t.ms_since(p.t) <= limit {
            return Some(format!("its position is fresh ({:.1} s old)", t.ms_since(p.t) as f64 / 1000.0));
        }
    }
    if let Some(h) = v.heard_t()
        && heard.is_none_or(|b| h > b)
    {
        return Some(format!("it was heard at {} ms, after {}", h.0, at(heard)));
    }
    None
}

fn at(t: Option<Time>) -> String {
    t.map_or("none".to_string(), |t| format!("{} ms", t.0))
}

/// One item of evidence, cited by the event it is in (`seq`; none: a fact of the vehicle's own,
/// logged with the presumption), with its time and what it says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub t: Time,
    pub detail: String,
}

/// A presumption in force (`node.presumed`): its last report then (`since`), when it was last
/// heard then (`heard`), the evidence and the world's time.
#[derive(Clone, Debug, PartialEq)]
pub struct Presumption {
    pub since: Option<Time>,
    pub heard: Option<Time>,
    pub evidence: Vec<Evidence>,
    pub t: Time,
}

/// A loss in force: confirmed in this log (`node.lost`, `from` none) or carried over from an
/// earlier one (`run.started`'s `carried_from`, `nodes_lost`; `from` that log). Lost since its
/// report at `since` (none: it never reported a position), last heard at `heard` then, confirmed
/// by `by` at `t`, on the vehicle `vehicle` (platform, callsign and system id, as the bridge knew
/// it), with the lost-link values its confirmation closed (`restore.closed` lost: owed back again
/// if it is heard). What a later start carries ([`NodeLedger::marks`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LostMark {
    pub uid: Uid,
    #[serde(default)]
    pub since: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard: Option<Time>,
    pub by: OperatorId,
    pub t: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, f64>,
}

/// How a loss was revoked (`node.loss_revoked`'s `cause`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Revocation {
    /// The vehicle was heard again: its facts at `world_t` (replay recomputes it), logged before
    /// any decision reads the report.
    Heard {
        world_t: Time,
        vehicle: Box<VehicleFacts>,
        detail: String,
    },
    /// An operator revoked it, under their own session, saying why (any operator may: it only
    /// restricts).
    Operator {
        operator: OperatorId,
        session: SessionId,
        note: String,
    },
}

impl Revocation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Revocation::Heard { .. } => "heard",
            Revocation::Operator { .. } => "operator",
        }
    }
}

/// Our vehicles' standing in force. The engine steps it with every event it writes, replay with every event it reads.
#[derive(Clone, Debug, Default)]
pub struct NodeLedger {
    presumed: BTreeMap<Uid, Presumption>,
    lost: BTreeMap<Uid, LostMark>,
}

impl NodeLedger {
    /// The ledger after one logged event (`seq` at `t`), as logged: replay checks it.
    pub fn step(&mut self, _seq: u64, t: Time, body: &EventBody) {
        match body {
            // what the start carried (newest log first: its standing stands)
            EventBody::RunStarted { carried_from, .. } => {
                for c in carried_from.iter().filter(|c| c.error.is_none()) {
                    for m in &c.nodes_lost {
                        let mut m = m.clone();
                        m.from.get_or_insert_with(|| c.log.clone());
                        self.lost.entry(m.uid.clone()).or_insert(m);
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
                self.presumed.insert(
                    uid.clone(),
                    Presumption {
                        since: *since,
                        heard: vehicle.heard_t(),
                        evidence: evidence.clone(),
                        t: *world_t,
                    },
                );
            }
            EventBody::NodePresumptionCleared { uid, .. } => {
                self.presumed.remove(uid);
            }
            EventBody::NodeLost {
                uid,
                since,
                by,
                vehicle,
                ..
            } => {
                self.presumed.remove(uid);
                self.lost.insert(
                    uid.clone(),
                    LostMark {
                        uid: uid.clone(),
                        since: *since,
                        heard: vehicle.heard_t(),
                        by: by.clone(),
                        t,
                        from: None,
                        values: BTreeMap::new(),
                    },
                );
            }
            EventBody::NodeLossRevoked { uid, .. } => {
                self.lost.remove(uid);
            }
            _ => {}
        }
    }

    /// The evidence that presumes a vehicle lost on its facts `v` (logged with the presumption):
    /// damage it reported of itself (its propulsion lost or its payload out of action) before it
    /// went silent. Empty: nothing presumes it lost (silence alone never does).
    pub fn evidence(&self, _uid: &Uid, v: &VehicleFacts) -> Vec<Evidence> {
        let mut out = Vec::new();
        if let Some(d) = v.damage.as_ref().filter(|d| d.value.mobility || d.value.payload_loss) {
            out.push(Evidence {
                seq: None,
                t: d.t,
                detail: format!("it reported {}", d.value.describe()),
            });
        }
        out
    }

    /// The presumption in force on `uid`, if any.
    pub fn presumption(&self, uid: &Uid) -> Option<&Presumption> {
        self.presumed.get(uid)
    }

    /// Every presumption in force.
    pub fn presumptions(&self) -> &BTreeMap<Uid, Presumption> {
        &self.presumed
    }

    /// The loss in force on `uid`, if any (this log's or carried over).
    pub fn lost_mark(&self, uid: &Uid) -> Option<&LostMark> {
        self.lost.get(uid)
    }

    /// Every loss in force, this log's (`from` none) and those carried over: what a later start
    /// carries (`carried_from`'s `nodes_lost`).
    pub fn marks(&self) -> Vec<LostMark> {
        self.lost.values().cloned().collect()
    }


    /// The losses a decision reads (its `nodes_lost` fact): each in force on a vehicle of
    /// `vehicles` that has not been heard since on those facts at `t` (one heard is revoked before
    /// the decision is logged).
    pub fn lost_for(&self, vehicles: &BTreeMap<Uid, VehicleFacts>, t: Time, cfg: &RulesConfig) -> BTreeMap<Uid, LostNode> {
        self.lost
            .iter()
            .filter_map(|(u, m)| {
                let v = vehicles.get(u)?;
                if heard_since(v, m.since, m.heard, t, cfg).is_some() {
                    return None;
                }
                Some((u.clone(), m.node()))
            })
            .collect()
    }

    /// The losses in force that `v` (the facts a decision or step reads for `uid`) shows heard
    /// again, with why: what the engine revokes before that decision is logged.
    pub fn heard(&self, uid: &Uid, v: &VehicleFacts, t: Time, cfg: &RulesConfig) -> Option<(LostMark, String)> {
        let m = self.lost.get(uid)?;
        heard_since(v, m.since, m.heard, t, cfg).map(|why| (m.clone(), why))
    }
}

impl LostMark {
    /// The decision fact for it ([`LostNode`]).
    pub fn node(&self) -> LostNode {
        LostNode {
            since: self.since,
            by: self.by.clone(),
            t: self.t,
            from: self.from.clone(),
        }
    }
}

impl LostNode {
    /// Whether it holds the vehicle under the uid now: one confirmed in this run does; one carried
    /// over from an earlier run only when the vehicle under the uid is shown to be the one lost
    /// (platform, callsign and system id, never the home: [`VehicleIdentity::same_vehicle`]).
    pub fn holds(&self) -> bool {
        self.other_vehicle().is_none()
    }

    /// Why a carried loss does not hold the vehicle under the uid now, if it does not.
    pub fn other_vehicle(&self) -> Option<String> {
        self.from.as_ref()?;
        // FLEETMESH-REVIEW: neutral behaviour (a carried loss is taken to hold the vehicle).
        None
    }

    /// In words, for a check's detail.
    pub fn describe(&self) -> String {
        let carried = self.from.as_ref().map(|f| format!(", carried over from {f}")).unwrap_or_default();
        format!("confirmed lost by {} at {} ms (silent since {}){carried}", self.by, self.t.0, at(self.since))
    }
}
