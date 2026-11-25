//! Management by exception through the bridge (rules 2, docs/DECISIONS.md 2026-11-25): the look
//! at a drop zone before departure reads as the drop-zone rule reads it, the drop-time look needs
//! a fresh clear look and is given to deliveries only, the holds automation waits out are never a
//! limit, a zone's approval follows standing approvals or the strict procedure, and an issue
//! automation raised is an alert that says what happened, what automation did and what to do.

use super::*;
use crate::decision::delivery_rt::{Issue, WAIT_OUT};
use fleetmesh_decide::delivery::{DzObservation, ObservedState};
use fleetmesh_decide::intent::Release;

fn now(r: &Rig) -> fleetmesh_decide::Time {
    fleetmesh_decide::Clock::now(&r.clock)
}

/// What the zone's feed saw `ago_ms` before now.
fn see(r: &Rig, zone: &str, state: ObservedState, count: u32, ago_ms: i64) {
    let t = now(r).plus_ms(-ago_ms);
    r.d.feed().observations.insert(zone.into(), DzObservation { t, state, count });
}

fn deliver_to(r: &Rig, zone: &str) -> fleetmesh_decide::Task {
    fleetmesh_decide::Task::Deliver {
        parcel: "p-1".into(),
        drop_zone: zone.into(),
        at: r.d.places().drop_zones[zone].at(),
        route: Vec::new(),
        release: Release::Winch,
        window_end: None,
    }
}

#[tokio::test(start_paused = true)]
async fn the_look_before_departure_reads_as_the_rule_and_the_drop_time_look_needs_a_fresh_clear_one() {
    let r = rig();
    let t = now(&r);
    // a populated doorstep (dz-harbour-17): no look at all, then each kind of look
    assert_eq!(r.d.zone_look("dz-harbour-17", t, false), Err("no look within 30 s".into()));
    assert_eq!(r.d.zone_look("dz-harbour-17", t, true), Err("no look within 30 s".into()));
    see(&r, "dz-harbour-17", ObservedState::Clear, 0, 1_000);
    assert!(r.d.zone_look("dz-harbour-17", t, false).is_ok());
    assert!(r.d.zone_look("dz-harbour-17", t, true).is_ok());
    see(&r, "dz-harbour-17", ObservedState::Occupied, 2, 1_000);
    assert_eq!(r.d.zone_look("dz-harbour-17", t, false), Err("occupied, 2 seen".into()));
    assert_eq!(r.d.zone_look("dz-harbour-17", t, true), Err("occupied, 2 seen".into()));
    see(&r, "dz-harbour-17", ObservedState::Unknown, 0, 1_000);
    assert_eq!(r.d.zone_look("dz-harbour-17", t, false), Err("the look could not tell".into()));
    see(&r, "dz-harbour-17", ObservedState::Clear, 0, 31_000);
    assert!(r.d.zone_look("dz-harbour-17", t, false).is_err(), "a look older than obs_max_age_s is stale");
    // a zone where nobody lives (dz-pier-dock): the rule asks no look before departure, but over it
    // the vehicle must see it clear itself
    assert!(r.d.zone_look("dz-pier-dock", t, false).is_ok());
    assert!(r.d.zone_look("dz-pier-dock", t, true).is_err());
    see(&r, "dz-pier-dock", ObservedState::Occupied, 1, 0);
    assert!(r.d.zone_look("dz-pier-dock", t, false).is_err(), "people there: never");
}

#[tokio::test(start_paused = true)]
async fn the_drop_time_look_is_given_to_deliveries_and_reads_the_zones_feed() {
    let r = rig();
    assert!(r.d.drop_look_hook(&fleetmesh_decide::Task::hold()).is_none());
    let look = r.d.drop_look_hook(&deliver_to(&r, "dz-harbour-17")).expect("a delivery looks");
    assert_eq!(look.wait, Duration::from_secs(120), "the default hover wait");
    see(&r, "dz-harbour-17", ObservedState::Occupied, 3, 0);
    assert_eq!((look.look)(), Err("occupied, 3 seen".into()));
    see(&r, "dz-harbour-17", ObservedState::Clear, 0, 0);
    assert_eq!((look.look)(), Ok("clear".into()));
}

