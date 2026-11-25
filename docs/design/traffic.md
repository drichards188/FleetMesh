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
- (Phase 3.) A network feed is the kind of what it delivers (`adsb_1090`, `adsb_uat`, `ais`) with
  `owner` `network`, as `/sensors` lists it; the tracker's `SiteKind` lost its `adsb_feed` and
  `ais_feed`, which nothing emitted (`SensorSite::is_network` reads the owner).

## 6. How the fleet uses the picture (built in phase 3: section 10)

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
   **Built 2026-11-25 (rules 3): section 10.**
4. Console: a traffic layer (icons by class, altitude labels, source and age, a coasting fade,
   receiver coverage) and the issue alerts.

## 9. The console (as built, 2026-11-25)

`web/src`: `traffic.ts` (types, declutter, fade, the bearing-only wedge, prediction, truth
matching, feed health; checked by `traffic.check.ts`), `trafficText.ts` (words), `trafficSymbols.ts`
(the symbols, drawn by milsymbol through `milsymbolCore.js`), `trafficLayers.ts` (deck.gl layers and label candidates), `useTraffic.ts` (the store),
`TrafficBar.tsx` (toggles, counts, picture health, coverage, truth), `TrafficDetails.tsx` (a
selected track), the four alert kinds in `decisions.ts`, `bluf.ts` and `InboxPanel.tsx`.

What it expects of the bridge:

- `GET /api/traffic` is `{t, tracks, picture}`; a 404 means the run has no traffic picture (the bar
  says so). The WebSocket push `{"type": "traffic", t, tracks, picture}` is the live path; the GET
  is asked every 2 s only while no push has come for 3 s. Ages are measured against the picture's
  `t`, never the browser's clock.
- `picture.feeds` may have any keys (shown as given); each is `{connected, last_t, lines_per_s}`,
  `last_t` Unix ms or null. A feed is shown amber after 15 s without a line, red after 60 s or
  when not connected.
- Optional: `disagreements` beside `tracks`, the tracker's `disagreements()` (`track`, `sensed`,
  `offset_m`, `sensor`). Without it the console pairs each flagged cooperative track with the
  nearest flagged non-cooperative one within 3 km.
- `GET /api/traffic/sensors` is a list of `/sensors` entries (or `{sensors: [...]}`). Rings are
  drawn only for company receivers with a position and a nominal range up to 30 km.
- `GET /api/traffic/truth` is asked only in a dev run (`/api/auth/session` says `auth: dev`) with
  the overlay on: a list of the simulator's truth objects (or `{truth: [...]}`); 404 or 403 means
  none. A credentials run never shows the control and never asks.
- A traffic alert carries `track_id` (and `track_ids` for a broadcast and what the sensors see
  instead), `uid` the vehicle it is about (absent, null or a uid not in the fleet for
  `traffic_picture_degraded`), and `issue` as the other issues do; the console has plain fallback
  words for an alert without one.
- An onboard sensor's `sources[].rx` is the vehicle's uid, named by its callsign.

Declutter: a track is near (full-size icon, speed leader, a label candidate) within 3 km and 600 m
vertically of one of our aircraft or their routes (aircraft) or within 1.5 km of our boats or their
routes (vessels), mid (smaller, fainter, no label) within 8 km and below 1,500 m or within 5 km,
far (small and faint) otherwise; an airliner above 3,000 m is always far. Flagged tracks
(`sensor_disagree`, `position_jump`, `emergency`), the selected one and those an alert names are
near wherever they are. At most 40 are drawn in full, the nearest. Coasting tracks draw at half
strength, stale ones at two thirds (both with a dashed frame), tentative ones hollow and dotted.
Traffic labels never displace our vehicles' (lowest priority in the one label budget). Aircraft
labels give metres, as the fleet's do, with the reference (`baro` for pressure altitude, `MSL` for
geometric); the details give feet too. A bearing-only track is a sector on the surface (its range interval, two bearing
sigmas either side) with its line of sight in 3D when selected or hovered, never a point.

