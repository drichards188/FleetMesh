//! The fleet organised as delivery operators organise theirs (docs/design/fleet-organisation.md):
//! every vehicle belongs to a home base, a hub (Wing's nests at its stores, Zipline's hubs and
//! docks, a robotaxi fleet's depots), and is in one operational state at a time, which the bridge
//! derives from what it already knows: the dispatcher's work on it, the orders running and waiting,
//! its link, its navigation, its battery, the alerts and issues about it. `GET /api/fleet` serves
//! the states and each base's counts; the console's Fleet list and map are organised by them.
//!
//! Display and organisation only: nothing here is logged, no check reads it, and it orders
//! nothing. The one decision home bases change is the assigner's (rules 4,
//! [`fleetmesh_decide::HOME_BASE_SINCE`]: the way back of a trip goes home), with the return the
//! dispatcher proposes after a delivery ([`super::delivery_rt`]).
//!
//! A vehicle's home base is the hub its fleet file names for it (`home_hub`, carried by the
//! manifest), else the hub it was launched at, else the hub its autopilot's home is at; a vehicle
//! launched away from every hub has none.

use std::collections::BTreeMap;

use fleetmesh_decide::delivery::{NavTrust, Places, WxLimits};
use fleetmesh_decide::Clock as _;
use fleetmesh_decide::{LonLat, Source, State, Uid as DUid};
use serde_json::{Value, json};

use super::Decision;
use super::alerts::Severity;
use super::delivery_rt::{DISPATCHER, SWAP_BELOW_PCT, hub_catchment};
use super::snapshot::bridge_uid;
use crate::clock;

/// A vehicle within this of its delivery's drop zone is delivering (hovering, looking, letting go),
/// m; farther, outbound.
pub const AT_ZONE_M: f64 = 80.0;

/// One vehicle's operational state. Derived in this order of precedence ([`derive()`]): out of
/// service, offline, needs attention, holding, then what its work is, then where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FleetState {
    Attention,
    Offline,
    OutOfService,
    Holding,
    Ready,
    Loading,
    Outbound,
    Delivering,
    Returning,
    Charging,
    Tasked,
}

impl FleetState {
    /// Every state, problems first, then a delivery's cycle in order (the console's order).
    pub const ALL: [FleetState; 11] = [
        FleetState::Attention,
        FleetState::Offline,
        FleetState::OutOfService,
        FleetState::Holding,
        FleetState::Ready,
        FleetState::Loading,
        FleetState::Outbound,
        FleetState::Delivering,
        FleetState::Returning,
        FleetState::Charging,
        FleetState::Tasked,
    ];

    pub fn id(self) -> &'static str {
        match self {
            FleetState::Attention => "attention",
            FleetState::Offline => "offline",
            FleetState::OutOfService => "out_of_service",
            FleetState::Holding => "holding",
            FleetState::Ready => "ready",
            FleetState::Loading => "loading",
            FleetState::Outbound => "outbound",
            FleetState::Delivering => "delivering",
            FleetState::Returning => "returning",
            FleetState::Charging => "charging",
            FleetState::Tasked => "tasked",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FleetState::Attention => "Needs attention",
            FleetState::Offline => "Offline",
            FleetState::OutOfService => "Out of service",
            FleetState::Holding => "Holding",
            FleetState::Ready => "Ready",
            FleetState::Loading => "Loading",
            FleetState::Outbound => "Outbound",
            FleetState::Delivering => "Delivering",
            FleetState::Returning => "Returning",
            FleetState::Charging => "Charging",
            FleetState::Tasked => "On another order",
        }
    }

    /// What the state means, as docs/design/fleet-organisation.md defines it.
    pub fn means(self) -> &'static str {
        match self {
            FleetState::Attention => "something about it needs a person: an order of it waits for a decision, an alert or an issue is open about it, its navigation is not trusted, its battery is under its reserve away from a hub, or it stopped away from a hub with no order",
            FleetState::Offline => "no telemetry for longer than its link's stale limit: where it is and what it does are not known now",
            FleetState::OutOfService => "an operator confirmed it lost, or it reports damage that stops it moving: not available, and not expected back in this run",
            FleetState::Holding => "automation is waiting something out for it (a drop zone that does not look clear yet, other traffic, a stale reading, an order waiting for its link) and will go on by itself",
            FleetState::Ready => "at a hub, its battery above the swap threshold, with no order: free for the dispatcher",
            FleetState::Loading => "its pickup is proposed or running, or its parcel is aboard at the hub and its delivery is about to go",
            FleetState::Outbound => "its delivery is running and it is on the way to the drop zone",
            FleetState::Delivering => "its delivery is running and it is at the drop zone: hovering, looking, letting go",
            FleetState::Returning => "on its way back to a hub, empty or bringing an undelivered parcel back",
            FleetState::Charging => "at a hub having its battery swapped, or due a swap (under the swap threshold), or waiting for a charged pack",
            FleetState::Tasked => "running an order that is not the dispatcher's: a person's, another agent's, the navigation monitor's",
        }
    }

    /// A state the person should look at (listed first, wherever the vehicle's base).
    pub fn problem(self) -> bool {
        matches!(self, FleetState::Attention | FleetState::Offline | FleetState::OutOfService)
    }
}

