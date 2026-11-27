//! Rules 7: the AI watch officer triages the Inbox (docs/design/watch-officer.md).
//!
//! While the policy has an `[officer]` and its agent is on watch (it made an MCP call within
//! `absent_after_s`), each order waiting for a person and each alert that appears goes to the
//! officer first ([`fleetmesh_decide::officer::route`]): the person's Inbox leaves it out until the
//! officer passes it on (escalates it, with a recommendation), or its deadline passes, or the
//! officer goes off watch. What the officer handles stays out of the person's Inbox and in the
//! audit trail (`GET /api/triage`, the audit log's `triage.*` events). Items already in the Inbox
//! when the officer comes on watch stay with the person.
//!
//! Lock order: the decision lock (`Decision::lock`) before this one, never the other way (the tick
//! takes the decision lock, then this; a reader that wants both reads the engine first and lets go).
//!
//! Everything here is a view and a log: no order's state, checks or routing change, and the
//! officer approves nothing. An order it holds stays `pending_approval` and expires as before;
//! the routing makes sure it reaches the person first.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use fleetmesh_decide::officer::{ItemClass, OfficerPolicy, Release, Route, TriageInput, Verdict};
use fleetmesh_decide::{AgentId, EventBody, Intent, IntentId, Outcome, Source, Time};
use serde_json::{Value, json};

use super::alerts::Alert;

/// How many triaged items `GET /api/triage` keeps after they leave the Inbox.
const HISTORY: usize = 1000;

/// An item's key: `intent:<id>` or `alert:<id>`.
pub fn intent_key(id: IntentId) -> String {
    format!("intent:{}", id.0)
}

pub fn alert_key(id: &str) -> String {
    format!("alert:{id}")
}

/// Where an item stands in triage.
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// With the officer until `deadline`.
    Officer { deadline: Time },
    /// Straight to the person (the policy's bypass).
    Person { why: String },
    /// The officer's verdict.
    Decided { verdict: Verdict, note: String, recommendation: Option<String>, intents: Vec<IntentId>, at: Time },
    /// Reached the person without a verdict.
    Released { why: Release, at: Time },
}

#[derive(Clone, Debug)]
pub struct Record {
    pub item: String,
    pub input: TriageInput,
    pub about: Value,
    pub entered: Time,
    pub stage: Stage,
    /// When the item left the Inbox (the order decided, the alert cleared).
    pub ended: Option<(Time, String)>,
}

impl Record {
    /// Whether the person's Inbox shows it.
    pub fn shown(&self) -> bool {
        match &self.stage {
            Stage::Officer { .. } => false,
            Stage::Person { .. } | Stage::Released { .. } => true,
            Stage::Decided { verdict, .. } => *verdict == Verdict::Escalated,
        }
    }

    /// What the person's Inbox card says of it (escalated with the officer's recommendation,
    /// untriaged), or nothing.
    pub fn note(&self) -> Option<Value> {
        match &self.stage {
            Stage::Decided { verdict: Verdict::Escalated, note, recommendation, intents, at } => Some(json!({
                "stage": "escalated", "note": note, "recommendation": recommendation, "intents": intents, "at_t": at.0 as f64 / 1000.0,
            })),
            Stage::Released { why, at } => Some(json!({ "stage": "untriaged", "why": why, "at_t": at.0 as f64 / 1000.0 })),
            Stage::Person { why } => Some(json!({ "stage": "bypassed", "why": why })),
            _ => None,
        }
    }

    pub fn json(&self) -> Value {
        let (stage, extra) = match &self.stage {
            Stage::Officer { deadline } => ("with_officer", json!({ "deadline_t": deadline.0 as f64 / 1000.0 })),
            Stage::Person { why } => ("bypassed", json!({ "why": why })),
            Stage::Decided { verdict, note, recommendation, intents, at } => (
                match verdict {
                    Verdict::Handled => "handled",
                    Verdict::Escalated => "escalated",
                    Verdict::TurnedDown => "turned_down",
                },
                json!({ "note": note, "recommendation": recommendation, "intents": intents, "at_t": at.0 as f64 / 1000.0 }),
            ),
            Stage::Released { why, at } => ("untriaged", json!({ "why": why, "at_t": at.0 as f64 / 1000.0 })),
        };
        let mut v = json!({
            "item": self.item,
            "class": self.input.class,
            "kind": self.input.kind,
            "about": self.about,
            "entered_t": self.entered.0 as f64 / 1000.0,
            "stage": stage,
        });
        if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
            o.extend(e);
        }
        if let Some((t, why)) = &self.ended {
            v["ended_t"] = json!(t.0 as f64 / 1000.0);
            v["ended"] = json!(why);
        }
        v
    }
}

