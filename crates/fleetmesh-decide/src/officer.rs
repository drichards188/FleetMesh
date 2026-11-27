//! The watch officer's triage (rules 7, [`crate::rules::OFFICER_TRIAGE_SINCE`];
//! docs/design/watch-officer.md).
//!
//! While an AI watch officer is on watch, every item that would reach the person's Inbox (an
//! order waiting for a person, an alert) goes to the officer first. The officer handles what it
//! can within the policy's limits and passes the rest on with a recommendation; an item it has
//! not triaged by its deadline reaches the person anyway, marked untriaged. Some items never wait
//! for it at all (the policy's `[officer.bypass]`: an emergency landing, an operator's own order).
//!
//! This module is the policy's `[officer]` section and [`route`], the pure function that sends an
//! item to the officer or straight to the person and sets its deadline. The bridge calls it as an
//! item appears and logs the result (`triage.routed`, with the item's inputs by value); replay
//! calls it again on the logged inputs and the logged policy and compares. The officer's own
//! verdicts are inputs to the log, as a person's decisions are; they decide nothing here.

use serde::{Deserialize, Serialize};

use crate::Time;

/// `[officer]` in the policy file (policy schema 4, rules 7). Absent: no triage, whatever runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfficerPolicy {
    /// The agent that triages (its MCP id). Only it is the officer; its presence on MCP is what
    /// puts the run on watch.
    #[serde(default = "default_agent")]
    pub agent: String,
    /// The officer counts as gone when it has made no MCP call for this long, s: every item it
    /// holds then goes to the person at once.
    #[serde(default = "default_absent_after_s")]
    pub absent_after_s: u32,
    #[serde(default)]
    pub deadline_s: Deadlines,
    #[serde(default)]
    pub bypass: Bypass,
    #[serde(default)]
    pub handle: Handle,
}

fn default_agent() -> String {
    "agent-watch".into()
}

fn default_absent_after_s() -> u32 {
    20
}

/// `[officer.deadline_s]`: how long an item waits for the officer's verdict before it goes to the
/// person, s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Deadlines {
    /// An order waiting for a person.
    pub held: u32,
    /// An alert.
    pub alert: u32,
    /// An order also reaches the person at least this long before its approval expires, so a
    /// person always has time to decide it; one that expires sooner than this goes to the person
    /// at once.
    pub before_expiry_s: u32,
}

impl Default for Deadlines {
    fn default() -> Self {
        Deadlines { held: 30, alert: 60, before_expiry_s: 15 }
    }
}

/// `[officer.bypass]`: what never waits for the officer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bypass {
    /// Order kinds that go straight to the person (`land_now`, `abort`: their approval expires in
    /// seconds and a delay can cost the vehicle).
    pub kinds: Vec<String>,
    /// Alert kinds that go straight to the person.
    pub alerts: Vec<String>,
    /// Alert severities that go straight to the person (`high`, `medium`, `info`).
    pub severity: Vec<String>,
}

/// `[officer.handle]`: what the officer may settle without the person (docs/design/watch-officer.md
/// section 6). Everything else it may only pass on, with its recommendation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Handle {
    /// Alert kinds it may mark handled.
    pub alerts: Vec<String>,
    /// The dispatcher's held orders it may turn down: by kind, and only when every holding
    /// check's code is one of `codes`. Never a safety kind ([`SAFETY_KINDS`]).
    pub turn_down: TurnDown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TurnDown {
    pub kinds: Vec<String>,
    pub codes: Vec<String>,
}

/// The dispatcher's agent id (the bridge's own automation): the only proposer whose held orders
/// the officer may turn down, by having it withdraw them.
pub const DISPATCHER: &str = "dispatcher";

/// Order kinds the officer may never turn down: withdrawing one can raise a vehicle's exposure.
pub const SAFETY_KINDS: [&str; 5] = ["land_now", "abort", "return_to_hub", "hold", "rtl"];