/// The dispatcher's work on a vehicle, by stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Pickup,
    Deliver,
    Return,
    Swap,
}

/// The dispatcher's work on one vehicle ([`super::delivery_rt`]'s job for it).
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    pub stage: Stage,
    /// its order is out and running (dispatched)
    pub running: bool,
    pub parcel: Option<String>,
    /// where it goes: a delivery's drop zone, a pickup's, return's or swap's hub
    pub to: Option<String>,
    /// a delivery's drop zone, to tell outbound from delivering
    pub zone_at: Option<LonLat>,
}

/// What the state of one vehicle is derived from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub position: Option<LonLat>,
    /// seconds since its last telemetry, when that is longer than its link's stale limit
    pub silent_s: Option<f64>,
    /// why it is out of service, if it is
    pub out_of_service: Option<String>,
    /// what about it needs a person, in words
    pub attention: Vec<String>,
    /// what automation waits out for it, in words
    pub holding: Option<String>,
    pub work: Option<Work>,
    /// a running order that is not the dispatcher's: its kind and whose it is
    pub other_order: Option<(String, String)>,
    /// an order of it waits until its link can carry it (rules 5's queue)
    pub queued: bool,
    /// the hub it is at, if any
    pub at_hub: Option<String>,
    pub battery_pct: Option<f64>,
}

fn words(kind: &str) -> String {
    kind.replace('_', " ")
}

