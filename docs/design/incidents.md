# Incidents and plays: one decision for a pattern

Status: rules 8 (`PLAYS_SINCE`), 2026-11-27. Drafted after a wind-event run where one cause (gusts
over the drones' soft limit) produced about six identical decisions a minute, each with the same
obvious answer, and every order turned down came back a few minutes later: whoever decided, the
person or the watch officer, was knocking the same item down again and again. The officer's own
notes said "Same wind incident as #1" on every one.

## 1. What other fields do

- **Process alarm management** (ISA-18.2): more than about ten alarms in ten minutes is a flood;
  the system shows the alarm that started it (first-out) and suppresses the ones its state makes
  expected (state-based alarming).
- **Air traffic flow management**: weather at an airport becomes one ground stop or ground delay
  programme over every flight in its scope, with an update time when it is looked at again.
- **Supervisory control of many vehicles** (Miller and Parasuraman's Playbook): the operator calls
  a play, a goal with the limits within which automation carries it out, instead of directing
  each vehicle; workload and reaction times fell against item-by-item control.
- **IT incident tools**: alerts that match by content and arrive together become one incident.

## 2. Incidents (the bridge, display)

An incident is the items in the Inbox that share a cause at one place: the held orders and alerts
of one hub whose reasons are all of one family (weather: `WX_*`, `DZ_GUST_AT_ZONE`, the payload
derate; traffic: `SEP_TRAFFIC_*`, `SEP_SHIP_*`; navigation: `NAV_*`; link; energy; drop zone;
airspace). Three or more open items make one. An item whose reasons span families is never in
one: it stands on its own, and an incident lists the items at its hub that are not part of it
("1 held item here is not about the wind"), so grouping never hides the odd one out.

An incident has the hub's measure of its cause where there is one (the gust against the
strictest limits of the platforms based there, and how it moved over the last minute) and offers
the plays that fit it. It is a view: deterministic from the Inbox and the fleet view, never
logged, so replay is unaffected by it.

## 3. Plays (the engine, rules 8)

A play is a person's standing decision for a pattern: a scope (a hub), what it does, an end and the
exceptions it leaves to the person. Only a person adopts one (`play.adopted`, with their session
under credentials); agents and the watch officer may recommend one. The engine holds the plays in
force, fills them into each decision's inputs (`plays`), and routes by them; replay tracks
`play.adopted` and `play.ended`, checks that each decision read exactly the plays in force, and
re-derives each routing from them.

- **Ground stop** (`ground_stop`): no agent's pickup or delivery leaves the hub without a person.
  The policy routes such an order to a person (`(ground stop)`); the dispatcher proposes none, and
  withdraws its own that wait for a person. Its vehicles in the air carry on.
- **Within limits** (`limits`): an order held only for the codes the play names (soft-band codes
  the policy lets a play answer, `[plays] codes`), whose trip, parcel and battery are within its
  limits, goes as the person decided (`(play)`); one outside them waits for a person as before.
  The dispatcher proposes only trips within the limits.
- **Exceptions**: a parcel at or above the play's `except_priority` is outside the play: its
  order is decided as if there were none, so an urgent parcel still asks the person.
- **End**: a play ends when its condition clears (the bridge: the hub's gust under the soft limit
  for five minutes, logged as `play.ended` with why), when the person lifts it, or at its
  `until_t` (at most `[plays] max_s`), which the engine enforces and replay checks.

## 4. The officer (rules 7 unchanged)

The officer sees incidents in its update and may recommend a play for one (`recommend_play`): the
incident's card leads with it, the person adopts it or not. Orders a play sends need no verdict;
what a play leaves (its exceptions, the odd ones out) the officer triages as before.

## 5. Being told the officer is on it

Wherever an item shows, it says who has it: a vehicle whose order or alert is with the officer is
marked so on the map and in its row (never "needs you"), a hub's line counts what the officer
holds, and the Inbox's head shows the officer at work (what it is deciding and how long it has,
what it just did and why, its counts) and says plainly when its decisions are not landing.

## 6. The scenario (wind-event)

The arc of docs/design/scenarios.md section 4, with what makes a pattern worth one decision:
the gust's trend before the soft limit; an urgent parcel (priority 4) at about 4 minutes, which
no play decides; one drone held for something else (its navigation degraded near the hub) at
about 5 minutes, which grouping must not hide; the hard limit at about 8 minutes, when
automation brings the fleet home; and the end of the play when the gust has stayed under the
soft limit for five minutes, the backlog then going out most urgent first. Run item by item and
with plays, it measures decisions, time to the first, late parcels and whether the odd one out
was caught.
