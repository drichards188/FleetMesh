#!/usr/bin/env bash
# Run one of the first scenarios (sim/scenarios/<name>/, docs/design/scenarios.md) on stand-ins:
#   make start SCENARIO=harbour-city      (or scripts/scenario.sh harbour-city)
# Its fleet, hubs, drop zones, parcels, areas and scripted events come from its directory; real
# tide, current and wind from tidewater when its data is in the checkout. Settings (all optional):
#   SCALE=0.25     a fraction of the fleet and of the demand (every unit keeps one vehicle)
#   FLEETMESH_RATE=5  a faster scenario clock (needs FLEETMESH_AT, which the scenario gives)
# make down stops it.
set -euo pipefail
cd "$(dirname "$0")/.."
name="${1:?usage: scripts/scenario.sh <scenario> (one of: $(ls sim/scenarios | tr '\n' ' '))}"
shift || true
rate_override="${FLEETMESH_RATE:-}"
eval "$(.venv/bin/python scripts/scenario_env.py "$name")"
[ -n "$rate_override" ] && export FLEETMESH_RATE="$rate_override"
if [ -n "${SCALE:-}" ]; then export FLEETMESH_SCENARIO_SCALE="$SCALE"; fi
echo "scenario $name: fleet $FLEETMESH_FLEET, clock $FLEETMESH_AT at ${FLEETMESH_RATE}x${FLEETMESH_SCENARIO:+, conditions from $FLEETMESH_SCENARIO}"
exec scripts/up.sh --fleet "$FLEETMESH_FLEET" "$@"
