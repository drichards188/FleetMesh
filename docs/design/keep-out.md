# Keep-out prediction

Status: design draft, 2026-11-24. Nothing here is built yet.

## 1. Purpose

The geofence rule ([delivery-rules.md](delivery-rules.md), section 4) checks a route when an order
is proposed and again when it is approved. A route that is legal then can stop being legal while
the vehicle is flying it: a temporary restriction begins, a weather cell moves in, a no-fly area is
added by a scenario event, or the wind pushes the vehicle off its planned track. Keep-out
prediction watches running routes for this, early enough that a person or an agent can do
something calm about it.

It answers one question for each vehicle on each decision tick: **when, if ever, will this
vehicle's planned route enter an area it must not be in, and what should happen first?**

## 2. Concepts

**Keep-out area.** Any region a vehicle must not be inside at a given time: a `no_go` or `restricted`
area (static or temporary), a weather cell, or an area around another vehicle's closed corridor
(not in v1). The area set in the delivery rules covers the first two; weather cells are defined
below.

**Weather cell.** A polygon where a weather value is above a platform's soft limit. The weather
sampler answers by nearest model point (`fleetmesh-weather` does no spatial interpolation), so a cell
is the union of the nearest-point regions of the grid points whose values exceed the limit,
clipped to the archive's bounds. Scenarios can also script cells as polygons with a velocity
(a squall line moving east at 8 m/s), which is the main use in scenario 2.

**Horizon.** How far ahead the prediction looks: `keepout.horizon_s`. Air 600 s, ground 1800 s,
surface 3600 s (a boat is slow and cannot stop quickly in current).

**Prediction.** For a vehicle, a list of entries: area, enter time, exit time, severity (`no_go`,
`restricted`, `weather`), margin (distance and time to boundary), uncertainty, and the part of the
route concerned.

**Standoff point.** The last point on the route before the area at which the vehicle can stop
safely, given its stopping distance and the buffer.

**Keep-out monitor.** A deterministic agent with a fixed id (`keepout`), part of the bridge. It
proposes orders from predictions. It never approves.

## 3. How the prediction works

Inputs per decision tick, all from the snapshot:

