"""The three first delivery scenarios (sim/scenarios/*, docs/design/scenarios.md) are well formed:
the files parse, the fleet plans on stand-ins, every parcel and demand names a hub and a drop
zone that exist, link and GNSS events are in the formats the emulator and stand-ins read."""

import os
import sys
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "scripts"))
import scenario_env  # noqa: E402
import swarm  # noqa: E402

NAMES = ["harbour-city", "wind-event", "coastal-boat"]


def load(name, file):
    return tomllib.loads((ROOT / "scenarios" / name / file).read_text())


def test_the_three_scenarios_exist():
    assert set(NAMES) <= set(scenario_env.names())


@pytest.mark.parametrize("name", NAMES)
def test_fleet_plans_on_standins(name):
    sc = load(name, "scenario.toml")
    fleet = swarm.load_fleet(ROOT / "scenarios" / name / sc["fleet"])
    vehicles = swarm.plan(fleet, swarm.load_platforms())
    assert vehicles and all(v["sim"] == "standin" for v in vehicles)


@pytest.mark.parametrize("name", NAMES)
def test_scale_keeps_every_unit(name, monkeypatch):
    sc = load(name, "scenario.toml")
    monkeypatch.setenv("FLEETMESH_SCENARIO_SCALE", "0.01")
    fleet = swarm.load_fleet(ROOT / "scenarios" / name / sc["fleet"])
    assert all(u["count"] >= 1 for u in fleet["unit"])
    monkeypatch.delenv("FLEETMESH_SCENARIO_SCALE")


@pytest.mark.parametrize("name", NAMES)
def test_parcels_and_demand_name_places_that_exist(name):
    d = load(name, "dispatch.toml")
    hubs = {h["id"] for h in d["hub"]}
    zones = {z["id"] for z in d["dropzone"]}
    for p in d.get("parcel", []):
        assert p["from"] in hubs and p["to"] in zones, p
    g = d.get("demand")
    if g:
        assert set(g["hubs"]) <= hubs and set(g["zones"]) <= zones
    for z in d["dropzone"]:
        if z.get("hub"):
            assert z["hub"] in hubs


@pytest.mark.parametrize("name", NAMES)
def test_every_unit_has_a_home_base_its_vehicles_start_at(name):
    """Each unit names its home base (docs/design/fleet-organisation.md), a hub of the scenario, and
    starts at it (within the bridge's catchment, max(3 x radius, 150 m)), so the hub it is based at
    is the hub it starts at; each site groups hubs of the scenario, each hub at most once."""
    import math
    sc = load(name, "scenario.toml")
    d = load(name, "dispatch.toml")
    hubs = {h["id"]: h for h in d["hub"]}
    fleet = swarm.load_fleet(ROOT / "scenarios" / name / sc["fleet"])
    for u in fleet["unit"]:
        assert u.get("home_hub") in hubs, u
        h = hubs[u["home_hub"]]
        lat, lon = u["spawn"][0], u["spawn"][1]
        dn = (lat - h["lat"]) * 111_320
        de = (lon - h["lon"]) * 111_320 * math.cos(math.radians(lat))
        assert math.hypot(dn, de) <= max(3 * h.get("radius_m", 50), 150), (u, h)
    vehicles = swarm.plan(fleet, swarm.load_platforms())
    assert all(v["home_hub"] in hubs for v in vehicles)
    seen = set()
    for site in d.get("site", []):
        assert set(site) == {"id", "name", "hubs"}, site
        assert set(site["hubs"]) <= set(hubs) and not set(site["hubs"]) & seen, site
        seen |= set(site["hubs"])


@pytest.mark.parametrize("name", NAMES)
def test_every_vehicle_stands_on_its_home_base(name):
    """Rules 5: each vehicle's stand is where it was launched, and it lands there again; the order
    rule takes a point within its home hub's radius, so every stand lies within it (and a boat's
    berth is tied up until it first casts off)."""
    import math
    sc = load(name, "scenario.toml")
    d = load(name, "dispatch.toml")
    hubs = {h["id"]: h for h in d["hub"]}
    fleet = swarm.load_fleet(ROOT / "scenarios" / name / sc["fleet"])
    for v in swarm.plan(fleet, swarm.load_platforms()):
        h = hubs[v["home_hub"]]
        lat, lon = v["home"][0], v["home"][1]
        dn = (lat - h["lat"]) * 111_320
        de = (lon - h["lon"]) * 111_320 * math.cos(math.radians(lat))
        assert math.hypot(dn, de) <= h.get("radius_m", 50), (v["callsign"], round(math.hypot(dn, de)), h["id"], h.get("radius_m"))
        if v["platform"] == "delivery_boat":
            assert v.get("moored"), v["callsign"]


