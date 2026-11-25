// Checks for trafficText.ts (no test runner in web/): node web/src/trafficText.check.ts, as make
// check does. The claims: a track is named by its callsign or name, else its address, else
// "anonymous" (a broadcast without identity) or "unidentified" (only our sensors see it); an
// aircraft's altitude is in metres as the fleet's with its reference, and in full in feet too; a
// vessel's speed in knots; a source says what it is, where it came from and how old; flags and
// states are plain sentences, what is wrong first; a bearing-only track's uncertainty is its line,
// not a radius; AIS static data reads as rows. Throws on the first failure, prints
// "trafficText: ok" otherwise.
import assert from 'node:assert/strict'
import type { Track } from './traffic.ts'
import { ageText, altDetail, altLabel, classWords, distText, flagLines, flagWords, knotsLabel, labelLines, mapName, sourceLine, speedText, staticRows, trackCode, trackTitle, trackTooltip, uncertaintyText } from './trafficText.ts'

function track(over: Partial<Track>): Track {
  return {
    id: 'trk-1',
    t: 100_000,
    domain: 'air',
    class: 'airliner',
    cooperative: true,
    ids: { icao24: 'a1b2c3', callsign: 'UAL123  ', anonymous: false },
    lat: 37.7,
    lon: -122.35,
    alt_m: 900,
    alt_ref: 'baro',
    h_sigma_m: 15,
    v_sigma_m: 10,
    vel_mps: [-60, -40],
    vs_mps: -4,
    heading_deg: 236,
    state: 'confirmed',
    sources: [{ kind: 'adsb_1090', rx: 'hub-pier', last_t: 99_400 }],
    flags: [],
    static: {},
    ...over,
  }
}

// ---- identity ----
assert.equal(trackTitle(track({})), 'UAL123', 'the callsign, trimmed')
assert.equal(trackTitle(track({ ids: { icao24: 'a1b2c3', anonymous: false } })), 'ICAO A1B2C3', 'no callsign: the address')
assert.equal(trackTitle(track({ domain: 'surface', ids: { mmsi: 366123456, anonymous: false }, static: { name: 'SEA STAR' } })), 'SEA STAR', "a vessel's AIS name")
assert.equal(trackTitle(track({ ids: { icao24: '~adf812', anonymous: true } })), 'anonymous', 'an anonymous address is not an identity')
assert.equal(trackTitle(track({ cooperative: false, ids: { anonymous: false } })), 'unidentified', 'what only our sensors see')
assert.equal(trackCode(track({ ids: { uas_id: '1581F5FKD229400B5E1J', anonymous: false } })), 'UAS 1581F5FKD229400B5E1J', 'a Remote ID serial')
assert.equal(mapName(track({ cooperative: false, ids: { anonymous: false } })), '?', 'on the map a non-cooperative track is a question')
assert.equal(mapName(track({ ids: { mmsi: 366123456, anonymous: false } })), '366123456', 'an MMSI alone')
assert.ok(mapName(track({ ids: { uas_id: '1581F5FKD229400B5E1J', anonymous: false } })).length <= 10, 'a long serial cut short')
assert.equal(classWords('ga'), 'light aircraft', 'classes in words')
assert.equal(classWords('small_craft'), 'small craft', 'and every one has some')
assert.equal(classWords('new_thing'), 'new thing', 'an unknown class as its own name')

// ---- altitude, speed ----
assert.equal(altLabel(track({})), '900 m baro ↓', 'metres as the fleet, pressure altitude, descending')
assert.equal(altLabel(track({ alt_ref: 'geo', alt_m: 95.4, vs_mps: 0 })), '95 m MSL', 'geometric: above mean sea level')
assert.equal(altLabel(track({ alt_m: null, alt_ref: 'none' })), 'alt ?', 'unknown altitude said')
assert.equal(altLabel(track({ domain: 'surface', alt_m: null })), null, 'no altitude for a vessel')
assert.ok(altDetail(track({})).startsWith('900 m (2,953 ft), pressure altitude'), 'in full, feet too and the reference spelled out')
assert.equal(speedText(track({ vel_mps: [3, 4] })), '5.0 m/s (10 kn)', 'speed in both units')
assert.equal(knotsLabel(track({ vel_mps: [6, 0] })), '12 kn', 'a vessel in knots')
assert.equal(knotsLabel(track({ vel_mps: [0.1, 0] })), null, 'nothing for a still one')
assert.equal(speedText(track({ vel_mps: null })), null, 'no velocity, no speed')