/// A vehicle's state and why, in words: a pure function of its facts.
pub fn derive(f: &Facts) -> (FleetState, String) {
    if let Some(why) = &f.out_of_service {
        return (FleetState::OutOfService, why.clone());
    }
    if let Some(s) = f.silent_s {
        return (FleetState::Offline, format!("no telemetry for {s:.0} s"));
    }
    if !f.attention.is_empty() {
        return (FleetState::Attention, f.attention.join("; "));
    }
    if let Some(h) = &f.holding {
        return (FleetState::Holding, h.clone());
    }
    if f.queued {
        return (FleetState::Holding, "an order waits until its link can carry it".into());
    }
    let parcel = |w: &Work| w.parcel.clone().unwrap_or_else(|| "its parcel".into());
    let to = |w: &Work, what: &str| w.to.clone().unwrap_or_else(|| what.into());
    if let Some(w) = &f.work {
        return match w.stage {
            Stage::Swap => (FleetState::Charging, format!("swapping its battery at {}", to(w, "the hub"))),
            Stage::Pickup if w.running => (FleetState::Loading, format!("loading {} at {}", parcel(w), to(w, "the hub"))),
            Stage::Pickup => (FleetState::Loading, format!("about to load {} at {}", parcel(w), to(w, "the hub"))),
            Stage::Deliver if !w.running => (FleetState::Loading, format!("{} aboard, about to leave for {}", parcel(w), to(w, "its drop zone"))),
            Stage::Deliver => {
                let near = matches!((f.position, w.zone_at), (Some(p), Some(z)) if fleetmesh_decide::geo::haversine_m(p, z) <= AT_ZONE_M);
                if near {
                    (FleetState::Delivering, format!("at {} with {}", to(w, "its drop zone"), parcel(w)))
                } else {
                    (FleetState::Outbound, format!("taking {} to {}", parcel(w), to(w, "its drop zone")))
                }
            }
            Stage::Return => match &w.parcel {
                Some(p) => (FleetState::Returning, format!("bringing {p} back to {}", to(w, "a hub"))),
                None => (FleetState::Returning, format!("back to {}", to(w, "a hub"))),
            },
        };
    }
    if let Some((kind, by)) = &f.other_order {
        let what = format!("{} ({by})", words(kind));
        let state = match kind.as_str() {
            "return_to_hub" | "rtl" => FleetState::Returning,
            "swap_battery" => FleetState::Charging,
            "pickup" => FleetState::Loading,
            "deliver" => FleetState::Outbound,
            _ => FleetState::Tasked,
        };
        return (state, what);
    }
    let battery = f.battery_pct.map_or("battery not known".to_string(), |b| format!("{b:.0}%"));
    match (&f.at_hub, f.position) {
        (Some(h), _) if f.battery_pct.is_some_and(|b| b < SWAP_BELOW_PCT) => {
            (FleetState::Charging, format!("{battery} at {h}: a battery swap is due"))
        }
        (Some(h), _) => (FleetState::Ready, format!("{battery} at {h}")),
        (None, None) => (FleetState::Attention, "no position reported".into()),
        (None, Some(_)) => (FleetState::Attention, "stopped away from a hub with no order".into()),
    }
}

/// A vehicle's home base: the hub its fleet file names (when the world has it), else the hub it
/// was launched at, else the hub its autopilot's home is at (within a hub's catchment); none for
/// a vehicle launched away from every hub.
pub fn resolve_home(places: &Places, named: Option<&str>, launched: Option<LonLat>, ap_home: Option<LonLat>) -> Option<String> {
    if let Some(h) = named.filter(|h| places.hubs.contains_key(*h)) {
        return Some(h.to_string());
    }
    for p in [launched, ap_home].into_iter().flatten() {
        if let Some((hub, d)) = places.nearest_hub(p)
            && d <= hub_catchment(hub.radius_m)
        {
            return Some(hub.id.clone());
        }
    }
    None
}

/// What an alert about a vehicle asks of a person, in a few words; none for one another state says
/// (a lost link: offline; damage: out of service) or one about a parcel more than the vehicle (a
/// drop zone that did not look clear: automation kept the parcel and brings it back).
fn alert_words(kind: &str) -> Option<String> {
    Some(
        match kind {
            "lost_link" | "damaged" | "drop_not_clear" | "zone_not_approved" | "nav_monitor" | "node_lost" => return None,
            "not_moving" => "not moving",
            "left_task" => "left its order",
            "task_failed" => "its order failed",
            "ack_timeout" => "a command went unanswered",
            "may_be_running" => "may be running an order nothing confirmed",
            "restore_pending" => "its lost-link failsafe is not confirmed back",
            "delivery_failed" => "a delivery failed its tries",
            "return_failed" => "could not be sent home",
            "traffic_conflict" => "a traffic conflict outlasted automation",
            "traffic_noncooperative_near" => "an aircraft that broadcasts nothing is near",
            other => return Some(words(other)),
        }
        .to_string(),
    )
}

/// One vehicle as the fleet view lists it.
struct Listed {
    uid: crate::vehicle::Uid,
    duid: DUid,
    base: Option<String>,
    at_hub: Option<String>,
    state: FleetState,
    why: String,
    parcel: Option<String>,
    /// its platform's id in the catalog, from its identity
    platform: Option<String>,
}