def test_harbour_city_demand_fits_its_bases():
    """A base's parcels are what its vehicles carry: the Pier's drone pads take drone-sized ones
    (within the payload rule's hold band of a drone's 2.5 kg in a moderate gust), and a boat's
    parcel is due in its run's time."""
    g = load("harbour-city", "dispatch.toml")["demand"]
    assert g["mass_by_hub"]["hub-pier"][1] <= 2.16
    assert set(g["mass_by_hub"]) <= set(g["hubs"]) and set(g.get("due_by_hub", {})) <= set(g["hubs"])
    assert g["due_by_hub"]["hub-pier-dock"] >= 3600


@pytest.mark.parametrize("name", NAMES)
def test_events_are_in_the_formats_the_readers_take(name):
    ev = load(name, "events.toml")
    for e in ev.get("event", []):
        assert ("at_s" in e) != ("at_scenario" in e), e
        assert "circle" in e or "uids" in e, e
        if "gnss" in e:
            assert e["gnss"]["effect"] in ("jam", "spoof", "degrade")
    times = [f["at_s"] for f in ev.get("weather", {}).get("timeline", [])]
    assert times == sorted(times)


@pytest.mark.parametrize("name", NAMES)
def test_the_environment_a_run_gets(name):
    env = scenario_env.env_for(name)
    assert env["FLEETMESH_SCENARIO_DIR"].endswith(name)
    assert (Path(env["FLEETMESH_FLEET"])).exists() or (ROOT.parent / env["FLEETMESH_FLEET"]).exists()
    assert env["FLEETMESH_AT"].endswith("Z")


@pytest.mark.parametrize("name", NAMES)
def test_every_scenario_names_a_traffic_file_the_run_exports(name):
    sc = tomllib.loads((ROOT / "scenarios" / name / "scenario.toml").read_text())
    assert sc["traffic"] == "traffic.toml"
    assert (ROOT / "scenarios" / name / sc["traffic"]).exists()
    env = scenario_env.env_for(name)
    assert env["FLEETMESH_TRAFFIC_SCENARIO"] == f"sim/scenarios/{name}/traffic.toml"
    assert (ROOT.parent / env["FLEETMESH_TRAFFIC_SCENARIO"]).exists()


def test_the_wind_event_ramp_crosses_the_drone_limits():
    frames = load("wind-event", "events.toml")["weather"]["timeline"]
    gusts = [f["values"]["gust_mps"] for f in frames]
    assert min(gusts) < 11.0 < 14.0 < max(gusts)


@pytest.mark.parametrize("name", NAMES)
def test_every_drop_zone_is_an_approved_delivery_point(name):
    # rules 2: delivery points are approved ahead of time (surveyed, or chosen by the customer);
    # a zone without `approval` would be a new point that a person approves once
    d = load(name, "dispatch.toml")
    for z in d["dropzone"]:
        assert z.get("approval") in ("survey", "customer"), z
    a = d.get("automation", {})
    assert set(a) <= {"depart_wait_s", "hover_wait_s", "retry_after_s", "max_tries", "hold_retry_s", "hold_retry_max_s", "issue_show_s"}, a


def test_the_sim_places_are_approved_delivery_points():
    import tomllib

    d = tomllib.loads((ROOT / "hubs.toml").read_text())
    for z in d["dropzone"]:
        assert z.get("approval") in ("survey", "customer"), z


def test_policy_in_the_environment_runs_a_scenario_under_the_strict_procedure(monkeypatch):
    monkeypatch.setenv("POLICY", "sim/policy-strict.toml")
    assert scenario_env.env_for(NAMES[0])["FLEETMESH_POLICY"] == "sim/policy-strict.toml"
    monkeypatch.delenv("POLICY")
    assert scenario_env.env_for(NAMES[0])["FLEETMESH_POLICY"] == "sim/policy.toml"