// ---- sources and ages ----
const name = (u: string) => (u === '12' ? 'DRONE 3' : u)
assert.equal(sourceLine({ kind: 'adsb_1090', rx: 'hub-pier', last_t: 99_400 }, 100_000, name), "ADS-B (1090 MHz), hub-pier's receiver, 0.6 s ago", 'a hub receiver with its age')
assert.equal(sourceLine({ kind: 'daa', rx: '12', last_t: 98_000 }, 100_000, name), 'detect-and-avoid camera, on DRONE 3, 2.0 s ago', 'a sensor on one of ours, by its callsign')
assert.ok(sourceLine({ kind: 'ais_a', rx: 'net-ais', last_t: 90_000 }, 100_000, name).includes('commercial feed net-ais (seconds late)'), 'the network feed said to be late')
assert.equal(ageText(0.42), '0.4 s', 'ages')
assert.equal(ageText(95), '95 s', 'ages')
assert.equal(ageText(600), '10 min', 'ages')
assert.equal(ageText(null), 'unknown', 'ages')
assert.equal(distText(1234), '1.2 km', 'distances')
assert.equal(distText(350), '350 m', 'distances')

// ---- flags, states ----
assert.equal(flagWords('sensor_disagree').tone, 'bad', 'a disagreement is bad')
assert.ok(flagWords('sensor_disagree').text.includes('faulty or spoofed'), 'and says what it may mean')
assert.equal(flagWords('moored').tone, 'info', 'moored is information')
assert.equal(flagWords('brand_new').text, 'brand new', 'an unknown flag as its name')
assert.deepEqual(flagLines(track({ flags: ['moored', 'stale', 'position_jump'] })).map((f) => f.flag), ['position_jump', 'stale', 'moored'], 'what is wrong first')

// ---- uncertainty ----
assert.equal(uncertaintyText(track({})), '±15 m horizontally (1 sigma), ±10 m vertically.', 'a radius and a height')
const bo = track({ cooperative: false, ids: { anonymous: false }, vel_mps: null, heading_deg: null, flags: ['bearing_only'], sources: [{ kind: 'daa', rx: '12', last_t: 99_000 }], bearing_only: { sensor_lat: 37.8, sensor_lon: -122.45, sensor_alt_m: 90, bearing_deg: 41.5, sigma_bearing_deg: 2, elev_deg: 3.1, range_m: [20, 2000], t: 99_000 } })
assert.equal(uncertaintyText(bo, name), 'Somewhere on the line of bearing 42° ±2.0° from DRONE 3, 3° up, 20 m to 2.0 km out.', 'a bearing-only track is its line, not a radius')
assert.deepEqual(labelLines(bo, true), ['?', 'bearing only'], 'and its label says so')
assert.ok(trackTooltip(bo, 100_000, name).includes('bearing only from DRONE 3'), 'as does its hover')

// ---- labels and hover ----
assert.deepEqual(labelLines(track({}), false), ['UAL123'], 'short: the name')
assert.deepEqual(labelLines(track({}), true), ['UAL123', '900 m baro ↓'], 'full: and the altitude')
assert.deepEqual(labelLines(track({ state: 'tentative', cooperative: false, ids: { anonymous: false } }), false), ['? ?'], 'a tentative track marked provisional')
assert.deepEqual(labelLines(track({ domain: 'surface', alt_m: null, static: { name: 'SEA STAR' }, ids: { mmsi: 1, anonymous: false }, vel_mps: [6, 0] }), true), ['SEA STAR', '12 kn'], 'a vessel: its speed')
const tip = trackTooltip(track({ flags: ['sensor_disagree'] }), 100_000)
assert.ok(tip.startsWith('UAL123 · airliner · 900 m baro ↓') && tip.includes('DISAGREES WITH OUR SENSORS') && tip.endsWith('0.6 s old'), 'the hover: who, what, how high, what is wrong, how old')

// ---- static data ----
const rows = staticRows({ name: 'ORIENT EXPRESS', imo: 9768435, ship_type: 83, ship_type_text: 'tanker', length_m: 191, beam_m: 32, draught_m: 14.3, nav_status_text: 'at anchor', destination: ' ' })
assert.deepEqual(rows, [['Name', 'ORIENT EXPRESS'], ['IMO', '9768435'], ['Ship type', 'tanker'], ['Size', '191 m long, 32 m beam'], ['Draught', '14.3 m'], ['Status', 'at anchor']], 'AIS static data as rows, empties left out')
assert.deepEqual(staticRows({}), [], 'nothing, no rows')
assert.deepEqual(staticRows(null), [], 'none at all, no rows')

console.log('trafficText: ok')