impl OfficerPolicy {
    /// The bounds a policy file must meet ([`crate::Policy::validate`]).
    pub fn validate(&self) -> Result<(), String> {
        if self.agent.trim().is_empty() {
            return Err("officer.agent must name an agent".into());
        }
        if self.absent_after_s == 0 {
            return Err("officer.absent_after_s must be > 0".into());
        }
        if self.deadline_s.held == 0 || self.deadline_s.alert == 0 {
            return Err("officer.deadline_s.held and .alert must be > 0".into());
        }
        if let Some(k) = self.handle.turn_down.kinds.iter().find(|k| SAFETY_KINDS.contains(&k.as_str())) {
            return Err(format!("officer.handle.turn_down.kinds: {k} is a safety order, which the officer may never turn down"));
        }
        if let Some(s) = self.bypass.severity.iter().find(|s| !matches!(s.as_str(), "high" | "medium" | "info")) {
            return Err(format!("officer.bypass.severity: {s} is not high, medium or info"));
        }
        Ok(())
    }
}

/// What kind of Inbox item it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemClass {
    /// An order waiting for a person (`pending_approval`).
    Held,
    /// An alert (derived from the state; an issue automation raised is one).
    Alert,
}

/// What [`route`] reads of an item, logged by value in `triage.routed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageInput {
    pub class: ItemClass,
    /// The order's task kind, or the alert's kind.
    pub kind: String,
    /// An alert's severity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// An order's source: `human`, `agent` or `handover`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// An order's proposing agent, when an agent proposed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The codes of an order's holding checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codes: Vec<String>,
    /// When an order's approval expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_t: Option<Time>,
}

/// Where an item goes: to the officer, with the time it reaches the person if the officer has not
/// triaged it, or straight to the person, with why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "route", rename_all = "snake_case")]
pub enum Route {
    Officer { deadline_t: Time },
    Person { why: String },
}

/// Where an item that appeared at `now` goes (docs/design/watch-officer.md sections 2 and 5).
pub fn route(p: &OfficerPolicy, item: &TriageInput, now: Time) -> Route {
    let person = |why: String| Route::Person { why };
    match item.class {
        ItemClass::Held => {
            if p.bypass.kinds.contains(&item.kind) {
                return person(format!("a {} goes straight to the person", item.kind));
            }
            // an operator's own order, or one a fleet file handed over, is the person's already
            if item.source.as_deref() != Some("agent") {
                return person("an operator's own order".into());
            }
            let mut deadline = now.plus_ms(i64::from(p.deadline_s.held) * 1000);
            if let Some(exp) = item.expires_t {
                let latest = exp.plus_ms(-i64::from(p.deadline_s.before_expiry_s) * 1000);
                if latest.0 <= now.0 {
                    return person("its approval expires too soon to wait for the officer".into());
                }
                if latest.0 < deadline.0 {
                    deadline = latest;
                }
            }
            Route::Officer { deadline_t: deadline }
        }
        ItemClass::Alert => {
            if p.bypass.alerts.contains(&item.kind) {
                return person(format!("a {} alert goes straight to the person", item.kind));
            }
            if let Some(s) = &item.severity
                && p.bypass.severity.iter().any(|b| b == s)
            {
                return person(format!("a {s}-severity alert goes straight to the person"));
            }
            Route::Officer { deadline_t: now.plus_ms(i64::from(p.deadline_s.alert) * 1000) }
        }
    }
}

/// The officer's verdict on an item it holds (`triage.verdict`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Settled without the person, within `[officer.handle]`: an alert it dealt with (by the
    /// orders it proposed, or no action, with why).
    Handled,
    /// Passed on to the person, with its recommendation.
    Escalated,
    /// A dispatcher's held order it had the dispatcher withdraw (within
    /// `[officer.handle.turn_down]`).
    TurnedDown,
}

