# First three scenarios

Status: design draft, 2026-11-24. Nothing here is built yet. All coordinates, counts and times
are first-draft values to be checked against tidewater and tuned.

## 1. Purpose

Each scenario is chosen to put a different pressure on the operator and to exercise a different
part of the stack, so that the first studies have something to compare:

1. **Harbour city**: a mixed fleet from two hubs. Steady, varied load; the baseline.
2. **Drone hub wind event**: one platform type and one hub, and a weather ramp that forces holds.
   A burst of decisions with a known cause.
3. **Coastal boat leg**: tide and current, link failover offshore, and GNSS interference near the
   port. Slow vehicles, degraded information, and scripted wrong answers.

Each has a seed, a duration, a fleet, a demand pattern, scripted events and a table of the
decisions the operator should face, with the reference answer. The reference answers feed the
error measures in [attention-cost.md](attention-cost.md).

## 2. Where they run

Tidewater supports two scenario areas in this repository: `golden-gate-2026-06` (15 m terrain,
spring tides 13 to 16 June 2026, SFBOFS currents, HRRR wind; 37.77 to 37.88 N, 122.56 to
122.38 W) and `bay-region-2026-06` (Bodega Bay to Monterey Bay, 90 m terrain, 36.55 to 38.35 N,
123.60 to 121.75 W). Real coordinates in San Francisco are used throughout; they are rough and must
be confirmed with `tidewater probe` (boats in water, cars on land, drop zones where a vehicle can
actually be). The default scenario clock is `FLEETMESH_AT=2026-06-14T17:00:00Z` (10:00 local), at 1x.

The existing example files `sim/areas/golden-gate.geojson` (an approved area and two no-go areas)
and `sim/fleets/delivery-demo.toml` (8 vehicles) are starting points. Scenario files go in
`sim/scenarios/<name>/` with: `scenario.toml` (clock, tidewater scenario, fleet file, policy file,
seed), a fleet file (each unit with its `home_hub`, [fleet-organisation.md](fleet-organisation.md)),
`dispatch.toml` (hubs, the sites that group them, drop zones, parcels or demand), `areas.geojson`,
`events.toml` (link, GNSS and restriction events) and `reference.toml` (the scripted decisions).

```toml
# sim/scenarios/harbour-city/scenario.toml
name = "harbour-city"
tidewater = "golden-gate-2026-06"
at = "2026-06-14T17:00:00Z"
rate = 1
duration_s = 3600
seed = 11
fleet = "fleet.toml"
dispatch = "dispatch.toml"
areas = "areas.geojson"
events = "events.toml"
reference = "reference.toml"
policy = "sim/policy.toml"
```

## 3. Scenario 1: Harbour city

**Question.** What does a normal hour look like at moderate scale, and how many decisions does the
operator face when most things go well?

**Setting.** The north San Francisco waterfront, from the Marina to Fisherman's Wharf, with
crossings to Sausalito and Angel Island. Two hubs.

| hub | position (lat, lon) | vehicles |
| --- | --- | --- |
| `hub-marina` (Marina Green side) | 37.8057, -122.4370 | 24 drones, 8 cars, 2 planes |
| `hub-pier` (Pier 39 side) | 37.8087, -122.4100 | 16 drones, 4 boats |

Total 54 vehicles. Towers (cellular ground stations) at Crissy Field (37.8068, -122.4460), Fort
Mason (37.8060, -122.4290) and Pier 39 (37.8100, -122.4110). The plane serves Sausalito (37.859,
-122.485), about 7 km across the Gate; the boats serve a dock at Sausalito and one at Angel Island
(37.870, -122.430); drones and cars serve the waterfront and the Marina streets.

**Demand.** A seeded generator: 3 parcels a minute on average for 60 minutes (about 180), mass
from 0.2 to 4 kg (above 2.5 kg only cars, boats and the plane can carry it), 60% normal priority,
30% high, 8% urgent, 2% low. About 40 drop zones, each an approved delivery point (surveyed, or
chosen by its customer; rules 2). Under the strict comparison procedure (`sim/policy-strict.toml`)
the doorsteps and kerbs start unconfirmed.

**Policy.** The defaults in [delivery-orders.md](delivery-orders.md), section 9. The weather rule
bands at their defaults.

**Scripted events** (`at_s` from the start; the first a minute in, so a run starts on something):

