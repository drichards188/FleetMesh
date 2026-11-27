# fleetmesh-traffic

The traffic FleetMesh does not control, and what a private delivery company could know about it.
It simulates the world (airliners, helicopters, seaplanes, other operators' drones, ships,
ferries, tugs and small craft around the Golden Gate), the company's passive receivers and own
sensors observing it, and what that equipment really emits: SBS-1 BaseStation lines, NMEA 0183
`!AIVDM` sentences, and JSON lines for Remote ID and sensor detections. The contract is
[docs/design/traffic.md](../../docs/design/traffic.md) (sections 1, 3 and 5); the company-side
tracker is `crates/fleetmesh-tracker`.

**Illustrative.** Positions are close to real (runway thresholds, the traffic separation scheme,
the Bay's traffic lanes, ferry terminals) and rates are plausible for a Sunday morning in June,
but timetables are invented and every name, call sign, ICAO address and MMSI is made up. Nothing
here is connected to anything real. The fleet only listens on other traffic's frequencies: this
crate simulates other parties' broadcasts, the company's receivers and its collision-avoidance
sensors, and transmits nothing.

## What it simulates

**The world** (region 37.45 to 38.05 N, 122.90 to 122.10 W), generated per stream from a seed and
started warm (schedules begin four hours before the start):

| Stream | What |
| --- | --- |
| `sfo-arrivals`, `sfo-departures` | West flow: landing 28L/28R (32 an hour, staggered, about the 2026 cap), departing 01L/01R (30 an hour); arrival routes from the north, east, south and west joining a 10 nm final on a 3 degree path, the rollout to a runway exit; departures rolling, lifting off and climbing out on runway heading, then a turn on one of four routes. Airlines by share, 6 % business jets. |
| `oak-arrivals`, `oak-departures` | Runway 30, six an hour each way. |
| `overflights` | 24 an hour at FL250 to FL390. |
| `heli-tour`, `ga-bay-tour`, `ga-coastal` | Tour helicopters out of SFO round the Bay Bridge, Alcatraz and the Golden Gate at 1,000 to 1,200 ft; the Bay VFR tour and coastal transits at 1,500 ft, squawking 1200. |
| `seaplane` | Off Richardson Bay near Sausalito: taxi, take-off run on the water, a tour of the Gate and the city, landing. |
| `uscg-heli`, `medevac` | An occasional Coast Guard patrol from SFO along Ocean Beach and the Gate; a medical flight from the East Bay to the Mission Bay helipad, 25 minutes on the pad. |
| `drones` | Other operators' drones at a dozen spots (Crissy Field, Marina Green, Fort Mason, Sausalito, Treasure Island...): take-off, legs, hovers and orbits at 25 to 120 m, landing. 85 % with Remote ID. |
| `ships-oakland`, `ships-richmond`, `ships-north-bay` | Deep-draft ships in and out through the traffic separation scheme (northern, western and southern approaches, lane centres), slowing for the pilot at the station in the precautionary area, the bar channel, the Golden Gate lanes, inbound south of Alcatraz and outbound north of it, the deep water lane north to Richmond and San Pablo Bay; berths at Oakland's outer harbour and Richmond's Long Wharf; moored 10 to 30 hours. Each movement has a pilot launch from Pier 9 meeting it at the station, and escort tugs at Oakland and Richmond. |
| `pilot-station`, `anchorage-9`, `moored-oakland`, `cruise-pier27`, `tugs-idle` | The pilot station boat holding near the boarding area, ships at anchor south of the Bay Bridge, ships at their berths, a cruise ship at Pier 27, tugs at their piers. |
| `ferry-*` | Larkspur, Sausalito and Tiburon to the Ferry Building, Oakland and Alameda, Richmond, Pier 41 to Sausalito and Tiburon, Tiburon to Angel Island, Pier 33 to Alcatraz every 30 minutes; each route's vessels shuttle all day at weekend headways and lie moored overnight. |
| `bay-cruise`, `tug-barge`, `fishing-return`, `fishing-drift` | Hour-long cruises under the bridge from Pier 39/41; tug and barge transits; charter boats coming home through the Gate to Fisherman's Wharf; boats drifting for halibut off Alcatraz. |
| `sailing-cityfront`, `sailing-east-bay`, `pleasure`, `kayaks` | A Sunday's sailboats on the Cityfront (and off Berkeley), motorboats out of the marinas, kayaks off Aquatic Park. Areas keep clear of the shore and islands; craft leave and return by their marina's channel. |

Every vessel position was checked against the baked land/water grid
(`sim/terrain/golden-gate.water.grid.gz`): none of the three scenarios puts a vessel on land
(`tests/world.rs`, `vessels_keep_to_the_water`).

**Equipage**, drawn per object at the rates in `[equipage.<class>]`: ADS-B 1090 extended squitter,
UAT 978, a Mode S transponder without ADS-B (radar replies only), or none; AIS Class A, Class B
(some also sending message 19), none, or fitted but off; Remote ID over Bluetooth 4, Bluetooth 5
long range or Wi-Fi, or none. Privacy ICAO Addresses (PIA): `[addresses] pia_block`
(ADF800 to ADFBFF, a contiguous block chosen for the simulation: the FAA does not publish its
pool), a fresh address every flight and a third-party callsign (`XPV` and digits) instead of the
registration; this block is what the tracker needs as `anonymous_icao_ranges`. LADD: the
aircraft broadcasts its callsign as usual (the company's own receivers hear it), and the network
feed withholds it. UAT anonymous mode: a self-assigned address (written `~` on SBS) and no
callsign. Military and government aircraft (the Coast Guard): `[addresses] military_block`.

**Sensors** (all listed at `/sensors`):

| Sensor | Where | What it gives |
| --- | --- | --- |
| `adsb_1090`, `adsb_uat`, `ais`, `rid` | each hub in sim/hubs.toml, on a 10 m mast | what it hears, in the wire formats below |
| `net-adsb`, `net-uat`, `net-ais` | a commercial aggregator's feeders on Twin Peaks, Mount Tamalpais, the Berkeley Hills, San Bruno Mountain and others | wider coverage, 2 to 5 s late (AIS 3 to 10 s), LADD honoured |
| `daa` | each own drone or plane in the air | bearing and elevation (vision: no range), 1 Hz; 50 % detection at about 1.8 km for a light aircraft, 6 km for an airliner, 350 m for a drone; a 60 degree vertical field of view |
| `marine_radar` | each own boat, antenna 3 m | range and bearing every 2.5 s; 50 % at 16 km for a ship, 3 km for a sailboat, 600 m for a kayak; the radar horizon; sea clutter |
| `rid` (onboard) | each own aircraft in the air | Remote ID it hears within 800 m |
| `ground_radar` | `radar-pier`, off by default (`enabled = false`) | range, bearing, elevation and Doppler, 1 Hz |

The company's other own vehicles appear in onboard detections like anything else (a camera does
not know whose drone it is); the tracker drops them against the fleet's positions.

**Physics.**

- Range from radio line of sight: the radio horizon for a 4/3 Earth, the terrain profile from the
  baked surface grid the headless link emulator already reads (`sim/terrain/golden-gate.grid.gz`,
  tidewater's `max(elevation, water level)` on a 20 m grid; it has data for 37.77 to 37.88 N,
  122.56 to 122.38 W: the Marin Headlands, the Presidio, the northern city hills, Angel Island,
  Alcatraz) and, beyond it, a mask of 19 named hills as smooth domes (Twin Peaks, Mount Sutro, San
  Bruno Mountain, Mount Tamalpais, Yerba Buena Island, the East Bay hills...). The profile is
  judged by the link emulator's own `fleetmesh_linkemu::terrain::path_obstruction_m` (reused: a
  dependency on fleetmesh-linkemu); the grid file is read in the format of the bridge's
  `decision/terrain.rs`. Answers for fixed receivers are cached by target cell and altitude band
  and computed from the cell's centre, so the cache never changes a result.
