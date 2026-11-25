# Delivery rules

Status: design draft, 2026-11-24. Nothing here is built yet.

## 1. Purpose

The decision engine runs a fixed set of deterministic checks on every order before policy routes
it. This document defines the delivery checks: what each reads, what it tests, what it returns,
and which numbers an operator or study can change. It replaces the inherited rules for orbit,
patrol, the other non-delivery task kinds, identification, and space tasking, which are being removed.

Each check returns one of three outcomes with one or more reason codes:

- `allow`: nothing to say.
- `hold`: within hard limits, but marginal or unverified; a person must judge it. The order goes
  to the inbox whatever its kind's default tier is.
- `refuse`: a hard limit is broken or a required fact is missing in a way that cannot be judged.
  The order is rejected before policy and never reaches the inbox. The reason is logged and
  returned to the proposer.

Engine mapping: `allow` is `Outcome::Pass`, `refuse` is `Outcome::Fail`, `hold` is a new
`Outcome::Hold` (see [delivery-orders.md](delivery-orders.md), section 2). The inherited
`NotEvaluated` stays for deliberately skipped checks and blocks nothing unless the order kind is
consequential; for delivery that means `deliver`, `abort` over people and `land_now` outside a
hub: if a rule returns `NotEvaluated` for these, the order is refused. Fail closed.

## 2. Principles

1. **Facts carry ages.** Every input is a fact with the time it arrived. A fact older than its
   limit counts as unknown. Unknown is never replaced by a default.
2. **Read once, log by value.** A rule reads the weather, tide, terrain, link and traffic answers
   from the snapshot. The decision logs the values it read. Replay does not query anything.
3. **Hold is cheap, refuse is final.** Use `hold` where a person could reasonably say yes, and
   `refuse` only where no person should be asked (the vehicle cannot do it, or it enters
   forbidden space).
4. **One margin band.** Most numeric rules use two thresholds: a soft one (start of hold) and a
   hard one (start of refuse). The soft threshold is the knob that decides how many orders reach
   a person. Scenario 2 in [scenarios.md](scenarios.md) is built to show this.
5. **Safety orders are not blocked by comfort rules.** `hold`, `return_to_hub`, `land_now` and
   `abort` skip the energy, payload and weather rules (they may still be refused by geofence
   and navigation rules that make them impossible). A vehicle that cannot take a delivery can
   still be told to stop.
6. **Rules are versioned.** The rules below are rules version 1 except where a section says
   "from rules 2" or later. Changing any of them means a new `*_SINCE` constant
   ([rules-versioning.md](rules-versioning.md)); rules 2 is `STANDING_APPROVALS_SINCE` (standing
   approved delivery points, section 10), rules 3 `TRAFFIC_SEPARATION_SINCE` (other traffic,
   section 8.1), rules 5 `HUB_SEQUENCING_SINCE` (hubs sequence their own traffic, section 8.2),
   rules 6 `FLOOD_ALONG_ROUTE_SINCE` (the ground and the water where an order goes, section 7).
7. **A person is asked about exceptions only.** Routine deliveries run with no person (rules 2,
   docs/DECISIONS.md 2026-11-25). What real operations automate (a stale fact read again,
   departures sequenced round other traffic, a hub's own arrivals and departures, the drop-time
   look) is automation's: the dispatcher waits it out before anything reaches the inbox, and no
   check's threshold moved for it.

## 3. Rule list and applicability

| id | rule | applies to |
| --- | --- | --- |
| `geofence` | areas, airspace, temporary restrictions | pickup, deliver, reroute, return_to_hub, land_now (site), hold (plane) |
| `domain` | land, water or air for the platform (inherited, kept) | pickup, deliver, reroute, return_to_hub |
| `energy` | battery and range reserve with wind | pickup, deliver, reroute |
| `payload` | parcel mass and wind derating | pickup, deliver |
| `weather` | wind, gusts, visibility, precipitation, tide, current | pickup, deliver, reroute |
| `separation` | route conflicts and minimum distance | pickup, deliver, reroute, return_to_hub |
| `dropzone` | zone state and conditions | deliver, confirm_drop_zone |
| `nav_integrity` | position fix trust | all kinds, with the exceptions in section 9 |
| `lost_link` | failsafe plan and coverage along the route | pickup, deliver, reroute |

The rules run in this order and all run; the engine does not stop at the first failure so the
inbox card and the proposer see every reason.

## 4. Geofences and airspace (`geofence`)

