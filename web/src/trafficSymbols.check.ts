// Checks for trafficSymbols.ts (no test runner in web/): node web/src/trafficSymbols.check.ts, as
// make check does. The claims: every traffic class has its civilian function code (and milsymbol
// knows every code it can write), an unknown class or one of the other domain is its domain's frame
// alone; the frame says neutral only for traffic that broadcasts who it is, unknown for the rest,
// pending while tentative, and is dashed while coasting or stale; only the neutral, unknown and
// pending identities are ever written, never friend or hostile; the drawing has the standard's
// light green or yellow fill (never the civilian purple), the anticipated dash when dashed, and a
// hollow dashed yellow frame when pending; aircraft and vessels differ, as do the functions; no
// traffic symbol is round or wears our vehicles' colours; icons are built once, at twice their near
// size, plain far away; an unidentified track out of focus is fainter, never gone. Throws on the
// first failure, prints "trafficSymbols: ok" otherwise.
import assert from 'node:assert/strict'
import ms from './milsymbolCore.js'
import { AIR_CLASSES, MIN_ALPHA, SURFACE_CLASSES, type Track, type TrafficClass } from './traffic.ts'
import {
  bearingIcon,
  classSvg,
  frameOf,
  functionOf,
  iconExtent,
  IDENTITY_CODE,
  isIdentified,
  NEUTRAL_FILL,
  sidcIcon,
  sidcOf,
  symbolAlpha,
  SYMBOL_SIZE,
  TRAFFIC_FUNCTION,
  trackIcon,
  trackSidc,
  UNKNOWN_FILL,
  type Frame,
} from './trafficSymbols.ts'

// ---- the mapping -------------------------------------------------------------------------------

const EXPECTED: Record<TrafficClass, string> = {
  airliner: '01:120100',
  bizjet: '01:120100',
  ga: '01:120100',
  seaplane: '01:120100',
  helicopter: '01:120200',
  drone: '01:120300',
  unknown_air: '01:000000',
  cargo: '30:140101',
  tanker: '30:140109',
  passenger: '30:140110',
  ferry: '30:140105',
  tug: '30:140111',
  pilot: '30:140100',
  fishing: '30:140200',
  sailing: '30:140400',
  pleasure: '30:140500',
  small_craft: '30:000000',
  unknown_surface: '30:000000',
}
for (const c of [...AIR_CLASSES, ...SURFACE_CLASSES]) {
  const f = TRAFFIC_FUNCTION[c]
  assert.equal(`${f.set}:${f.entity}`, EXPECTED[c], `${c}: its function code`)
  assert.equal(f.set, AIR_CLASSES.includes(c) ? '01' : '30', `${c}: air or sea surface as its domain`)
  // civilian (air 12xxxx, sea surface 14xxxx) or no function at all: never a military one
  assert.ok(f.entity === '000000' || f.entity.startsWith(f.set === '01' ? '12' : '14'), `${c}: a civilian function`)
}
assert.equal(Object.keys(TRAFFIC_FUNCTION).length, AIR_CLASSES.length + SURFACE_CLASSES.length, 'a function for every class, no more')
assert.equal(functionOf('made_up', 'surface').entity, '000000', 'a class it does not know: the frame alone')
assert.deepEqual(functionOf('cargo', 'air'), functionOf('unknown_air', 'air'), 'a class of the other domain: the track domain frame, alone')
assert.equal(functionOf('helicopter', 'air').entity, '120200')

// ---- the frame rules ---------------------------------------------------------------------------

type T = Pick<Track, 'cooperative' | 'ids' | 'flags' | 'state' | 'static' | 'class' | 'domain'>
const tr = (over: Partial<T>): T => ({ cooperative: true, ids: { icao24: 'a1b2c3', callsign: 'N123', anonymous: false }, flags: [], state: 'confirmed', static: {}, class: 'helicopter', domain: 'air', ...over })
const noIds = { anonymous: false }

