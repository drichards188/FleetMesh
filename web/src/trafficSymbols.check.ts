// Checks for trafficSymbols.ts (no test runner in web/): node web/src/trafficSymbols.check.ts, as
// make check does. The claims: every traffic class has an icon of its own shape (a pilot boat and a
// pleasure boat share the small-boat one), air and surface traffic differ in colour, a tentative
// track is dashed and unfilled, none of them wears our vehicles' round frame, the shapes without a
// direction are not turned, and each icon is built once. Throws on the first failure, prints
// "trafficSymbols: ok" otherwise.
import assert from 'node:assert/strict'
import { AIR_CLASSES, SURFACE_CLASSES } from './traffic.ts'
import { AIR_TRAFFIC, orients, SURFACE_TRAFFIC, trafficIcon, trafficSvg } from './trafficSymbols.ts'

const air = AIR_CLASSES.map((c) => trafficSvg(c, 'air', false, 26))
const sea = SURFACE_CLASSES.map((c) => trafficSvg(c, 'surface', false, 26))
assert.equal(new Set(air).size, AIR_CLASSES.length, 'one drawing for each class in the air')
assert.equal(new Set(sea).size, SURFACE_CLASSES.length - 1, 'one for each on the water, a pilot boat and a pleasure boat alike')
assert.ok(air.every((s) => s.includes(AIR_TRAFFIC) && !s.includes(SURFACE_TRAFFIC)), 'air traffic in its own colour')
assert.ok(sea.every((s) => s.includes(SURFACE_TRAFFIC) && !s.includes(AIR_TRAFFIC)), 'surface traffic in another')
assert.ok([...air, ...sea].every((s) => !s.includes('<circle cx="20" cy="20" r="18"')), "never our vehicles' round frame")
const tentative = trafficSvg('helicopter', 'air', true, 26)
assert.ok(tentative.includes('stroke-dasharray') && tentative.includes('fill="none"') && !trafficSvg('helicopter', 'air', false, 26).includes('stroke-dasharray'), 'a tentative track is dashed and unfilled')
assert.ok(trafficSvg('made_up', 'surface', false, 26) === trafficSvg('unknown_surface', 'surface', false, 26), 'an unknown class is the unknown shape of its domain')
assert.ok(!orients('unknown_air', 'air') && !orients('drone', 'air') && orients('airliner', 'air') && orients('ferry', 'surface'), 'only shapes with a front are turned')
const i = trafficIcon('ferry', 'surface', false)
assert.equal(trafficIcon('ferry', 'surface', false), i, 'built once')
assert.ok(i.url.startsWith('data:image/svg+xml') && i.width === i.height, 'a data URL, square')
assert.notEqual(trafficIcon('pilot', 'surface', false).id, trafficIcon('pilot', 'surface', true).id, 'tentative is its own icon')

console.log('trafficSymbols: ok')