- Message loss rising with range: `base + (1 - base) (d / range)^4` (Remote ID `^3`), each draw
  keyed by object, channel, event and receiver. AIS slot loss: `1 - exp(-load x slot_factor)`
  with the channel load (reports a minute heard around the receiver over 2 x 2,250 slots)
  recomputed every 10 s.
- Update intervals: ADS-B airborne position and velocity every 0.5 s (+-0.1 s), identification
  every 5 s, the DF11 acquisition squitter every second, surface position every 0.5 s moving and
  5 s still, replies to the terminal radar's 4.8 s sweep (`MSG,5`, `MSG,6`), TCAS replies (`MSG,7`)
  from airliners and business jets; UAT once a second with the callsign one in four. AIS Class A by
  ITU-R M.1371-5 table 1 (3 min moored or at anchor below 3 kn, else 10 s, 3 1/3 s turning,
  6 s above 14 kn, 2 s turning or above 23 kn), message 5 every 6 minutes; Class B CS 30 s above
  2 kn and 3 min below, message 24 parts A and B every 6 minutes. Remote ID location once a second.
- Latency: own receivers 5 to 80 ms; networks as above; detections 150 to 250 ms.
- Position error by stated accuracy: each object's navigation error is a smooth, correlated
  process (two minutes) of 3 m (airliners, AIS with DGNSS), 6 m (light aircraft) or 2 m (drones),
  plus a little white noise per message; pressure altitude from the QNH (`[world] qnh_hpa`, 1016),
  25 ft steps; vertical rate in 64 ft/min steps.