/// Of the limits of a base's platforms, the strictest on gusts: the platform the wind holds first
/// (a hub of drones and planes is held by the drones' 11 m/s before the planes' 13).
fn strictest_gust(cands: impl IntoIterator<Item = (String, WxLimits)>) -> Option<(String, WxLimits)> {
    cands
        .into_iter()
        .filter(|(_, l)| l.gust_soft_mps.is_some())
        .min_by(|(_, a), (_, b)| a.gust_soft_mps.partial_cmp(&b.gust_soft_mps).unwrap_or(std::cmp::Ordering::Equal))
}

impl Decision {
    /// A vehicle's home base ([`resolve_home`]) from the manifest and `ap_home`, its autopilot's
    /// home.
    pub(super) fn home_hub(&self, uid: &DUid, ap_home: Option<LonLat>) -> Option<String> {
        let base = bridge_uid(uid).and_then(|b| self.world.manifest().bases.get(&b).cloned()).unwrap_or_default();
        resolve_home(&self.places, base.home_hub.as_deref(), base.launched.map(|(lon, lat)| LonLat::new(lon, lat)), ap_home)
    }

    /// `GET /api/fleet`: the states and what each means, the sites, each base (hub) with its
    /// vehicles' counts by state and what is parked at it now, and each vehicle's base, state and
    /// why. Read-only.
    pub fn fleet_json(&self) -> Value {
        let now = self.clock.now();
        let wall = clock::wall();
        // the alerts that ask something of a person, by vehicle; one the watch officer still holds
        // is not the person's yet (rules 7): it is with the officer
        let mut alerts: BTreeMap<crate::vehicle::Uid, Vec<&'static str>> = BTreeMap::new();
        let mut with_officer: BTreeMap<DUid, String> = BTreeMap::new();
        for a in self.current_alerts() {
            if a.severity == Severity::Info {
                continue;
            }
            if self.person_sees(&super::triage::alert_key(&a.id)) {
                alerts.entry(a.uid).or_default().push(a.kind);
            } else {
                with_officer.entry(super::snapshot::duid(a.uid)).or_insert_with(|| format!("its {} alert is with the watch officer", words(a.kind)));
            }
        }
        let (work, holds) = self.fleet_work(now);
        // orders waiting for a person, running orders that are not the dispatcher's, orders held
        // for a link, and vehicles confirmed lost
        let (pending, running, queued, lost) = {
            let core = self.lock();
            let mut pending: Vec<(DUid, fleetmesh_decide::IntentId, String)> = Vec::new();
            for i in core.engine.inbox() {
                for u in &i.spec().uids {
                    pending.push((u.clone(), i.id(), words(i.spec().kind().as_str())));
                }
            }
            let mut running: BTreeMap<DUid, (u64, String, String)> = BTreeMap::new();
            let mut queued: std::collections::BTreeSet<DUid> = std::collections::BTreeSet::new();
            for i in core.engine.intents() {
                if i.state() == State::Queued {
                    queued.extend(i.spec().uids.iter().cloned());
                }
                if i.state() != State::Dispatched {
                    continue;
                }
                let by = match &i.spec().source {
                    Source::Agent { id } if id.as_str() == DISPATCHER => continue,
                    Source::Agent { id } => format!("{id}'s"),
                    Source::Human { id } => format!("{id}'s"),
                    Source::Handover { .. } => "the previous watch's".to_string(),
                };
                for u in &i.spec().uids {
                    let newer = running.get(u).is_none_or(|(id, _, _)| i.id().0 > *id);
                    if newer {
                        running.insert(u.clone(), (i.id().0, i.spec().kind().as_str().to_string(), by.clone()));
                    }
                }
            }
            (pending, running, queued, self.lost_uids(&core))
        };
        // an order waiting for a decision is the person's once the officer has passed it on (or
        // there is no officer); before that it is with the officer, who decides or passes it
        // within its deadline (the triage lock after the decision lock's block, never inside it)
        let pending: BTreeMap<DUid, Vec<String>> = pending.into_iter().fold(BTreeMap::new(), |mut m, (u, id, kind)| {
            if self.person_sees(&super::triage::intent_key(id)) {
                m.entry(u).or_default().push(format!("its {kind} order waits for your decision"));
            } else {
                with_officer.entry(u).or_insert_with(|| format!("its {kind} order is with the watch officer"));
            }
            m
        });
        let (nav, sites): (BTreeMap<DUid, NavTrust>, Vec<super::delivery_rt::Site>) = {
            let rt = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
            (rt.nav.iter().map(|(u, n)| (u.clone(), n.state)).collect(), rt.scenario.as_ref().map(|s| s.sites.clone()).unwrap_or_default())
        };
        let packs = self.feed().hub_packs.clone();
        let reserve = self.rules.reserve_pct;
        let mut list: Vec<Listed> = Vec::new();
        for cell in self.world.vehicles() {
            let b = cell.uid;
            let u = super::snapshot::duid(b);
            let (pos, ap_home, batt, damage, platform) =
                cell.read(|v| (v.pos, v.home, v.battery_pct, v.damage, v.identity.as_ref().and_then(|i| i.str("platform")).map(str::to_string)));
            let position = pos.map(|p| LonLat::new(p.1, p.0));
            let at_hub = position.and_then(|p| self.places.nearest_hub(p)).filter(|(h, d)| *d <= hub_catchment(h.radius_m)).map(|(h, _)| h.id.clone());
            let link = cell.link_snapshot(clock::mono());
            // as the inbox's lost_link alert reads it (a cell exists once its vehicle was heard)
            let silent_s = link.last_rx_t.filter(|last| wall - last > link.stale_after_s()).map(|last| wall - last);
            let out_of_service = if lost.contains(&b) {
                Some("confirmed lost by an operator".to_string())
            } else {
                damage.filter(|d| d.mobility).map(|d| format!("reports {}: it cannot move", d.describe()))
            };
            let battery_pct = batt.map(f64::from);
            let mut attention: Vec<String> = pending.get(&u).cloned().unwrap_or_default();
            match nav.get(&u) {
                Some(NavTrust::Untrusted) => attention.push("navigation untrusted".into()),
                Some(NavTrust::Degraded) => attention.push("navigation degraded".into()),
                _ => {}
            }
            for kind in alerts.get(&b).into_iter().flatten() {
                if let Some(w) = alert_words(kind)
                    && !attention.contains(&w)
                {
                    attention.push(w);
                }
            }
            if damage.is_some_and(|d| !d.mobility) {
                attention.push("reports damage".into());
            }
            if at_hub.is_none()
                && let Some(p) = battery_pct.filter(|p| *p < reserve)
            {
                attention.push(format!("battery {p:.0}%, under its reserve"));
            }
            let facts = Facts {
                position,
                silent_s,
                out_of_service,
                attention,
                // automation's words name the vehicle first ("51 waits on the ground..."): the row
                // already does
                holding: holds.get(&u).map(|h| h.strip_prefix(&format!("{u} ")).unwrap_or(h).to_string()).or_else(|| with_officer.get(&u).cloned()),
                work: work.get(&u).cloned(),
                other_order: running.get(&u).map(|(_, k, by)| (k.clone(), by.clone())),
                queued: queued.contains(&u),
                at_hub: at_hub.clone(),
                battery_pct,
            };
            let (state, why) = derive(&facts);
            let base = self.home_hub(&u, ap_home.map(|h| LonLat::new(h.1, h.0)));
            let parcel = facts.work.as_ref().and_then(|w| w.parcel.clone());
            list.push(Listed { uid: b, duid: u, base, at_hub, state, why, parcel, platform });
        }
        // since when each has been in its state
        let since: BTreeMap<DUid, i64> = {
            let mut rt = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
            rt.fleet_since.retain(|u, _| list.iter().any(|x| x.duid == *u));
            list.iter()
                .map(|x| {
                    let e = rt.fleet_since.entry(x.duid.clone()).or_insert((x.state.id(), now.0));
                    if e.0 != x.state.id() {
                        *e = (x.state.id(), now.0);
                    }
                    (x.duid.clone(), e.1)
                })
                .collect()
        };
        let count = |f: &dyn Fn(&Listed) -> bool| -> Value {
            let mut m = serde_json::Map::new();
            for s in FleetState::ALL {
                let n = list.iter().filter(|x| x.state == s && f(x)).count();
                if n > 0 {
                    m.insert(s.id().into(), json!(n));
                }
            }
            Value::Object(m)
        };
        let site_of = |hub: &str| sites.iter().find(|s| s.hubs.iter().any(|h| h == hub)).map(|s| s.id.clone());
        // rules 8: the plays in force, by hub
        let plays = self.plays_in_force();
        // The wind at each hub against the limits of the platforms based there (the strictest):
        // whether the weather rule holds or refuses what leaves it. Display only; the rule itself
        // reads each order's own samples.
        let rules = self.rules.delivery_rules();
        let wind_at = |h: &fleetmesh_decide::delivery::Hub| -> Option<Value> {
            let w = self.weather_point_json(h.lat, h.lon)?;
            let mut seen = std::collections::BTreeSet::new();
            let lim = strictest_gust(list.iter().filter(|x| x.base.as_deref() == Some(h.id.as_str())).filter_map(|x| {
                let p = x.platform.as_deref()?;
                let facts = self.catalog.get(p)?;
                let ap = facts.autopilot?;
                seen.insert(p.to_string()).then(|| (p.to_string(), fleetmesh_decide::delivery_rules::weather_limits(&rules, facts, ap)))
            }));
            let e = w["wind_east_mps"].as_f64().unwrap_or(0.0);
            let n = w["wind_north_mps"].as_f64().unwrap_or(0.0);
            let l = lim.as_ref().map(|(_, l)| l);
            Some(json!({
                "gust_mps": w["gust_mps"], "wind_mps": (e.hypot(n) * 10.0).round() / 10.0,
                "platform": lim.as_ref().map(|(p, _)| p),
                "gust_soft_mps": l.and_then(|l| l.gust_soft_mps), "gust_hard_mps": l.and_then(|l| l.gust_hard_mps),
                "wind_soft_mps": l.and_then(|l| l.wind_soft_mps), "wind_hard_mps": l.and_then(|l| l.wind_hard_mps),
            }))
        };
        let bases: Vec<Value> = self
            .places
            .hubs
            .values()
            .map(|h| {
                let id = h.id.as_str();
                json!({
                    "id": h.id, "name": h.name, "at": [h.lon, h.lat], "radius_m": h.radius_m, "open": h.open,
                    "site": site_of(id), "pads": h.pads, "packs": packs.get(id).copied().unwrap_or(h.packs),
                    "vehicles": list.iter().filter(|x| x.base.as_deref() == Some(id)).count(),
                    "counts": count(&|x| x.base.as_deref() == Some(id)),
                    "here": list.iter().filter(|x| x.at_hub.as_deref() == Some(id)).count(),
                    "visitors": list.iter().filter(|x| x.at_hub.as_deref() == Some(id) && x.base.as_deref() != Some(id)).count(),
                    "parked": count(&|x| x.at_hub.as_deref() == Some(id)),
                    "weather": wind_at(h),
                    "plays": plays.iter().filter(|p| p.hub == h.id).collect::<Vec<_>>(),
                })
            })
            .collect();
        let vehicles: Vec<Value> = list
            .iter()
            .map(|x| {
                json!({
                    "uid": x.uid, "base": x.base, "at_hub": x.at_hub, "state": x.state.id(), "why": x.why, "parcel": x.parcel,
                    "since_s": ((now.0 - since.get(&x.duid).copied().unwrap_or(now.0)) as f64 / 1000.0).max(0.0).round(),
                })
            })
            .collect();
        let states: Vec<Value> = FleetState::ALL
            .iter()
            .map(|s| json!({ "id": s.id(), "label": s.label(), "problem": s.problem(), "means": s.means() }))
            .collect();
        json!({
            "states": states,
            "sites": sites,
            "bases": bases,
            "vehicles": vehicles,
            "counts": count(&|_| true),
            "home_base_rules": self.rules.at_least(fleetmesh_decide::HOME_BASE_SINCE),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(lon: f64, lat: f64) -> Option<LonLat> {
        Some(LonLat::new(lon, lat))
    }

    fn work(stage: Stage, running: bool) -> Work {
        Work { stage, running, parcel: Some("p-7".into()), to: Some("dz-city-04".into()), zone_at: at(-122.43972, 37.80106) }
    }

    fn ready() -> Facts {
        Facts { position: at(-122.437, 37.8057), at_hub: Some("hub-marina".into()), battery_pct: Some(90.0), ..Facts::default() }
    }

    #[test]
    fn a_hubs_wind_is_judged_by_the_platform_it_holds_first() {
        use fleetmesh_decide::AutopilotKind;
        let drone = ("delivery_drone".to_string(), WxLimits::defaults(AutopilotKind::Copter));
        let plane = ("delivery_plane".to_string(), WxLimits::defaults(AutopilotKind::Plane));
        let car = ("delivery_car".to_string(), WxLimits::defaults(AutopilotKind::Ugv));
        let (p, l) = strictest_gust([plane.clone(), drone, car.clone()]).expect("the drone's limits");
        assert_eq!((p.as_str(), l.gust_soft_mps, l.gust_hard_mps), ("delivery_drone", Some(11.0), Some(14.0)));
        assert_eq!(strictest_gust([plane]).map(|x| x.0).as_deref(), Some("delivery_plane"));
        assert!(strictest_gust([car]).is_none(), "a car has no gust limit: no line for its hub");
    }

    #[test]
    fn a_vehicle_at_a_hub_with_no_order_is_ready_and_low_it_is_charging() {
        assert_eq!(derive(&ready()), (FleetState::Ready, "90% at hub-marina".into()));
        let low = Facts { battery_pct: Some(30.0), ..ready() };
        assert_eq!(derive(&low).0, FleetState::Charging);
        assert!(derive(&low).1.contains("swap is due"), "{:?}", derive(&low));
    }

    #[test]
    fn the_dispatchers_work_reads_as_the_delivery_cycle() {
        let with = |w: Work, pos: Option<LonLat>| Facts { work: Some(w), position: pos, at_hub: None, ..ready() };
        assert_eq!(derive(&with(work(Stage::Pickup, false), at(-122.437, 37.8057))).0, FleetState::Loading);
        assert_eq!(derive(&with(work(Stage::Pickup, true), at(-122.437, 37.8057))).0, FleetState::Loading);
        assert_eq!(derive(&with(work(Stage::Deliver, false), at(-122.437, 37.8057))).0, FleetState::Loading, "aboard, about to leave");
        let out = derive(&with(work(Stage::Deliver, true), at(-122.437, 37.8057)));
        assert_eq!(out, (FleetState::Outbound, "taking p-7 to dz-city-04".into()));
        let drop = derive(&with(work(Stage::Deliver, true), at(-122.4398, 37.8011)));
        assert_eq!(drop.0, FleetState::Delivering, "within {AT_ZONE_M} m of the zone: {drop:?}");
        let mut back = work(Stage::Return, true);
        back.to = Some("hub-marina".into());
        back.parcel = None;
        assert_eq!(derive(&with(back.clone(), at(-122.43, 37.80))), (FleetState::Returning, "back to hub-marina".into()));
        back.parcel = Some("p-7".into());
        assert!(derive(&with(back, at(-122.43, 37.80))).1.contains("bringing p-7 back"));
        let mut swap = work(Stage::Swap, true);
        swap.to = Some("hub-marina".into());
        assert_eq!(derive(&with(swap, at(-122.437, 37.8057))).0, FleetState::Charging);
    }

    #[test]
    fn problems_come_before_the_work_and_out_of_service_before_offline() {
        let busy = Facts { work: Some(work(Stage::Deliver, true)), ..ready() };
        let held = Facts { holding: Some("waits for dz-city-04 to look clear".into()), ..busy.clone() };
        assert_eq!(derive(&held).0, FleetState::Holding);
        let asked = Facts { attention: vec!["its deliver order waits for your decision".into()], ..held.clone() };
        assert_eq!(derive(&asked), (FleetState::Attention, "its deliver order waits for your decision".into()));
        let silent = Facts { silent_s: Some(12.4), ..asked.clone() };
        assert_eq!(derive(&silent), (FleetState::Offline, "no telemetry for 12 s".into()));
        let lost = Facts { out_of_service: Some("confirmed lost by an operator".into()), ..silent };
        assert_eq!(derive(&lost).0, FleetState::OutOfService);
        let queued = Facts { queued: true, ..ready() };
        assert_eq!(derive(&queued).0, FleetState::Holding);
    }

    #[test]
    fn another_order_is_said_by_its_kind_and_whose_it_is() {
        let held = Facts { other_order: Some(("hold".into(), "op-1's".into())), at_hub: None, ..ready() };
        assert_eq!(derive(&held), (FleetState::Tasked, "hold (op-1's)".into()));
        let home = Facts { other_order: Some(("return_to_hub".into(), "op-1's".into())), ..held.clone() };
        assert_eq!(derive(&home).0, FleetState::Returning);
    }

    #[test]
    fn stopped_away_from_a_hub_with_no_order_needs_attention() {
        let stopped = Facts { at_hub: None, position: at(-122.43, 37.80), ..ready() };
        assert_eq!(derive(&stopped), (FleetState::Attention, "stopped away from a hub with no order".into()));
        let nowhere = Facts { at_hub: None, position: None, ..ready() };
        assert_eq!(derive(&nowhere).0, FleetState::Attention);
    }

    #[test]
    fn every_state_has_an_id_a_label_and_a_meaning_and_the_problems_come_first() {
        let ids: Vec<&str> = FleetState::ALL.iter().map(|s| s.id()).collect();
        assert_eq!(ids.len(), 11);
        let mut dedup = ids.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), ids.len());
        let first_fine = FleetState::ALL.iter().position(|s| !s.problem()).expect("some");
        assert!(FleetState::ALL[first_fine..].iter().all(|s| !s.problem()), "problems first");
        for s in FleetState::ALL {
            assert!(!s.label().is_empty() && s.means().len() > 20, "{s:?}");
            assert!(!s.means().contains('\u{2014}') && !s.label().contains('\u{2014}'), "no em dash in {s:?}");
        }
    }

    fn places() -> Places {
        let text = r#"
[[hub]]
id = "hub-marina"
lon = -122.437
lat = 37.8057
radius_m = 80
[[hub]]
id = "hub-marina-kerb"
lon = -122.4360
lat = 37.8045
radius_m = 40
"#;
        Places::from_toml("test", text).expect("places")
    }

    #[test]
    fn the_home_base_is_the_fleet_files_else_where_it_was_launched_else_its_autopilots_home() {
        let p = places();
        assert_eq!(resolve_home(&p, Some("hub-marina-kerb"), at(-122.437, 37.8057), None).as_deref(), Some("hub-marina-kerb"), "the file's word wins");
        assert_eq!(resolve_home(&p, Some("hub-nowhere"), at(-122.4361, 37.8045), None).as_deref(), Some("hub-marina-kerb"), "an unknown hub: where it was launched");
        assert_eq!(resolve_home(&p, None, at(-122.4369, 37.8058), None).as_deref(), Some("hub-marina"));
        assert_eq!(resolve_home(&p, None, None, at(-122.4369, 37.8058)).as_deref(), Some("hub-marina"), "its autopilot's home");
        assert_eq!(resolve_home(&p, None, at(-122.40, 37.80), None), None, "launched away from every hub");
    }
}