/// The officer's verdict on an item, as it gives it (`triage_item`).
pub struct VerdictIn {
    pub verdict: Verdict,
    /// What it did, or why no action.
    pub note: String,
    /// Its recommendation to the person (an escalation needs one).
    pub recommendation: Option<String>,
    /// The orders it proposed for the item.
    pub intents: Vec<IntentId>,
}

/// The triage this run keeps.
#[derive(Default)]
pub struct Triage {
    /// The officer's last MCP call (wall clock, s).
    last_seen: Option<f64>,
    on_watch: bool,
    /// The items in the Inbox now that triage has a record of.
    active: BTreeMap<String, Record>,
    /// The items in the Inbox when the officer came on watch: the person's already.
    before_watch: BTreeSet<String>,
    /// Triaged items that left the Inbox, newest last.
    history: VecDeque<Record>,
}

/// What one tick found: the current Inbox items, read by the decision loop.
pub struct Items {
    pub held: Vec<(String, TriageInput, Value)>,
    pub alerts: Vec<(String, TriageInput, Value)>,
    /// Why each held order is no longer waiting, for the ones that left (its state now).
    pub left: BTreeMap<String, String>,
}

/// An order waiting for a person, as triage reads it.
pub fn held_item(i: &Intent) -> (String, TriageInput, Value) {
    let spec = i.spec();
    let (source, agent) = match &spec.source {
        Source::Human { .. } => ("human", None),
        Source::Agent { id } => ("agent", Some(id.to_string())),
        Source::Handover { .. } => ("handover", None),
    };
    let codes: Vec<String> = i.checks().iter().filter(|c| c.outcome != Outcome::Pass).flat_map(|c| c.codes.clone()).collect();
    let input = TriageInput {
        class: ItemClass::Held,
        kind: spec.task.kind().as_str().to_string(),
        severity: None,
        source: Some(source.into()),
        agent,
        codes,
        expires_t: i.expires_t(),
    };
    let about = json!({ "intent": i.id(), "uids": spec.uids, "rationale": spec.rationale });
    (intent_key(i.id()), input, about)
}

/// An alert, as triage reads it.
pub fn alert_item(a: &Alert) -> (String, TriageInput, Value) {
    let input = TriageInput {
        class: ItemClass::Alert,
        kind: a.kind.to_string(),
        severity: serde_json::to_value(a.severity).ok().and_then(|v| v.as_str().map(str::to_string)),
        source: None,
        agent: None,
        codes: vec![],
        expires_t: None,
    };
    let about = json!({ "alert": a.id, "uid": a.uid, "callsign": a.callsign, "detail": a.detail, "issue": a.issue });
    (alert_key(&a.id), input, about)
}

impl Triage {
    /// The officer made an MCP call (wall clock, s).
    pub fn seen(&mut self, wall: f64) {
        self.last_seen = Some(wall);
    }

    /// One tick: presence, new items routed, deadlines, items that left. Returns the events to
    /// log, in order (the caller logs them under the decision lock).
    pub fn step(&mut self, p: &OfficerPolicy, now: Time, wall: f64, items: Items) -> Vec<EventBody> {
        let mut out = Vec::new();
        let agent = AgentId::new(&p.agent);
        // presence
        let present = self.last_seen.is_some_and(|s| wall - s <= f64::from(p.absent_after_s));
        let mut current: BTreeSet<String> = items.held.iter().chain(items.alerts.iter()).map(|(k, _, _)| k.clone()).collect();
        if present != self.on_watch {
            self.on_watch = present;
            out.push(EventBody::TriageWatch { agent: agent.clone(), on: present });
            if present {
                // what the person already has stays theirs
                self.before_watch = current.iter().filter(|k| !self.active.contains_key(*k)).cloned().collect();
            } else {
                for r in self.active.values_mut() {
                    if matches!(r.stage, Stage::Officer { .. }) {
                        r.stage = Stage::Released { why: Release::OfficerAbsent, at: now };
                        out.push(EventBody::TriageReleased { item: r.item.clone(), why: Release::OfficerAbsent });
                    }
                }
            }
        }
        self.before_watch.retain(|k| current.contains(k));
        // items that left the Inbox
        let gone: Vec<String> = self.active.keys().filter(|k| !current.contains(*k)).cloned().collect();
        for k in gone {
            let mut r = self.active.remove(&k).expect("listed");
            let why = items.left.get(&k).cloned().unwrap_or_else(|| if r.input.class == ItemClass::Alert { "cleared".into() } else { "left the inbox".into() });
            if matches!(r.stage, Stage::Officer { .. }) {
                out.push(EventBody::TriageClosed { item: k.clone(), why: why.clone() });
            }
            r.ended = Some((now, why));
            self.history.push_back(r);
        }
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        // deadlines
        for r in self.active.values_mut() {
            if let Stage::Officer { deadline } = r.stage
                && now.0 >= deadline.0
            {
                r.stage = Stage::Released { why: Release::Deadline, at: now };
                out.push(EventBody::TriageReleased { item: r.item.clone(), why: Release::Deadline });
            }
        }
        // new items, while on watch
        if self.on_watch {
            for (k, input, about) in items.held.into_iter().chain(items.alerts) {
                if self.active.contains_key(&k) || self.before_watch.contains(&k) {
                    continue;
                }
                let route = fleetmesh_decide::officer::route(p, &input, now);
                let stage = match &route {
                    Route::Officer { deadline_t } => Stage::Officer { deadline: *deadline_t },
                    Route::Person { why } => Stage::Person { why: why.clone() },
                };
                out.push(EventBody::TriageRouted { item: k.clone(), input: input.clone(), route, about: about.clone() });
                self.active.insert(k.clone(), Record { item: k, input, about, entered: now, stage, ended: None });
            }
        }
        current.clear();
        out
    }