assert.deepEqual(frameOf(tr({})), { identity: 'neutral', dashed: false }, 'a broadcast with its address: identified')
assert.equal(frameOf(tr({ ids: { mmsi: 366123456, anonymous: false }, domain: 'surface', class: 'ferry' })).identity, 'neutral', 'an AIS vessel with its MMSI')
assert.equal(frameOf(tr({ ids: noIds, static: { name: 'BAY STAR' }, domain: 'surface', class: 'ferry' })).identity, 'neutral', 'a name alone identifies too')
assert.equal(frameOf(tr({ ids: { icao24: 'adf812', anonymous: true } })).identity, 'unknown', 'an anonymous address: broadcasting, but not who it is')
assert.equal(frameOf(tr({ flags: ['anonymous'] })).identity, 'unknown', 'the anonymous flag too')
assert.equal(frameOf(tr({ ids: { uas_id: '  ', anonymous: false } })).identity, 'unknown', 'a blank identity is none')
assert.equal(frameOf(tr({ cooperative: false, ids: noIds })).identity, 'unknown', 'seen only by our sensors')
assert.equal(frameOf(tr({ cooperative: false })).identity, 'unknown', 'not broadcasting is never identified, whatever ids it carries')
assert.ok(isIdentified(tr({ flags: ['sensor_disagree'] })), 'a broadcast our sensors contradict keeps its frame (the red ring says the rest)')
assert.equal(frameOf(tr({ state: 'tentative' })).identity, 'pending', 'tentative: pending, identified or not')
assert.equal(frameOf(tr({ state: 'tentative', cooperative: false, ids: noIds })).identity, 'pending')
assert.deepEqual(frameOf(tr({ state: 'coasting' })), { identity: 'neutral', dashed: true }, 'coasting: dashed')
assert.deepEqual(frameOf(tr({ flags: ['stale'], cooperative: false })), { identity: 'unknown', dashed: true }, 'stale: dashed')

// ---- the codes: only neutral, unknown and pending ----------------------------------------------

const FRAMES: Frame[] = (['neutral', 'unknown', 'pending'] as const).flatMap((identity) => [false, true].map((dashed) => ({ identity, dashed })))
assert.deepEqual(Object.values(IDENTITY_CODE).sort(), ['0', '1', '4'], 'pending 0, unknown 1, neutral 4: no other identity is written')
for (const c of [...AIR_CLASSES, ...SURFACE_CLASSES])
  for (const f of FRAMES) {
    const sidc = sidcOf(TRAFFIC_FUNCTION[c], f)
    assert.match(sidc, /^100[014](01|30)[01]000\d{6}0000$/, `${c} ${JSON.stringify(f)}: ${sidc}`)
    assert.ok(!/^10.[2356]/.test(sidc), `${sidc}: never friend, assumed friend, suspect or hostile`)
    assert.equal(sidc[6], f.dashed ? '1' : '0', `${sidc}: dashed is status anticipated`)
    assert.ok(new ms.Symbol(sidc, {}).isValid(), `milsymbol draws ${sidc}`)
  }
assert.equal(trackSidc(tr({})), '10040100001202000000', 'an identified helicopter: neutral civilian rotary wing')
assert.equal(trackSidc(tr({ cooperative: false, ids: noIds, state: 'coasting', class: 'small_craft', domain: 'surface' })), '10013010000000000000', 'a coasting radar contact: unknown sea surface, anticipated')

// ---- the drawing -------------------------------------------------------------------------------

