// Checks for playWords.ts: node web/src/plays.check.ts (prints "plays: ok"). The claims: an incident
// says its cause at its hub and its count; a play says what it does and how its end stands; an
// incident's item finds its card in the Inbox.
import assert from 'node:assert/strict'
import { cardKey, incidentCount, incidentStand, incidentTitle, liftWords, playWords } from './playWords.ts'

assert.equal(incidentTitle({ family: 'weather', codes: ['WX_GUST_LIMIT_MARGINAL', 'PAYLOAD_NEAR_MAX'] }, 'Crissy Field hub'), 'Wind at Crissy Field hub')
assert.equal(incidentTitle({ family: 'weather', codes: ['TIDE_SHALLOW'] }, 'Pier 39 dock'), 'Weather at Pier 39 dock')
assert.equal(incidentTitle({ family: 'traffic', codes: [] }, 'Marina'), 'Traffic at Marina')
assert.equal(incidentCount({ held: 4, alerts: 2 }), '4 held orders and 2 alerts')
assert.equal(incidentCount({ held: 1, alerts: 0 }), '1 held order')

assert.equal(playWords({ kind: 'ground_stop' }), 'Ground stop: nothing leaves without you')
assert.equal(playWords({ kind: 'limits', limits: { max_trip_m: 1500, max_mass_kg: 1, min_battery_pct: 60 } }), 'Within limits: trips up to 1.5 km, parcels up to 1 kg, battery 60% or more')
assert.equal(playWords({ kind: 'limits', limits: { max_trip_m: 800 } }), 'Within limits: trips up to 800 m')

assert.equal(liftWords({ lift: { below_mps: 11, clear_s: 0, needs_s: 300 } }), 'lifts when gusts stay under 11.0 m/s for 5 min')
assert.equal(liftWords({ lift: { below_mps: 11, clear_s: 130, needs_s: 300 } }), 'lifts when gusts stay under 11.0 m/s for 5 min (2 of 5 min so far)')
assert.equal(liftWords({ left_s: 1750 }), 'ends in 30 min unless you lift it')

assert.equal(cardKey('intent:12'), 'i12')
assert.equal(cardKey('alert:delivery:p-4'), 'delivery:p-4')
assert.equal(incidentStand({ mine: 2, with_officer: 3, settled: 9 }), 'in 10 min: 2 for you, 3 with the AI watch officer, 9 settled')
assert.equal(incidentStand({ mine: 0, with_officer: 0, settled: 4 }), 'in 10 min: 4 settled')
console.log('plays: ok')
