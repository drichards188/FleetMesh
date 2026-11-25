# Other traffic: aircraft and vessels FleetMesh does not control

Status: design draft, 2026-11-25. The contract between the traffic simulator
(`crates/fleetmesh-traffic`), the company-side tracker (`crates/fleetmesh-tracker`) and the
bridge. Section 8 is the build order.

## 1. Purpose

A delivery fleet shares its airspace and water with traffic it does not control: airliners in and
out of SFO and OAK, helicopters and seaplanes on tours, other operators' drones, container ships,
tankers, ferries, tugs, pilot boats, sailboats and kayaks. A real delivery operator sees that
traffic only through what it can receive or detect, never as ground truth. FleetMesh simulates the
traffic (the world) and, separately, what a private delivery company could know about it (the
picture), and decides only on the picture.

The world and the picture meet only through the wire formats in section 5. The tracker must work
unchanged on a real ADS-B receiver (dump1090's SBS output) or a real AIS receiver (NMEA AIVDM),
which is the test that it uses nothing a company would not have.

## 2. What a private company can see

| Traffic | Broadcasts | The company receives it with | Cannot see |
| --- | --- | --- | --- |
| Airliners, business jets, most general aviation, helicopters | ADS-B Out (14 CFR 91.225: required in Class B and C, inside the 30 nm Mode C veil around SFO, above 10,000 ft MSL) on 1090ES; some GA on UAT 978 | Its own receivers at hubs (radio line of sight, about 1 Hz positions); a commercial aggregated feed (wider coverage, seconds of delay) | Aircraft with no ADS-B Out (rare inside the veil: no electrical system, some exempt operations); callsigns blocked by LADD; owners behind a rotating anonymous address (PIA) |
| Other operators' drones | Broadcast Remote ID (14 CFR Part 89: Bluetooth or Wi-Fi, about 1 Hz, receivable from a few hundred metres to about 1 km) | Remote ID receivers at hubs and on its own vehicles | Non-compliant drones with no Remote ID; anything beyond receiver range |
| Ships, tankers, ferries, passenger vessels, tugs, pilot boats, larger fishing boats | AIS Class A (33 CFR 164.46): positions every 2 to 10 s under way by speed and turn, 3 min at anchor or moored; static data every 6 min | Its own AIS receiver (VHF line of sight, tens of km over water); a commercial aggregated feed (delayed) | Vessels with AIS off |
| Some sailboats and pleasure craft | AIS Class B: every 30 s under way, 3 min slow | Same receivers | Most small boats carry no AIS |
| Kayaks, paddleboards, small sailboats, non-compliant drones, aircraft with no ADS-B | Nothing | Only its own sensors: onboard detect-and-avoid on drones (vision or acoustic, about 1 to 2 km for a light aircraft, less for a drone), marine radar on its boats (a few nm, weak on small craft), an optional ground radar at a hub | Anything outside those sensors' reach |

Airspace information (TFRs, NOTAMs, the UAS facility map grid) is out of scope here; the keep-out
spec (keep-out.md) covers it.

## 3. The world (fleetmesh-traffic)

A deterministic simulation, seeded per scenario, stepped on the scenario clock (`FLEETMESH_AT`
plus elapsed x `FLEETMESH_RATE`), usable as a library (headless runs step it as fast as they like)
and as a live binary. Region: latitude 37.45 to 38.05, longitude -122.90 to -122.10 (SFO, OAK,
the Golden Gate approaches and every scenario's area).

- **Airports.** SFO and OAK arrivals and departures on real runway geometry and the usual west
  flow (SFO landing 28L/28R and departing 01L/01R, OAK landing and departing 30), at a rate
  set per scenario and plausible for the hour, with climb and descent profiles.
- **Low-level aircraft.** Tour helicopters and the Bay VFR tour around the Golden Gate and
  Alcatraz at about 1,000 to 1,500 ft, seaplanes taking off and landing on Richardson Bay near
  Sausalito, an occasional Coast Guard or medical helicopter, other operators' drones with and
  without Remote ID.
- **Vessels.** Deep-draft ships in and out through the Golden Gate in the traffic separation
  scheme and the main ship channel to the Port of Oakland and Richmond, with pilot boats meeting
  them; ferries on the real routes (Ferry Building and Pier 41 to Sausalito, Tiburon, Angel
  Island, Larkspur, Oakland and Alameda; Pier 33 to Alcatraz); a cruise ship at Pier 27; tugs;
  fishing boats from Fisherman's Wharf; sailboats on the Cityfront; kayaks off Aquatic Park.
- **Equipage** per object (ADS-B 1090 or UAT or none, AIS A or B or none, Remote ID or none,
  anonymous address, LADD), drawn per class at realistic rates.
- **Sensors** emit what each would really produce, with range from radio line of sight (and the
  terrain where it matters, as behind the Marin Headlands), update intervals, latency, position
  error by stated accuracy, message loss rising with range, and AIS slot loss.
- **Truth** is served separately for evaluation and the debug overlay, and is never an input to
  the tracker or the engine.

## 4. The picture (fleetmesh-tracker)

A library the bridge and the headless simulator link. It does not depend on fleetmesh-traffic.

- Decoders for SBS-1 (BaseStation) lines, NMEA 0183 AIVDM/AIVDO (multi-sentence assembly, 6-bit
  payloads, message types 1, 2, 3, 5, 18, 19 and 24, NMEA 4.10 tag blocks) and the JSON lines of
  section 5.
- Tracks keyed by identity for cooperative traffic (ICAO address, MMSI, UAS ID), a
  constant-velocity Kalman filter in local metres, de-duplication across receivers, and
  association of sensor detections (bearing, optional elevation and range) to tracks by gating;
  unassociated detections start tentative non-cooperative tracks, confirmed after M of N looks.
- Track states tentative, confirmed and coasting, then dropped after a per-source silence.
- Quality and integrity flags: stale, anonymous address, a cooperative position that disagrees
  with the company's own sensors (a faulty or spoofed broadcast), position jumps, invalid MMSI.
- A prediction of each track's position over the next minutes, for conflict checks.

## 5. Wire contract (the live binary)

| Port (env var) | Default | Content |
| --- | --- | --- |
| `FLEETMESH_TRAFFIC_HTTP_PORT` | 8767 | `GET /health`, `/sensors` (each receiver and sensor: id, kind, owner company or network, position, nominal range), `/truth` (evaluation only), `/stats` |
| `FLEETMESH_TRAFFIC_SBS_PORT` | 8768 | TCP, SBS-1 BaseStation CSV lines as dump1090 serves on 30003. The receiver is named in the session-id field (numbered as in `/sensors`); "date/time generated" is the transmission time and "date/time logged" the receipt time. |
| `FLEETMESH_TRAFFIC_AIS_PORT` | 8769 | TCP, NMEA 0183 `!AIVDM` sentences, each with an NMEA 4.10 tag block `\s:<receiver id>,c:<unix seconds>*hh\` |
| `FLEETMESH_TRAFFIC_JSON_PORT` | 8771 | TCP, JSON lines for what has no common wire format: Remote ID and sensor detections (below) |

All ports sit below the ephemeral range (CLAUDE.md). Own-vehicle positions, which the onboard
sensors need, come from the bridge's `GET /api/state`, read-only (a simulation shortcut: on a real
vehicle the sensor rides on the aircraft).

Remote ID (the ASTM F3411 fields):

```json
{"type":"rid","rx":"hub-pier","t_rx":1781456400123,"t":1781456400050,"uas_id":"1581F5FKD229400B5E1J",
 "id_type":"serial","ua_type":"multirotor","status":"airborne","lat":37.8075,"lon":-122.4312,
 "alt_geo_m":96.0,"alt_baro_m":94.5,"height_agl_m":90.0,"speed_mps":6.2,"track_deg":271.0,
 "vspeed_mps":0.0,"h_acc_m":3.0,"v_acc_m":4.0,"op_lat":37.8069,"op_lon":-122.4288}
```

A detection by the company's own sensor:

```json
{"type":"det","sensor":"daa","platform":"12","t":1781456400200,"sensor_lat":37.80,"sensor_lon":-122.45,
 "sensor_alt_m":90.0,"bearing_deg":41.5,"sigma_bearing_deg":2.0,"elev_deg":3.1,"sigma_elev_deg":2.0,
 "range_m":null,"sigma_range_m":null,"radial_speed_mps":null,"size":"medium"}
```

`sensor` is `daa` (onboard detect-and-avoid; `platform` is the own vehicle's uid), `marine_radar`
(on an own boat) or `ground_radar` (`platform` is the site id). Absent measurements are null.

The track the tracker produces (and the bridge serves at `/api/traffic`):

```json
{"id":"trk-000123","t":1781456400250,"domain":"air","class":"airliner","cooperative":true,
 "ids":{"icao24":"a1b2c3","callsign":"UAL123","anonymous":false},
 "lat":37.70,"lon":-122.35,"alt_m":900.0,"alt_ref":"baro","h_sigma_m":15.0,"v_sigma_m":10.0,
 "vel_mps":[-60.0,-40.0],"vs_mps":-4.0,"heading_deg":236.0,"state":"confirmed",
 "sources":[{"kind":"adsb_1090","rx":"hub-pier","last_t":1781456400100}],
 "flags":[],"static":{}}
```

`domain` is air or surface; `class` is one of airliner, bizjet, ga, helicopter, seaplane, drone,
unknown_air, cargo, tanker, passenger, ferry, tug, pilot, fishing, sailing, pleasure, small_craft,
unknown_surface (a class the source does not give is the domain's unknown). `static` carries what
AIS type 5 or 24 gave (name, ship type, length, draught, destination, navigation status).
Coordinates are WGS84 degrees, altitudes metres, times Unix milliseconds.

As built in fleetmesh-tracker (2026-11-25; its README has the detail):

- `ids` carries `icao24`, `mmsi` or `uas_id` as the source gives, and `callsign` when known
  (withheld for an aircraft on the LADD list); absent ones are left out. `alt_m`, `v_sigma_m` and
  `vs_mps` are null for surface traffic and when the altitude is unknown, with `alt_ref` `none`;
  otherwise `alt_ref` is `baro` (pressure altitude) or `geo` (geometric, above mean sea level, as
  the fleet flies; Remote ID's ellipsoid heights are converted with the local geoid height).
- `flags` are `stale`, `anonymous`, `ladd`, `id_changed`, `sensor_disagree`, `position_jump`,
  `invalid_mmsi`, `on_ground`, `moored`, `at_anchor`, `emergency`, `bearing_only` and
  `triangulated`; `sources[].kind` is `adsb_1090`, `adsb_uat`, `ais_a`, `ais_b`, `remote_id`,
  `daa`, `marine_radar` or `ground_radar`. `static` also carries an aircraft's last squawk and
  Remote ID's id type, aircraft type and operator position.
- Added: a track flagged `bearing_only` (a DAA bearing no geometry has fixed yet) carries a
  `bearing_only` object (sensor position, bearing and sigma, elevation, range interval), and its
  `vel_mps` and `heading_deg` are null. Such a track is a line, not a point: its `lat`/`lon` are
  the middle of the range interval and `h_sigma_m` half its length, which keeps a consumer that
  ignores the flag cautious, but only the line itself lets the console draw it and a conflict
  check test against it.
- An ADS-B class is not in SBS (dump1090 drops the emitter category), so it comes from an
  aircraft registry the operator supplies or an airline-shaped callsign, else `unknown_air`. AIS
  high-speed craft (ship types 40-49) are read as `ferry`.
- A tag block's `c:` above 1e11 is read as milliseconds (some receivers write them).

As built in fleetmesh-traffic (2026-11-25; its README has the detail):

- `/sensors` is a list of `{id, kind, num, owner, platform, lat, lon, alt_m, nominal_range_m,
  feeders, enabled, up}`. A hub's receivers share the hub's id (`hub-pier` is the `rx` of its
  ADS-B, UAT, AIS and Remote ID receivers alike), so an entry is unique by `id` and `kind`; `num`
  is unique and is the SBS session id. The commercial feeds are `net-adsb` (`adsb_1090`),
  `net-uat` (`adsb_uat`) and `net-ais` (`ais`), `owner` `network`, `feeders` their site count and
  no position. Onboard sensors (`daa`, `marine_radar`, `rid`) have the own vehicle's uid as `id`
  and `platform`, and its last known position; the ground radar's `enabled` is false by default.
- SBS: dates and times are UTC; the callsign is as broadcast, eight characters padded with
  spaces; a UAT anonymous (self-assigned) address is written `~` and six hex digits; a network
  feed delivers 2 to 5 s late ("logged" is its delivery time) and withholds the callsign (no
  `MSG,1`) of an aircraft on the LADD list.
- AIS: every sentence, both fragments of a message 5 included, carries the tag block
  `\s:<receiver id>,c:<unix seconds>*hh\` and nothing else (no `g:` group); fragments are 60
  characters; the sequence id runs 0 to 9 per receiver. `c` is the receiving station's second
  (for `net-ais`, its feeder's), and the network delivers 3 to 10 s later.
- Remote ID `alt_geo_m` is height above the WGS84 ellipsoid (geoid height -32 m on the Bay,
  `[world] geoid_n_m`); `alt_baro_m` is pressure altitude; `height_agl_m` is above the take-off
  point. Detection `size` is `small`, `medium` or `large`; DAA and radar see the company's own
  other vehicles too (the tracker drops them).
- Privacy ICAO Addresses come from one block, `[addresses] pia_block` in sim/traffic.toml
  (ADF800 to ADFBFF, illustrative), a fresh address each flight, with a third-party callsign;
  that block is what the bridge gives the tracker as `anonymous_icao_ranges`.

## 6. How the fleet uses the picture (phase 2)

- **Separation.** The separation rule reads the picture as well as the fleet. Manned aircraft
  have the right of way (14 CFR 107.37): a drone keeps well clear (draft: 2,000 ft horizontally and
  250 ft vertically, the ASTM F3442 small-UAS well-clear volume) and gives way. A delivery boat
  stays out of the way of vessels that can only navigate in the channel (COLREGs rule 9) and keeps
  a closest point of approach of at least a draft 0.25 nm from ships.
- **Automation first.** A predicted conflict is resolved by automation: hold on the ground, delay a
  departure, hover or descend, take a route clear of the approach, slow or stand off for a ship.
  It is logged and shown, not asked about.
- **A person only for issues.** The inbox gets an item only when automation cannot clear it: a
  conflict that persists after resolution, a non-cooperative track close to a drone, a
  broadcast that disagrees with the company's own sensors, or the picture itself degraded (the
  ADS-B or AIS feed silent, a receiver down), with the automation's proposed response.
- The rule is a new behaviour, gated by its own `*_SINCE` constant (rules-versioning.md). The
  picture each decision read is logged with it (as weather and links are), so replay decides on
  the same picture without the simulator.

## 7. What a run measures

Truth against the picture: tracks missed, late and false, position error, identity errors; and
conflicts the fleet met against those it predicted. These go into the evaluation's metrics next to
the operator-load numbers.

## 8. Build order

1. fleetmesh-traffic: world, equipage, sensors, encoders, live binary, truth (no bridge change).
2. fleetmesh-tracker: decoders, fusion, flags, prediction (no bridge change).
3. Bridge and engine: ingest the feeds, `/api/traffic`, log the picture, the separation rule and the
   automatic responses, issue alerts; `scripts/up.sh` starts the simulator; headless links both.
4. Console: a traffic layer (icons by class, altitude labels, source and age, a coasting fade,
   receiver coverage) and the issue alerts.
