#!/usr/bin/env python3
"""Print the environment a scenario (sim/scenarios/<name>/) runs under, as `export` lines for
scripts/scenario.sh: the scenario's directory (hubs, drop zones, parcels), its link and GNSS
events, its areas, its policy, its clock and, when tidewater's data for it is in the checkout, the
tidewater scenario that gives real tide, current and wind.

    eval "$(.venv/bin/python scripts/scenario_env.py harbour-city)"

`POLICY` in the environment replaces the scenario's policy file: `make start SCENARIO=harbour-city
POLICY=sim/policy-strict.toml` runs the same scenario under the strict drop-zone procedure (a person
confirms each zone every hour), the comparison setting of docs/DECISIONS.md (2026-11-25).
"""
import os
import shlex
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def names():
    return sorted(p.parent.name for p in (REPO / "sim" / "scenarios").glob("*/scenario.toml"))


def env_for(name):
    d = REPO / "sim" / "scenarios" / name
    if not (d / "scenario.toml").exists():
        sys.exit(f"no scenario {name!r}; available: {', '.join(names())}")
    sc = tomllib.loads((d / "scenario.toml").read_text())
    rel = d.relative_to(REPO)
    out = {
        "FLEETMESH_SCENARIO_NAME": name,
        "FLEETMESH_SCENARIO_DIR": str(rel),
        "FLEETMESH_AT": sc["at"],
        "FLEETMESH_RATE": str(sc.get("rate", 1)),
        "FLEETMESH_FLEET": str(rel / sc.get("fleet", "fleet.toml")),
        "FLEETMESH_AREAS": str(rel / sc.get("areas", "areas")),
        "FLEETMESH_POLICY": os.environ.get("POLICY") or sc.get("policy", "sim/policy.toml"),
    }
    events = d / sc.get("events", "events.toml")
    if events.exists():
        out["FLEETMESH_LINK_EVENTS"] = str(events.relative_to(REPO))
    tw = sc.get("tidewater")
    twfile = REPO / "tidewater" / "scenarios" / f"{tw}.toml" if tw else None
    if twfile and twfile.exists() and (REPO / ".venv/bin/tidewater").exists():
        out["FLEETMESH_SCENARIO"] = str(twfile.relative_to(REPO))
    return out


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: scenario_env.py <scenario>")
    for k, v in env_for(sys.argv[1]).items():
        print(f"export {k}={shlex.quote(v)}")
