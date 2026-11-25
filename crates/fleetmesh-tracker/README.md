# fleetmesh-tracker

The company-side traffic picture (docs/design/traffic.md, sections 4 and 5): what a private
delivery company can know about the aircraft and vessels it does not control, built only from
what it can receive. ADS-B as SBS-1 lines, AIS as NMEA AIVDM, and Remote ID and its own sensors'
detections as the JSON lines of section 5 go in; tracks with identities, states, quality flags
and predictions come out.

It does not depend on `fleetmesh-traffic` and uses nothing beyond those formats, so it runs
unchanged on a real dump1090 or readsb SBS stream (port 30003) or a real AIS receiver's NMEA.
The bridge and the headless simulator link it; that integration is not done here.

## Using it

```rust
use fleetmesh_tracker::{Config, Tracker};

let mut tr = Tracker::new(Config::default());
tr.set_sensors(sites);                       // the simulator's /sensors entries deserialise directly
tr.set_own_fleet(&own_ids);                  // own ICAO, MMSI and UAS ids: not traffic
tr.set_qnh(Some(1016.2));                    // from weather, when known

tr.ingest_sbs(line, receipt_ms);             // per SBS line
tr.ingest_nmea(line, receipt_ms);            // per NMEA line (fragments are held)
tr.ingest_json(line);                        // per rid or det line (times are in the line)
tr.set_own_vehicles(&positions);             // own vehicles now, so sensors' detections of them drop

tr.update(t_ms);                             // bring the picture to t_ms
for t in tr.tracks() { /* serde_json::to_string(t) is the section 5 JSON */ }
let p = tr.predict(&tr.tracks()[0], 120.0);  // position and sigma two minutes on
let alerts = tr.disagreements();             // broadcasts the company's own sensors contradict
let counters = tr.stats();
```

| Call | Notes |
| --- | --- |
| `Tracker::new(Config)` | `Config::default()` is tuned for the Bay; every threshold below is a field. |
| `ingest_sbs(line, receipt_ms)` | The receiver is the session-id field, mapped to a site id through `SensorSite::sbs_session` (the `/sensors` `num`). |
| `ingest_sbs_from(line, receipt_ms, rx)` | For a real dump1090, whose session id is constant. |
| `ingest_nmea(line, receipt_ms)` | The receiver is the tag block's `s:`, else `"ais"`. |
| `ingest_nmea_from(line, receipt_ms, rx)` | For a stream without tag blocks. |
| `ingest_json(line)`, `ingest_rid(&Rid)`, `ingest_det(&Det)` | Remote ID is fused at once; detections are held until their scan is complete (the next detection from that sensor at another time, or `update`). |
| `set_sensors(Vec<SensorSite>)` | Receivers and sites: SBS session numbers, radar positions, ranges and scan intervals. Own-vehicle sensor positions come in each `det`. |
| `set_own_fleet(&OwnFleet)`, `set_own_vehicles(&[OwnVehicle])` | The fleet's own broadcasts are not traffic; a detection that matches an own vehicle's position is dropped. |
| `set_qnh(Option<f64>)` | Pressure and geometric altitudes are compared through it. |
| `update(t_ms)` | Processes held detections, ages, confirms and drops tracks, checks integrity, rebuilds `tracks()` at `t_ms`. |
| `tracks() -> &[Track]` | In track-number order, every position predicted to the last `update` time (`picture_time()`). |
| `predict(&Track, dt_s)`, `predict_path(&Track, horizon_s, step_s)` | Constant-velocity projection with growing 1-sigma ellipse. |
| `disagreements() -> &[Disagreement]` | Each cooperative track whose broadcast the company's own sensors contradict, with the track the sensors see instead. |
| `stats() -> &Stats` | Counters (lines, errors, duplicates, late reports, associations, tracks made and dropped) and the picture's make-up. |

Each ingest call returns an `Outcome`: `Fused`, `Duplicate`, `Pending`, `Ignored(why)` or
`Error(why)`. Malformed, partial and unknown lines never panic.

