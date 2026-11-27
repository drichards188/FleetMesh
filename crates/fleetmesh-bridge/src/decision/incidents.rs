//! Incidents (docs/design/incidents.md section 2): the items of one cause at one hub over the last
//! ten minutes, so the person answers the pattern instead of each item. A view only: computed from
//! the Inbox, the watch officer's items and what each settled lately, never logged, and deciding
//! nothing.
//!
//! An item's cause is the family of its reason codes (a held order's non-passing checks, a failed
//! delivery's refusing checks, else its alert's kind). Items of one hub whose codes are all of one
//! family, seen within [`WINDOW_MS`], make an incident once there are [`MIN_ITEMS`] of them and one
//! was seen within [`ACTIVE_MS`]: open ones (the person's, or the officer's still) and ones already
//! settled (decided, turned down, expired, withdrawn). An item whose codes span families is in
//! none: it stands on its own, and each incident lists the open items at its hub that are not part
//! of it, so grouping never hides the odd one out.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

/// This many items of one cause at one hub make an incident.
pub const MIN_ITEMS: usize = 3;
/// How far back an incident counts its items, ms.
pub const WINDOW_MS: i64 = 600_000;
/// An incident is open while one of its items was seen this recently, ms.
pub const ACTIVE_MS: i64 = 180_000;

/// An item as the grouping reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// `intent:<id>` or `alert:<id>`, as triage names it
    pub key: String,
    /// `held` or `alert`
    pub class: &'static str,
    /// the order's kind or the alert's
    pub kind: String,
    /// the hub of its vehicle: at it now, else its base
    pub hub: Option<String>,
    /// its reason codes (none for an alert that names no check)
    pub codes: Vec<String>,
    /// in the Inbox now (else settled since)
    pub open: bool,
    /// open and in the person's Inbox (an open one not: the officer's still)
    pub mine: bool,
    /// when it was last seen open (the decision clock, ms)
    pub last_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Incident {
    /// `<family>@<hub>`
    pub id: String,
    pub family: &'static str,
    pub hub: String,
    /// its items in the person's Inbox now (their cards fold into its card), held orders first
    pub items: Vec<String>,
    /// its items over the window: held orders and alerts
    pub held: usize,
    pub alerts: usize,
    /// of them: in the person's Inbox now, with the watch officer now, settled since
    pub mine: usize,
    pub with_officer: usize,
    pub settled: usize,
    /// the reason codes among its items
    pub codes: Vec<String>,
    /// the person's open items at its hub of another cause, or of several: never folded into it
    pub others: Vec<String>,
}

/// The family of a reason code.
pub fn family(code: &str) -> Option<&'static str> {
    let c = code;
    if c.starts_with("WX_") || c == "DZ_GUST_AT_ZONE" || c.starts_with("PAYLOAD_") || c.starts_with("TIDE_") {
        Some("weather")
    } else if c.starts_with("SEP_TRAFFIC") || c.starts_with("SEP_SHIP") || c == "SEP_PICTURE_DEGRADED" {
        Some("traffic")
    } else if c.starts_with("SEP_") {
        Some("separation")
    } else if c.starts_with("NAV_") {
        Some("navigation")
    } else if c.starts_with("LINK_") {
        Some("link")
    } else if c.starts_with("ENERGY_") {
        Some("energy")
    } else if c.starts_with("GEOFENCE_") {
        Some("airspace")
    } else if c.starts_with("DZ_") {
        Some("drop zone")
    } else {
        None
    }
}

/// The family of an alert that names no check, by its kind.
fn kind_family(kind: &str) -> Option<&'static str> {
    if kind.starts_with("traffic_") {
        Some("traffic")
    } else if kind == "nav_monitor" {
        Some("navigation")
    } else if kind == "lost_link" {
        Some("link")
    } else if kind == "drop_not_clear" {
        Some("drop zone")
    } else {
        None
    }
}

/// An item's one cause: the family all its codes share (its kind's when it has none), or none
/// when they span families or say nothing.
pub fn cause(item: &Item) -> Option<&'static str> {
    if item.codes.is_empty() {
        return kind_family(&item.kind);
    }
    let fams: BTreeSet<Option<&'static str>> = item.codes.iter().map(|c| family(c)).collect();
    match fams.into_iter().collect::<Vec<_>>().as_slice() {
        [Some(f)] => Some(f),
        _ => None,
    }
}

