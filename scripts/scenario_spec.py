#!/usr/bin/env python3
"""The headless episode spec (crates/fleetmesh-headless `sim::spec::Spec`) of a delivery scenario
(sim/scenarios/<name>/): its fleet as sim/swarm.py plans it, every vehicle a stand-in, and its link
and GNSS events. Printed as one JSON line for the headless driver (the end-to-end test,
crates/fleetmesh-headless/tests/delivery_e2e.rs).

    scenario_spec.py harbour-city --scale 0.15 --workdir /tmp/ep --http-port 18765
"""
import argparse
import json
import os
import sys
import tomllib
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "sim"))
import swarm  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("scenario")
    ap.add_argument("--scale", type=float, default=1.0)
    ap.add_argument("--workdir", required=True)
    ap.add_argument("--http-port", type=int, default=18765)
    ap.add_argument("--vehicle-base", type=int, default=24000)
    ap.add_argument("--bridge-base", type=int, default=25000)
    args = ap.parse_args()
    d = REPO / "sim" / "scenarios" / args.scenario
    sc = tomllib.loads((d / "scenario.toml").read_text())
    os.environ["FLEETMESH_SCENARIO_SCALE"] = str(args.scale)
    platforms = swarm.load_platforms()
    fleet = swarm.load_fleet(d / sc.get("fleet", "fleet.toml"))
    vehicles = swarm.plan(fleet, platforms)
    for i, v in enumerate(vehicles):
        v["uid"] = i + 1
        v["sysid"] = i % 250 + 1
        v["port"] = args.vehicle_base + i
        v["sim"] = "standin"
    standins = [
        {**v, "spec": platforms[v["platform"]], "params": swarm.standin_params(v["platform"], platforms[v["platform"]])}
        for v in vehicles
    ]
    events = tomllib.loads((d / sc.get("events", "events.toml")).read_text()).get("event", [])
    epoch = datetime.fromisoformat(sc["at"].replace("Z", "+00:00")).timestamp()
    spec = {
        "seed": sc.get("seed", 1),
        "epoch": epoch,
        "root": str(REPO),
        "workdir": args.workdir,
        "http_port": args.http_port,
        "vehicle_base": args.vehicle_base,
        "bridge_base": args.bridge_base,
        "fleet": vehicles,
        "standins": standins,
        "links": str(REPO / "sim" / "links.toml"),
        "events": events,
        "space": False,
        "swarms": swarm.fleet_swarms(fleet),
    }
    print(json.dumps(spec))


if __name__ == "__main__":
    main()