- the vehicle's position fact, ground velocity and `h_acc`;
- its remaining route (the active order's legs and the orders queued behind it);
- the area set and its hash, including temporary areas with their windows;
- weather samples for the cells, and any scripted cells;
- the energy model's ground speed along each remaining leg, which includes wind or current.

Procedure, which is the same code the geofence rule uses on a whole route (the monitor calls the
rule's sampler, so there is one implementation):

1. Walk the remaining legs, producing positions at `keepout.step_s` (default 5 s) from now to the
   horizon, or the end of the route.
2. Each position carries an uncertainty radius `r(t) = h_acc + keepout.drift_mps * t` (default 0.5
   m/s), plus a cross-track allowance for the platform (a plane's turn, a boat's set from current).
3. At each step, test the circle `(position, r(t) + buffer)` against every area active at that
   time. A spatial index (an R-tree over areas, built when the area set hash changes) keeps this
   linear in the number of steps, not areas.
4. Merge consecutive hits on the same area into one entry with enter and exit times.
5. Emit the earliest entry per area per vehicle.

**Incremental.** Predictions are recomputed when a vehicle's route changes, the area set hash
changes, a weather cell moves (scripted cells update once a second), or every `keepout.refresh_s`
(default 10 s) otherwise. Recomputing every vehicle every tick is not needed: at 5,000 vehicles
and a 10 s refresh the monitor does 500 predictions per tick.

**What it does not know.** Anything that is not in the area set, the weather samples or the
scripted cells. Other operators' flights (see section 7), people, and any change the scenario has
not published yet.

## 4. Behaviour: what happens as an entry approaches

Let `t_enter` be the time to the predicted entry and `t_stop` be the time the vehicle needs to
reach the standoff point and stop (its speed over its braking or turn-away distance plus
`keepout.reaction_s`).

| stage | when | what the monitor does |
| --- | --- | --- |
| beyond horizon | `t_enter` above the horizon | nothing |
| advisory | `t_enter` within the horizon and above `keepout.act_s` | logs `keepout.predicted`; shows the entry on the console; tells the dispatcher, which may propose a `reroute` |
| act | `t_enter` at or below `keepout.act_s` (default 180 s air, 600 s boat, 300 s car) and the order has no reroute proposed or pending | proposes a `reroute` around the area itself (planner as in `plan.rs`, with the area's buffer and the next fifteen minutes of its schedule) |
| hold | `t_enter` at or below `t_stop + keepout.hold_margin_s` (default 20 s) | proposes a `hold` at the standoff point (a plane: a loiter circle outside the buffer) |
| entered | position inside the area | logs `keepout.entered`, marks the vehicle for the navigation and geofence rules; the monitor proposes a `hold` or `return_to_hub` for the way out |

Routing follows the orders table: a `reroute` is auto when its checks allow and it fits the
`reroute.auto_added_pct` limit; otherwise it is held for a person. A `hold` is auto. A copter
hovering at a standoff point is a stable state, so the common outcome is: a vehicle holds, the
inbox gets a `reroute` card (if the detour is long), and a person decides between waiting, rerouting
and going home. A fixed-wing plane cannot hover, so its hold is a loiter circle outside the buffer
and the energy rule's reserve gets checked on the loiter's cost. If the reserve is thin the monitor
proposes `return_to_hub` instead.

**Weather cells** follow the same stages with two differences. A cell is predicted as moving, so
the entry time is computed against the cell's polygon at each future step (scripted cells have
velocity, archived cells change by the hour with no interpolation in space). And a cell is
`hold`-severity, not `refuse`: the vehicle is not forbidden to enter, the weather rule would hold
the order for a person, so the monitor proposes a `hold` before the entry and lets the person
choose to continue, wait, or reroute.

**Temporary restrictions** with a start time in the future are predicted as areas that exist from
`valid_from`. A route that will reach the area before `valid_from` and leave before it begins is
not an entry; one that is still inside at `valid_from` is. This is the case the geofence rule
already holds for with `GEOFENCE_TEMP_SOON`; the monitor covers the vehicle already in flight.

## 5. Data shapes

A temporary restriction, added by an operator through the console or an event file:

```toml
[[restriction]]
id = "tr-0007"
kind = "no_go"                 # no_go | restricted
name = "Marina Green event"
valid_from = "2026-06-14T10:30:00Z"
valid_until = "2026-06-14T13:00:00Z"
source = "operator"            # operator | scenario | notam | blender
polygon = [[-122.4410, 37.8060], [-122.4300, 37.8060], [-122.4300, 37.8110], [-122.4410, 37.8110]]
ceiling_m = 120
```

A scripted weather cell:

```toml
[[weather_cell]]
id = "squall-1"
at_s = 600
duration_s = 900
polygon = [[-122.50, 37.80], [-122.47, 37.80], [-122.47, 37.83], [-122.50, 37.83]]
velocity_mps = [8.0, 0.0]      # east, north
values = { wind_mps = 11, gust_mps = 15, precipitation_mm = 6 }
```

A scripted cell is a synthetic override. It is labelled synthetic in the sample (as the weather
crate already does for overrides) and in the logged decision inputs.

The prediction as logged:

```json
{"type": "keepout.predicted", "t": 1796132410000, "vehicle": "drone-3",
 "order": 142, "areas_sha256": "...",
 "entries": [{"area": "tr-0007", "severity": "no_go", "enter_s": 74, "exit_s": 131,
              "margin_m": 310, "uncertainty_m": 9.5, "leg": 2}],
 "stage": "act", "proposed": 147}
```

## 6. Deterministic and logged

Deterministic: the sampler, the spatial test, the stage thresholds, the standoff computation and
the proposals the monitor makes. The monitor reads no wall clock. It runs on the decision tick and
its outputs are a function of the snapshot.

Logged: each prediction that changes a stage (not every tick, to keep the log small), with the
inputs: the route legs with times, the position and uncertainty, the area set hash and the
temporary restrictions and cells active, the weather sample ids and values. Each proposal the
monitor makes is an ordinary intent. Replay recomputes the entries from the logged inputs and
checks that the stage and the proposal follow from the thresholds. Replay checks that a
proposal exists when a stage required it, and does not check the person's reaction.

Adding temporary restrictions during a run is a logged operator or scenario action (`areas.loaded`
with the new hash), as the inherited engine does for area changes.

## 7. Integration option: Flight Blender and the InterUSS DSS

[Flight Blender](https://github.com/openutm/flight-blender) (OpenUTM, Apache-2.0) and the
[InterUSS DSS](https://github.com/interuss/dss) (Apache-2.0) implement standards that cover what
this document would otherwise define for the fleet's own routes: ED-269 geofences, Remote ID
(ASTM F3411) and strategic coordination between operators (ASTM F3548). Both are listed in
[RELATED-PROJECTS.md](../RELATED-PROJECTS.md). Nothing was run for this design, so the endpoint
names and field shapes below are leads to confirm against their documentation.

**Option A: pull geofences.** A bridge adapter polls Flight Blender's geofence service on a timer
and converts each ED-269 geofence (with its time window, altitude limits and `uasZone` restriction
type) into a `temporary` area in the area set, source `blender`. The adapter logs the pulled set
by value in `areas.loaded` with the hash. The engine never calls Flight Blender during a decision.
This gives geofences from a real geospatial source and lets a scenario add or remove them through
Blender's own tooling. Cost: one HTTP service (and its database).

**Option B: publish plans and read conflicts.** The bridge acts as a USS client. For each approved
`deliver` and `return_to_hub` it submits an operational intent (a 4D volume per leg) to the DSS
and reads back the intents of other (simulated) operators that intersect. Those intersections are
inputs to the separation rule as `SEP_EXTERNAL_INTENT` (hold: another operator's flight is
planned through the same volume at that time; a person can accept it or reroute). The test
harness `interuss/monitoring` includes a mock USS that can stand in for other operators. Cost:
the DSS needs CockroachDB or YugabyteDB, so run it only when the scenario needs it.

**Option C: Remote ID.** The bridge publishes each vehicle's position as Remote ID messages and
ingests those of simulated external traffic, which the separation rule then treats like another
vehicle (with the uncertainty the broadcast carries). Needs B's service stack or a lighter
Blender-only path.

**Time.** Flight Blender and the DSS run on wall time. Scenario time maps to them by a fixed
offset in 1x runs. Fast forward ([FASTFORWARD.md](../FASTFORWARD.md)) breaks that mapping, so an
integrated run refuses any rate other than 1.

**Boundary and replay.** Every answer from these services is an input, logged in full when it is
read (the geofence set, the intersecting intents, the traffic list) and referenced by hash in the
decision that used it. Replay never contacts them. This preserves the engine's property that replay
needs only the log. A service being down is `GEOFENCE_AREAS_UNKNOWN` or `SEP_TRAFFIC_UNKNOWN`
(fail closed), not a silent pass.

**Recommendation.** Build the native monitor first (sections 3 to 6): it needs nothing beyond the
repository. Add Option A next since it is small and useful, and leave B and C until a scenario asks
for multi-operator airspace. Do not take a hard dependency: the `make up` default runs without
either service.

## 8. Policy knobs and first-draft defaults

```toml
[keepout]
enabled = true
step_s = 5
refresh_s = 10
drift_mps = 0.5
reaction_s = 5
hold_margin_s = 20

[keepout.horizon_s]
air = 600
ground = 1800
surface = 3600

[keepout.act_s]
air = 180
ground = 300
surface = 600

[keepout.integration]
blender_url = ""               # empty: off
blender_poll_s = 30
dss_url = ""                   # empty: off
```

## 9. Tests to write

- A route crossing a static no-go: one entry with the right enter time and a margin; no entry for a
  route that skirts the buffer.
- Temporary restriction starting in 120 s: entry only when the route is inside at start; not when
  the vehicle clears before it.
- Moving weather cell: entry time for a cell moving toward the route; none for a cell moving away.
- Stage transitions: advisory, act, hold, entered, each at its threshold; stage never skips back
  without a route or area change.
- Copter stops at the standoff point; plane loiters outside the buffer; boat's stopping distance
  includes current.
- Wind: a crosswind moving the ground track into an area changes the entry (the prediction uses
  ground speed with wind).
- Spatial index: results equal brute force on random areas.
- Incremental: no recompute without a change; recompute on hash change.
- Determinism and replay: same snapshot gives same predictions; a tampered stage is reported.
- Integration (when built): an ED-269 fixture converts to the expected area; a Blender timeout
  yields the unknown reason; the logged set replays without network.

## 10. Open questions

1. Who owns the standoff computation for a fixed-wing plane: the monitor or the task layer? The
   loiter circle position is a mission fact.
2. Should the monitor propose `hold` and `reroute` together (hold now, reroute next to it) or only
   one at a time? Together gives the person a ready alternative; singly keeps the inbox smaller.
3. Weather cells from the archive are 3 km nearest-point regions, which are crude edges. Is a
   smoothed boundary worth the extra nondeterminism risk?
4. Is a vehicle entering a restricted area during a lost link a monitor matter or a failsafe matter?
   The draft says failsafe, with a log entry.
5. Which Blender or DSS endpoints and versions to target, and whether the standards' time semantics
   (UTC, 4D volumes with start and end) map cleanly to our scenario clock.
6. If both options B and C are built, the DSS's own conflict resolution may disagree with ours
   (it assumes operators negotiate). Which wins when they do?
