// Checks for symbols.ts (no test runner in web/): node web/src/symbols.check.ts, as make check does.
// The claims: a vehicle is drawn as a plain delivery icon by kind (air multirotor, air fixed wing,
// ground car, water boat; a dot for an unknown one), the multirotor kinds share one icon, a stale
// link greys it with a dashed ring, and the same request answers the same drawing. Throws on the
// first failure, prints "symbols: ok" otherwise.
import assert from 'node:assert/strict'
import type { Kind } from './types.ts'
import { frameIcon, symbolSvg } from './symbols.ts'

const svg = (kind: Kind | null, stale = false) => symbolSvg({ kind }, { stale, size: 22 }).svg

const kinds: Kind[] = ['copter', 'plane', 'ugv', 'boat', 'unknown']
assert.equal(new Set(kinds.map((k) => svg(k))).size, kinds.length, 'one icon each for a multirotor, a fixed wing, a car, a boat and an unknown')
assert.equal(svg('copter'), svg('vtol'), 'a vtol is drawn as a multirotor')
assert.equal(svg('copter'), svg('blimp'), 'so is a blimp')
assert.equal(svg(null), svg('unknown'), 'no kind: the plain dot')
assert.ok(svg('boat', true).includes('stroke-dasharray') && !svg('boat').includes('stroke-dasharray'), 'a stale link: a dashed ring')
assert.notEqual(svg('plane', true), svg('plane'), 'and the grey colour')
assert.ok(svg('plane').startsWith('<svg') && svg('plane').includes('width="22"'), 'inline SVG at the requested size')
assert.equal(svg('ugv'), svg('ugv'), 'the same request, the same drawing')
const f = frameIcon({ kind: 'copter' }, false)
assert.ok(f.url.startsWith('data:image/svg+xml') && f.id === 'copter|false' && f.width === f.height, 'a map icon is a data URL, one per kind and state')
assert.equal(frameIcon({ kind: 'copter' }, false), f, 'built once')

console.log('symbols: ok')