#[test]
fn automation_waits_out_only_stale_facts_traffic_and_the_look_never_a_limit() {
    // the delivery rules' codes, and (rules 3) the separation rule's for other traffic
    let rules = std::fs::read_to_string(root().join("crates/fleetmesh-decide/src/delivery_rules.rs")).expect("rules")
        + &std::fs::read_to_string(root().join("crates/fleetmesh-decide/src/traffic.rs")).expect("traffic rules");
    for code in WAIT_OUT {
        assert!(rules.contains(&format!("\"{code}\"")), "{code} is a code the delivery rules give");
        let family_ok = code.starts_with("SEP_")
            || code.starts_with("DZ_PEOPLE")
            || code.starts_with("DZ_OBSERVATION")
            || code.ends_with("_UNKNOWN");
        assert!(family_ok, "{code}: automation waits out traffic, the look and stale facts only");
    }
    // the issues the person keeps: weather, energy, navigation, link, geofence, payload, approval
    for kept in [
        "WX_GUST_LIMIT_MARGINAL", "WX_WIND_LIMIT_MARGINAL", "ENERGY_MARGIN_THIN", "ENERGY_NO_HUB_REACHABLE",
        "NAV_DEGRADED_ACCURACY", "NAV_UNTRUSTED", "LINK_COVERAGE_GAP", "LINK_BACKUP_SLOW", "GEOFENCE_RESTRICTED",
        "GEOFENCE_TEMP_SOON", "PAYLOAD_NEAR_MAX", "TIDE_SHALLOW", "DZ_GUST_AT_ZONE", "DZ_NOT_APPROVED", "DZ_UNCONFIRMED",
        "ENERGY_ENDURANCE_UNKNOWN",
    ] {
        assert!(!WAIT_OUT.contains(&kept), "{kept} is a person's");
    }
}

#[tokio::test(start_paused = true)]
async fn a_zones_approval_follows_standing_approvals_or_the_strict_procedure() {
    let r = rig();
    let t = now(&r);
    assert!(r.d.zone_approved_now("dz-harbour-17", t), "a doorstep the customer chose");
    assert!(r.d.zone_approved_now("dz-crissy-pad", t), "a hub pad");
    assert!(!r.d.zone_approved_now("dz-nowhere", t));
    let strict = rig_with(Policy::load(&root().join("sim/policy-strict.toml")).expect("strict policy"));
    let t = now(&strict);
    assert!(!strict.d.zone_approved_now("dz-harbour-17", t), "the strict procedure: a person confirms it first");
    assert!(strict.d.zone_approved_now("dz-crissy-pad", t), "a hub pad needs no confirmation in either");
}