Symbols (2026-11-25, DECISIONS.md). Each track is a standard map symbol (MIL-STD-2525D / APP-6D
number codes, drawn by milsymbol, MIT, as the earlier project drew its contacts): the frame says how
far it is identified, a glyph inside what kind it is, from civilian functions only and not
over-specific (`TRAFFIC_FUNCTION`: every airplane civilian fixed wing, a helicopter rotary wing, a
drone unmanned; cargo, tanker, passenger, ferry, tug, fishing, sailing and motor leisure craft their
own; a pilot boat, which the standard lacks, the general merchant ship; unknown aircraft, small
craft and unknown vessels the frame alone). Only three identities are written: neutral (green
square) for traffic that broadcasts and says who it is, unknown (yellow; a cloud in the air, four
lobes on the water) for what only our sensors see or what broadcasts no identity, pending (hollow,
dotted) while tentative; never friend or hostile. Coasting or stale: status anticipated, a dashed
frame. Symbols stand upright (the speed leader, as before, shows the course), at milsymbol sizes
20, 16 and 10 by tier, far ones the frame alone (a glyph would be a few pixels). An unidentified
track out of focus (not near, selected, hovered or flagged) draws at 60 % of its strength. A
bearing-only track keeps its wedge, with a small plain unknown-air frame on its line: something in
the air along it, not a position. Labels and rings fit each frame's own extent. The bar's Key says
what the frames, the dashed frame, the line ahead, the glyphs, the red ring and the wedge mean, in
plain words, and that the round icons are ours: no traffic frame is round or in our colours. Only
milsymbol's air and sea surface sets are bundled (about 110 kB minified of its 840 kB).

## 10. The bridge and the engine (phase 3, as built 2026-11-25)