| at | event | effect |
| --- | --- | --- |
| 60 s | `restriction` tr-street-fair, no_go, polygon over Marina Green, 1800 s | routes crossing it need reroutes; keep-out monitor proposes them |
| 360 s | `dropzone.occupied` at dz-fort-mason-3 (a crowd), 240 s | a delivery to that zone finds it occupied |
| 660 s | cellular congestion, circle 1.2 km around Pier 39, extra loss 5%, 40 ms, 600 s | link degraded for vehicles there; some drones fail over to LOS |
| 960 s | tower outage: Fort Mason tower `cut` for 300 s | vehicles near it lose cellular; failover to the backup |
| 1460 s | GNSS degraded in the urban canyon near Fisherman's Wharf, 240 s | positions less accurate there; navigation trust degrades |
| 2100 s | one drone's battery fact stops reporting (stand-in fault) | `ENERGY_BATTERY_UNKNOWN` holds for it |
| 2400 s | low tide at a boat dock (tidewater supplies the level) | `TIDE_DOCK_LEVEL` or `TIDE_SHALLOW` holds on the Angel Island dock |

**Expected operator decisions** (the table `reference.toml` encodes; correct answers in bold).
The generator makes many more items; these are the scripted ones the error measures use.

| item | cause (reason code) | reference |
| --- | --- | --- |
| first delivery to each of 12 unconfirmed doorsteps (strict procedure only) | `DZ_UNCONFIRMED` | **approve** where the zone is clear in the observation feed; **reject** where an observation says people near |
| delivery routed across the street fair | `GEOFENCE_TEMP_SOON` or monitor reroute held for length | **approve** reroute under 25% added distance, **reject** longer |
| delivery to dz-fort-mason-3 during the crowd | from rules 2 an alert, `drop_not_clear`: automation waited, kept the parcel and brought it back; under the strict procedure `DZ_PEOPLE_NEAR` | nothing to decide (rules 2); **reject** (strict) |
| drones on LOS after failover with thin reserve | `ENERGY_MARGIN_THIN` | **approve** if margin positive and parcel urgent; **reject** otherwise |
| boat order to Angel Island dock at low tide | `TIDE_SHALLOW` | **reject** or wait; approve only after the level rises |
| drone with no battery reading | `ENERGY_BATTERY_UNKNOWN` | **reject**, recall the vehicle |

**Expected scale of load.** About 180 orders generate 180 `deliver` orders, 180 `pickup` (auto), 180
`return_to_hub` (auto), and a smaller number of `swap_battery` (auto). The inbox should see roughly
10 to 20 decisions an hour: the 12 first-time doorsteps, the scripted items above, and a few
marginal energy or separation holds. Roughly one to three orders per hour per hundred vehicles is
the working hypothesis that makes the scenario a baseline. If the first runs show 60 an hour, the
defaults need tuning, not the scenario.

**What it exercises.** Dispatch and batching, both hubs and all four platforms, temporary
restrictions, keep-out reroutes, drop-zone confirmation, cellular and failover, tide at a dock,
battery swaps.

**Success looks like.** No harmful false approvals under an ideal scripted operator; every
vehicle ends the hour at a hub; the log replays; the attention report is produced.

## 4. Scenario 2: Drone hub during a wind event

**Question.** When conditions move from fine to marginal to forbidden within a quarter of an hour,
how does the number and kind of decisions change, and does the operator keep up?

**Setting.** One hub at Crissy Field (37.8068, -122.4460), 40 delivery drones, no other platform,
delivering to about 30 drop zones within 4 km (Presidio, Marina, Fort Mason, Fort Point). One
tower at the hub. The scenario runs 25 minutes starting at 10:00 local; the wind turns about a
minute in, so a run starts on the event rather than on a quiet half hour.

**Weather.** Mean wind comes from the HRRR archive for 14 June (tidewater's Golden Gate scenario
notes a funnelling peak of 7.6 m/s through the Gate). The weather crate does not accept scripted
mean-wind overrides (it validates against them to keep sensing and vehicle forcing consistent), so
the event is scripted gusts, which it does accept:

```toml
# events.toml (weather timeline, in the scenario's weather config)
[[weather.timeline]]
at_s = 0
values = { gust_mps = 7.0 }
[[weather.timeline]]
at_s = 20
values = { gust_mps = 7.5 }
[[weather.timeline]]
at_s = 70              # ramp up through the soft band (drone soft gust 11, hard 14)
values = { gust_mps = 12.5 }
[[weather.timeline]]
at_s = 420
values = { gust_mps = 13.5 }
[[weather.timeline]]
at_s = 600             # a spike above the hard limit
values = { gust_mps = 15.5 }
[[weather.timeline]]
at_s = 780
values = { gust_mps = 12.0 }
[[weather.timeline]]
at_s = 1200
values = { gust_mps = 8.0 }
```

The values interpolate linearly and hold the last. The scripted gust affects the rules and the
keep-out monitor, not the vehicles' physics (SITL wind comes from tidewater and has no gust
channel from this file). A scripted weather cell also covers the hub region from about 55 s so that
in-flight drones receive keep-out stages. Open question 1 asks whether to build a stronger
archive slice instead.

**Demand.** 6 parcels a minute from 30 s for the whole run (about 150), mass 0.2 to 2.4 kg, a third above 1.5
kg. The payload rule derates multirotors from a gust of 7 m/s down to 60% at the hard limit, so the
effective maximum is about 2.1 kg at a gust of 10 and about 1.7 kg at 12.5: heavy parcels become
`PAYLOAD_OVER_DERATED` refusals early in the ramp.

**Timeline and what should happen.** Linear interpolation of the gust table puts the soft limit
(11 m/s) at about 55 s, the hard limit (14 m/s) at about 465 s, the fall back below the hard
limit at about 677 s and below the soft limit at about 885 s. (Until 2026-11-27 the same arc ran
over 70 minutes with the soft limit at 22 minutes; a run then spent its first 20 minutes on
nothing.)

| time | gust m/s | rules | expected inbox |
| --- | --- | --- | --- |
| 0 to 55 s | 7 to 11 | all allow (below soft 11) | almost empty |
| 55 to 465 s | 11 to 14 | every `deliver` returns `WX_GUST_MARGINAL` hold; heavy parcels refused; in-flight drones get keep-out `hold` stages from the weather cell | **surge**: about 6 held orders a minute arriving |
| 465 to 677 s | above 14 (peak 15.5 at 600 s) | hard limit: new `deliver`, `pickup` and `reroute` refused (`WX_GUST_LIMIT`); `return_to_hub` auto for every airborne drone | held items expire or are rejected; the inbox drains as refusals replace holds |
| 677 to 885 s | 14 down to 11 | back in the soft band | a second, shorter surge |
| after 885 s | below 11 | allow | normal; the backlog drains |

The inbox surge between about 55 and 465 s is the point. Without a policy change, 6 arrivals a minute at
a handling time of 10 s is utilisation 1.0 for one operator. A well-run response is for the
operator to approve only deliveries that are short, light and well-provisioned, and to reject or
let expire the rest, and to bring the fleet home before the spike.

**Expected operator decisions.** A scripted subset, by reference answer:

| item | reference |
| --- | --- |
| `deliver` during the soft band, parcel under 1 kg, distance under 1.5 km, battery over 60% | **approve** |
| `deliver` during the soft band, distance over 3 km or battery under 45% | **reject** |
| `deliver` approved before the gust crosses 14, in flight when it does | the drone should already be `return_to_hub` (auto); an item that remains aloft is a failure of the monitor, not of the person |
| `deliver` held and still pending when the spike arrives | **reject** (void); the check will refuse it at approval anyway |
| any `deliver` at gust above 14 | refused: no item reaches the operator |

**Knob sweeps (the study).**

- Soft gust threshold 9, 11, 13 (band width). Narrow band: more refusals, fewer holds.
- Dispatcher rate limit: 6, 3, 1 proposals a minute during the soft band.
- `deliver` to a confirmed zone as `human` rather than `auto` before the event.

**What it exercises.** The weather rule and its band, the payload derate, the keep-out monitor on a
weather cell, expiry, `return_to_hub`, inbox surge and recovery, and the attention measures'
utilisation, latency and expiry.

**Success looks like.** Every held item either gets a decision or expires visibly with a logged
consequence; every airborne drone has been ordered home within 60 s of the gust crossing the hard limit,
unless a person explicitly decided otherwise; the attention report shows the
surge, the drain and the recovery time.

## 5. Scenario 3: Coastal boat leg