**Inputs.** The area set (`sim/areas/*.geojson`, hashed and logged in `run.started`); the route as
legs (the inherited `route.rs` samples every leg every 20 m); the time each point of the route
will be reached (ETA from the energy model's ground speed); the vehicle's position uncertainty
(`h_acc` from the receiver, or the platform default); the platform's domain.

Area kinds (extending the inherited `approved` and `no_go`):

```json
{"type": "Feature",
 "properties": {"id": "gg-ops", "kind": "approved", "ceiling_m": 120},
 "geometry": {"type": "Polygon", "coordinates": [[[-122.53,37.77],[-122.39,37.77],[-122.39,37.87],[-122.53,37.87],[-122.53,37.77]]]}}
```

| kind | meaning |
| --- | --- |
| `approved` | the operating area; every point of every route must be inside one |
| `no_go` | no route may enter |
| `restricted` | entry needs a person (an airspace class, a school at pickup time, a ship channel) |
| `populated` | people live or gather here; no refusal, but routes over it are marked and several rules react (hold for a plane loiter, abort routing, descent limits) |
| `landing` | an approved landing area outside a hub |
| `temporary` | any of the above with `valid_from` and `valid_until` (RFC 3339), also `notam_ref` or `source` text |

Temporary restrictions are added to the set by an operator, a scenario event or an integration
([keep-out.md](keep-out.md)). The area set hash changes when one is added, and each decision logs
the hash it read.

**Check.** For each leg point at its ETA, with a buffer of `geofence.buffer_m` plus `h_acc`:

- outside every `approved` area (for its domain and altitude): refuse `GEOFENCE_OUTSIDE_APPROVED`;
- inside a `no_go` that is active at the ETA: refuse `GEOFENCE_NO_GO`;
- inside a `restricted` that is active at the ETA: hold `GEOFENCE_RESTRICTED`;
- inside a `temporary` area that becomes active within `geofence.soon_s` after the ETA: hold
  `GEOFENCE_TEMP_SOON` (the route is legal now and not for long);
- above an area's `ceiling_m`, or the route altitude is above the platform's `typical_alt_m`
  plus `geofence.alt_slack_m`: refuse `GEOFENCE_CEILING`;
- route enters `populated` for a plane's hold or loiter: hold `GEOFENCE_LOITER_POPULATED`;
- the area set is missing or its hash does not match the run's: refuse `GEOFENCE_AREAS_UNKNOWN`.

**Knobs.** `geofence.buffer_m`: air 30, ground 10, surface 20. `geofence.soon_s` 600.
`geofence.alt_slack_m` 20. `geofence.sample_m` 20 (leg sampling, as inherited).

## 5. Battery and range reserve with wind (`energy`)

**Inputs.** Battery percent fact and its age; platform `endurance_h`, `cruise_mps`, `payload_kg`
and (new) `battery_wh` if known; parcel mass; the route legs; the wind vector (and for boats the
current vector) sampled at the start, the end and every `energy.sample_km` of the route from the
snapshot's weather and tide samples; the nearest reachable hub for the return.

**Model.** One function, `energy::trip_cost(platform, legs, wind, current, payload_kg)`, shared
with dispatch ([dispatch.md](dispatch.md)) so the assigner and the rule agree. For each leg:

1. Ground speed is the vector sum of the platform's through-medium speed (`cruise_mps`) and the
   wind (air) or current (surface), solved for the heading that holds the track. If the ground
   speed along the track is below `energy.min_ground_speed_frac` of cruise (default 0.3), the leg is
   not makeable.
2. Leg time is length over ground speed.
3. Leg energy, as a percentage of a full battery, is time times draw per hour, where draw per hour
   is `100 / endurance_h` times a load factor. Load factors follow the stand-in battery model:
   multirotor `0.85 + 0.15 (v/cruise)^2`; plane `0.7 + 0.3 (v/cruise)^2` plus up to `+0.5`
   while climbing; car and boat `0.1 + 0.9 (v/cruise)^3`, with the boat's `v` taken relative to
   the water (so a head current costs time but a following current costs less power).
4. Payload multiplies the load factor by `1 + energy.payload_load * payload_kg / payload_max_kg`
   with `energy.payload_load` defaulting to 0.35 for multirotor, 0.15 for plane, 0.10 for car and
   boat.
5. Add `energy.drop_hover_s` (default 20 s, copter only) at draw 1.0 per delivery stop.

Required energy is the cost of the order's route plus the cost of the return to the nearest hub
reachable from the route's end, times `1 + energy.wind_margin` (default 0.15), plus the reserve.
The return uses the wind at the drop zone for the first leg and the hub's wind for the last;
there is no forecast (see weather, section 7).

**Check.** Let `available` be the battery percent fact, `required` as above, and
`margin = available - required` in percentage points.

- battery fact missing or older than `energy.max_age_s`: hold `ENERGY_BATTERY_UNKNOWN`;
- any leg not makeable: refuse `ENERGY_WIND_NO_HEADWAY`;
- `margin < 0`: refuse `ENERGY_INSUFFICIENT`;
- `0 <= margin < energy.hold_band_pct`: hold `ENERGY_MARGIN_THIN`;
- no hub reachable from the route's end with reserve: hold `ENERGY_NO_HUB_REACHABLE`;
- otherwise allow.

An operator's `constraints.reserve_pct` raises the reserve for one order and never lowers it.

**Knobs.**

| knob | default |
| --- | --- |
| `energy.reserve_pct` | copter 25, plane 20, car 15, boat 20 |
| `energy.wind_margin` | 0.15 |
| `energy.hold_band_pct` | 5 |
| `energy.max_age_s` | 10 (streaming link), 60 (satcom or other slow link) |
| `energy.min_ground_speed_frac` | 0.3 |
| `energy.sample_km` | 1.0 |
| `energy.drop_hover_s` | 20 |

**Reason code example.** `ENERGY_MARGIN_THIN: battery 57%, need 52.9% (route 17.2 plus return
7.1, times 1.15 for wind margin, equals 27.9, plus reserve 25), margin 4.1 points, hold band 5`.
The text states the numbers; the code is the stable key.

## 6. Payload limits per platform (`payload`)

**Inputs.** Parcel mass (kg), platform `payload_kg`, the wind sample at the origin and drop zone,
and the parcel's `class`.

**Check.**

- parcel mass unknown: hold `PAYLOAD_UNKNOWN`;
- parcel class `hazmat` or `excluded`: refuse `PAYLOAD_CLASS` (v1 carries no dangerous goods);
- the effective maximum is `payload_kg * derate(gust)`, where for multirotor `derate` falls
  linearly from 1.0 at `payload.derate_from_mps` (default 7) gust to `payload.derate_floor`
  (default 0.6) at the weather rule's hard gust limit; plane, car and boat do not derate;
- mass above `payload_kg` (full): refuse `PAYLOAD_OVER_MAX`;
- mass above the effective maximum but at or below `payload_kg`: refuse `PAYLOAD_OVER_DERATED`;
- mass above `payload.hold_frac` of the effective maximum (default 0.9): hold `PAYLOAD_NEAR_MAX`;
- otherwise allow.

Defaults come from `sim/platforms.toml` (`payload_kg`: drone 2.5, plane 5, car 40, boat 60). The
catalog field stops being informational: the bridge copies it into `PlatformFacts` so replay needs
no catalog file.

## 7. Weather limits (`weather`)

**Inputs.** Weather samples (`fleetmesh-weather::Sample`: wind east and north, gust, visibility,
precipitation, with source and checksum) at the origin, the drop zone, the destination hub, and
along the route every `weather.sample_km`; the vehicle's own wind estimate if it reports one;
tidewater's tide and current samples for surface vehicles.

**Time.** The default is current conditions only (`weather.lookahead_s = 0`). The archived data
is retrospective replay, not a forecast that would have existed, so the rule does not read future
rows unless a scenario turns lookahead on, and then the rule labels those values `forecast` in the
reason. See open question 2.

**Check, air** (platforms `delivery_drone`, `delivery_plane`). Soft and hard limits:

| value | drone soft | drone hard | plane soft | plane hard |
| --- | --- | --- | --- | --- |
| sustained wind, m/s | 8 | 10 | 10 | 12 |
| gust, m/s | 11 | 14 | 13 | 16 |
| visibility, m | 1500 | 800 | 2000 | 1000 |
| precipitation, mm/h | 2 | 5 | 4 | 8 |

Above hard: refuse `WX_WIND_LIMIT`, `WX_GUST_LIMIT`, `WX_VISIBILITY`, `WX_PRECIP`. Above soft but
under hard: hold with the same code and the suffix `_MARGINAL` (`WX_GUST_MARGINAL`). Missing weather
at a place the vehicle will be: hold `WX_UNKNOWN`. The drop-zone descent has its own lower gust
limit for copters (section 10).

**Check, surface** (`delivery_boat`):

- wind soft 10, hard 13 m/s; gust soft 13, hard 16 (`WX_WIND_LIMIT`, `WX_GUST_LIMIT`);
- current across the route: `ratio = |current| / cruise_mps`; soft 0.5, hard 0.8. Hard: refuse
  `WX_CURRENT_LIMIT`; soft: hold `WX_CURRENT_MARGINAL`. With cruise 3 m/s, soft is 1.5 m/s and
  hard 2.4 m/s;
- headway against current and wind together is the energy rule's `ENERGY_WIND_NO_HEADWAY`;
- tide: sampled water depth along the route and at the destination at the ETA. Required depth is
  `draft_m + weather.keel_clear_m`. Below required (default clearance 0.5 m): hold `TIDE_SHALLOW`;
  below `draft_m + weather.keel_hard_m` (0.2 m): refuse `TIDE_AGROUND`. A boat's `draft_m` is a new
  catalog field, first draft 0.4 m. The ETA is computed from the route, and the tide sample is
  the tidewater value at the ETA on the scenario clock. Tidewater's tide comes from the fetched
  station record, so it is deterministic; this is the one place a future sample is allowed by
  default, labelled `tide_prediction`.
- A drop at a dock whose deck level is `dock_level_m` flags `TIDE_DOCK_LEVEL` (hold) when the
  water level at the ETA is within 0.3 m of the deck or above it.

**Check, ground** (`delivery_car`): precipitation soft 8, hard 20 mm/h (`WX_PRECIP`); visibility soft
300, hard 100 m (`WX_VISIBILITY`); flood: if tide level at any route point exceeds terrain
elevation minus 0.3 m, hold `WX_FLOOD_RISK`. Wind does not limit a car.

**Where the ground and the water are read (from rules 6, `FLOOD_ALONG_ROUTE_SINCE`).** The weather
samples a run feeds are taken at the hubs, the drop zones with open parcels and the vehicles. A
car's flood check and a boat's depth under the keel (`TIDE_SHALLOW`, `TIDE_AGROUND`) read only the
samples within `FLOOD_NEAR_M` (60 m) of the vehicle's position, its route's points and its way
back's end: where it will be. Before rules 6 they read every fresh sample, so a boat dock's seabed
(4.3 m under a 1 m tide, harbour-city's Pier 39 dock) held every car's order `WX_FLOOD_RISK`, and
the shallowest water anywhere judged every boat. The wind, gusts, visibility, precipitation and
current are still the worst of every fresh sample, and no threshold moved.

**Knobs.** The table values above, each overridable per platform in `sim/rules.toml`
(`[weather.delivery_drone] gust_soft_mps = 11`). `weather.sample_km` 1.0 (the sampler answers
with the nearest 3 km model point anyway, so closer spacing adds nothing). `weather.lookahead_s` 0.
`weather.max_age_s` 120.

**Why two thresholds.** At the soft threshold the order is legal and a person decides; at the hard
threshold nobody is asked. In a wind event the soft threshold is what creates the stream of holds
that scenario 2 measures. Narrowing the band (hard minus soft) converts holds into refusals and
shrinks the inbox. Widening it does the opposite. Whether that trade is wise is the study's
question, so the band is a first-class knob.

## 8. Separation and route conflicts (`separation`)

**Inputs.** The order's route with times (from the energy model), and for every other vehicle with
an active order or on the move: its current position and velocity fact, and the time-parametrised
route of its dispatched or approved orders. Pending orders in the inbox are not included (they
are not yet intended), which is called out in the inbox card when it matters.

**Check.** Sample all routes at `separation.step_s` (default 5 s) from now to `separation.horizon_s`
(default 300 s). For each pair of vehicles that could be close (spatial grid, cell size
`2 * min_h + vmax * step`), compute the horizontal and vertical distance at each sample, with a
position uncertainty that grows by `separation.drift_mps` (0.5) per second. Minimums by pair type:

| pair | horizontal m | vertical m |
| --- | --- | --- |
| air, air | 50 | 30 |
| air, hub zone (within 100 m of a hub) | 20 | 15 |
| ground, ground | 10 | n/a |
| surface, surface | 30 | n/a |
| air, ground or surface | no minimum while the air vehicle is above 20 m | |

Violation means both horizontal and vertical are below minimum (air) or horizontal below minimum
(others). Result:

- predicted violation with a vehicle that is moving: refuse `SEP_ROUTE_CONFLICT`, with a suggested
  fix in the detail (`delay_s` until clear, or a different cruise layer); the dispatcher can
  re-propose with the fix;
- predicted violation in a hub zone: hold `SEP_HUB_ZONE` (two vehicles for one pad or the same
  corridor; under rules 1 to 4 a person could decide who waits, from rules 5 the hub sequences
  them, section 8.2);
- closest approach between 1.0 and `separation.marginal_factor` (default 1.5) times the minimum:
  hold `SEP_MARGINAL`;
- another vehicle's route unknown (fact too old): hold `SEP_TRAFFIC_UNKNOWN` only if that vehicle is
  within `separation.relevant_km` (5) of the route; otherwise ignore it.

**Layers.** To avoid solving this by refusal, dispatch assigns each outbound air route a cruise
altitude from `layers = [60, 80, 100, 120]` metres by a stable hash of the vehicle uid and its
order sequence number (not randomness). Inbound routes use the intermediate layers
`[70, 90, 110]`. The plane uses its own `typical_alt_m`. This is a v1 convention in dispatch, not
a rule; the rule only sees altitudes.

**Not modelled.** Birds, people on the ground (aircraft and vessels that are not in the fleet: from
rules 3, section 8.1). Strategic coordination with other operators is in [keep-out.md](keep-out.md)
as an optional integration.

**Cost.** Pair checks are done on a grid of positions at each step; at 5,000 vehicles the cost is
bounded by vehicles per cell. Order of iteration is by uid so results are the same on any machine.
The check logs the list of pairs it found within 2x the minimum, with the times and distances;
replay recomputes from the logged routes.

**Knobs.** The table above; `separation.step_s`, `horizon_s`, `marginal_factor`, `drift_mps`,
`relevant_km`.

**As built: the hub-zone row.** The code applies the air and air minimum (50 m, 30 m) inside a hub
zone too and only names the meeting `SEP_HUB_ZONE`; `air_hub_h_m` and `air_hub_v_m` (20 m, 15 m)
are knobs the rule does not read. The table's smaller hub-zone minimum is not applied: doing so
would loosen what the rule enforces today, and that is the maintainer's call.

### 8.1 Other traffic (from rules 3, `TRAFFIC_SEPARATION_SINCE`)

From rules 3 the rule also reads the company's picture of the traffic it does not control
(docs/design/traffic.md, section 10): `DeliveryInputs::picture`, the tracker's tracks with their
predictions and the receivers heard, cut by the engine to the tracks of the order's vehicles'
domains within `relevant_km` of their positions and the order's points, and logged by value. Code
in `crates/fleetmesh-decide/src/traffic.rs`.

| our vehicle | traffic | minimum (horizontal, vertical) | ahead | hold |
| --- | --- | --- | --- | --- |
| air | manned aircraft (ADS-B; or one our sensors see whose broadcast disagrees) | `manned_h_m` 610 m (2,000 ft), `manned_v_m` 76 m (250 ft): ASTM F3442 well clear; manned aircraft have the right of way | `horizon_s` 60 s | `SEP_TRAFFIC_CONFLICT` |
| air | another operator's drone (Remote ID) | `drone_h_m` 100 m, `drone_v_m` 30 m | 60 s | `SEP_TRAFFIC_CONFLICT` |
| air | an air track that broadcasts nothing | `noncoop_h_m` 300 m, `noncoop_v_m` 60 m; a bearing-only track along its whole line, moving at `noncoop_speed_mps` 20 m/s since | 60 s (a line: `bearing_horizon_s` 20 s) | `SEP_TRAFFIC_NONCOOP` |
| surface | a ship under way (`ship_classes`: cargo, tanker, passenger, ferry; at `underway_mps` 0.5 m/s or more, not moored or at anchor) | closest approach `ship_cpa_m` 463 m (0.25 nm); and its course ahead is not crossed within `ship_cross_ahead_s` 300 s of its arrival (COLREGs rule 9) | `surface_horizon_s` 300 s | `SEP_SHIP_CPA`, `SEP_SHIP_AHEAD` |
| surface | any other vessel | `surface_surface_m` 30 m | 60 s | `SEP_TRAFFIC_CONFLICT` |
| ground | none: no traffic model | | | |

Each minimum is widened by the prediction's 1-sigma times `sigma_factor` (1.0); the vertical
1-sigma grows at most `vrate_mps` (1 m/s) from the track's own (a level aircraft's altitude does
not random-walk; its reported vertical rate is in the prediction). Both altitudes are above mean
sea level (a pressure altitude brought there with the QNH, its conversion's uncertainty in the
sigma: 120 m without a QNH). A vehicle under way (in the air above 5 m, a boat making way) that is
inside a minimum now and opening the distance is leaving it: not a conflict. Every conflict is a
`hold`: automation waits it out on the ground, holds or re-plans under way, and a person's own
order waits for that person. Also: `SEP_PICTURE_DEGRADED` when no ADS-B receiver heard covers an
air route (every point within a hub receiver's range, or a network feed heard), no AIS one a
boat's; `SEP_TRAFFIC_UNKNOWN` for a picture older than `max_age_s` (10 s). A run with no picture
(no traffic source) checks nothing of it and says so. Knobs: `[rules.delivery.separation.traffic]`
(policy schema 3), every figure a draft. The result's detail names each conflict (the track, when,
how far against its widened minimum) and the closest track that did not conflict.

### 8.2 The fleet's own vehicles at and near a hub (from rules 5, `HUB_SEQUENCING_SINCE`)

In real operations a hub sequences its own arrivals and departures, as a vertiport or a drone nest
does: who lands or leaves first is never a person's decision, and a routine return does not wait
for one (docs/DECISIONS.md 2026-11-25, "Hubs sequence their own traffic"). From rules 5:

- **Prediction.** Another of the fleet's vehicles with a dispatched route (the bridge gives the
  rule each vehicle's route still ahead of it, `DeliveryInputs::traffic[..].route`, logged by
  value) flies it at its platform's cruise speed (or faster, as it goes) and its cruise height
  (so a drone lifting off its pad beside a car's way is an aircraft above 20 m, not a vehicle on the
  ground), and is at rest at its end. One with no route to fly that is still (on the ground, or slower than
  `STILL_MPS`, 0.5 m/s: a velocity estimate's noise) is predicted where it is. The uncertainty
  that grows with time (`drift_mps`, 0.5 m/s) grows while either vehicle of the pair moves and
  stops once both are at rest. Before rules 5 a vehicle with no route was carried along its
  velocity for the whole horizon (a parked or hovering drone's 0.3 m/s of noise moved it 90 m over
  300 s, through the hub's other stands), and the uncertainty grew for the whole horizon too: a car
  back in its bay and a drone on its pad 130 m away "met" at +300 s, inside 10 m plus 150 m. No
  minimum moved.
- **Stands and pads.** Every vehicle parks on its own stand at its home base (where it was
  launched) and lands there again; a vehicle at another hub gets a free visitor pad (the hub's
  point, else a ring of pads round it clear of every stand, every vehicle on the ground and every
  return bound there). A pickup by a vehicle on the hub's stands names its own position: it loads
  where it stands (a parcel is brought to it), so it has no route to separate (`PICKUP_IN_PLACE_M`,
  10 m, the task layer's own tolerance). The order rule takes a pickup's or a return's point
  anywhere within the hub's `radius_m` (before rules 5, within 5 m of the hub's point).
- **Parked vehicles are the hub's.** A vehicle parked on a hub's stand (still, not in the air,
  inside the hub's radius) is not counted against an order's route while that route is on the
  same hub's stands, nor against an order that leaves or reaches that hub: the hub's layout (its
  bays, berths and pads, its lane in and out) keeps a vehicle leaving or reaching its stand clear
  of the parked ones. One of ours whose route ends on a hub's stand counts as parked there from the
  time it arrives (its way in is checked as before). Against any other order every minimum applies
  to them as before. A car leaving its kerb bay 6 m from the next car's, or a boat its berth 20 m from the
  next, was held `SEP_HUB_ZONE` before rules 5 for as long as its neighbour stayed parked.
- **Sequencing.** A meeting of two of the fleet's vehicles within `hub_zone_m` of a hub still holds
  `SEP_HUB_ZONE`, every minimum as before; its words say the hub sequences them ("automation holds
  one until the other is clear"; before rules 5, "a person decides who waits"). The dispatcher
  waits it out (`SEQUENCE_CODES`: `SEP_HUB_ZONE`, `SEP_MARGINAL`; and a refusal only for another
  vehicle's route, `SEP_ROUTE_CONFLICT`, whose own fix is a short delay) and never sends the order
  to a person for it: a departure waits on its stand, an arrival where it is. A return waits out
  anything automation waits out the same way. A wait that outlasts `hold_retry_max_s` (300 s) is an
  issue alert (`fleet_conflict`, or `return_held` for a return held by something else) and
  automation goes on waiting.

No minimum, threshold or policy key changed.

## 9. Navigation integrity (`nav_integrity`)

A vehicle whose position fix is wrong is a vehicle that does not know where it is. The rule keeps
a per-vehicle **trust state** and reads it for every order. The vehicle's own autopilot failsafes
(EKF, GPS glitch) still act; this rule decides what the fleet is allowed to ask of it.

**Inputs (per vehicle, each with its age).** From telemetry: GPS fix type, satellites tracked,
HDOP, reported horizontal accuracy (`h_acc`), the EKF's `gps_glitch` and navigation source flags
(GPS, dead-reckoning, constant position), the time the fix began (`gps_fix_from`), the position
report's age, and speed. From the bridge: the vehicle's integrated track (velocity times time
since the last good fix), the link emulator's range from the serving station (cellular and LOS
only), and the areas the position falls in.

**Trust states.**

| state | meaning |
| --- | --- |
| `trusted` | 3D fix, `h_acc <= nav.trusted_acc_m` (air 5, ground 8, surface 10), at least `nav.min_sats` (6) tracked, `hdop <= nav.max_hdop` (2.0), fix age at most `nav.max_age_s` (5), no glitch flag, consistency checks pass |
| `degraded` | 3D fix but `h_acc` above trusted and at most `nav.degraded_acc_m` (air 15, ground 20, surface 25), or fewer satellites, or HDOP above max |
| `untrusted` | no fix, 2D only, glitch flag, dead reckoning or constant position, `h_acc` above degraded, or a consistency check failing |
| `unknown` | no position fact, or older than `nav.max_age_s` |

**Consistency checks.** Spoofing is built so that fix flags look healthy (the stand-in model walks
the position off truth with good satellite count, DOP and fix type). Flags alone cannot catch it.
The rule therefore also checks, from facts the bridge already has:

- **jump:** the new position differs from the previous position plus velocity times dt by more
  than `nav.jump_m` (default 30) plus three times `h_acc`: reason `NAV_JUMP`;
- **drift against ranging:** for a vehicle on cellular or LOS, the emulator reports slant range
  to its serving station; if that disagrees with the range implied by the reported position by
  more than `nav.range_tol_m` (default 150, tower geometry is coarse), `NAV_RANGE_MISMATCH`;
- **impossible place:** the position is on the wrong surface for the platform (a car in water, a
  boat on land by more than 40 m), `NAV_WRONG_SURFACE`;
- **speed:** reported speed above `max_mps` times 1.2, `NAV_IMPOSSIBLE_SPEED`.

These catch a spoofer that walks fast or far, not a slow, careful one. The design says so plainly:
the rule reduces exposure, it does not guarantee detection. A scenario can test how far off a
vehicle can be walked before anything notices.

**State changes have hysteresis.** A vehicle moves to a worse state at once and returns to a
better one only after `nav.recover_s` (default 20 s) of meeting the better state's conditions. After
a spoof ends the EKF resets and the check notices the jump; the recover timer then applies.

**Outcomes by order kind.**

| kind | trusted | degraded | untrusted | unknown |
| --- | --- | --- | --- | --- |
| pickup, deliver, reroute | allow | hold `NAV_DEGRADED_ACCURACY` | refuse `NAV_UNTRUSTED` | hold `NAV_TRUST_UNKNOWN` |
| return_to_hub | allow | allow | hold `NAV_UNTRUSTED` | hold `NAV_TRUST_UNKNOWN` |
| hold | allow | allow | allow for ground and surface; for air hold `NAV_UNTRUSTED` (copter without position cannot hold) | allow |
| land_now | allow | allow | allow (needs no horizontal position) | allow |
| abort | allow | allow | allow if `then` is `land_now` or `hold`, else hold | allow |
| swap_battery, confirm_drop_zone | n/a (vehicle on ground, or not a vehicle order) | | | |

Reason codes seen in the log also give the cause: `NAV_NO_FIX`, `NAV_2D_FIX`, `NAV_GLITCH`,
`NAV_DEAD_RECKONING`, `NAV_CONST_POSITION`, `NAV_STALE_POSITION`, `NAV_LOW_SATS`, `NAV_HIGH_HDOP`.

**In flight.** The same state is evaluated every decision tick for every vehicle with a running
order, not only when an order is proposed. A fall to `untrusted` makes the navigation monitor (a
deterministic agent with a fixed id) propose a `hold` for ground and surface vehicles and a
`land_now` for copters, a `hold` (loiter) for planes. They route by the tables above and the
operator is told in the console. The vehicle's own failsafe (EKF failsafe LAND for copters, Hold
for rovers) may act first and that is logged as a vehicle event.

**Knobs.** The numbers in the table and the lists above, under `[nav]` in `sim/rules.toml`.

**Gap to close.** The standins' GNSS sky is currently fixed (14 satellites, HDOP 1.0) until a
delivery GNSS availability model replaces it (see [link-and-gnss.md](link-and-gnss.md)). Until
then `NAV_LOW_SATS` and `NAV_HIGH_HDOP` fire only under scripted degrade and jam events.

## 10. Drop-zone conditions (`dropzone`)

A **drop zone** is a place a vehicle may release a parcel.

```toml
[[dropzone]]
id = "dz-harbour-17"
kind = "doorstep"          # pad | doorstep | locker | dock | kerb
lon = -122.4119
lat = 37.8087
radius_m = 4
surface = "land"           # land | water
elevation_m = 3.1          # optional; terrain gives it otherwise
confirmed_t = ""           # last confirmation time, set by the engine from approved confirm_drop_zone
closed = false
populated = true
approval = "customer"      # rules 2: survey | customer | withdrawn; absent: a new delivery point
```

**Standing approvals (rules 2).** Real operators approve delivery spots ahead of time: Amazon
surveys obstacle-free delivery zones and marks reachable delivery points in them, and Wing delivers
to a spot the customer chose (about 6 by 6 ft, nothing overhead) until the customer picks another.
Nobody authorises individual drops. A zone's `approval` records how it became an approved delivery
point: `survey`, `customer`, or `withdrawn` (it was, and is not now). A hub pad is approved with its
hub. A person approves any other point once (`confirm_drop_zone`, or by approving a delivery held
for it), and that approval stands from then on.

**Inputs.** The drop-zone record; the area set (is the zone in a `populated` area); the terrain and
tide at the zone; the weather sample at the zone; the latest observation of the zone (scenario
events and vehicle camera simulations: `clear`, `occupied` with count, `unknown`); the time of the
last confirmation.

**Check.**

- zone `closed` or unknown id: refuse `DZ_CLOSED`;
- wrong surface for the platform (a car to a `water` zone): refuse `DZ_WRONG_SURFACE`;
- gust at the zone above `dropzone.descent_gust_mps` (copter default 8, below its flying gust soft
  limit because it descends to a low hover): hold `DZ_GUST_AT_ZONE`; above 10: refuse;
- latest observation `occupied` and recent (`dropzone.obs_max_age_s`, 30): refuse `DZ_OCCUPIED` for
  a pad, hold `DZ_PEOPLE_NEAR` for a doorstep or kerb (a person could judge it a passer-by);
- observation unknown or older than the limit, and the zone is `populated`: hold `DZ_OBSERVATION_STALE`;
- from rules 2, with `dropzone.standing_approvals` (the default): the zone is not an approved
  delivery point (no standing approval, not a hub pad, not approved by a person; or its approval
  was withdrawn): hold `DZ_NOT_APPROVED`. An approved point needs no confirmation and has no time
  limit;
- under rules 1, and from rules 2 with `standing_approvals = false` (the strict comparison
  procedure, `sim/policy-strict.toml`): zone not confirmed within `dropzone.confirm_valid_s`
  (3600) and its kind is not `pad` at a hub: hold `DZ_UNCONFIRMED`;
- terrain slope at the zone above `dropzone.max_slope_deg` (10) for a land release: refuse
  `DZ_SLOPE`;
- boat to a `dock` when the tide check flags `TIDE_DOCK_LEVEL`: the weather rule's hold already
  applies.
- otherwise allow.

At execution time the vehicle looks again before release (from rules 2, the drop-time look,
[delivery-orders.md](delivery-orders.md) section 5.2): clear releases; occupied, unknown or a look
older than `obs_max_age_s` makes it wait over the zone and look again, and a zone that stays not
clear for the bounded wait ends the `deliver` as `failed` with the parcel aboard, the vehicle
brought home and the person told by an issue alert. The look is the vehicle's behaviour after
dispatch, recorded as op states and `delivery.event`s, not a decision replay re-derives. Before
departure the dispatcher reads the zone's look as this rule does and waits while it would hold,
so a busy doorstep costs a delay, not a person's decision.

**Knobs.** `dropzone.standing_approvals` (rules 2; absent: true), `dropzone.confirm_valid_s` 3600
(with standing approvals off), `dropzone.obs_max_age_s` 30, `dropzone.descent_gust_mps` 8,
`dropzone.max_slope_deg` 10.

## 11. Lost link (`lost_link`)

Lost link has two parts. The vehicle's firmware acts without the fleet (the failsafe). The rule
checks, before dispatch, that the failsafe is set the way policy wants and that the route does not
make it likely to fire without a safe outcome.

### 11.1 What each platform does when the link is lost

Defaults, taken from `sim/links.toml` (timers) and the stand-in and SITL parameter files (actions).
Timers are seconds without a GCS heartbeat; each platform's timer follows its main link's profile.

| platform | main / backup | timer | firmware action | policy intent |
| --- | --- | --- | --- | --- |
| delivery_drone (ArduCopter) | cellular / los | 10 s main (cellular `lost_link_s`) | RTL (`FS_GCS_ENABLE=1`); critical battery lands | fly home on the straight line at RTL altitude; land at home pad |
| delivery_car (Rover) | cellular / mesh | 10 s | **hold** then RTL: set `FS_ACTION` to hold, with a fleet-side timer that turns it into RTL after `lost_link.car_hold_s` (60) | stop where it is (in a safe place by the route's design), do not drive blind |
| delivery_boat (Rover boat) | cellular / satcom_leo | 10 s near shore, 30 s once on satcom | RTL | head for home dock; if energy cannot make it, hold station |
| delivery_plane (ArduPlane) | los / satcom_leo | 5 s on los; 30 s on satcom_leo | RTL then loiter at home (`FS_LONG_ACTN=1`) | return to hub and loiter at the home point |

All four: while the link is down the parcel is **never released**. Release needs a confirmation
the vehicle can only receive on a working link. A vehicle at a drop zone with no link holds
there for `lost_link.release_wait_s` (default 30) and then follows its failsafe.

Open: the car default of hold-then-RTL needs a firmware parameter and a fleet-side timer that does
not exist yet (open question 4). Until then the car uses RTL like the others.

### 11.2 The check

**Inputs.** The vehicle's reported lost-link plan fact (action and timeout from its parameters,
logged as `lost_link`); the platform's expected plan from policy; the link coverage along the
route from the emulator's route prediction (`crates/fleetmesh-linkemu/src/coverage.rs`, `route_answer`),
which walks the failsafe timer at the vehicle's speed; the energy rule's return cost.

**Check.**

- the vehicle's failsafe action or timer differs from policy's: refuse `LINK_FAILSAFE_MISMATCH`
  (the dispatcher re-proposes after the bridge applies the parameters; a person is not asked);
- the plan fact is missing: hold `LINK_PLAN_UNKNOWN`;
- the predicted coverage walk says the failsafe trips on the route and the return from that
  point exceeds the energy available: refuse `LINK_FAILSAFE_UNSAFE`;
- the failsafe trips on the route but the return is energy-safe: hold `LINK_COVERAGE_GAP` if the
  predicted time without link exceeds `lost_link.gap_hold_s` (default 30) and the vehicle has no
  backup link that covers it; otherwise allow;
- the main link is down now and the backup carries (failover active): allow, but add the note
  `LINK_ON_BACKUP` to the logged result; `deliver` for a vehicle on a backup radio with
  `link.rate_low` (satcom iridium) holds `LINK_BACKUP_SLOW`.
- orders for a vehicle that is not reachable now are queued, not refused ([delivery-orders.md](delivery-orders.md)).

**Knobs.** `lost_link.gap_hold_s` 30, `lost_link.release_wait_s` 30, `lost_link.car_hold_s` 60, and
the per-platform expected plan table above, under `[lost_link.<platform>]`.

## 12. Deterministic and logged

Every rule is a pure function of its logged inputs, rules config and rules version. `check.result`
for each rule records: rule id, outcome, reason codes and text, and the inputs: the vehicle facts
with ages, the area set hash and the active temporary list, weather and tide values by sample
(source, model, archive checksum), the energy model's per-leg numbers, the traffic routes and the
conflict list, the drop-zone record and observation, the link coverage answer, and the nav trust
state with the checks that set it. Replay recomputes every outcome from the log alone.

Not logged because not an input: the operator's reasoning, a vehicle's firmware behaviour after
dispatch.

## 13. Tests to write

- Geofence: route crossing no_go refuses; route inside approved allows; temporary area active at
  ETA refuses; area starting after ETA but within `soon_s` holds; area hash mismatch refuses.
- Energy: tailwind and headwind change cost in the right direction; ground speed under the
  threshold refuses; margin bands (allow, hold, refuse) at the boundaries; stale battery holds;
  operator reserve only tightens.
- Payload: over max, over derated, near max, unknown mass; hazmat refuses.
- Weather: each platform's soft and hard thresholds at exact boundary values; missing sample holds;
  boat current ratio and tide depth; flood rule for the car.
- Separation: two head-on routes conflict; crossing routes at different layers do not; hub zone
  contention holds; 5,000-vehicle test finishes in the time budget and gives the same result when
  the vehicle order is shuffled.
- Nav integrity: each trust state from facts; hysteresis; jump check; range mismatch; the order
  table; slow spoof under the thresholds passes (documenting the gap).
- Drop zone: each branch; a standing approval passes with no time limit, a new point holds
  `DZ_NOT_APPROVED` until a person approves it, a withdrawn approval holds; the strict procedure
  and rules 1 hold `DZ_UNCONFIRMED` outside `confirm_valid_s`; the drop-time look waits and keeps
  the parcel when the zone stays not clear.
- Lost link: expected plan mismatch refuses; coverage gap holds; failover note logged.
- Replay: every rule round-trips; a tampered check outcome in the log is reported.
- Property: `refuse` for a safety order only when the rule's own list says so.

## 14. Open questions

1. Hold band widths (soft against hard). The draft uses about 20% of the hard value. Different
   values change inbox load a lot; start from these and let scenario 2 tune them.
2. Whether to allow weather lookahead from the archive and call it a forecast. It is information
   the vehicle would not have had in the replayed past, but the operator of a real fleet would
   have a forecast. A honest option is to add a forecast error model with logged parameters.
3. Do holds from separate rules on one order get one inbox card or several? Draft: one card with
   all reasons.
4. The car's hold-then-RTL needs a fleet-side timer. Implement in the bridge (it sees heartbeats),
   or choose RTL for the car and accept driving home?
5. Should `refuse` for an agent's proposal still appear in the operator's log view (count of
   refusals)? The attention-cost doc counts them as non-interruptions by default.
6. Population data for `populated` areas: hand-drawn polygons for now; OpenStreetMap landuse could
   be baked into the area set later.
7. Whether `hold` and `unverified` should stay two outcomes after the first studies, or merge.

## 15. As built (2026-11-25)

Rules 2 (`STANDING_APPROVALS_SINCE`): the drop-zone rule's confirmation part, `policy_facts`'s
`hub_or_confirmed` for a `deliver`, and the order rule's `ORDER_NO_DELIVER_WAITING` for a
`confirm_drop_zone` branch on `RulesConfig::standing_approvals`; each keeps its rules 1 words for
rules 1 and the strict procedure, so a rules 1 log replays unchanged. Tests:
`crates/fleetmesh-decide/tests/delivery.rs` (each branch, both versions, both settings, a rules 1
log and a rules 2 log replayed, each given the other's header).


`crates/fleetmesh-decide/src/delivery_rules.rs`, tests in `crates/fleetmesh-decide/tests/delivery.rs`.
Every check returns one result per vehicle with its reason codes (`CheckResult::codes`), and a
`hold` blocks nothing but sends the order to a person whatever its tier. Gaps, each an explicit
input (`delivery::DeliveryInputs`) the rule reads and never defaults:

- Weather, drop-zone observations, parcels and link predictions are fed to the bridge
  (`Decision::feed`); nothing feeds them in a live run yet, so a delivery there holds with
  `WX_UNKNOWN` and `DZ_OBSERVATION_STALE` until a source is wired (phases 4, 6, 7).
- The route altitude of the geofence ceiling check is the platform's `typical_alt_m`; a reroute is
  not checked for entering an area the old route did not cross; the geofence does not compare the
  area set's hash with the run's.
- Navigation: the bridge reads satellites, HDOP, accuracy and the position jump from the vehicle's
  GNSS report; the range check, the wrong-surface check and the hysteresis state are inputs the
  bridge does not track yet (`NavFacts::range_mismatch_m`, `wrong_surface`, `prev_state`).
- The in-flight monitor that proposes a hold or land when trust falls (section 9) is not built.
- Separation reads other vehicles' positions and velocities; their routes are not yet in the
  bridge's traffic facts. From rules 3 it reads the picture of other traffic too (section 8.1);
  tests in `crates/fleetmesh-decide/src/traffic.rs` and `tests/delivery.rs` (a low helicopter held
  for and a high one not, the trimmed picture logged and replayed, a stale or uncovered picture, no
  picture, a car; rules 1, 2 and 3 logs replayed, each with the wrong header found).