- Remote ID `alt_geo_m` is height above the WGS84 ellipsoid, as ASTM F3411 defines geodetic
  altitude: mean sea level height plus the geoid height `[world] geoid_n_m` (-32 m on the Bay,
  EGM96). `alt_baro_m` is pressure altitude, `height_agl_m` above the take-off point.

**Scripted issues** (`[[issue]]` in a scenario's traffic.toml): `drone_no_rid` (a drone with no
Remote ID at a point, for a while), `ship_arrival` (a ship of a destination stream under the
Golden Gate Bridge at `at_s`, with its pilot launch), `adsb_bad_position` (an aircraft of a route
stream whose ADS-B position is `offset_m` off on `bearing_deg` for `duration_s`: a faulty or
spoofed navigation source), `ais_off` (a vessel that switches its AIS off), `receiver_down` (a
receiver, by id and optionally `sensor` kind, or a network feed, silent for a while).

| Scenario | Seed | Issues |
| --- | --- | --- |
| harbour-city | 11 | medical helicopter low across the waterfront (360 s), drone without Remote ID at Marina Green (360 s), container ship under the bridge (960 s), tour helicopter's ADS-B 700 m off (960 s, 10 min), hub-pier AIS down (1860 s, 10 min) |
| wind-event | 22 | fewer drones, kayaks, seaplanes, tours; more sailors. Drone without Remote ID by the Crissy Field hub (30 s, 5 min), hub-presidio 1090 down (120 s, 10 min), Bay tour aircraft's ADS-B 1.2 km off (360 s, 10 min), network ADS-B outage (960 s, 4 min) |
| coastal-boat | 33 | container ship (1800 s) and tanker (4500 s) under the bridge, a fishing boat dark off Alcatraz (2400 s, 30 min), hub-pier 1090 down (4000 s), drone without Remote ID over the pier (5000 s) |

## Wire formats (docs/design/traffic.md section 5)

