"""Unit tests for the fleet generator (sim/swarm.py): the four delivery platforms, the manifest
it writes (with `backup_link`), swarms and starting tasks. No simulator, bridge or network.

    .venv/bin/python -m pytest -q sim/tests
"""

import json
import subprocess
import sys
from pathlib import Path

import pytest

SIM = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SIM))
import swarm  # noqa: E402

DELIVERY = {"delivery_drone", "delivery_car", "delivery_boat", "delivery_plane"}
MANIFEST_KEYS = {"platform", "callsign", "sim", "home", "name", "domain", "link", "backup_link",
                 "space_class", "cruise_alt_m", "min_turn_radius_m"}


@pytest.fixture(scope="module")
def platforms():
    return swarm.load_platforms()


def test_the_catalog_is_the_four_delivery_platforms(platforms):
    assert set(platforms) == DELIVERY
    assert set(swarm.VEHICLES) == {"copter", "ugv", "boat", "plane"}
    assert {p["vehicle"] for p in platforms.values()} == set(swarm.VEHICLES)


def unit(pid, **kw):
    return {"platform": pid, "spawn": [37.8, -122.45, 0, 0], **kw}


def test_every_vehicle_carries_its_backup_link(platforms):
    fleet = {"unit": [unit(p, count=2) for p in sorted(DELIVERY)]}
    vehicles = swarm.plan(fleet, platforms)
    assert len(vehicles) == 8
    for v in vehicles:
        assert MANIFEST_KEYS <= set(v), v
        assert v["backup_link"] == platforms[v["platform"]]["backup_link"]
        assert v["backup_link"] and v["backup_link"] != v["link"]
        assert v["link"] == platforms[v["platform"]]["link"]


def test_a_units_home_hub_is_in_the_manifest_and_only_when_it_names_one(platforms):
    fleet = {"unit": [unit("delivery_drone", count=2, home_hub="hub-marina"), unit("delivery_car")]}
    vehicles = swarm.plan(fleet, platforms)
    assert [v.get("home_hub") for v in vehicles] == ["hub-marina", "hub-marina", None]
    assert "home_hub" not in vehicles[2], "a fleet without home hubs writes the manifest it always did"
    with pytest.raises(SystemExit, match="home_hub is a hub id"):
        swarm.plan({"unit": [unit("delivery_drone", home_hub=3)]}, platforms)


def test_callsigns_number_per_platform(platforms):
    fleet = {"unit": [unit("delivery_drone", count=2), unit("delivery_car"), unit("delivery_drone")]}
    assert [v["callsign"] for v in swarm.plan(fleet, platforms)] == ["DRONE 1", "DRONE 2", "CAR 1", "DRONE 3"]


def test_no_effect_or_other_non_delivery_fields_in_the_manifest(platforms):
    v = swarm.plan({"unit": [unit("delivery_plane")]}, platforms)[0]
    assert not {"armed", "sidc", "short", "role", "space_class_alt"} & set(v)


def test_an_unknown_platform_is_refused(platforms):
    with pytest.raises(SystemExit, match="unknown platform"):
        swarm.plan({"unit": [unit("no_such_platform")]}, platforms)


def test_a_platform_of_another_kind_is_refused(platforms):
    odd = {**platforms, "x": {**platforms["delivery_drone"], "vehicle": "tracker"}}
    with pytest.raises(SystemExit, match="not one of the delivery vehicles"):
        swarm.plan({"unit": [unit("x")]}, odd)


def test_sitl_command_uses_the_vehicles_firmware(platforms):
    class A:
        speedup, bridge_host, keep_eeprom = 1.0, "127.0.0.1", False

    got = {}
    for pid, spec in platforms.items():
        v = swarm.plan({"unit": [unit(pid)]}, platforms)[0]
        v.update(instance=0, sysid=1, port=14560)
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            cmd = swarm.sitl_command(v, spec, A, Path(d))
            got[pid] = Path(cmd[0]).name
    assert got == {"delivery_drone": "arducopter", "delivery_car": "ardurover",
                   "delivery_boat": "ardurover", "delivery_plane": "arduplane"}


def test_lost_link_timer_comes_from_the_link_profile(platforms):
    links = {"class": {"cellular": {"lost_link_s": 12}, "los": {"lost_link_s": 7}}}
    drone = platforms["delivery_drone"]
    assert swarm.lost_link_params("delivery_drone", drone, "arducopter", links) == {"FS_GCS_TIMEOUT": 12}
    plane = platforms["delivery_plane"]
    assert swarm.lost_link_params("delivery_plane", plane, "arduplane", links) == {"FS_LONG_TIMEOUT": 7}


def test_a_swarm_expands_in_layout_and_names_its_platform(platforms):
    fleet = {"swarm": [{"id": "RING-1", "role": "delivery", "platform": "delivery_drone", "count": 6,
                        "layout": "ring", "center": {"lat": 37.8, "lon": -122.45}, "radius_m": 100}]}
    vehicles = swarm.plan(fleet, platforms)
    assert len(vehicles) == 6 and {v["swarm"] for v in vehicles} == {"RING-1"}
    assert {v["platform"] for v in vehicles} == {"delivery_drone"}


def test_a_swarm_role_outside_the_delivery_set_is_refused():
    with pytest.raises(SystemExit, match="role"):
        swarm.fleet_swarms({"swarm": [{"id": "S", "role": "picket"}]})


def test_a_starting_task_is_routine_only():
    assert swarm.check_start({"task": "hold"}, "u") == {"task": "hold"}
    with pytest.raises(SystemExit, match="orbit, patrol or hold"):
        swarm.check_start({"task": "deliver"}, "u")


def test_cli_manifest_only_writes_fleet_json_with_backup_links(tmp_path):
    fleet = tmp_path / "f.toml"
    fleet.write_text('[[unit]]\nplatform = "delivery_boat"\ncount = 2\nspawn = [37.82, -122.41, 0, 90]\n')
    run = SIM / "run"
    existed = (run / "fleet.json").exists()
    before = (run / "fleet.json").read_bytes() if existed else None
    try:
        subprocess.run([sys.executable, str(SIM / "swarm.py"), "--fleet", str(fleet), "--manifest-only"],
                       check=True, capture_output=True)
        doc = json.loads((run / "fleet.json").read_text())
        assert [v["backup_link"] for v in doc["vehicles"]] == ["satcom_leo", "satcom_leo"]
        count = subprocess.run([sys.executable, str(SIM / "swarm.py"), "--fleet", str(fleet), "--count-only"],
                               check=True, capture_output=True, text=True).stdout.strip()
        assert count == "2"
    finally:
        if existed:
            (run / "fleet.json").write_bytes(before)
        else:
            (run / "fleet.json").unlink(missing_ok=True)


def test_the_demo_fleet_loads(platforms):
    vehicles = swarm.plan(swarm.load_fleet("delivery-demo"), platforms)
    assert {v["platform"] for v in vehicles} == DELIVERY