#[tokio::test(start_paused = true)]
async fn an_issue_is_an_alert_saying_what_happened_what_automation_did_and_what_to_do() {
    let r = rig();
    r.vehicle(7, Firmware::Copter, "delivery_drone", (LAT, LON));
    let until = now(&r).plus_ms(300_000);
    r.d.raise(
        "delivery:p-9".into(),
        Issue {
            kind: "drop_not_clear",
            uid: Some(fleetmesh_decide::Uid::new("7")),
            problem: "p-9 was not delivered: over dz-harbour-17, occupied, 4 seen after waiting 60 s over it".into(),
            did: "7 did not let it go and is bringing it back to hub-presidio".into(),
            recommend: "nothing is needed for safety; automation tries p-9 again in 10 min".into(),
            at_wall_s: clock::wall(),
            until_ms: Some(until.0),
            intent: Some(IntentId(3)),
            track_ids: Vec::new(),
        },
    );
    let alerts = r.d.current_alerts();
    let a = alerts.iter().find(|a| a.id == "delivery:p-9").expect("the issue is in the inbox");
    assert_eq!((a.kind, a.severity, a.uid, a.intent_id), ("drop_not_clear", alerts::Severity::Medium, 7, Some(3)));
    let i = a.issue.as_ref().expect("its three parts");
    assert!(i.problem.contains("not delivered") && i.did.contains("bringing it back") && i.recommend.contains("tries p-9 again"), "{i:?}");
    assert!(a.detail.contains(&i.problem) && a.detail.contains(&i.did) && a.detail.contains(&i.recommend), "{}", a.detail);
    // it says it once in the log, and goes from the inbox after its time
    assert!(r.log.text().contains("drop_not_clear: p-9 was not delivered"), "logged as a delivery event");
    r.clock.advance_ms(301_000);
    assert!(!r.d.current_alerts().iter().any(|a| a.id == "delivery:p-9"));
    r.replays();
}

#[tokio::test(start_paused = true)]
async fn a_pickup_on_the_hubs_pads_loads_where_the_vehicle_stands() {
    let r = rig();
    let hub = r.d.places().hubs["hub-presidio"].clone();
    // one drone 20 m from the hub's point (inside its 60 m landing zone), one 300 m off
    let near = (hub.lat + 20.0 / 111_320.0, hub.lon);
    let far = (hub.lat + 300.0 / 111_320.0, hub.lon);
    r.vehicle(8, Firmware::Copter, "delivery_drone", near);
    r.vehicle(9, Firmware::Copter, "delivery_drone", far);
    sleep(Duration::from_millis(500)).await;
    let task = fleetmesh_decide::Task::Pickup {
        parcel: "p-1".into(),
        site: fleetmesh_decide::intent::Place { id: Some("hub-presidio".into()), at: hub.at() },
        load_by_s: None,
    };
    let order_for = |uid: Uid| {
        let req = task_request(&task, &[fleetmesh_decide::Uid::new(uid.to_string())]).expect("request");
        crate::tasks::TaskOrder::new(req, &r.d.fleet()).expect("order")
    };
    let mut on_pad = order_for(8);
    r.d.pickup_in_place(&task, &mut on_pad);
    let at = on_pad.params.route.as_ref().and_then(|p| p.first().copied()).expect("a route");
    let moved = fleetmesh_decide::geo::haversine_m(LonLat::new(at[0], at[1]), LonLat::new(near.1, near.0));
    assert!(moved < 2.0, "it loads where it stands: {at:?}");
    let mut away = order_for(9);
    r.d.pickup_in_place(&task, &mut away);
    assert_eq!(away.params.route, Some(vec![[hub.lon, hub.lat]]), "a vehicle outside the landing zone flies to the hub");
    // the task layer then loads it with no flight
    assert!(fleetmesh_decide::geo::haversine_m(LonLat::new(at[0], at[1]), hub.at()) <= hub.radius_m);
}

// ---- rules 3: the picture of other traffic ---------------------------------------------------------

/// An SBS-1 airborne position (`MSG,3`) and velocity (`MSG,4`) as a receiver serves them.
fn sbs_pair(session: u32, hex: &str, t_ms: i64, lat: f64, lon: f64, alt_ft: u32, kt: u32, track: u32) -> [String; 2] {
    let d = chrono::DateTime::from_timestamp_millis(t_ms).expect("time");
    let (date, time) = (d.format("%Y/%m/%d"), d.format("%H:%M:%S%.3f"));
    [
        format!("MSG,3,{session},1,{hex},1,{date},{time},{date},{time},,{alt_ft},,,{lat:.5},{lon:.5},,,0,0,0,0"),
        format!("MSG,4,{session},1,{hex},1,{date},{time},{date},{time},,,{kt},{track},,,0,,0,0,0,0"),
    ]
}