const ALL = [...AIR_CLASSES.map((c) => [c, 'air'] as const), ...SURFACE_CLASSES.map((c) => [c, 'surface'] as const)]
const N: Frame = { identity: 'neutral', dashed: false }
const U: Frame = { identity: 'unknown', dashed: false }
const P: Frame = { identity: 'pending', dashed: false }
for (const [c, d] of ALL) {
  const n = classSvg(c, d, N, 20)
  const u = classSvg(c, d, U, 20)
  assert.ok(n.includes(`fill="${NEUTRAL_FILL}"`) && !n.includes(UNKNOWN_FILL), `${c}: identified is green`)
  assert.ok(u.includes(`fill="${UNKNOWN_FILL}"`) && !u.includes(NEUTRAL_FILL), `${c}: unidentified is yellow`)
  for (const s of [n, u, classSvg(c, d, P, 20)]) {
    assert.ok(!s.includes('rgb(255,161,255)') && !s.includes('rgb(128,224,255)') && !s.includes('rgb(255,128,128)'), `${c}: never the civilian purple, friend blue or hostile red`)
    assert.ok(!s.includes('<circle'), `${c}: no round frame (ours are round)`)
    assert.ok(!/#4fa3ff|#53c28b|#46c6d6/i.test(s), `${c}: not our vehicles' colours`)
  }
  assert.ok(!n.includes('stroke-dasharray') && classSvg(c, d, { identity: 'neutral', dashed: true }, 20).includes('stroke-dasharray="8,12"'), `${c}: dashed (anticipated) only when dashed`)
  const p = classSvg(c, d, P, 20)
  // the frame dashed and unfilled in yellow, and no shape filled (a glyph's letter is text)
  assert.ok(p.includes(`stroke-dasharray="12,10" stroke="${UNKNOWN_FILL}" fill="none"`) && !/<path[^>]*fill="rgb/.test(p), `${c}: tentative is a hollow dashed yellow frame`)
}
const distinct = (cs: readonly TrafficClass[], d: 'air' | 'surface') => new Set(cs.map((c) => classSvg(c, d, N, 20))).size
assert.equal(distinct(AIR_CLASSES, 'air'), 4, 'in the air: fixed wing, rotary wing, unmanned, none')
assert.equal(distinct(SURFACE_CLASSES, 'surface'), 10, 'on the water: nine functions and none')
assert.notEqual(classSvg('unknown_air', 'air', U, 20), classSvg('unknown_surface', 'surface', U, 20), 'an unidentified aircraft and vessel have different frames')

// ---- icons -------------------------------------------------------------------------------------

const heli = tr({})
const near = trackIcon(heli, 'near')
assert.equal(trackIcon(heli, 'near'), near, 'built once')
assert.equal(trackIcon(heli, 'mid'), near, 'near and mid share an icon (drawn smaller)')
assert.notEqual(trackIcon(heli, 'far').id, near.id, 'far away: its own, plain')
assert.notEqual(trackIcon(heli, 'far').url, near.url)
assert.ok(near.url.startsWith('data:image/svg+xml') && near.width > 0 && near.height > 0)
const e = iconExtent(near, 'near')
assert.ok(Math.abs(near.height - 2 * e.height) <= 0.01, 'rasterised at twice its near size')
assert.ok(e.right > 0 && e.reach >= e.right && e.reach * 2 >= e.height - 0.01, 'its reach covers it')
assert.ok(iconExtent(near, 'mid').height < e.height && iconExtent(near, 'far').height < iconExtent(near, 'mid').height, 'smaller off the fleet')
assert.ok(SYMBOL_SIZE.far < SYMBOL_SIZE.mid && SYMBOL_SIZE.mid < SYMBOL_SIZE.near)
const contact = tr({ cooperative: false, ids: noIds, class: 'unknown_surface', domain: 'surface' })
assert.ok(iconExtent(trackIcon(contact, 'near'), 'near').reach > iconExtent(trackIcon(tr({ class: 'ferry', domain: 'surface', ids: { mmsi: 1, anonymous: false } }), 'near'), 'near').reach, "an unknown frame reaches further than a neutral one: labels sit beside each")
const b = bearingIcon(tr({ cooperative: false, ids: noIds, class: 'unknown_air', flags: ['bearing_only'] }))
assert.equal(b.id, `${sidcOf(functionOf('unknown_air', 'air'), U)}|f`, 'a bearing-only track: a plain unidentified air frame')
assert.equal(sidcIcon('10010100000000000000', false), b, 'shared with any such frame')

// ---- strength ----------------------------------------------------------------------------------

assert.equal(symbolAlpha(0.78, U, false), 0.47, 'an unidentified track out of focus: 60 % of its strength')
assert.equal(symbolAlpha(0.78, U, true), 0.78, 'in focus (near, selected, hovered, flagged): its own')
assert.equal(symbolAlpha(0.78, N, false), 0.78, 'an identified one keeps its own')
assert.equal(symbolAlpha(0.78, P, false), 0.78, 'a tentative one is hollow already')
assert.equal(symbolAlpha(0.2, U, false), MIN_ALPHA, 'never below the floor')

console.log('trafficSymbols: ok')
