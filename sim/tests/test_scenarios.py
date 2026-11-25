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
