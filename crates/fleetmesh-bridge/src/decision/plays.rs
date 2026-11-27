//! Plays through the bridge (rules 8, docs/design/incidents.md section 3): a person adopts one for a
//! hub (`POST /api/plays`), lifts it (`POST /api/plays/{id}/lift`), and the bridge lifts one whose
//! end condition clears (the gust at its hub under the soft limit of the platforms based there
//! for [`PLAY_CLEAR_S`]). The engine holds the plays in force and routes by them; the dispatcher
//! holds back what they hold (`delivery_rt`).

use serde::Deserialize;
use serde_json::{Value, json};

use fleetmesh_decide::Clock as _;
use fleetmesh_decide::plays::{PlayKind, PlayLimits, PlayRequest};

use super::delivery_rt::{PLAY_CLEAR_S, PlayLift};
use super::{Actor, Decision, DecisionError, engine_err};

/// What a person asks for at the console: a play, and whether it lifts itself when the wind drops.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayAsk {
    pub kind: PlayKind,
    pub hub: String,
    #[serde(default)]
    pub codes: Vec<String>,
    #[serde(default)]
    pub limits: PlayLimits,
    /// parcels at or above this priority stay the person's (default: urgent, 3)
    #[serde(default = "urgent")]
    pub except_priority: Option<u8>,
    /// how long at most, s (default: the policy's longest)
    #[serde(default)]
    pub for_s: Option<u32>,
    /// `"wind"`: it lifts itself once the gust at the hub has stayed under the soft limit of the
    /// platforms based there for five minutes
    #[serde(default)]
    pub lift: Option<String>,
}

fn urgent() -> Option<u8> {
    Some(3)
}