/// Why an item the officer held reached the person without its verdict (`triage.released`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Release {
    /// Its deadline passed.
    Deadline,
    /// The officer went off watch (no MCP call for `absent_after_s`).
    OfficerAbsent,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> OfficerPolicy {
        toml::from_str(
            r#"
            [bypass]
            kinds = ["land_now", "abort"]
            alerts = ["traffic_noncooperative_near"]
            severity = []
            [handle]
            alerts = ["drop_not_clear"]
            turn_down = { kinds = ["deliver"], codes = ["ENERGY_MARGIN_THIN"] }
            "#,
        )
        .expect("policy")
    }

    fn held(kind: &str, source: &str, expires_s: Option<i64>) -> TriageInput {
        TriageInput {
            class: ItemClass::Held,
            kind: kind.into(),
            severity: None,
            source: Some(source.into()),
            agent: (source == "agent").then(|| "dispatcher".into()),
            codes: vec!["ENERGY_MARGIN_THIN".into()],
            expires_t: expires_s.map(Time::from_secs),
        }
    }

    fn alert(kind: &str, severity: &str) -> TriageInput {
        TriageInput { class: ItemClass::Alert, kind: kind.into(), severity: Some(severity.into()), source: None, agent: None, codes: vec![], expires_t: None }
    }

    #[test]
    fn the_defaults_are_the_drafts() {
        let p = policy();
        assert_eq!(p.agent, "agent-watch");
        assert_eq!((p.deadline_s.held, p.deadline_s.alert, p.deadline_s.before_expiry_s), (30, 60, 15));
        assert_eq!(p.absent_after_s, 20);
        p.validate().expect("valid");
    }

    #[test]
    fn an_agents_held_order_waits_for_the_officer_until_its_deadline() {
        let now = Time::from_secs(1000);
        assert_eq!(route(&policy(), &held("deliver", "agent", Some(1300)), now), Route::Officer { deadline_t: Time::from_secs(1030) });
    }

    #[test]
    fn the_deadline_leaves_the_person_time_before_expiry() {
        let now = Time::from_secs(1000);
        // expires at 1040: the person must have it by 1025
        assert_eq!(route(&policy(), &held("deliver", "agent", Some(1040)), now), Route::Officer { deadline_t: Time::from_secs(1025) });
        // expires at 1010: too soon to wait
        assert!(matches!(route(&policy(), &held("deliver", "agent", Some(1010)), now), Route::Person { .. }));
    }

    #[test]
    fn some_items_never_wait_for_the_officer() {
        let now = Time::from_secs(1000);
        let p = policy();
        assert!(matches!(route(&p, &held("land_now", "agent", Some(1030)), now), Route::Person { .. }));
        assert!(matches!(route(&p, &held("deliver", "human", Some(1300)), now), Route::Person { .. }));
        assert!(matches!(route(&p, &alert("traffic_noncooperative_near", "high"), now), Route::Person { .. }));
        assert_eq!(route(&p, &alert("drop_not_clear", "medium"), now), Route::Officer { deadline_t: Time::from_secs(1060) });
        let mut sev = p.clone();
        sev.bypass.severity = vec!["high".into()];
        assert!(matches!(route(&sev, &alert("traffic_conflict", "high"), now), Route::Person { .. }));
    }

    #[test]
    fn a_safety_order_can_never_be_turned_down() {
        let mut p = policy();
        p.handle.turn_down.kinds.push("return_to_hub".into());
        assert!(p.validate().expect_err("a safety order").contains("safety order"));
    }

    #[test]
    fn the_logged_input_round_trips() {
        let i = held("deliver", "agent", Some(1300));
        let back: TriageInput = serde_json::from_value(serde_json::to_value(&i).expect("json")).expect("parse");
        assert_eq!(back, i);
        let r = Route::Officer { deadline_t: Time::from_secs(5) };
        assert_eq!(serde_json::to_value(&r).expect("json")["route"], "officer");
    }
}