**Deterministic.** There is no clock inside: every time is a receipt time the caller passes or
a time in the message, and nothing that reaches the output depends on hash-map order. The same
calls in the same order give the same tracks, bit for bit on one build, so the bridge can log
the inputs or the picture and replay can rebuild it. (Across platforms, `sin`, `atan2` and
friends may differ in the last bit; replay on the logged build.)

## Inputs

**SBS-1.** All `MSG` transmission types: 1 callsign, 2 surface position (sets `on_ground`), 3
airborne position and pressure altitude, 4 ground speed, track and vertical rate, 5 and 7
altitude, 6 altitude and squawk (7500, 7600, 7700 raise `emergency`), 8 heard only. `ID` lines
update a known aircraft's callsign; `SEL`, `AIR`, `STA` and `CLK` are ignored. A `~` before the
address marks a non-ICAO address (readsb, and the simulator) and the identity is anonymous. The
"generated" time is the report's time when it falls within the feed-delay window of the receipt
time, otherwise the receipt time is used; dates are read as UTC plus `sbs_utc_offset_min` (a
dump1090-fa writes local time unless run with `TZ=UTC`).

**AIVDM/AIVDO.** Checksums (sentence and tag block), multi-sentence assembly keyed by
receiver, channel and sequence id (or the tag block's group), the 6-bit armour, types 1, 2, 3
(Class A position), 5 (static and voyage), 18 (Class B position), 19 (Class B extended) and 24
(parts A and B, merged per MMSI into the track). NMEA 4.10 tag blocks: `s:` names the receiver,
`c:` is the time (Unix seconds, or milliseconds if above 1e11); the report's own UTC second moves
the time back when a feed delivers it late. VDO (the receiver's own vessel) and other types are
ignored. Bad checksums, lost or out-of-order fragments (a partial message expires after
`nmea_fragment_ms`), short payloads and bad characters are counted and dropped.

**JSON lines.** `rid` and `det` exactly as section 5; unknown fields are ignored, absent
measurements may be null or missing, and a numeric `platform` is accepted.

## The model

**Local metres.** Positions are projected onto the tangent plane at the region's centre (an
orthographic projection of the WGS84 ellipsoid, altitude kept apart), accurate to about 1 m at
40 km. Bearings and velocities cross the boundary through the grid convergence (a few tenths of
a degree at the region's edge). Sensor geometry includes the Earth's curvature over the path
(4/3 Earth: 15 m at 16 km, 150 m at 50 km).

**Filter.** One constant-velocity Kalman filter per track, state x, y, z and their rates, with
continuous white-noise acceleration (spectral density 4 m²/s³ horizontally and 1 vertically for
cooperative aircraft, 9 for non-cooperative ones, 0.05 for vessels and 0.2 for non-cooperative
boats). Surface tracks hold z at sea level. Polar sensor measurements are linearised at the
prediction. A report up to `oosm_max_ms` (2 s) older than its track is fused by retrodiction
(the prediction is carried back, the interval's process noise added to the innovation); older
ones are counted as late and not fused.

**Altitude references.** A track keeps altitude in the reference of its first altitude source:
`baro` (pressure altitude, the ADS-B and Mode S altitude) or `geo` (geometric, above mean sea
level, as the fleet flies). Remote ID's `alt_geo_m` is an ellipsoid height (ASTM F3411) and is
converted with `geoid_n_m` (-32 m on the Bay). Comparing the two references uses the QNH from
`set_qnh` (8.32 m per hPa, 15 m sigma) or, unknown, no offset with a 120 m sigma; that sigma
enters every gate that compares them.

**Identity and duplicates.** Cooperative tracks are keyed by ICAO address (with its non-ICAO
mark), MMSI or UAS id. The same transmission heard by several receivers (same content within
`dedup_window_ms` from a different receiver) is fused once and every receiver is listed in
`sources`; the same content from the same receiver is a new report, since one receiver never
hears one transmission twice. An ADS-B track is confirmed after 2 position reports, Remote ID
after 2, AIS after 1 (CRC-protected, and a moored ship reports every 3 minutes).

**Position jumps.** A cooperative position more than `jump_chi2` (40) from the prediction and
more than 300 m away is not fused and raises `position_jump` for a minute. Two consecutive
rejected reports that agree with each other (at a plausible speed) move the track there: a
lasting shift is followed, a single bad report is not.

**Sensor detections.** A scan's detections are gated against every track of the sensor's
domain (DAA and ground radar: air; marine radar: surface) at chi-square probability 99.9%,
using whichever of bearing, elevation, range and range rate the detection has. Candidates are
ranked by d² + ln|S| (so a large, vague track does not win by being vague), a tentative track
pays a penalty of 3 so a confirmed track that also gates keeps the detection, and detections and
tracks are paired one to one, best first. A cooperative track whose broadcast is fresh is
**corroborated, not moved**: the sensor is listed among its sources and the corroboration is
remembered for the integrity check. Once the broadcast is coasting, detections update it. A
non-cooperative track that turns out to sit on a cooperative one (position and velocity inside a
99% gate) is folded into it.

**Non-cooperative tracks.** A detection with a range that matches nothing starts a tentative
track. It is confirmed after M = 3 hits in N = 5 looks, a look being one scan interval of the
sensor that started it while that sensor is reporting; more than N - M misses drop it. Marine
radar returns marked `small` are `small_craft`; otherwise the class is the domain's unknown.

**Bearing-only (DAA without range).** One bearing cannot place a target, so the tracker does
not pretend it can:

- An unmatched bearing-only detection starts a *line-of-bearing track*: the history of its
  lines (sensor position, bearing, elevation, time). It is output flagged `bearing_only`, with a
  `bearing_only` object giving the line itself (sensor position, bearing, sigma, elevation and
  the range interval from 20 m to the DAA's detection range for the target's size: 600, 1,500
  or 2,500 m, 2,000 m when the size is not given). `lat`/`lon` are the middle of that interval, `h_sigma_m` half its length, and the
  velocity is null. A consumer that ignores the flag still sees a sigma of hundreds of metres.
- A new line is associated with the track by its predicted bearing (and elevation) from the same
  sensor, or, from a different sensor, by crossing the track's latest line (within 2 s) inside
  both detection ranges (with a 30% margin) at a crossing angle of at least 8° with consistent
  elevations.
- Whenever a line is added the lines of the last 30 s are fitted with a constant-velocity
  target (x, y, vx, vy) by weighted least squares on the bearing residuals: a pseudo-linear first
  guess, Gauss-Newton, and a weak zero-mean prior on velocity. The fit is accepted only when the
  geometry really fixes the target: the sensors' positions must depart from a single straight,
  constant-speed path by at least tan 8° of the range (two sensors, or one that turned; a
  constant-velocity target is unobservable from a sensor flying straight), the lines must cross
  at 8° or more at the target, the residuals must pass a chi-square test, every line must point
  at the solution within its range, the speed must be plausible, the fitted range must beat
  ranges 40% shorter and 60% longer by a chi-square margin of 9 (a profile test, because the
  linearised covariance of a short, biased range looks deceptively tight), and the 1-sigma
  ellipse (reported doubled in variance as an allowance for the nonlinearity) must be under
  max(120 m, 15% of range).
- An accepted fit makes the track an ordinary position track flagged `triangulated`. Further
  bearing-only lines refit the batch rather than feeding a filter with bearings alone; a
  detection with a range (a radar) turns it into a plain filtered track.

In the tests, two drones 1.2 km apart fix an aircraft 1 km away within about 100 m in a few
seconds; one drone flying straight never claims a fix, and fixes a hovering drone 700 m away only
once its turn has built enough parallax (about 15 s after the turn), typically within 1.5 sigma
(the test allows 3.5).

**Timeouts.** Every source has a normal interval; AIS's comes from each report (ITU-R M.1371:
Class A 10 s under way (3.3 s when turning), 6 s above 14 kn (2 s when turning), 2 s above 23 kn,
3 minutes at anchor or moored below 3 kn; Class B 30 s moving, 3 minutes below 2 kn). A track is `stale` after its stale factor
times that interval, coasts (extrapolates, state `coasting`) after the coast factor, and is
dropped after the drop factor, each with a floor:

| Source | Interval | Stale | Coast | Drop |
| --- | --- | --- | --- | --- |
| ADS-B | 1 s | 3 s | 5 s | 30 s |
| Remote ID | 1 s | 3 s | 4 s | 20 s |
| AIS Class A under way | 10 s | 25 s | 35 s | 6 min |
| AIS Class A at anchor or moored | 3 min | 7.5 min | 10.5 min | 18 min |
| AIS Class B moving | 30 s | 75 s | 105 s | 6 min |
| Ground radar (non-cooperative) | scan, 1 s | 2.5 s | 4 s | 15 s |
| Marine radar (non-cooperative) | scan, 2.5 s | 6.25 s | 7.5 s | 20 s |
| DAA (non-cooperative) | 1 s | 2.5 s | 3 s | 8 s |
| Bearing-only line | | | 3 s | 8 s |

A vessel that slows and anchors switches to the 3-minute interval with its first "at anchor"
report, so its track neither coasts nor drops between reports.

**Flags.** `stale`; `anonymous` (a `~` address, an address in `anonymous_icao_ranges` such as a
Privacy ICAO Address pool the operator knows, or a Remote ID session id); `ladd` (an address on
the configured LADD list: tracked, callsign withheld); `id_changed` (an anonymous identity that
rotated, for a minute); `sensor_disagree`; `position_jump`; `invalid_mmsi` (not a ship-station,
SAR-aircraft, handheld, auxiliary-craft or SART/MOB/EPIRB pattern with a MID in an ITU region,
or a placeholder such as 123456789); `on_ground`; `moored`; `at_anchor`; `emergency`;
`bearing_only`; `triangulated`.

**Integrity (`sensor_disagree`).** A cooperative track is flagged when all of these hold for
4 s (and the flag holds for 15 s after): a confirmed non-cooperative track lies within 3 km of
its broadcast position but outside a 99.9% position gate; both move at 3 m/s or more; the
sensed track's velocity is known to 8 m/s (1 sigma) and matches the broadcast velocity within
max(3 m/s, 10%) and a 99% gate; their altitudes agree when both are known; the broadcast is not
from the ground or an anchored or moored vessel; and a sensor that reported the sensed track in
its last three scans has the broadcast position inside its range but has not corroborated the
broadcast within two scans of its last sighting of the sensed track. That is the signature of a faulty or spoofed position: the
company's sensors see an object moving exactly like the aircraft, elsewhere, and nothing where
it says it is. Both tracks are flagged, and `disagreements()` pairs them with the offset and the
sensor. Disagreements smaller than the sensor's own gate are not detectable.

**Anonymous rotation.** A new anonymous identity whose first report comes after an old
anonymous identity's last (within 30 s), at the old track's predicted position (2-D gate 13.8)
and velocity, continues the old track: same track id, new address, `id_changed`. Two anonymous
aircraft heard at the same time are never merged.

**Classes.** AIS ship types map to classes (30 fishing, 31, 32 and 52 tug, 36 sailing, 37
pleasure, 40-49 high-speed craft read as `ferry` since the Bay's high-speed craft are its ferries,
50 pilot, 60-69 passenger, 70-79 cargo, 80-89 tanker). SBS carries no ADS-B emitter category,
so an aircraft's class comes from `aircraft_classes` (a registry the operator holds) or, when
`classify_airline_callsigns` is on, an airline-shaped callsign (three letters and a flight
number) reads as `airliner`; otherwise `unknown_air`. Remote ID is `drone`.

**Prediction.** `predict` projects the filter state and covariance with the track's own
process noise, so the ellipse grows with the cube of time (an ADS-B aircraft's sigma is about
1.5 km after two minutes, a ship's about 170 m). A bearing-only track stays on its line with its
sigma growing at 80 m/s (air) or 15 m/s (surface).

## Output

`serde_json::to_string(&track)` is section 5's track, field for field and in order. What the
example does not show:

- `ids` has `icao24`, `mmsi` or `uas_id`, and `callsign` when known; absent ones are omitted.
  Non-cooperative tracks have only `anonymous`.
- `alt_m`, `v_sigma_m` and `vs_mps` are null for surface tracks and when the altitude is
  unknown (a 2-D radar track, a bearing line without elevation); `alt_ref` is then `none`.
- `vel_mps` (east, north) and `heading_deg` are null for bearing-only tracks. `heading_deg` is the
  AIS true heading while fresh, else the course over ground above 0.5 m/s.
- `h_sigma_m` is the semi-major axis of the 1-sigma position ellipse.
- `sources[].kind` is one of `adsb_1090`, `adsb_uat` (a receiver listed as UAT), `ais_a`,
  `ais_b`, `remote_id`, `daa`, `marine_radar`, `ground_radar`.
- `static` carries name, callsign, IMO, ship type and text, length, beam, draught, destination,
  ETA (`MM-DDTHH:MMZ`), navigation status and text, mother ship (types 5, 19, 24); the last squawk
  for aircraft; `id_type`, `ua_type` and the operator position for Remote ID. Empty is `{}`.
- `bearing_only` (an addition to section 5) is present only on bearing-only tracks.

## Limits

- No sensor registration: a radar with a range or bearing bias makes every cooperative track it
  sees look as if it disagrees. Bias estimation against cooperative traffic would fix that.
- No terrain or coverage model beyond each sensor's nominal range; the disagreement check can
  be fooled by a sensor masked by terrain (it then sees neither object, which raises nothing).
- Single-hypothesis association (greedy, one to one per scan). Closely spaced non-cooperative
  targets crossing at the same moment can exchange tracks; multiple-hypothesis tracking is out of
  scope. Bearing lines from several sensors with several targets can produce ghost crossings;
  elevation consistency and the fit's residual test reduce, but do not remove, them.
- Constant-velocity motion only: turns are absorbed by process noise, so a manoeuvring track's
  prediction over minutes is wide.
- Bearing-only fixes from one moving sensor need it to turn; a target that itself manoeuvres
  during the 30 s window breaks the constant-velocity fit (the fit is then rejected).
- DAA bearing lines are checked against cooperative tracks (corroboration) but never raise
  `sensor_disagree` themselves: a line cannot contradict a position.
- AIS types other than 1, 2, 3, 5, 18, 19 and 24 (base stations, aids to navigation, SAR aircraft
  type 9) are ignored. MLAT positions are not distinguished from ADS-B. SBS carries no NACp, so
  ADS-B accuracy is a configured 20 m.
- Remote ID's geometric altitude is assumed to be an ellipsoid height (F3411); set
  `geoid_n_m` to 0 if a source writes mean sea level.

## Tests

`cargo test -p fleetmesh-tracker` (40 tests: 9 unit, 12 decoder, 17 scenario, 1 performance, 1
doc). The scenarios: two aircraft crossing (cooperative with radar corroboration, and radar-only
keeping their tracks), three receivers hearing one aircraft, a Class A vessel slowing to anchor
(and another coasting and dropping), coasting and dropping, a kayak tracked from a moving boat's
radar among clutter, bearing-only from one drone, triangulation from two drones and from one
drone that turns, a detection with two candidates, an ADS-B position 1.2 km off what radar sees
(flagged, with the honest aircraft beside it not flagged, and the run repeated for identical
pictures), a position jump and a lasting shift, a rotating anonymous address, own fleet, LADD and
emergency, Class B static parts and a Remote ID session id, a delayed feed, and the `/sensors`
listing. They were also run over 81 noise seeds without a failure.

Decoder vectors come from published decodes: gpsd's AIVDM samples with gpsd's own decoded values
(`tests/data/gpsd-sample.aivdm`, BSD-2-Clause, notice inside) and the tag block example of gpsd's
AIVDM document, and the SBS-1 examples of node-sbs1 (`tests/data/sbs-examples.txt`, X11 licence,
attribution inside).

**Performance** (`cargo test -p fleetmesh-tracker --release --test perf -- --nocapture`): 300
aircraft, 120 vessels, 50 drones and 31 non-cooperative targets, with ADS-B from two or three
receivers, AIS from two, Remote ID from two, three ground radars and two marine radars: 1,937
reports a second and about 500 tracks. In a release build 300 s of that traffic (581,100
reports) takes about 2.0 s, 0.007 of real time (3.4 µs a report, the slowest `update` under
8 ms). A debug build runs 20 s of it in about 2.2 s.