impl Decision {
    /// A person adopts a play (rules 8). The dispatcher then withdraws its own held orders the
    /// play holds back.
    pub fn adopt_play(&self, by: &Actor, ask: PlayAsk) -> Result<Value, DecisionError> {
        let lift = match ask.lift.as_deref() {
            None => None,
            Some("wind") => {
                let soft = self.hub_gust_limit(&ask.hub).ok_or_else(|| DecisionError::new(422, format!("{}: no gust limit to lift by (no wind sample, or no platform based there has one)", ask.hub)))?;
                Some(PlayLift { hub: ask.hub.clone(), below_mps: soft, since_ms: None })
            }
            Some(other) => return Err(DecisionError::new(422, format!("lift {other:?}: the one end condition is \"wind\""))),
        };
        if !self.places.hubs.contains_key(&ask.hub) {
            return Err(DecisionError::new(404, format!("no hub {}", ask.hub)));
        }
        let play = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let max_s = core.engine.policy().plays.as_ref().map_or(3600, |p| p.max_s);
            let req = PlayRequest {
                kind: ask.kind,
                hub: ask.hub.clone(),
                codes: ask.codes,
                limits: ask.limits,
                except_priority: ask.except_priority,
                for_s: ask.for_s.unwrap_or(max_s),
                lifts: lift.as_ref().map(|l| format!("gusts at the hub under {:.1} m/s for {} min", l.below_mps, PLAY_CLEAR_S / 60)),
            };
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            core.engine.adopt_play(session.authority(), req).map_err(engine_err)?
        };
        if let Some(l) = lift {
            self.rt().play_lifts.insert(play.id, l);
        }
        self.apply_play(&play);
        self.publish();
        Ok(json!(play))
    }

    /// A person lifts a play (rules 8).
    pub fn lift_play(&self, by: &Actor, id: u64) -> Result<Value, DecisionError> {
        {
            let mut guard = self.lock();
            let core = &mut *guard;
            let session = Self::session(&mut core.engine, &mut core.sessions, &core.logins, by, &self.consequential_operators, self.auth)?;
            let why = format!("lifted by {}", by.operator());
            core.engine.end_play(id, &why, Some(session.authority())).map_err(engine_err)?;
        }
        self.rt().play_lifts.remove(&id);
        self.publish();
        Ok(json!({ "lifted": id }))
    }

    /// `GET /api/plays`: the plays in force, each with how its end condition stands.
    pub fn plays_json(&self) -> Value {
        let plays = self.plays_in_force();
        let now = self.clock.now();
        let rt = self.rt();
        let list: Vec<Value> = plays
            .iter()
            .map(|p| {
                let mut v = json!(p);
                if let Some(l) = rt.play_lifts.get(&p.id) {
                    let clear_s = l.since_ms.map_or(0, |s| (now.0 - s).max(0) / 1000);
                    v["lift"] = json!({ "below_mps": l.below_mps, "clear_s": clear_s, "needs_s": PLAY_CLEAR_S });
                }
                v["left_s"] = json!(((p.until_t.0 - now.0).max(0)) / 1000);
                v
            })
            .collect();
        json!({ "plays": list })
    }

    /// The decision loop's plays (after its locked part): those whose time ran out end (the
    /// engine's, logged at their time), and those whose end condition has held long enough lift.
    pub(super) fn step_plays(&self) {
        if !self.rules.at_least(fleetmesh_decide::PLAYS_SINCE) {
            return;
        }
        if let Err(e) = self.lock().engine.expire_plays() {
            eprintln!("decision: plays: {e}");
        }
        let in_force: Vec<u64> = self.plays_in_force().iter().map(|p| p.id).collect();
        let lifts: Vec<(u64, PlayLift)> = {
            let mut rt = self.rt();
            rt.play_lifts.retain(|id, _| in_force.contains(id));
            rt.play_lifts.iter().map(|(id, l)| (*id, l.clone())).collect()
        };
        let now = self.clock.now();
        for (id, l) in lifts {
            let Some(h) = self.places.hubs.get(&l.hub) else { continue };
            let gust = self.weather_point_json(h.lat, h.lon).and_then(|w| w["gust_mps"].as_f64());
            let since = match gust {
                Some(g) if g < l.below_mps => Some(l.since_ms.unwrap_or(now.0)),
                _ => None,
            };
            if since.is_some_and(|s| now.0 - s >= PLAY_CLEAR_S * 1000) {
                let why = format!("the wind dropped: gusts at {} under {:.1} m/s for {} min", l.hub, l.below_mps, PLAY_CLEAR_S / 60);
                match self.lock().engine.end_play(id, &why, None) {
                    Ok(()) => {
                        self.rt().play_lifts.remove(&id);
                    }
                    Err(e) => eprintln!("decision: play {id}: {e}"),
                }
            } else if let Some(x) = self.rt().play_lifts.get_mut(&id) {
                x.since_ms = since;
            }
        }
    }

    /// The watch officer recommends a play for an incident it sees (rules 8): kept for the
    /// incident's card, where the person adopts it or not, and logged as the officer's action.
    /// Only the run's watch officer, and only for an incident in its view.
    pub fn recommend_play(&self, by: &fleetmesh_decide::AgentId, incident: &str, kind: PlayKind, limits: PlayLimits, why: &str) -> Result<Value, DecisionError> {
        let officer = self.lock().engine.policy().officer.as_ref().map(|o| o.agent.clone());
        if officer.as_deref() != Some(by.as_str()) {
            return Err(DecisionError::new(403, "only the run's watch officer recommends a play".to_string()));
        }
        if why.trim().is_empty() {
            return Err(DecisionError::new(400, "a recommendation says why".to_string()));
        }
        let view = self.inbox_json_for_agents();
        let Some(inc) = view["incidents"].as_array().and_then(|l| l.iter().find(|i| i["id"] == incident)).cloned() else {
            return Err(DecisionError::new(404, format!("no incident {incident} in your view (get_inbox: incidents)")));
        };
        if kind == PlayKind::Limits && limits.is_empty() {
            return Err(DecisionError::new(400, "a limits play names its limits".to_string()));
        }
        let rec = json!({ "incident": incident, "hub": inc["hub"], "kind": kind, "limits": limits, "why": why, "by": by.as_str(), "t": self.clock.now().0 as f64 / 1000.0 });
        self.agent_action(by, "play.recommended", &rec.to_string())?;
        self.rt().play_recs.insert(incident.to_string(), rec.clone());
        self.publish();
        Ok(rec)
    }

    /// The officer's recommendations for the incidents now open (one per incident, its latest).
    pub(super) fn play_recs_for(&self, incidents: &[super::incidents::Incident]) -> Vec<Value> {
        let mut rt = self.rt();
        rt.play_recs.retain(|k, _| incidents.iter().any(|i| &i.id == k));
        rt.play_recs.values().cloned().collect()
    }

    /// The gust soft limit of the strictest platform based at `hub`: what a play there lifts by.
    fn hub_gust_limit(&self, hub: &str) -> Option<f64> {
        self.fleet_json()["bases"].as_array()?.iter().find(|b| b["id"] == hub)?["weather"]["gust_soft_mps"].as_f64()
    }
}