/// The incidents among `items` at `now_ms`, the largest first (then by id).
pub fn group(items: &[Item], now_ms: i64) -> Vec<Incident> {
    let mut by: BTreeMap<(&'static str, String), Vec<&Item>> = BTreeMap::new();
    for i in items.iter().filter(|i| now_ms - i.last_ms <= WINDOW_MS) {
        if let (Some(f), Some(h)) = (cause(i), i.hub.as_ref()) {
            by.entry((f, h.clone())).or_default().push(i);
        }
    }
    let mut out: Vec<Incident> = by
        .into_iter()
        .filter(|(_, v)| v.len() >= MIN_ITEMS && v.iter().any(|i| now_ms - i.last_ms <= ACTIVE_MS))
        .map(|((family, hub), mut v)| {
            v.sort_by_key(|i| (i.class != "held", i.key.clone()));
            let keys: BTreeSet<&str> = v.iter().map(|i| i.key.as_str()).collect();
            let others = items
                .iter()
                .filter(|i| i.mine && i.hub.as_deref() == Some(hub.as_str()) && !keys.contains(i.key.as_str()))
                .map(|i| i.key.clone())
                .collect();
            let codes: BTreeSet<String> = v.iter().flat_map(|i| i.codes.iter().cloned()).collect();
            Incident {
                id: format!("{family}@{hub}"),
                family,
                held: v.iter().filter(|i| i.class == "held").count(),
                alerts: v.iter().filter(|i| i.class == "alert").count(),
                mine: v.iter().filter(|i| i.mine).count(),
                with_officer: v.iter().filter(|i| i.open && !i.mine).count(),
                settled: v.iter().filter(|i| !i.open).count(),
                items: v.iter().filter(|i| i.mine).map(|i| i.key.clone()).collect(),
                codes: codes.into_iter().collect(),
                others,
                hub,
            }
        })
        .collect();
    out.sort_by(|a, b| (b.held + b.alerts).cmp(&(a.held + a.alerts)).then(a.id.cmp(&b.id)));
    out
}

impl super::Decision {
    /// The incidents now: every Inbox item open now (the person's and the watch officer's,
    /// `all_held` and `all_alerts`) remembered with the ones seen within the window and settled
    /// since; each marked the person's when their Inbox shows it.
    pub(super) fn incidents_of(&self, all_held: &[serde_json::Value], all_alerts: &[super::alerts::Alert]) -> Vec<Incident> {
        let now = fleetmesh_decide::Clock::now(&self.clock).0;
        let mut open: Vec<Item> = Vec::new();
        for v in all_held {
            let Some(id) = v["id"].as_u64() else { continue };
            let codes: Vec<String> = v["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["outcome"] != "pass")
                .flat_map(|c| c["codes"].as_array().cloned().unwrap_or_default())
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect();
            let uid = v["uids"].as_array().and_then(|u| u.first()).and_then(|u| u.as_u64().or_else(|| u.as_str().and_then(|s| s.parse().ok())));
            let key = format!("intent:{id}");
            let mine = self.person_sees(&key);
            open.push(Item { key, class: "held", kind: v["kind"].as_str().unwrap_or("").to_string(), hub: uid.and_then(|u| self.hub_of(u)), codes, open: true, mine, last_ms: now });
        }
        for a in all_alerts {
            let key = format!("alert:{}", a.id);
            let mine = self.person_sees(&key);
            open.push(Item { key, class: "alert", kind: a.kind.to_string(), hub: self.hub_of(a.uid), codes: self.alert_codes(&a.id), open: true, mine, last_ms: now });
        }
        let items: Vec<Item> = {
            let mut rt = self.rt();
            let seen = &mut rt.incident_seen;
            for i in seen.values_mut() {
                i.open = false;
                i.mine = false;
            }
            for i in open {
                seen.insert(i.key.clone(), i);
            }
            seen.retain(|_, i| now - i.last_ms <= WINDOW_MS);
            seen.values().cloned().collect()
        };
        group(&items, now)
    }

    /// The hub a vehicle is at (within a hub's catchment), else its base.
    pub(super) fn hub_of(&self, uid: crate::vehicle::Uid) -> Option<String> {
        let cell = self.world.vehicles().into_iter().find(|c| c.uid == uid)?;
        let (pos, ap_home) = cell.read(|v| (v.pos, v.home));
        let at = pos
            .map(|p| fleetmesh_decide::LonLat::new(p.1, p.0))
            .and_then(|p| self.places.nearest_hub(p))
            .filter(|(h, d)| *d <= super::delivery_rt::hub_catchment(h.radius_m))
            .map(|(h, _)| h.id.clone());
        at.or_else(|| self.home_hub(&super::snapshot::duid(uid), ap_home.map(|h| fleetmesh_decide::LonLat::new(h.1, h.0))))
    }

    /// The codes behind a delivery's alert: the checks that refused or held its last try.
    fn alert_codes(&self, id: &str) -> Vec<String> {
        let Some(p) = id.strip_prefix("delivery:") else { return Vec::new() };
        let rt = self.rt();
        let Some(r) = rt.parcels.get(p) else { return Vec::new() };
        r.failure.as_ref().or(r.ends.last()).map(|e| e.codes.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn held(id: u32, hub: &str, codes: &[&str]) -> Item {
        Item { key: format!("intent:{id}"), class: "held", kind: "pickup".into(), hub: Some(hub.into()), codes: codes.iter().map(|c| c.to_string()).collect(), open: true, mine: true, last_ms: NOW }
    }

    fn alert(id: &str, kind: &str, hub: &str, codes: &[&str]) -> Item {
        Item { key: format!("alert:{id}"), class: "alert", kind: kind.into(), hub: Some(hub.into()), codes: codes.iter().map(|c| c.to_string()).collect(), open: true, mine: true, last_ms: NOW }
    }

    #[test]
    fn items_of_one_cause_at_one_hub_are_one_incident_and_the_odd_one_out_stays_out() {
        let items = vec![
            held(1, "hub-crissy", &["WX_GUST_LIMIT_MARGINAL"]),
            held(2, "hub-crissy", &["WX_GUST_LIMIT_MARGINAL", "PAYLOAD_NEAR_MAX"]),
            alert("delivery:p-4", "delivery_failed", "hub-crissy", &["DZ_GUST_AT_ZONE"]),
            held(3, "hub-crissy", &["NAV_DEGRADED"]),
            held(5, "hub-crissy", &["WX_GUST_LIMIT_MARGINAL", "ENERGY_MARGIN_THIN"]),
            held(6, "hub-pier", &["WX_GUST_LIMIT_MARGINAL"]),
        ];
        let got = group(&items, NOW);
        assert_eq!(got.len(), 1, "{got:?}");
        let i = &got[0];
        assert_eq!((i.id.as_str(), i.family), ("weather@hub-crissy", "weather"));
        assert_eq!(i.items, vec!["intent:1", "intent:2", "alert:delivery:p-4"], "held orders first; the alert joins by its codes");
        assert_eq!((i.held, i.alerts, i.mine, i.with_officer, i.settled), (2, 1, 3, 0, 0));
        assert_eq!(i.others, vec!["intent:3", "intent:5"], "navigation, and wind with energy: not folded in");
    }

    #[test]
    fn an_incident_counts_what_the_officer_holds_and_has_settled_in_its_window() {
        let items = vec![
            Item { mine: false, ..held(1, "hub-crissy", &["WX_GUST_LIMIT_MARGINAL"]) },
            Item { open: false, mine: false, last_ms: NOW - 120_000, ..held(2, "hub-crissy", &["WX_GUST_LIMIT_MARGINAL"]) },
            Item { open: false, mine: false, last_ms: NOW - 300_000, ..held(3, "hub-crissy", &["WX_GUST_LIMIT_MARGINAL"]) },
            Item { open: false, mine: false, last_ms: NOW - 700_000, ..held(4, "hub-crissy", &["WX_GUST_LIMIT_MARGINAL"]) },
        ];
        let got = group(&items, NOW);
        assert_eq!(got.len(), 1, "the officer's and the settled count; one past the window does not");
        assert_eq!((got[0].mine, got[0].with_officer, got[0].settled), (0, 1, 2));
        assert!(got[0].items.is_empty(), "no card of the person's to fold in");
        let quiet: Vec<Item> = items.iter().map(|i| Item { open: false, last_ms: NOW - 200_000, ..i.clone() }).collect();
        assert!(group(&quiet, NOW).is_empty(), "nothing seen for three minutes: the incident has closed");
    }

    #[test]
    fn fewer_than_three_or_no_hub_is_no_incident() {
        assert!(group(&[held(1, "a", &["WX_GUST_LIMIT_MARGINAL"]), held(2, "a", &["WX_GUST_LIMIT_MARGINAL"])], NOW).is_empty());
        let no_hub = Item { hub: None, ..held(1, "a", &["WX_GUST_LIMIT_MARGINAL"]) };
        assert!(group(&[no_hub.clone(), no_hub.clone(), no_hub], NOW).is_empty());
        let traffic = [alert("t1", "traffic_noncooperative_near", "a", &[]), alert("t2", "traffic_noncooperative_near", "a", &[]), alert("t3", "traffic_conflict", "a", &[])];
        assert_eq!(group(&traffic, NOW)[0].family, "traffic", "an alert with no codes by its kind");
    }
}