    /// Whether the person's Inbox shows an item, and what its card says of triage. While the
    /// officer is on watch, an item that appeared since the last tick waits for that tick's
    /// routing (at most a second) rather than showing and then vanishing.
    pub fn view(&self, key: &str) -> (bool, Option<Value>) {
        match self.active.get(key) {
            None if self.on_watch && !self.before_watch.contains(key) => (false, Some(json!({ "stage": "arriving" }))),
            None => (true, None),
            Some(r) => (r.shown(), r.note()),
        }
    }

    /// The officer's verdict on an item it holds, checked against the policy
    /// (docs/design/watch-officer.md section 4): the event to log, or why it is refused.
    pub fn verdict(&mut self, p: &OfficerPolicy, by: &AgentId, item: &str, v: VerdictIn, now: Time) -> Result<EventBody, String> {
        self.check(p, by, item, &v)?;
        let VerdictIn { verdict, note, recommendation, intents } = v;
        let r = self.active.get_mut(item).expect("checked");
        r.stage = Stage::Decided { verdict, note: note.clone(), recommendation: recommendation.clone(), intents: intents.clone(), at: now };
        Ok(EventBody::TriageVerdict { item: item.to_string(), by: by.clone(), verdict, note, recommendation, intents })
    }

    /// Whether the officer may give this verdict on this item now (the policy's limits), without
    /// giving it.
    pub fn check(&self, p: &OfficerPolicy, by: &AgentId, item: &str, v: &VerdictIn) -> Result<(), String> {
        let (verdict, note, recommendation) = (v.verdict, &v.note, &v.recommendation);
        if by.as_str() != p.agent {
            return Err(format!("only the watch officer ({}) triages the Inbox", p.agent));
        }
        if !self.on_watch {
            return Err("the officer is not on watch".into());
        }
        let r = self.active.get(item).ok_or_else(|| {
            if item.starts_with("intent:") || item.starts_with("alert:") {
                format!("{item} is not with the officer: it is arriving (its triage starts within a second: try again) or no longer in the Inbox")
            } else {
                format!("{item}: an item is intent:<id> or alert:<id>, as get_inbox's triage.item names it")
            }
        })?;
        if !matches!(r.stage, Stage::Officer { .. }) {
            return Err(format!("{item} is not the officer's to triage (it is {})", r.json()["stage"].as_str().unwrap_or("")));
        }
        if note.trim().is_empty() {
            return Err("a verdict needs a note: what the officer did, or why not".into());
        }
        match verdict {
            Verdict::Escalated => {
                if recommendation.as_deref().is_none_or(|s| s.trim().is_empty()) {
                    return Err("an escalation needs a recommendation for the person".into());
                }
            }
            Verdict::Handled => {
                if r.input.class != ItemClass::Alert || !p.handle.alerts.contains(&r.input.kind) {
                    return Err(format!("the policy does not let the officer handle a {} ([officer.handle] alerts: {:?}); escalate it", r.input.kind, p.handle.alerts));
                }
            }
            Verdict::TurnedDown => {
                let t = &p.handle.turn_down;
                let ok = r.input.class == ItemClass::Held
                    && r.input.agent.as_deref() == Some(fleetmesh_decide::officer::DISPATCHER)
                    && t.kinds.contains(&r.input.kind)
                    && !r.input.codes.is_empty()
                    && r.input.codes.iter().all(|c| t.codes.contains(c));
                if !ok {
                    return Err("the policy does not let the officer turn this order down ([officer.handle.turn_down]: the dispatcher's orders of those kinds, held only for those codes); escalate it".into());
                }
            }
        }
        Ok(())
    }