**Question.** With slow vehicles, a changing current, a link that changes radio, and a position
fix that is attacked, can the operator tell what is wrong and what to do?

**Setting.** Uses `bay-region-2026-06` so that the route can leave the Golden Gate. Hub `hub-pier` at
Pier 39 (37.8087, -122.4100). Fleet: 4 delivery boats, 6 delivery drones (harbour traffic that is
also affected by the interference), 1 delivery plane (for contrast, on a LOS link). Towers: Pier 39
(37.8100, -122.4110), Crissy Field (37.8068, -122.4460), and Point Bonita (37.8157, -122.5293). The
cellular range of the last reaches about 8 km (to about 122.62 W).

**The leg.** Boat 1 carries a 40 kg supply parcel from the hub to an anchored vessel, `ves-offshore`
(37.815, -122.700), about 28 km west of Pier 39, through the Golden Gate. At 3 m/s cruise this is
about 2.6 hours each way, within the boat's 8 h endurance. A second boat carries a smaller parcel
to a mooring at Sausalito (37.857, -122.482), and two boats stay in the harbour on short runs.

**Tide and current.** The route crosses the Gate at maximum ebb (tidewater's README: about 1.8 m/s
through the Gate on 14 June spring tides). With an ebb, the outbound boat has a following current
and the energy rule sees a gain; at the return, the current turns and is against, up to 0.6 of the
boat's cruise speed. Planned departure is set so the outbound leg meets the ebb; the return plan
meets slack and then flood. The checks:

- on the return, `WX_CURRENT_MARGINAL` (ratio 0.5 to 0.8) holds the order: the operator decides
  whether to wait for slack water. The reference answer is **wait** (reject and reschedule one hour
  later), unless reserve is above 40%.
- the planned route's shortest line goes near the Potato Patch shoal; at low water `TIDE_SHALLOW`
  holds it (draft 0.4 m, clearance 0.5 m); a route around the shoal is the fix, so the reference
  answer is **approve the reroute**.

**Link.** Boat 1 leaves cellular range of Point Bonita at about 8 km west of the tower (around
122.62 W). Cellular loss rises with range from 3 km (ok) to 8 km (max). The main link's loss averaged
over 3 s passes 0.5 shortly before the maximum range and the vehicle fails over to `satcom_leo`
(gap 2 s). With the satellite model cut, the SATCOM profile is static: 25 ms latency, 2 Mbps,
lost-link timer 30 s. The return crossing repeats the failover in reverse after 10 s of healthy
cellular (hold). Neither the switch nor the gap is an operator decision, but both must appear on
the console (see [link-and-gnss.md](link-and-gnss.md), section 8).

**GNSS interference near the port.** A scripted jammer and then a spoof:

```toml
# events.toml
[[event]]
name = "port interference: jammer"
at_s = 1800
duration_s = 900
circle = { lat = 37.8095, lon = -122.4180, radius_m = 1800 }
gnss = { effect = "jam", power_w = 5.0, height_m = 15 }

[[event]]
name = "approach spoof"
at_s = 4200
duration_s = 1200
uids = [3]                          # the second boat, near the Sausalito mooring
circle = { lat = 37.857, lon = -122.482, radius_m = 2500 }
gnss = { effect = "spoof", walk_mps = 1.5, bearing_deg = 100, max_offset_m = 600 }
```

The jammer sits in the harbour approach, near the hub. Vehicles in the circle lose their fix (the
jammer is strong enough to deny a fix across about its circle); the effects differ by platform
(see section 6 of the link document and the rules): copters' EKF failsafe lands them
(`NAV_NO_FIX`, then `land_now` proposals; landings over water are bad), the boat holds, a boat
dead-reckons for 7 to 15 s then loses horizontal position. The jammer does not touch the datalink
because a GNSS-only event leaves the link alone, so the operator can still send commands.

The spoof walks boat 2 east of its true track at 1.5 m/s toward a restricted area (a ship channel
polygon marked `restricted`), reporting a good fix throughout. Nothing in the fix flags shows it.
Detection depends on the consistency checks: the reported position's range to the serving tower
against the emulator's slant range (`NAV_RANGE_MISMATCH`) when the boat is within cellular range,
and on `NAV_JUMP` when the spoof ends and the position snaps back 25 m or more. Until then the
boat's reported position drifts toward the channel while its truth does not; the geofence rule sees
the false position and may hold or refuse orders on that basis. This is a deliberate test of the
detection gap (open question 3).

