// Checks for bluf.ts (no test runner in web/): node web/src/bluf.check.ts, as make check does.
// Throws on the first failure, prints "bluf: ok" otherwise. Not part of the bundle.
import { alertParts, checksSummary, parseRationale, reasonBluf } from './bluf.ts'

function eq(a: unknown, b: unknown, what: string) {
  const x = JSON.stringify(a)
  const y = JSON.stringify(b)
  if (x !== y) throw new Error(`${what}: got ${x}, want ${y}`)
}

// the convention
eq(
  parseRationale('Orbit DRONE 1 over the depot\n- 1.5 km west of the bridge, clear of the lane\n- orbit clears the lane\n\nThe site has been quiet since 6:10.'),
  { headline: 'Orbit DRONE 1 over the depot', bullets: ['1.5 km west of the bridge, clear of the lane', 'orbit clears the lane'], more: 'The site has been quiet since 6:10.' },
  'convention',
)
eq(parseRationale('Hold DRONE 2'), { headline: 'Hold DRONE 2', bullets: [], more: '' }, 'one line')
eq(parseRationale('Headline\n\nDetail here.'), { headline: 'Headline', bullets: [], more: 'Detail here.' }, 'headline and detail')
eq(parseRationale('A\n- 1\n- 2\n- 3\n- 4\n- 5\n- 6').bullets.length, 4, 'bullets capped')
eq(parseRationale('A\n- 1\n- 2\n- 3\n- 4\n- 5\n- 6').more, '- 5\n- 6', 'extra bullets behind more')
// free text: first sentence, the rest behind More
const old = 'A delivery boat is holding 1.5 km west of the bridge, out of cellular range. It has lost cellular and came in at 15 m/s. An orbit would let it regain the link.'
eq(
  parseRationale(old),
  { headline: 'A delivery boat is holding 1.5 km west of the bridge, out of cellular range.', bullets: [], more: 'It has lost cellular and came in at 15 m/s. An orbit would let it regain the link.' },
  'free text',
)
const lp = parseRationale('x'.repeat(300))
if (lp.headline.length > 141 || !lp.more) throw new Error('a long headline is cut and kept behind more')
eq(parseRationale(''), { headline: '', bullets: [], more: '' }, 'empty')

// other reasons split on "; "
eq(
  reasonBluf('no telemetry; link marginal; battery 31%'),
  { headline: 'no telemetry', bullets: ['link marginal', 'battery 31%'], more: 'no telemetry; link marginal; battery 31%' },
  'split',
)
eq(reasonBluf('approved by op-1'), { headline: 'approved by op-1', bullets: [], more: '' }, 'short reason')

// checks
eq(checksSummary(Array.from({ length: 7 }, () => ({ rule: 'x', outcome: 'pass' as const }))), { text: '7 checks passed', tone: 'ok' }, 'all pass')
eq(checksSummary([{ rule: 'domain', outcome: 'fail' }, { rule: 'a', outcome: 'pass' }]).text, '1 failed: domain', 'one failed')
eq(checksSummary([{ rule: 'a', outcome: 'pass' }, { rule: 'link', outcome: 'unverified' }]).tone, 'warn', 'unverified warns')

// the consequence first, naming the vehicle (R10, ledger 2 section 6.1)
eq(alertParts({ kind: 'lost_link', detail: 'no telemetry since t=1.0' }, '4 min', 'CAR 1').headline, 'CAR 1 has had no telemetry for 4 min: its link is down', 'lost link names the vehicle')

console.log('bluf: ok')