    /// The items waiting for the officer's verdict now, with what to tell it of each (the
    /// change feed's `triage.item`).
    pub fn waiting(&self) -> Vec<(String, Value)> {
        self.active
            .values()
            .filter_map(|r| match r.stage {
                Stage::Officer { deadline } => Some((
                    r.item.clone(),
                    json!({
                        "item": r.item,
                        "class": r.input.class,
                        "item_kind": r.input.kind,
                        "severity": r.input.severity,
                        "codes": r.input.codes,
                        "proposed_by": r.input.agent,
                        "about": r.about,
                        "deadline_t": deadline.0 as f64 / 1000.0,
                    }),
                )),
                _ => None,
            })
            .collect()
    }

    /// The officer's view of what it holds, and the counts the person's Inbox shows.
    pub fn counts(&self) -> Value {
        let with_officer = self.active.values().filter(|r| matches!(r.stage, Stage::Officer { .. })).count();
        let handled = self
            .active
            .values()
            .chain(self.history.iter())
            .filter(|r| matches!(&r.stage, Stage::Decided { verdict, .. } if *verdict != Verdict::Escalated))
            .count();
        json!({ "on_watch": self.on_watch, "with_officer": with_officer, "handled": handled })
    }

    /// `GET /api/triage`: every item triaged this run, the open ones first, then the rest newest
    /// first.
    pub fn json(&self, p: Option<&OfficerPolicy>) -> Value {
        let open: Vec<Value> = self.active.values().map(Record::json).collect();
        let done: Vec<Value> = self.history.iter().rev().map(Record::json).collect();
        json!({
            "officer": p.map(|p| p.agent.clone()),
            "counts": self.counts(),
            "open": open,
            "history": done,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> OfficerPolicy {
        toml::from_str(
            r#"
            [bypass]
            kinds = ["land_now"]
            [handle]
            alerts = ["drop_not_clear"]
            turn_down = { kinds = ["deliver"], codes = ["ENERGY_MARGIN_THIN"] }
            "#,
        )
        .expect("policy")
    }

    fn held(kind: &str, agent: &str) -> (String, TriageInput, Value) {
        (
            format!("intent:{kind}"),
            TriageInput {
                class: ItemClass::Held,
                kind: kind.into(),
                severity: None,
                source: Some("agent".into()),
                agent: Some(agent.into()),
                codes: vec!["ENERGY_MARGIN_THIN".into()],
                expires_t: Some(Time::from_secs(1300)),
            },
            Value::Null,
        )
    }

    fn alert(kind: &str) -> (String, TriageInput, Value) {
        (
            format!("alert:{kind}"),
            TriageInput { class: ItemClass::Alert, kind: kind.into(), severity: Some("medium".into()), source: None, agent: None, codes: vec![], expires_t: None },
            Value::Null,
        )
    }

    fn items(held: Vec<(String, TriageInput, Value)>, alerts: Vec<(String, TriageInput, Value)>) -> Items {
        Items { held, alerts, left: BTreeMap::new() }
    }

    fn kinds(ev: &[EventBody]) -> Vec<&'static str> {
        ev.iter()
            .map(|e| match e {
                EventBody::TriageWatch { .. } => "watch",
                EventBody::TriageRouted { .. } => "routed",
                EventBody::TriageVerdict { .. } => "verdict",
                EventBody::TriageReleased { .. } => "released",
                EventBody::TriageClosed { .. } => "closed",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn with_no_officer_on_watch_the_person_sees_everything() {
        let mut t = Triage::default();
        let ev = t.step(&policy(), Time::from_secs(1000), 1000.0, items(vec![held("deliver", "dispatcher")], vec![]));
        assert!(ev.is_empty());
        assert!(t.view("intent:deliver").0);
    }

    #[test]
    fn what_the_person_had_stays_theirs_and_new_items_go_to_the_officer() {
        let p = policy();
        let mut t = Triage::default();
        t.seen(999.0);
        let ev = t.step(&p, Time::from_secs(1000), 1000.0, items(vec![], vec![alert("delivery_failed")]));
        assert_eq!(kinds(&ev), ["watch"]);
        assert!(t.view("alert:delivery_failed").0, "already in the Inbox: the person's");
        let ev = t.step(&p, Time::from_secs(1001), 1001.0, items(vec![held("deliver", "dispatcher"), held("land_now", "dispatcher")], vec![alert("delivery_failed")]));
        assert_eq!(kinds(&ev), ["routed", "routed"]);
        assert!(!t.view("intent:deliver").0, "with the officer");
        assert!(t.view("intent:land_now").0, "bypassed");
        assert!(!t.view("intent:new").0, "not routed yet: it waits for the next tick");
    }

    #[test]
    fn a_deadline_or_an_absent_officer_sends_items_to_the_person() {
        let p = policy();
        let mut t = Triage::default();
        t.seen(1000.0);
        t.step(&p, Time::from_secs(1000), 1000.0, items(vec![], vec![]));
        t.step(&p, Time::from_secs(1001), 1001.0, items(vec![], vec![alert("drop_not_clear")]));
        t.seen(1010.0);
        t.step(&p, Time::from_secs(1010), 1010.0, items(vec![held("deliver", "dispatcher")], vec![alert("drop_not_clear")]));
        // the alert's 60 s pass
        let ev = t.step(&p, Time::from_secs(1061), 1015.0, items(vec![held("deliver", "dispatcher")], vec![alert("drop_not_clear")]));
        assert_eq!(kinds(&ev), ["released", "released"], "the held order's 30 s passed too");
        assert!(t.view("alert:drop_not_clear").0 && t.view("intent:deliver").0);
        assert_eq!(t.view("alert:drop_not_clear").1.expect("note")["stage"], "untriaged");
        // a new item, then the officer goes quiet
        let ev = t.step(&p, Time::from_secs(1062), 1029.0, items(vec![held("deliver", "dispatcher"), held("pickup", "dispatcher")], vec![alert("drop_not_clear")]));
        assert_eq!(kinds(&ev), ["routed"]);
        let ev = t.step(&p, Time::from_secs(1063), 1031.0, items(vec![held("deliver", "dispatcher"), held("pickup", "dispatcher")], vec![alert("drop_not_clear")]));
        assert_eq!(kinds(&ev), ["watch", "released"]);
        assert!(t.view("intent:pickup").0);
    }

    #[test]
    fn verdicts_stay_within_the_policy() {
        let p = policy();
        let officer = AgentId::new("agent-watch");
        let mut t = Triage::default();
        t.seen(1000.0);
        t.step(&p, Time::from_secs(1000), 1000.0, items(vec![], vec![]));
        let now = Time::from_secs(1001);
        t.step(&p, now, 1001.0, items(vec![held("deliver", "dispatcher"), held("pickup", "dispatcher"), held("reroute", "agent-planner")], vec![alert("drop_not_clear"), alert("nav_monitor")]));
        let v = |t: &mut Triage, by: &AgentId, item: &str, verdict: Verdict, rec: Option<&str>| {
            t.verdict(&p, by, item, VerdictIn { verdict, note: "because".into(), recommendation: rec.map(str::to_string), intents: vec![] }, now)
        };
        assert!(v(&mut t, &AgentId::new("agent-planner"), "alert:drop_not_clear", Verdict::Handled, None).is_err(), "not the officer");
        assert!(v(&mut t, &officer, "alert:nav_monitor", Verdict::Handled, None).is_err(), "not an alert it may handle");
        assert!(v(&mut t, &officer, "intent:pickup", Verdict::TurnedDown, None).is_err(), "not a kind it may turn down");
        assert!(v(&mut t, &officer, "intent:reroute", Verdict::TurnedDown, None).is_err(), "another agent's order");
        assert!(v(&mut t, &officer, "alert:nav_monitor", Verdict::Escalated, None).is_err(), "an escalation needs a recommendation");
        assert!(v(&mut t, &officer, "alert:drop_not_clear", Verdict::Handled, None).is_ok());
        assert!(v(&mut t, &officer, "intent:deliver", Verdict::TurnedDown, None).is_ok());
        assert!(v(&mut t, &officer, "alert:nav_monitor", Verdict::Escalated, Some("recall DRONE 4")).is_ok());
        assert!(!t.view("alert:drop_not_clear").0 && !t.view("intent:deliver").0);
        let (shown, note) = t.view("alert:nav_monitor");
        assert!(shown);
        assert_eq!(note.expect("note")["recommendation"], "recall DRONE 4");
        assert!(v(&mut t, &officer, "alert:nav_monitor", Verdict::Escalated, Some("again")).is_err(), "decided already");
        assert_eq!(t.counts()["handled"], 2);
    }
}
