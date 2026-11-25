// Checks for approvalText.ts: the ask sentence per kind, the button names.
import assert from 'node:assert/strict'
import { askSentence, DONT_SEND, placeLine, sendLabel, subjectOf, type AskFleet } from './approvalText.ts'

const fleet: AskFleet = {
  callsign: (u) => `V${u}`,
  model: () => 'Drone',
  swarm: (u) => (Number(u) < 10 ? 'ROUTE' : null),
}
const ring: [number, number][] = [
  [-122.45, 37.8],
  [-122.45, 37.81],
  [-122.44, 37.81],
]
const many = (n: number) => Array.from({ length: n }, (_, k) => String(k))

assert.equal(subjectOf({ kind: 'hold', params: {}, uids: ['3'] }, fleet), 'V3')
assert.equal(subjectOf({ kind: 'hold', params: {}, uids: ['1', '2', '3'] }, fleet), 'V1, V2, V3')
assert.equal(subjectOf({ kind: 'hold', params: {}, uids: many(8) }, fleet), '8 ROUTE vehicles')
assert.equal(subjectOf({ kind: 'hold', params: {}, uids: ['20', '21', '22', '23'] }, fleet), '4 Drone vehicles')
assert.equal(subjectOf({ kind: 'hold', params: {}, uids: many(5), group: { swarm: 'HARBOR' } }, fleet), '5 HARBOR vehicles')

assert.equal(askSentence({ kind: 'orbit', params: { radius_m: 2000, center: ring[0] }, uids: ['1'] }, fleet), 'Send V1 to orbit, 2.0 km radius')
assert.equal(askSentence({ kind: 'orbit', params: { radius_m: 250 }, uids: ['1'] }, fleet, true), 'Sent V1 to orbit, 250 m radius')
assert.match(askSentence({ kind: 'patrol', params: { route: ring }, uids: many(10) }, fleet), /^Send 10 ROUTE vehicles to patrol a [\d.]+ km loop of 3 waypoints$/)
assert.match(askSentence({ kind: 'transit', params: { route: ring }, uids: ['1'] }, fleet), /^Send V1 along a .* route of 3 points/)
assert.equal(askSentence({ kind: 'rtl', params: {}, uids: ['1', '2'] }, fleet), 'Send V1, V2 to return to launch')

// the delivery orders read as sentences
assert.equal(askSentence({ kind: 'deliver', params: { parcel: 'p-0412', drop_zone: 'dz-harbour-17' }, uids: ['3'] }, fleet), 'Send V3 to deliver p-0412 to dz-harbour-17')
assert.equal(askSentence({ kind: 'pickup', params: { parcel: 'p-1', site: { id: 'hub-presidio' } }, uids: ['3'] }, fleet), 'Send V3 to pick up p-1 at hub-presidio')
assert.equal(askSentence({ kind: 'land_now', params: {}, uids: ['3'] }, fleet), 'Send V3 to land now')
assert.equal(askSentence({ kind: 'confirm_drop_zone', params: { drop_zone: 'dz-1', parcel: 'p-1' }, uids: ['3'] }, fleet), 'Confirm drop zone dz-1 is clear for parcel p-1 (V3 waiting)')
assert.equal(askSentence({ kind: 'confirm_drop_zone', params: { drop_zone: 'dz-1', parcel: 'p-1' }, uids: ['3'] }, fleet, true), 'Confirm drop zone dz-1 is clear for parcel p-1 (V3 waiting)')
assert.equal(sendLabel('confirm_drop_zone'), 'Confirm the drop zone')
assert.equal(sendLabel('swap_battery'), 'Swap the battery')

assert.equal(placeLine({ kind: 'orbit', params: { center: [-122.458, 37.8125] }, uids: [] }), 'centre 37.8125, -122.4580')
assert.equal(placeLine({ kind: 'hold', params: {}, uids: [] }), null)

assert.equal(sendLabel('patrol'), 'Send the patrol')
assert.equal(sendLabel('something'), 'Send the order')
assert.equal(DONT_SEND, "Don't send")
console.log('approvalText.check ok')