#[tokio::test(start_paused = true)]
async fn the_picture_of_other_traffic_is_what_a_decision_reads_and_what_the_api_serves() {
    let r = rig();
    r.vehicle(7, Firmware::Copter, "delivery_drone", (LAT, LON));
    sleep(Duration::from_millis(500)).await;
    // no source: no picture, and the API says so (404 is the route's; the body here)
    assert!(r.d.traffic().built().is_none());
    assert!(r.d.snapshot().delivery.as_ref().is_some_and(|d| d.picture.is_none()), "a run with no traffic source reads no picture");
    let pic = r.d.traffic();
    pic.attach("in_process");
    pic.set_sensors(&[json!({"id": "net-adsb", "kind": "adsb_1090", "num": 9, "owner": "network", "nominal_range_m": 400000.0})], &BTreeSet::new());
    // a helicopter at 500 ft, 600 m north of the drone, coming south at 80 kt: a minute of it
    let t0 = r.d.traffic_now_ms();
    for k in 0..6 {
        let t = t0 + k * 1000;
        let lat = LAT + 0.0054 - 0.00037 * k as f64;
        for l in sbs_pair(9, "A0B1C2", t, lat, LON, 500, 80, 180) {
            pic.ingest(super::traffic::Feed::Sbs, &l, t + 2_500);
        }
    }
    r.d.traffic_tick();
    let api = r.d.traffic_json(None);
    let track = api["tracks"].as_array().and_then(|t| t.first()).cloned().expect("the helicopter is a track");
    assert_eq!(track["ids"]["icao24"], json!("a0b1c2"));
    assert!(api["picture"]["feeds"]["sbs"]["connected"] == json!(true) && api["picture"]["feeds"]["sbs"].get("lines_per_s").is_some(), "{api}");
    assert!(api["disagreements"].is_array() && api["automation"].is_array(), "{api}");
    // the decision layer's inputs carry it, its altitude above mean sea level and its prediction
    let w = r.d.snapshot();
    let p = w.delivery.as_ref().and_then(|d| d.picture.as_ref()).expect("rules 3 reads the picture");
    let tf = p.tracks.first().expect("the track");
    assert_eq!(tf.alt_ref, "baro");
    assert!(tf.alt_m.is_some_and(|a| (a - 152.4).abs() < 30.0), "500 ft of pressure altitude: {tf:?}");
    assert!(p.qnh_hpa.is_none() && tf.v_sigma_m.is_some_and(|s| s >= 100.0), "no QNH in this rig: standard pressure, the vertical sigma widened");
    assert!(!tf.path.is_empty());
    assert_eq!(p.config_sha256, pic.config().1);
    // and a delivery the dispatcher would send now is held for it (automation waits it out)
    r.d.feed().parcels.insert("p-1".into(), fleetmesh_decide::delivery::ParcelFact { mass_kg: Some(0.5), class: "normal".into(), state: fleetmesh_decide::delivery::ParcelState::Loaded, vehicle: Some(fleetmesh_decide::Uid::new("7")) });
    let zone = "dz-harbour-17";
    let req = IntentRequest {
        task: "deliver".into(),
        uids: vec![7],
        params: crate::tasks::TaskParams { parcel: Some("p-1".into()), drop_zone: Some(zone.into()), release: Some("winch".into()), ..Default::default() },
        constraints: Default::default(),
        rationale: Some("test".into()),
        group: None,
    };
    let preview = r.d.agent_dry_run(&fleetmesh_decide::AgentId::new("dispatcher"), &req).await.expect("dry run");
    let sep = preview.checks.iter().find(|c| c.rule == fleetmesh_decide::RuleId::Separation).expect("separation");
    assert!(sep.codes.iter().any(|c| c == "SEP_TRAFFIC_CONFLICT"), "{sep:?}");
    assert!(sep.codes.iter().all(|c| WAIT_OUT.contains(&c.as_str())), "a conflict with traffic is automation's to wait out: {sep:?}");
}