- **SBS** (TCP `FLEETMESH_TRAFFIC_SBS_PORT`, 8768): dump1090's port 30003 lines,
  `MSG,<type>,<session>,1,<hex>,1,<date gen>,<time gen>,<date logged>,<time logged>,<callsign>,<alt>,<gs>,<track>,<lat>,<lon>,<vr>,<squawk>,<alert>,<emergency>,<spi>,<ground>\r\n`,
  formatted as dump1090-fa's and readsb's `modesSendSBSOutput` (five-decimal positions, whole
  knots and degrees, flags `-1`/`0`/empty, the callsign padded to eight characters). The session
  is the receiver's `num` in `/sensors`; dates and times are UTC; "generated" is the transmission
  time and "logged" the receipt (a network's delivery) time; a non-ICAO address is `~` and six hex
  digits (readsb's convention). Types 1 to 8 are all emitted.
- **AIS** (TCP `FLEETMESH_TRAFFIC_AIS_PORT`, 8769): `\s:<receiver id>,c:<unix seconds>*hh\!AIVDM,...*hh\r\n`
  on every sentence (both fragments of message 5 too), messages 1, 2, 3, 5, 18, 19 and 24 A and
  B, channels A and B alternating, 60-character fragments, sequence ids 0 to 9 per receiver. `c`
  is the receiving station's clock (for `net-ais`, the feeder's; the line arrives 3 to 10 s
  later).
- **JSON lines** (TCP `FLEETMESH_TRAFFIC_JSON_PORT`, 8771): `{"type":"rid",...}` and
  `{"type":"det",...}` exactly as in the contract, keys in its order, absent measurements null.
- **HTTP** (`FLEETMESH_TRAFFIC_HTTP_PORT`, 8767): `/health`; `/sensors` (each receiver and
  sensor: `id`, `kind`, `num` (SBS session), `owner` (`company` or `network`), `platform`, `lat`,
  `lon`, `alt_m`, `nominal_range_m`, `feeders`, `enabled`, `up`; a hub's receivers share its id,
  so entries are unique by id and kind); `/truth` (evaluation only: every object with class,
  position (altitude above mean sea level), velocity, identities, equipage and active faults);
  `/stats` (objects by class, lines by stream, messages and messages a second over the last
  minute by source, losses by reason, scans and detections, the bridge poll, TCP clients).

## The library (what the bridge and the headless simulator call)

```rust
use fleetmesh_traffic::{Options, OwnKind, OwnVehicle, Stream, Traffic};

let mut t = Traffic::from_files("sim/traffic.toml".as_ref(),
    Some("sim/scenarios/harbour-city/traffic.toml".as_ref()),
    Options { seed: None, start_ms: None, duration_s: None })?;   // the scenario's seed and `at`
let start = t.start_ms();
t.set_own_vehicles(vec![OwnVehicle { uid: "12".into(), kind: OwnKind::Drone, lat: 37.80, lon: -122.45,
    alt_msl_m: 93.0, alt_rel_m: Some(90.0), heading_deg: 270.0, speed_mps: 8.0 }]);
for e in t.step_to(start + 1_000) {          // every line delivered by then, in order
    match e.stream { Stream::Sbs => {}, Stream::Ais => {}, Stream::Json => {} }  // e.line: the bytes
}
let truth = t.truth();                        // Vec<TruthObject>, evaluation only
let sensors = t.sensors();                    // the /sensors entries
let stats = t.stats();
```

`step_to` takes any increasing times. The same seed and start give the same truth and the same
bytes whether it is stepped by 1 s, 10 s or irregularly (`tests/world.rs`): every random draw is
a pure function of the seed and a stable key, objects are named by stream and number, schedules
run ahead of the clock, and lines are sorted by delivery time and a total key before they are
returned. Onboard sensors scan from the own-vehicle positions last given, so their output depends
on when the caller updates them. Run as fast as you like: an hour of harbour-city takes about 3 s
in a release build (about 1,300 times real time).

## Running it

```sh
# live, on the scenario clock (FLEETMESH_AT, FLEETMESH_RATE; unset FLEETMESH_AT means the wall clock)
FLEETMESH_AT=2026-06-14T17:00:00Z FLEETMESH_RATE=1 cargo run -p fleetmesh-traffic --release -- \
    --config sim/traffic.toml --scenario sim/scenarios/harbour-city/traffic.toml --bridge http://127.0.0.1:8765
timeout 5 nc 127.0.0.1 8768 | head        # SBS
curl -s http://127.0.0.1:8767/stats

# headless: an hour of a scenario, the census and the stats
cargo run -p fleetmesh-traffic --release --example headless -- harbour-city 3600

cargo test -p fleetmesh-traffic
```

Options: `--seed N`, `--bridge URL` (`off` for none), `--bridge-poll-ms` (1000), `--step-ms`
(100), `--bind` (or `FLEETMESH_TRAFFIC_BIND`, default 127.0.0.1). Own vehicles come from the
bridge's `GET /api/state` (`vehicles[]`: uid, kind, position, alt_msl, alt_rel, heading,
groundspeed), read-only over plain HTTP/1.0; when the bridge does not answer, the onboard
sensors have no platforms and everything else runs. Live runs schedule 48 hours (`horizon_s`).

