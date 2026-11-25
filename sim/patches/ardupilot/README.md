# ArduPilot patches (non-physics)

Fixes to ArduPilot itself that FleetMesh relies on. SITL physics patches live in
`tidewater/patches/ardupilot/` instead. `make setup` applies both sets to a fresh ArduPilot
clone of the commit it pins, on branch `tidewater-water-depth`, in the order
`scripts/ardupilot-patches.sh` lists (tidewater's 0001, then this one);
`scripts/ardupilot-patches.sh check <dir>` names any an existing checkout lacks. Rebuild after
applying (see CLAUDE.md).

- `0001-AP_Param-...`: a `--defaults a.parm,b.parm` list kept only the last file's "all names
  found" flag, so MAVn_* stream rates set in an earlier file missed the MAVLink channels and they
  streamed built-in rates (murmur-yjq). FleetMesh also avoids it without the patch: `sim/swarm.py`
  passes one merged defaults file.
