"""The main menu's list and what a start may ask for (eval/launcher.py): only the delivery
scenarios in sim/scenarios and the options scripts/scenario.sh takes, each checked before
anything reaches a command line."""

import pytest

from eval import launcher as L

NAMES = ["harbour-city", "wind-event", "coastal-boat"]


@pytest.fixture(scope="module")
def entries():
    return [e for e in L.catalog() if e["kind"] == "scenario"]


def by_id(entries, sid):
    return next(e for e in entries if e["id"] == sid)


def test_the_menu_lists_the_delivery_scenarios_in_their_order(entries):
    assert [e["id"] for e in entries] == NAMES
    assert entries[0]["default"] and not any(e["default"] for e in entries[1:])
    for e in entries:
        assert e["runnable"] and e["title"] and e["brief"] and e["summary"]["bullets"]
        assert e["cli"] == f"make start SCENARIO={e['id']}"


def test_each_scenario_offers_its_sizes_rates_and_procedures(entries):
    for e in entries:
        assert [k["scale"] for k in e["scales"]] == list(L.SCALES)
        sizes = [k["vehicles"] for k in e["scales"]]
        assert sizes[0] == e["vehicles"] and sizes == sorted(sizes, reverse=True) and sizes[-1] >= 1
        assert e["rate"] in e["rates"]
        assert e["policy"] == "standard" and set(e["policies"]) == {"standard", "strict"}


def test_the_open_world_comes_last_when_it_runs_here():
    world = [e for e in L.catalog() if e["kind"] == "open_world"]
    if L.sitl_built():
        assert world and L.catalog()[-1]["kind"] == "open_world"
    else:
        assert not world


def test_a_start_takes_only_what_the_scenario_offers(entries):
    c = L.validate_start({"scenario": "harbour-city", "scale": 0.25, "policy": "strict"}, entries)
    assert (c.kind, c.scale, c.rate, c.policy) == ("scenario", 0.25, 1, "strict")
    assert L.command(c) == ["scripts/scenario.sh", "harbour-city"]
    assert L.cli_text(c, by_id(entries, "harbour-city")) == "make start SCENARIO=harbour-city SCALE=0.25 POLICY=sim/policy-strict.toml"
    d = L.validate_start({"scenario": "wind-event"}, entries)
    assert (d.scale, d.rate, d.policy) == (1.0, by_id(entries, "wind-event")["rate"], "standard")
    assert L.cli_text(d, by_id(entries, "wind-event")) == "make start SCENARIO=wind-event"


@pytest.mark.parametrize(
    "body",
    [
        {"scenario": "no-such"},
        {"scenario": "../harbour-city"},
        {"scenario": "harbour-city", "scale": 0.3},
        {"scenario": "harbour-city", "scale": True},
        {"scenario": "harbour-city", "rate": 5},
        {"scenario": "harbour-city", "policy": "sim/other.toml"},
        {"scenario": "harbour-city", "operator": "live"},
        {"scenario": "harbour-city", "replace": "yes"},
        ["harbour-city"],
    ],
)
def test_a_start_outside_the_menu_is_refused(entries, body):
    with pytest.raises(L.BadRequest):
        L.validate_start(body, entries)


def test_a_launch_runs_its_own_choice_whatever_the_shell_exported(entries):
    e = by_id(entries, "harbour-city")
    shell = {"PATH": "/bin", "SCALE": "4", "FLEETMESH_RATE": "9", "POLICY": "x", "FLEETMESH_SCENARIO_DIR": "elsewhere", "MAKEFLAGS": "-j"}
    env = L.launch_env(shell, 5173, L.validate_start({"scenario": "harbour-city"}, entries), e)
    assert env["PATH"] == "/bin" and env["FLEETMESH_WEB"] == "0" and env["FLEETMESH_WEB_PORT"] == "5173"
    assert "SCALE" not in env and "FLEETMESH_SCENARIO_DIR" not in env and "MAKEFLAGS" not in env
    assert env["FLEETMESH_RATE"] == f"{e['rate']:g}" and env["POLICY"] == "sim/policy.toml"
    quarter = L.launch_env(shell, 5173, L.validate_start({"scenario": "harbour-city", "scale": 0.25, "policy": "strict"}, entries), e)
    assert quarter["SCALE"] == "0.25" and quarter["POLICY"] == "sim/policy-strict.toml"