A minute at rate 1 on harbour-city with the bridge's 54 vehicles (2026-11-25): 148 objects
(sailing 56, pleasure 16, ferry 12, fishing 12, airliner 11, cargo 10, tug 9, tanker 6, pilot 5,
small craft 4, helicopter 3, and one each of bizjet, GA, passenger and seaplane); about 247 SBS
lines, 8 AIS sentences and 43 JSON lines a second. Messages a second by source: ADS-B hub-pier
71.7, hub-marina 50.5, hub-presidio 45.7, net-adsb 84.7; AIS hub-pier 2.2, hub-marina 1.4,
hub-presidio 1.0, net-ais 3.5; marine radar 35.9 and detect-and-avoid 8.0 over all own
vehicles (UAT and Remote ID are rarer: about 2 UAT lines and 0.2 Remote ID lines a second over
the hour).

## Configuration

- `sim/traffic.toml`: `[world]` (region, local time offset, QNH, geoid height, warm-up, horizon),
  `[addresses]` (PIA and military blocks), `[terrain]` (grid, hills), `[[airport]]` (runways,
  airlines), `[[stream]]` (one per stream, `kind` = `arrivals`, `departures`, `overflights`,
  `route`, `drones`, `ships`, `ferry`, `moored`, `wander`, `pilot_station`), `[ships]` (pilot
  station, launch route, bar channel, approaches), `[equipage.<class>]`, `[receivers]`
  (`hubs = "hubs.toml"`, kinds, mast, ranges, losses), `[[network]]`, `[onboard]` (`daa`,
  `marine_radar`, Remote ID), `[[ground_radar]]`. Route points are `[lat, lon]`,
  `[lat, lon, alt_ft, kt]` or `[lat, lon, alt_ft, kt, hold_s]`; a negative altitude is on the
  ground or water.
- `sim/scenarios/<name>/traffic.toml`: `seed`, `at`, `duration_s`, `[rates]` (a multiplier per
  stream name), `disable`, `[[issue]]`. The scenario.toml files are not read; wiring them in is a
  later step.

## Sources