**Scripted decisions** (`reference.toml`):

| time | item | cause | reference |
| --- | --- | --- | --- |
| about 300 s | boat 1 departure through the Gate | `WX_CURRENT_MARGINAL` on the return plan only, none outbound | **approve** outbound |
| about 1850 s | 6 drones in the harbour lose fix | `NAV_NO_FIX` then `land_now` proposals (outside hub) | **approve** landing on `landing` areas; **reject** water or street spots, hold instead |
| about 1900 s | boat 4 inside the jammer | `NAV_UNTRUSTED` `hold` auto; `return_to_hub` held | **reject** `return_to_hub` while untrusted; **approve** after recovery |
| about 3400 s | boat 1 return, current rising | `WX_CURRENT_MARGINAL` | **reject**, wait for slack |
| about 3500 s | boat 1 reroute around the shoal | `TIDE_SHALLOW` on direct line | **approve** reroute |
| about 4300 s | boat 2 held on suspicious position | `NAV_RANGE_MISMATCH` | **reject** further orders, hold, request recovery |
| about 5000 s | boat 2 after spoof ends | `NAV_JUMP`, trust restored after 20 s | **approve** only after trust is `trusted` |

**What it exercises.** Tide and current in the energy and weather rules, the keep-out monitor on
the shoal and the restricted channel, failover with the draft parameters, lost-link timers by
class, GNSS jam and spoof effects on three platforms, the navigation trust state, the
`land_now` routing, the console's honest display of untrusted position, and decision-making
under degraded information.

**Success looks like.** Failover and its gap are logged and visible; no order is dispatched to a
vehicle in the `untrusted` state except safety orders; the spoofed vehicle's drift is
detected and the time to detection is reported (or not, which is a result); the log replays with the
trust state re-derived.

## 6. Reference file shape

```toml
# reference.toml: scripted decisions, joined to the audit log by cause and time
[[item]]
id = "r-01"
when = { at_s = 1850, window_s = 120 }
match = { kind = "land_now", reason = "NAV_NO_FIX", uid = "drone-2" }
reference = "approve"
note = "landing site is an approved landing area"
```

The attention tool matches an item in the log by the `match` fields within the window, takes the
operator's decision, and scores it against `reference`. An item the log never produced is reported
as a missed scenario step, not an error.

## 7. Tests to write

- Each scenario loads (fleet, dispatch, areas, events, reference), validated by a `scenario check`
  command that also checks every position against tidewater: boats in water, cars on land, drop
  zones within an approved area.
- A seeded run twice gives the same audit log up to wall-clock fields; the demand generator is
  deterministic.
- Scenario 1: the scripted events occur at their times; a fixture run with an ideal operator ends
  with all vehicles at hubs.
- Scenario 2: the rule outcomes at each gust step match the table (a rules test over synthetic
  snapshots, not a full run); the surge appears in the attention report of a scripted-operator
  run.
- Scenario 3: the jam circle denies a fix at a boat's position; the spoof walk is as configured; the
  trust state follows the hand-computed sequence; failover switches at the expected range band.
- Replay of each scenario's log has no mismatches.

## 8. Open questions

1. Scenario 2 wants stronger mean wind. Scripted gusts affect the rules and the monitor but not
   the vehicles' physics. Build a committed archive slice with stronger HRRR-shaped wind (a
   deliberate, labelled synthetic change) or accept the limitation for the first runs?
2. Are 54, 40 and 10 vehicles a useful first size? The scaling study in
   [attention-cost.md](attention-cost.md) needs the same scenarios at 10 to 3,000 vehicles. The
   fleet files should be parameterised by a scale factor.
3. The spoof in scenario 3 may go undetected until it ends. That is informative, and also makes
   the reference answer for the operator unknowable for most of the run. Keep it as a separate
   variant (3b) so the main scenario has a fair reference?
4. Real names (Marina, Pier 39, Sausalito) in a testbed that is not connected to anything real:
   use them for realism, or neutral names to avoid any impression of operational planning?
5. Time of day and sun: delivery in the dark is a different problem (visibility, observations).
   All three run in daylight.
6. Whether the tide at `hub-pier` docks is realistic enough from tidewater's 15 m terrain;
   bay-region's 90 m terrain is coarse for harbour shoals.