**Launch.** Each scenario.toml names `traffic = "traffic.toml"`; scripts/scenario_env.py exports
`FLEETMESH_TRAFFIC_SCENARIO`, and scripts/up.sh starts `fleetmesh-traffic` before the bridge on the
scenario clock (`FLEETMESH_AT`, `FLEETMESH_RATE`), logs in logs/traffic.log, and sets
`FLEETMESH_TRAFFIC_URL` for the bridge; `make down` stops it with the rest. `FLEETMESH_TRAFFIC=0`
starts no simulator and leaves the bridge with no picture. A scenario's traffic.toml may set the
world's `qnh_hpa` for its hour (all three: 1014.5 hPa, the weather archive's sea-level pressure at
2026-06-14 17:00Z), switch on ground radars (`ground_radar = ["radar-pier"]`: harbour-city and
wind-event, so a wrong broadcast near the waterfront is seen for what it is) and script a
`flight` (an aircraft of a stream's kind along its own route: harbour-city's medical helicopter
crossing the drones' routes at 400 ft at 600 s).

**Ingest** (crates/fleetmesh-bridge/src/decision/traffic.rs). The bridge connects to the SBS, AIS
and JSON ports as a client, as it would to a receiver's feed, and reconnects every 2 s when one
drops. It reads `/sensors` at start (again each ten minutes) and gives the tracker the fixed sites
only: an onboard sensor (`daa`, `marine_radar`, or any entry on one of our vehicles) carries its
vehicle's last position, which a detection gives anyway, so it is listed apart and never a site.
The simulator's own `up` and `enabled` flags are dropped: whether a receiver works is what the
bridge hears. Every line gets its receipt time on the scenario clock (`FLEETMESH_AT` plus elapsed
times `FLEETMESH_RATE`, set to the simulator's own clock from its `/health` each minute). Once a
second the tracker is told our vehicles' positions (`set_own_vehicles`) and broadcast identities
(`set_own_fleet`, from the manifest's `icao24`, `mmsi`, `uas_id` where it names them: the fleet
broadcasts nothing today), the QNH (`set_qnh`: the scenario's weather archive's pressure at the
hubs, which stand a few metres above the sea; none without an archive, and the picture says
standard pressure was used), and brought to now. The tracker's configuration is its default with
the Bay's geoid height (-32 m, held to sim/traffic.toml's `[world] geoid_n_m` by a test) and the
Privacy ICAO Address block of sim/traffic.toml as `anonymous_icao_ranges`; the first picture logs
it whole (a `traffic_config` delivery event, with the sensors), and every decision's picture names
its sha256. The headless simulator steps `fleetmesh-traffic` in process on the episode's clock and
hands each emission to the same ingest with no sockets (crates/fleetmesh-headless/src/sim/traffic.rs).

**Degraded.** The picture is `degraded` while a live feed is not connected, a feed has delivered
nothing for 30 s, the receivers' list has not been read, or an ADS-B or AIS receiver that should be
heard is not: a hub's 1090 receiver quiet for 20 s (the network's 30 s), a hub's AIS receiver for
90 s (the network's 60 s), or one not heard 45 s after the first line. Remote ID and UAT receivers
are not judged (a quiet one cannot be told from an empty sky). A change between ok and degraded,
or in what is not heard, is a `traffic_picture` delivery event.

**Start-up grace.** The bridge may start before the simulator's (or a receiver's) feeds are up: for
its first `STARTUP_GRACE_S` (60 s of the scenario's clock from the first picture) a picture not yet
whole is `starting` (`/api/traffic`'s `picture.starting`), not a fault. The rules read it as they
read any degraded picture (a departure that needs it waits, `SEP_PICTURE_DEGRADED`, which
automation waits out), and no person is told; a picture still degraded after the grace is an issue
as before. Re-reading `/sensors` (each ten minutes) keeps what each receiver was heard as: it used
to reset them, so every receiver read "not heard since the start" for a second at 1200, 1800 and
3000 s of a run, and the picture went degraded with them. The `traffic_picture_degraded` issue
clears itself as soon as the picture is whole again (an `issue` delivery event with `cleared`).

**Altitudes.** A track keeps the tracker's reference. For the rules, a pressure altitude (`baro`)
is brought to mean sea level with the QNH (8.32 m per hPa) and its vertical sigma carries the
conversion (the tracker's 15 m with a QNH, 120 m without); a geometric one (`geo`) is above mean
sea level already (Remote ID's ellipsoid heights converted with the geoid). Our vehicles' altitude
is their cruise height above home plus home's height above sea level (their own MSL and relative
altitudes). The rules compare MSL with MSL.

**API** (read-only; times are the scenario clock's Unix ms, ages are `t - last_t`):

- `GET /api/traffic[?bbox=w,s,e,n]`: `{"t", "tracks": [Track...], "disagreements": [{"track",
  "sensed", "offset_m", "sensor", "since_t"}], "picture": {"state": "ok"|"degraded", "reasons":
  [...], "not_heard": [...], "feeds": {"sbs"|"ais"|"json": {"connected", "last_t", "lines_per_s",
  "lines"}}, "receivers": [{"id", "kind", "network", "up", "last_t", "silent_s", "lines"}],
  "qnh": {"hpa", "source": "weather"|"standard"}, "source": "live"|"in_process", "enabled",
  "counts": {"tracks", "by_class", "by_source", "in_rules"}, "tracker": {"config_sha256", "stats"},
  "clock"}, "automation": [{"uid", "since_s", "codes", "why", "did", "under_way", "parcel"}]}`.
  Tracks are section 5's, every one the tracker holds (tentative ones too; the rules read only
  confirmed and coasting ones within 10 km of the fleet and its hubs). `automation` is what
  automation is doing for other traffic now: each vehicle it keeps on the ground or holds, why,
  since when. 404 when the run has no picture (no source, or nothing delivered yet).
- `GET /api/traffic/sensors`: `{"read", "sensors": [/sensors entries without `up` and `enabled`,
  with `heard` ("up"|"silent") and `last_t`], "onboard": [entries by vehicle, without position]}`.
- `GET /api/traffic/truth`: a dev run only (404 in a credentials run and with no simulator):
  `{"simulator_truth": true, "note", "t", "truth": [TruthObject...]}`, labelled as the simulator's
  truth a real operator does not have; no decision and no tracker reads it.
- WebSocket: `{"type": "traffic", ...}` with `/api/traffic`'s body, once a second when the picture
  is rebuilt, cut to the bounds of the client's last `view` message (grown by a quarter).

**The rule** (rules 3, `TRAFFIC_SEPARATION_SINCE`; delivery-rules.md section 8.1). A decision's
inputs carry `delivery.picture`, cut by the engine to what the order's checks read
(`fleetmesh_decide::traffic::trim`: tracks of its vehicles' domains within `relevant_km` of their
positions and the order's points, and the receivers of those domains; nothing for a hold, a swap,
a car's order) and logged by value, so replay decides on the same picture with no simulator and no
tracker. The separation rule checks each air or surface order's route ahead against each track's
prediction (the tracker's path, every 10 s for aircraft and 30 s for vessels) widened by its 1-sigma
times `sigma_factor`, vertically capped at `vrate_mps` of growth: well clear of a manned aircraft
(2,000 ft, 250 ft), 100 m and 30 m of another drone, 300 m and 60 m of an air track that broadcasts
nothing (a bearing-only one along its whole line, for `bearing_horizon_s`, moving at
`noncoop_speed_mps`), a ship under way's closest approach of 0.25 nm and its course ahead (not
crossed within `ship_cross_ahead_s` of its arrival), other vessels' `surface_surface_m`. A
non-cooperative track flagged `sensor_disagree` counts as manned (it is where the aircraft is). A
vehicle under way that is inside a minimum now and leaving it is clear; one on the ground is not.
Holds: `SEP_TRAFFIC_CONFLICT`, `SEP_TRAFFIC_NONCOOP`, `SEP_SHIP_CPA`, `SEP_SHIP_AHEAD`,
`SEP_PICTURE_DEGRADED` (no ADS-B receiver heard covers an air route, no AIS one a boat's; a network
feed covers the region) and `SEP_TRAFFIC_UNKNOWN` (a picture older than `max_age_s`). A run with no
picture says so in the separation result and checks nothing else; a car has no traffic model.

**Automation** (crates/fleetmesh-bridge/src/decision/delivery_rt.rs). The traffic holds are in
`WAIT_OUT` and never reach the inbox (an order a person must decide for another reason still
shows them):

- *On the ground.* The dispatcher's dry run holds a departure: the vehicle waits at its hub and the
  order is tried every `hold_retry_s` (15 s). After `traffic_replan_after_s` (30 s, `[automation]`)
  a delivery is offered once a route round the conflict (`fleetmesh_decide::traffic::detour`: a
  waypoint beside the first conflict, away from the track first, within the reroute's
  `auto_added_pct`); it goes when the rules pass it.
- *Under way* (every 2 s). A delivery or return whose route ahead the picture predicts a conflict
  on is held where it is (`hold`, a safe order that goes at once: a drone hovers, a boat stops and
  stands off) and its order is proposed again as soon as the rules pass it; when the traffic comes
  at it where it is, a delivery takes a route round it (the deliver again, with waypoints), else it
  holds. Each response is a `traffic` delivery event with the conflict.
- *The person* hears only of issues, each with what happened, what automation did and what is
  recommended (the alert's `issue`), its `track_id` and, for a broadcast fault, `track_ids` (the
  broadcast's, then the sensed one):
  - `traffic_conflict` (high): a vehicle's conflict has lasted `hold_retry_max_s` (300 s) through
    automation's responses; automation goes on holding it;
  - `traffic_noncooperative_near` (high): an air track that broadcasts nothing (or a bearing-only
    one) within `2 x noncoop_h_m` of one of our drones for 10 s, one issue per object (a track that
    reappears within 500 m of one is the same);
  - `traffic_broadcast_disagrees` (high): the tracker's `sensor_disagree` on an aircraft within
    `relevant_km` of the fleet or a hub (vessels' are not raised: see the tracker below);
  - `traffic_picture_degraded` (medium, `uid` 0): a feed silent or a receiver not heard, saying
    which departures automation holds for it (`SEP_PICTURE_DEGRADED`), or that other receivers still
    cover the routes; never during the picture's start-up grace, and cleared when the picture is
    whole again.

**The tracker, changed for phase 3** (crates/fleetmesh-tracker/README.md has the detail): a ground
radar whose site names no domain takes a return that may be at the surface for a vessel; the
detect-and-avoid's ranges by size are its reach (600 m, 3 km, 9 km); and the integrity check ties a
broadcast to a sensed track by motion only for a fast mover (25 m/s and more: an aircraft), by a
split for a slow one (the sensor saw the broadcast's object, and the track it sees instead began
there and then), compares a marine radar only within half its range for the vessel's class, needs an
offset of 500 m (1 km for a slow mover), and now also compares tracks bearing lines fixed. In a
normal harbour-city or coastal-boat run no ordinary vessel or aircraft is flagged; the scripted
broadcast faults are (crates/fleetmesh-headless/tests/traffic_picture.rs).

**Limits.** At a `FLEETMESH_RATE` other than 1 the picture's velocities are per scenario second
while the fleet's are per wall second. A broadcast fault on a slow mover that begins outside our
sensors' view is not detected (no split to see); one only bearing lines see, and no fix, is not
either. The vertical growth cap is a draft. The rule predicts the fleet's own vehicles as before
(their routes are not in the bridge's facts); other traffic's predicted paths are the tracker's
constant-velocity ones, so a turning aircraft is wide.
