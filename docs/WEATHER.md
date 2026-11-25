# Weather and conditions

Status, 2026-11-24: the weather crate (`crates/fleetmesh-weather`) and tidewater's weather layer are
in the tree and tested. The bridge endpoints, the agent tool and the console layer described under
"Looking at it" come with the bridge and console port (phase 2 in [PLAN.md](PLAN.md)) and are not
here yet. How the delivery rules use weather is designed in
[design/delivery-rules.md](design/delivery-rules.md), sections 5 to 7.

FleetMesh replays archived surface weather offline. The console is meant to animate it by default,
every effect at once, beneath labels, vehicles and routes: wind streaks, fog, rain and cloud
shadows, each with its own toggle in the legend, which also gives each field's values now, where
they come from, and what the simulation does with them. A color field (visibility, surface wind,
cloud cover or precipitation on a continuous ramp) is an extra, off by default. The map draws what
the simulation senses: the sampler gives a position its nearest model point's values (no
interpolation in space), so every effect takes each model point's region's own values with the
region's true edges, and a field the scenario scripts (a gust ramp in the wind-event scenario) is
the same everywhere and is called scripted only while the timeline sets it. Fog is a drifting haze
whose mean opacity grows as visibility falls; rain falls where precipitation is above zero, as
densely as its amount, slanted by the wind; cloud shadows cover each region's cloud cover; the
last two are display only, as the legend says. Streaks are carried toward where the wind blows. The
shapes of the haze and clouds are a texture, every drift and streak's pace is exaggerated and
follows its own region's wind, and all of it is kept faint and slow beside the symbols. Under
reduced motion the map holds a still frame with static wind arrows, and nothing animates while the
layer is off or the tab is hidden. Changes between polls blend in as the scenario clock moves; the
archive's bounds are a faint dashed outline, missing values in the color field are faintly
hatched, and stale layers are hidden. Link coverage and weather use separate raster views to
avoid mixing their colors.

**Weather** in the console samples the selected vehicle's position, or the map center. Planned
bridge routes: `GET /api/weather?lat=37.821&lon=-122.460` and the agent's `get_weather {lat, lon}`
return the same current sample: source, model, artifact checksum, age, missing values, synthetic
status and the inputs the rules read. They expose no future schedule. The map uses
`GET /api/weather/grid?bbox=west,south,east,north`, a bounded 32 by 24 raster sampled at one
instant, with `regions`: each archived model point and its values now, the archive's bounds, the
values outside them, and `scripted`, the names of the fields the scenario's timeline sets now
(names only; no value or later frame). These are simulation conditions, not a forecast or an
operational weather observation.

## Try the wind event

The wind-event scenario ([design/scenarios.md](design/scenarios.md), scenario 2) is the weather
demonstration: one hub at Crissy Field, forty delivery drones, and a scripted gust ramp from 7 m/s
through the drone limits over about forty minutes. After `make setup` and once the scenario
files are built, run it as the scenario's README says. It needs no AI account. Mean wind comes from the
archive; the gusts are a `[[weather.timeline]]` on top of it, and the rules and the keep-out
monitor read them. The vehicles' own physics does not feel a scripted gust (SITL wind comes from
tidewater's mean wind), so this scenario tests supervision, not flight dynamics.

Removing the timeline blocks gives a comparison with the same fleet and demand and archived weather
only. There is no scripted link outage in it: ordinary radio geometry and packet loss remain in
force. That isolates a weather problem from a communications problem. This is a demonstration of
the implemented model, not a validated finding about real vehicles or human performance. FleetMesh
remains a working draft, and nothing in it is connected to anything real.

## Sources, cost and replay

A run with real conditions names a tidewater scenario file, for example
`FLEETMESH_SCENARIO=tidewater/scenarios/golden-gate-weather.toml FLEETMESH_AT=2026-06-14T17:00:00Z make up`
(`scripts/up.sh` starts `tidewater serve` and `tidewater drive` for it). The golden-gate-weather
scenario carries HRRR wind; the plain golden-gate scenario carries the same terrain, tide and
currents. A scenario that does not name an archive keeps the plain wind and fixed defaults.

The weather scenario shares the existing Golden Gate terrain, tide and SFBOFS current cache. Its
additional `weather.json` is NOAA HRRR through Open-Meteo's Historical Forecast API, sampled at a 5
by 4 grid of requested locations. HRRR is about 3 km horizontally and the cached fields are
hourly. This cannot resolve a gust front crossing individual streets or a bridge's span. Any fog or
squall used in a scenario is explicitly synthetic and applied uniformly across the scene.

Setup fetches once. For an already installed checkout, fetch the additional layer with:

```sh
.venv/bin/tidewater fetch tidewater/scenarios/golden-gate-weather.toml
```

No API key or paid service is required for the intended noncommercial use. The free endpoint has
usage limits; commercial API use needs a different arrangement. Data attribution remains required.
See [Open-Meteo terms](https://open-meteo.com/en/terms),
[pricing](https://open-meteo.com/en/pricing),
[historical forecasts](https://open-meteo.com/en/docs/historical-forecast-api) and
[HRRR variables](https://open-meteo.com/en/docs/gfs-api).

The raw API response is `weather-source.json`. The normalized artifact records the query, model,
fetch time, bounds and raw response SHA-256; the Rust sampler reports the normalized artifact's
SHA-256, and a run that pins it rejects a different file at the same path. Units are encoded in the
field names: metres, m/s, mm, percent, degrees C, and hPa. Wind is a velocity toward east/north,
converted from the provider's meteorological FROM bearing. The console converts it back to a FROM
direction.

The sampler uses the nearest requested grid point and linear interpolation between hourly
instantaneous samples. It does not interpolate wind bearings across north: it interpolates
components. Missing fields stay null, missing hours are not bridged, and neither time nor space is
extrapolated. Gust maxima and precipitation totals belong to the preceding hour at each stored
timestamp; between timestamps the enclosing hour's value applies. This is retrospective replay, not
information that would have been available as a forecast at the time. The delivery rules therefore
read only the sample for the present moment unless a scenario turns on lookahead, and then label
the values as forecast ([design/delivery-rules.md](design/delivery-rules.md), section 7).

A small slice of an archive can be pinned for a headless run with
`.venv/bin/tidewater weather-snapshot <cache>/weather.json <out>.json --start ... --end ...`,
choosing the cache file of the scenario and a time window. Changing a committed snapshot is a
deliberate change to the replay. Loading it never fetches new data. Old evaluation worlds do not
acquire weather implicitly. (The headless simulator and its committed snapshots are not in the tree
yet.)

Every decision that reads weather logs the values it read, the sample's source and the archive's
checksum, so replay never queries weather. A scenario opts in with an archive path
(`archive = "path/from/repository.json"`; the table name and file format are fixed when the
scenario files are built, see [design/scenarios.md](design/scenarios.md)). Optional `[[weather.timeline]]` entries contain
increasing nonnegative `at_s` and a `values` table using the normalized field names. They
interpolate linearly, begin at the first frame and hold the last, and overlay the archive's values
at every position. Overrides are labelled synthetic. Mean-wind overrides are refused: change the
shared archive to keep the rules and the vehicles' forcing consistent. Without weather, fixed
defaults remain in force.

## What changes in the simulation

| Input | Behavior in this release |
| --- | --- |
| Mean surface wind | Drives each vehicle's simulated wind through tidewater (SITL and stand-ins); read by the energy rule (ground speed and range) and the weather rule (sustained wind limit). |
| Gust | Read by the weather rule and the payload derate; not applied to vehicle physics. |
| Visibility, precipitation | Read by the weather rule's limits for air and ground platforms and by drop-zone observations. No sensor model remains to attenuate. |
| Cloud cover, temperature, humidity, pressure | Display and agent context only. No icing, density-altitude or battery-temperature effect yet. |
| Terrain, tide, current | Tidewater's layers, independent of this change. Boats read tide depth (grounding, dock level) and current (headway) in the weather and energy rules. |

HRRR mean wind is at 10 m, not wind aloft. A delivery drone flying at 80 to 120 m sees stronger
wind than the sample says; the rules' limits are first-draft values and a height correction is an
open question. The visibility is the surface visibility parameter; there is no vertically resolved
cloud or fog layer or integration through a weather volume.

Missing meteorology is shown as unavailable. A rule that needs a value it does not have returns a
hold (`WX_UNKNOWN`) and says which input was missing. Tidewater returns null wind outside coverage
without discarding terrain and current answers. Its SITL driver leaves the simulator's previous
wind in place and reports that status; this is not a measured calm.

## Communications and weather

Clouds do not by themselves interrupt a link, and heavy precipitation can interrupt some satellite
terminals (see [Starlink's weather guidance](https://starlink.com/bi/support/article/529bf751-3cad-f460-d653-4af162f195da)).
Line of sight, terrain, antennas, network availability and link margins still matter at the Golden
Gate. Weather does not currently modify datalink delivery: the link emulator reads no weather, and
the scenarios do not invent a weather-driven radio outage. A scripted loss or cut event on the
class is the way to model one. This keeps a weather problem and a communications problem separate
in the first scenarios.