- SFO runway ends and true headings (28L/28R 298, 01L/01R 028): FAA data via
  [SkyVector](https://skyvector.com/airport/SFO/San-Francisco-International-Airport) and
  [AOPA](https://www.aopa.org/destinations/airports/SFO/details); OAK runway 30 (310 true):
  [AirNav](https://www.airnav.com/airport/KOAK), [airport-data.com](https://airport-data.com/airport/OAK/).
- SFO rates: 386,507 operations in 2024, about 1,056 a day
  ([airport statistics](https://roadgenius.com/statistics/airports/usa/san-francisco-sfo-airport));
  the March 2026 end of side-by-side visual approaches and the cap of 36 arrivals an hour
  ([Airways](https://www.airwaysmag.com/new-post/faa-reduces-sfo-arrival-rate-to-36-per-hour-no-side-by-side-approaches)),
  later raised by about six ([Travelers Today](https://www.travelerstoday.com/articles/60829/20260813/sfo-adds-six-more-arrivals-per-hour-faa-partially-reverses-landing-restriction.htm));
  [FAA capacity profile 2019](https://www.faa.gov/sites/faa.gov/files/airports/planning_capacity/profiles/SFO-Airport-Capacity-Profile-2019.pdf).
  OAK rates, arrival and departure routes and the tour routes are illustrative, not procedures.
- The traffic separation scheme off San Francisco: [33 CFR 167.401](https://www.law.cornell.edu/cfr/text/33/167.401)
  to [.404](https://www.law.cornell.edu/cfr/text/33/167.404) (precautionary area, northern,
  southern and western approaches); the Golden Gate and Central Bay traffic lanes, the deep water
  lane and the 15 kn limit: [33 CFR 165.1181](https://www.law.cornell.edu/cfr/text/33/165.1181).
- AIS reporting intervals: [ITU-R M.1371-5](https://www.itu.int/rec/R-REC-M.1371) annex 2,
  tables 1 and 2 (the interval values from the recommendation as widely summarised; the search
  could only confirm the 3 min, 30 s and 2 s to 3 min figures in [ITU-R M.2084](https://www.itu.int/dms_pub/itu-r/opb/rep/R-REP-M.2084-2007-PDF-E.pdf)).
- ADS-B timing: position every 0.5 +- 0.1 s ([ITU-R M.2413](https://www.itu.int/dms_pub/itu-r/opb/rep/R-REP-M.2413-2017-PDF-E.pdf));
  identification (5 s) and acquisition squitter (1 s) rates are RTCA DO-260B's as commonly
  summarised, not checked against the standard. Carriage: 14 CFR 91.225; AIS carriage 33 CFR
  164.46; Remote ID 14 CFR 89 (broadcast at least once a second) and ASTM F3411. Remote ID ranges
  (350 m Bluetooth 4, 900 m Wi-Fi, 1.2 km Bluetooth 5 long range) are assumptions in the range of
  field reports (for example [ASSURE A40](https://www.assure.msstate.edu/wp-content/uploads/2021/06/A40_Final_Report.pdf)).
- SBS-1 format: dump1090-fa's and readsb's `net_io.c`
  ([dump1090-fa](https://github.com/flightaware/dump1090/blob/master/net_io.c),
  [readsb](https://github.com/wiedehopf/readsb/blob/dev/net_io.c)), consulted, nothing copied.
- AIVDM encoding: [gpsd's AIVDM/AIVDO document](https://gpsd.gitlab.io/gpsd/AIVDM.html) (no
  licence stated, so nothing copied from it). Test vectors (real sentences with decoded values,
  and tag block examples) are from [pyais](https://github.com/M0r13n/pyais)'s test suite under
  the MIT licence, whose notice is in `tests/reference.rs`.
- ICAO address blocks by state: ICAO Annex 10 Volume III (the US civil block A00001 to ADF7C7);
  ANSI/CTA-2063-A serial numbers for Remote ID; IMO number check digits; maritime identification
  digits for MMSIs. Ferry terminals and routes follow the operators' published route maps,
  placed on the water side of each dock with the baked water grid.

## Limits

- The world is the region only: aircraft beyond it (which real receivers hear out to 200 nm and
  more) are not simulated, nor San Jose, Hayward, San Carlos or Palo Alto traffic except as the
  routes' ends.
- Procedures are shaped like the real ones (STAR entry directions, the 3 degree final, departures
  climbing on runway heading), not flown from charts; no wind, so no crab angles in the air and no
  current set at sea (a small fixed heading offset on vessels with a gyro); no conflicts are
  avoided between objects.
- Terrain is the 20 m grid (with data only where tidewater has it) and 19 hill domes; buildings
  are not modelled, so a low aircraft over the city is visible where in reality downtown would
  block it. A 1090 receiver's ground-traffic reception at SFO is as line of sight allows.
- Losses are a range law, not a link budget: no antenna patterns, multipath, garble from
  overlapping replies or 1090 FRUIT. AIS slot collisions are a load model, not SOTDMA slot by slot.
- ADS-B and Mode S content is SBS-level (no CPR, no raw frames, no emitter category, NACp or
  geometric altitude): what dump1090 passes on. UAT arrives as SBS, as readsb gives it.
- Remote ID merges the basic ID, location and operator messages into one line per received
  location message, as a receiver application does. The geoid height is one number for the Bay.
- Onboard sensors use the latest own-vehicle positions given, not interpolated to the scan time;
  detect-and-avoid ignores aircraft on the ground and surface traffic.
- Ferry vessels keep to their timetable without delay; ships' berths, anchorage and dwell times
  are illustrative.
- Rates are tuned for a Sunday late morning. Streams with `active_local_h` stop outside their
  hours, but airport, overflight and ship rates do not follow the hour of day, so a live run
  through the night keeps daytime airport traffic.
