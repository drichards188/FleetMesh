# Delivery orders

Status: design draft, 2026-11-24. Nothing here is built yet. Written for the agents who will
implement it; the maintainer has not reviewed it.

## 1. Purpose

FleetMesh's engine (`crates/fleetmesh-decide`) turns every request into an intent,
checks it, routes it by policy and logs each step. The inherited task kinds (orbit, patrol,
track, identify, lay, collect, cover and other non-delivery kinds) have no delivery meaning and are being removed.
This document defines the nine order kinds that replace them, what each one carries, when it may
run, how it moves through the engine, and whether it goes out at once or waits for a person.

The nine kinds are `pickup`, `deliver`, `hold`, `reroute`, `return_to_hub`, `land_now`, `abort`,
`swap_battery` and `confirm_drop_zone`.

Related: [delivery-rules.md](delivery-rules.md) (the checks every order passes),
[dispatch.md](dispatch.md) (who proposes `pickup` and `deliver`), [keep-out.md](keep-out.md)
(who proposes `hold` and `reroute` ahead of a restriction), [rules-versioning.md](rules-versioning.md).

## 2. Concepts

**Order.** An operator-facing word for an intent whose task is one of the nine kinds. In code it
is the engine's `Intent`; the kind is the `Task` variant.

**Proposal.** A request before the engine gives it an id: a task, the vehicles it is for, optional
constraints and a rationale. Its source (human or agent) is set by the path it arrived on, never
by the request body. This is unchanged from the inherited engine.

**Outcome.** Each check returns `allow`, `hold` or `refuse` with a reason code.

- `allow` maps to the engine's `Outcome::Pass`.
- `refuse` maps to `Outcome::Fail`. A refused order ends `rejected` before policy runs.
- `hold` is new: the check passed the order's hard limits but found something a person should
  judge. Add `Outcome::Hold` next to `Unverified`. Policy treats both the same way (never
  auto-approved, routed to a person), but replay and the attention-cost measure need to tell
  "marginal" from "could not verify". The reason code says which.

**Routing.** After the checks, policy picks a tier. `auto` dispatches at once. `human` puts the
order in the inbox. A hold outcome from any check forces `human` whatever the kind's default says.
This is the only way a check can raise a tier; nothing lowers one.

**Hold order versus hold outcome.** `hold` is also an order kind (the vehicle stays where it is).
In this document "hold order" means the kind and "hold outcome" means a check result.

**Who may approve.** Only an operator session, as before. Agents (the dispatcher, the keep-out
monitor, any LLM agent) propose and explain. An operator's own order in a tier that has
`submitter_authorizes` is approved at submission and logged as `approval.at_submission`.

## 3. Shared data shapes

An order as the bridge accepts it (`POST /api/intents`) or an agent proposes it (MCP tool):

```json
{
  "kind": "deliver",
  "params": {
    "parcel": "p-0412",
    "drop_zone": "dz-harbour-17",
    "route": [[-122.4650, 37.8040], [-122.4510, 37.8105]],
    "release": "land"
  },
  "uids": ["drone-3"],
  "constraints": {"reserve_pct": 30},
  "rationale": "Nearest idle drone with 61% battery; 4 m/s headwind adds 6% energy."
}
```

`route` is optional. When omitted the engine plans it (grid A* around no-go areas, as the
inherited `plan.rs` does) and logs the plan with the decision's inputs. `constraints` can only
tighten what the policy file sets. A rationale is required from agents and optional from people.

Ids: `parcel` and `drop_zone` and `hub` are strings from the dispatch data (see
[dispatch.md](dispatch.md)); `uids` are fleet-unique vehicle ids, never MAVLink system ids.

Task variants (serialised as `{"kind": ..., "params": {...}}`, snake_case):

| kind | params |
| --- | --- |
| `pickup` | `parcel`, `site` (hub id or lon/lat), `load_by_s` (optional) |
| `deliver` | `parcel`, `drop_zone` (id), `route` (optional), `release` (`land`, `winch`, `dock`, `slip`), `window_end` (optional ISO time) |
| `hold` | `max_s` (optional; after it the vehicle's own lost-link style return applies) |
| `reroute` | `route` (new waypoints), `cause` (`keep_out`, `weather`, `conflict`, `battery`, `operator`) |
| `return_to_hub` | `hub` (optional; default nearest by energy cost) |
| `land_now` | `site` (optional lon/lat; default here) |
| `abort` | `target` (intent id of the order to stop), `then` (`return_to_hub`, `hold`, `land_now`) |
| `swap_battery` | `hub`, `pad` (optional) |
| `confirm_drop_zone` | `drop_zone`, `parcel`, `evidence` (optional list of observation ids), `decision` is not a param: the approval is the decision |

Platform support by autopilot kind, replacing the inherited table: all four delivery platforms
(copter, plane, ugv, boat) support all kinds except where a precondition says otherwise. `land_now`
means the platform's own safe stop: land (copter), loiter-then-land at the nearest approved
landing site (plane), stop and hold (car), moor or hold station at the nearest dock (boat). The
catalog gets a per-platform `land_now` behaviour field so the task layer does not guess.

## 4. Lifecycle

All orders use the engine's state machine unchanged:

```text
proposed -> checked -> auto_approved | pending_approval | rejected (checks refused)
pending_approval -> approved | rejected | expired
approved | auto_approved -> queued -> dispatched   (queued while the vehicle cannot be reached)
dispatched -> done | failed | aborted
any pre-dispatch state -> voided                    (withdrawn, or the world changed)
```

The `effected` state and its transitions are removed with the kinds that used them.
Each dispatched order also reports per-vehicle op states, recorded as `op.state` events. Vocabulary
per kind:

- `pickup`: `loading` (vehicle at the site, parcel being loaded), `loaded`, `done` at `loaded`.
- `deliver`: `en_route`, `at_drop_zone`, `awaiting_confirmation` (only if a confirmation is
  required), `released`, `returning`, `done` when the parcel is released and the vehicle has
  started its return. The return itself is a separate `return_to_hub` order. From rules 2 the
  vehicle looks at the zone when it gets there (the drop-time look, section 5.2): it waits over
  the zone while the look is not clear, and fails the order with the parcel aboard if it stays so.
- `hold`: `holding`; `done` when superseded or when `max_s` ends.
- `reroute`: `replanned` when the new route is accepted by the autopilot, `en_route`, `done` at the
  last new waypoint, which the next order then continues from.
- `return_to_hub`: `returning`, `landing` or `docking`, `done` when disarmed and on a pad.
- `land_now`: `descending` or `stopping`, `done` when on the ground and disarmed (or stopped).
- `abort`: `stopping`, then the `then` order's states. `done` when the target order is `aborted`.
- `swap_battery`: `queued_at_hub`, `swapping`, `done` when the vehicle reports a fresh battery.
  The hub is simulated: it resets the stand-in's battery after `swap_s` (default 120 s). SITL
  needs the same reset through a parameter write; open question 3.
- `confirm_drop_zone`: no vehicle op of its own. Approval makes the held `deliver` proceed (its op
  state moves from `awaiting_confirmation` to `released`). Rejection ends both orders in the way
  `decision_on_reject` says (section 5.9).

A new order for a vehicle supersedes an older one still queued or running on it. The engine voids
a queued older order and reports a running older order as `superseded`, as it does today.

## 5. The nine kinds

Each entry lists fields (see section 3), preconditions the engine checks before the rules in
[delivery-rules.md](delivery-rules.md), and the default routing. "Auto" below means auto when every
check allows; any hold outcome sends it to the inbox and any refuse outcome rejects it.

### 5.1 pickup

Send a vehicle to a site to load a parcel.

- Preconditions: the vehicle is idle or between orders, armed state compatible with the platform,
  battery known; the parcel exists, is `ready`, and is not assigned to another vehicle; the site is
  a hub or a registered pickup point; if the vehicle is already at the site only `load_by_s` runs.
- Lifecycle: as section 4. A parcel moves `ready` to `loading` to `loaded` as op states arrive.
- Default routing: **auto** when the site is a hub, **human** when it is any other point (a
  merchant's roof or kerb is a place nobody has declared safe).
- Why: loading at a hub is routine and the vehicle never leaves the hub's own protected area.

### 5.2 deliver

Fly, drive or sail to a drop zone and release the parcel.

- Preconditions: the parcel is `loaded` on this vehicle; the drop zone exists; the route (given or
  planned) exists; the vehicle's lost-link plan matches policy (see rules, section 9); the parcel
  fits the platform's payload.
- Lifecycle: section 4. If the drop-zone rule returns hold, the order stays in the inbox. If an
  operator approves a `deliver` that was held for `DZ_NOT_APPROVED` (rules 1 and the strict
  procedure: `DZ_UNCONFIRMED`), the approval counts as the zone's approval and no separate
  `confirm_drop_zone` is needed.
- **The drop-time look** (rules 2, management by exception). Real aircraft check the spot
  themselves at drop time (Amazon's MK30 hovers at about 12 ft and knows when not to deliver;
  Zipline's droid looks down before it lowers). Here:
  - before departure the dispatcher reads the zone's latest look as the rule does; while it is not
    clear (people there, or a populated zone with no recent look that could tell), the loaded
    vehicle waits at the hub and looks again, so no `DZ_PEOPLE_NEAR` or `DZ_OBSERVATION_STALE`
    hold reaches the inbox;
  - over the zone the vehicle looks again before it lets go: a multirotor holds its low hover
    (10 m) and its release is commanded only once the zone looks clear; a car or a boat looks
    before it releases; a plane cannot hover to look and keeps the look before departure. The
    release needs the link (delivery-rules.md 11.1): a link that drops low over the zone fails the
    release, the delivery is tried again, and the person is told if it fails its tries;
  - clear means release. Occupied, unknown or stale means wait and look again, every 2 s;
  - after a bounded wait (`[automation] depart_wait_s` 120 s at the hub, `hover_wait_s` 120 s over
    the zone, in a scenario's dispatch file) automation does the safe thing on its own: the parcel
    stays aboard (or at the hub), the vehicle comes home, and the parcel is tried again after
    `retry_after_s` (600 s), at most `max_tries` (2) times. There is no alternate zone yet;
  - that outcome raises an issue alert to the person (`drop_not_clear`): what happened, what
    automation did, what the person might do (nothing, or have the customer choose another spot,
    which as a new delivery point needs the person's approval). It is never an approval request.
- Default routing: **auto** when every check allows and the drop zone is an approved delivery point
  (rules 2: a standing approval, a hub pad, or a point a person approved once; the strict
  procedure: a zone confirmed within `dropzone.confirm_valid_s`, default 3600 s, or a hub pad).
  **Human** otherwise.
- Why: this is the consequential order. Most of them should be uneventful and fast; the inbox
  should hold only the ones where something is marginal. Whether that stays true as the fleet
  grows is the research question.

### 5.3 hold

Stay where you are. Copter hovers, plane loiters at its loiter radius, car stops, boat holds
station.

- Preconditions: the vehicle has a position fix the navigation rule trusts, or the platform can
  hold without one (a car stops, a boat can stop its motors and drift; a copter without position
  lands, see `land_now`).
- Lifecycle: `holding`; ends when superseded, at `max_s`, or when the operator releases it by
  ordering something else.
- Default routing: **auto**. A hold reduces risk, and delaying it for a person defeats it. This is
  also what the keep-out monitor proposes (see [keep-out.md](keep-out.md)).
- Exception: a hold over a populated area for a fixed-wing plane is not a hold, it is a loiter
  circle over people. The geofence rule returns hold for that case, so it goes to the inbox.
- Queue wait 0: a hold is never queued; it is dispatched whatever the link state.

### 5.4 reroute

Replace the remaining route of a running order.

- Fields: `route`, `cause`.
- Preconditions: a running `deliver`, `pickup` or `return_to_hub` exists on the vehicle; the new
  route starts at or near the vehicle's current position (within `reroute.join_m`, default 150 m).
- Lifecycle: section 4. The old order keeps its id and its parcel; the reroute is a separate intent
  that references it.
- Default routing: **auto** when all checks allow, the added distance is at most
  `reroute.auto_added_pct` (default 25%), the energy margin after the change is at least the rule's
  allow margin, and the new route enters no area the old one did not already cross. **Human**
  otherwise.
- Why: a small detour around a restriction is an ordinary correction. A long or risky detour
  changes what the delivery costs and where it flies, and a person should see it.

### 5.5 return_to_hub

Go to a hub and land or dock.

- Fields: `hub` optional. Default is the hub with the lowest energy cost that has a free pad at the
  estimated arrival (pad occupancy comes from the hub model in dispatch).
- Preconditions: the destination hub is open and has a pad class compatible with the platform.
  If the parcel is still aboard, the parcel returns to `ready` at that hub when the order is done.
- Default routing: **auto**. Returning to a hub is the platform's safe default.
- Exception: the energy rule may return hold when the vehicle cannot reach any hub with its
  reserve. Then it goes to the inbox with `ENERGY_NO_HUB_REACHABLE`, and the operator probably
  wants `land_now`.

### 5.6 land_now

Bring the vehicle to the ground (or a stop) as soon as it is safe, here or at a given site.

- Fields: `site` optional.
- Preconditions: none beyond a command path. This order is allowed to run with a degraded link and
  position; it does not need the navigation rule to trust the fix, because a copter's land mode
  needs no horizontal position.
- Lifecycle: section 4.
- Default routing: **auto** when the landing point is inside a hub's landing zone or an approved
  landing site (areas of kind `landing`), **human** when it is anywhere else. Landing outside a hub
  puts a vehicle and a parcel in a place nobody planned, possibly over people or water.
- Time critical: a `land_now` in the inbox sorts first, expires after `approval.expire_s.land_now`
  (default 30 s), and says on the card what the vehicle does if nobody decides (its own failsafe
  continues; see rules section 9). When an operator enters `land_now` themselves the tier's
  `submitter_authorizes` approves it at submission, so a person deciding to land a vehicle is never
  made to approve their own order twice.
- Queue wait 0.

### 5.7 abort

Stop an order in progress and do something safe instead.

- Fields: `target` (the intent to stop), `then` (default `return_to_hub`).
- Preconditions: the target is `dispatched` or `queued` on the same vehicle.
- Lifecycle: the target moves to `aborted` (existing transition). If the target was a `deliver` and
  the parcel has not been released, the parcel stays aboard. If it was released, the abort is
  refused (`ABORT_PARCEL_RELEASED`): a released parcel cannot be un-released.
- Default routing: **auto** when the vehicle is not over or within `abort.people_buffer_m` (default
  100 m) of a populated area and the `then` order would itself be auto. **Human** when the vehicle
  is over people, or when `then` is `land_now` at a non-hub site, or when the target is a `deliver`
  inside its final `abort.final_approach_s` (default 20 s) of the drop zone.
- Why: stopping is safe in most places and aborting early costs only time. Over people, "safe" is
  a judgement about where the vehicle goes next, so a person decides. In the last seconds before
  release, the question is whether to abort or to complete, and that is also a person's call.
- Queue wait 0.

### 5.8 swap_battery

Replace a landed vehicle's battery at a hub.

- Preconditions: the vehicle is on a pad at a hub, disarmed, with no parcel aboard (a loaded vehicle
  keeps the parcel; the hub unloads it first through `pickup` in reverse, handled by dispatch); the
  hub has a charged pack in stock.
- Lifecycle: section 4. The hub's stock of charged packs decreases by one, the removed pack joins
  the charging queue and returns to stock after `hub.charge_s` (default 2400 s).
- Default routing: **auto**. It happens on the ground at a hub.
- Hold outcomes: hub out of packs (`HUB_NO_PACKS`) goes to the inbox with the dispatcher's
  suggestion (wait or move the vehicle to another hub).

### 5.9 confirm_drop_zone

From rules 2: a person approves a new delivery point. Under rules 1 (and the strict procedure,
`standing_approvals = false`): a person says the drop zone is fit for this delivery now.

Delivery spots are approved ahead of time in real operations (Amazon surveys delivery zones and
marks delivery points in them; Wing delivers to the spot the customer chose), so nobody authorises
individual drops. Each drop zone of the places data carries its standing approval (`approval =
"survey"`, `"customer"`, or `"withdrawn"`; a hub pad is approved with its hub). A
`confirm_drop_zone` is the one human act left about a place: approving a point that is not on the
standing list, or whose approval was withdrawn. The approval stands from then on.

- Fields: `drop_zone`, `parcel` (the parcel that first asked for it), optional `evidence`
  (observation ids: a vehicle camera frame classification, a scenario event such as
  `dropzone.occupied` cleared, the last confirmation).
- Preconditions: the drop zone is not marked closed. Under rules 1 and the strict procedure also:
  a `deliver` for that parcel and drop zone is queued, or is waiting at the zone in
  `awaiting_confirmation`. From rules 2 no delivery need wait: it approves a place.
- Who proposes: the dispatcher (once per zone, before it loads any parcel for it; never per parcel
  or per hour) or an LLM agent, with a rationale and evidence. Anyone may propose. The approval is
  the decision.
- Default routing: **human**, always. No tier lets an agent's `confirm_drop_zone` run without a
  person, and the policy loader refuses a file that sets it to `auto`. This order exists so that
  the most direct human judgement in the system is a first-class logged act, not a side effect.
- Outcomes: approved, the zone is an approved delivery point from then on (rules 2) and the parcels
  that waited for it go; under the strict procedure the held `deliver` proceeds and the
  confirmation is valid for `dropzone.confirm_valid_s`. Rejected, a `deliver` that waited on it is
  voided; from rules 2 the parcels for the zone stay at their hubs, the dispatcher does not ask
  again, and the person is told (`zone_not_approved`, an issue alert) until they approve it
  themselves or the parcels go elsewhere. Expired (`approval.expire_s.confirm_drop_zone`, default
  120 s), the same, and the expiry is logged as "nobody decided", which the attention-cost measure
  counts as a missed decision.

## 6. How an order becomes an engine intent

1. **Arrive.** An operator posts it, or an agent calls the MCP tool `propose_order`, or the
   dispatcher or keep-out monitor (both are agents with fixed ids) call `AgentPort::propose`. The
   engine builds a `Proposal` and sets `source` from the path. Agents must give a rationale.
2. **Snapshot.** The bridge builds the `WorldSnapshot`: per-vehicle facts with the time each fact
   arrived (position, battery, navigation fields, link state, lost-link plan, parcel aboard),
   hub and pad state, parcel and drop-zone records, the area set, the weather and tide samples
   the checks will read, and the active routes of other vehicles. Unknown is `None`, never a
   default.
3. **Check.** The engine runs the rules ([delivery-rules.md](delivery-rules.md)), each returning an
   outcome and reason codes, and logs `check.result` with the exact inputs it read.
4. **Route.** Policy picks the tier from the kind, the checks, and the proposer. It logs
   `policy.decision` with the rule that matched.
5. **Wait or send.** `auto_approved` goes to the queue-or-dispatch step at once. `pending_approval`
   appears in `GET /api/inbox` and the console until an operator approves, rejects, or the order
   expires. An agent's order in a `human` tier always waits.
6. **Queue or dispatch.** If the vehicle cannot be reached now the order queues with the kind's
   expiry; when the link returns the engine re-runs the checks on the world then and compares them
   with what the approval rested on (`queue.recheck`). Otherwise it is dispatched as exactly the
   approved order.
7. **Execute.** The bridge's task layer turns the dispatched order into a MAVLink mission or mode
   change and reports op states back. The engine refuses op states for an order that was not
   dispatched.

At approval time the engine re-runs the checks against the world as it is then, not as it was at
proposal. An approval that would now fail voids the order (`ChecksFailedAtApproval`).

## 7. What the operator sees in the inbox

Each inbox card is built from the intent and its logged checks, with nothing the engine did not
read. The card has:

- A one-line summary: kind, vehicle, parcel, drop zone ("deliver p-0412 to dz-harbour-17 with drone-3").
- Why it is here: the policy rule and every hold outcome with its reason code and a plain sentence
  ("Gusts 9.4 m/s; the drone limit is 10 m/s and holds start at 8").
- The decisive numbers with their ages: battery percent and margin after the order, wind and gust
  at the vehicle and at the drop zone, link state and which radio carries it, navigation trust
  and accuracy, distance and time.
- Who proposed it and the rationale text, shown as the proposer's claim and not as a check.
- What happens if nobody decides: the expiry time and the consequence ("expires in 82 s, the
  drone keeps holding and its battery reserve falls to 22%").
- What approval does and what reject does, in plain words.
- A map thumbnail with the route, the area or cell involved, and the vehicle's position uncertainty.
- Buttons: approve, reject, and (where it makes sense) amend, which resubmits a tightened order
  as the operator's own. Amend never loosens a limit.

Sorting: orders that expire soonest first, then `land_now` and `abort`, then everything else by
age. The inbox shows a count by kind and a count of orders that have expired unseen. These counts
feed the attention-cost measure ([attention-cost.md](attention-cost.md)).

An agent's explanation is displayed separately from the engine's reasons so that the operator can
tell what was checked from what was claimed.

## 8. Deterministic and logged

Deterministic: the checks, the tier choice, the planned route (A* with integer costs and a fixed
neighbour order), expiry times, and every transition. Replay recomputes them from the log.

Logged for each order: the `Proposal` (task, uids, constraints, rationale), the source, the
`WorldSnapshot` facts it read with their ages, the weather and tide samples (by value, never by
re-query), the area set hash, the route plan, each `check.result` with reason codes, the
`policy.decision` with the matched rule, the approval (who, session, when, and whether it was an
approval at submission), each queue step, the dispatched order, every op state, and the end state.

Not deterministic and therefore not re-derived: the operator's decision, an agent's text, the
vehicle's behaviour after dispatch. These are recorded as events, never recomputed.

## 9. Policy knobs and first-draft defaults

`sim/policy.toml`, read at start and logged whole in `run.started`:

```toml
[[tier]]
name = "safe-stops"
when = { kind = ["hold", "return_to_hub"] }
tier = "auto"
reason = "stopping and going home reduce risk"

[[tier]]
name = "confirm-is-human"
when = { kind = ["confirm_drop_zone"] }
tier = "human"
reason = "a person approves a new delivery point"
locked = true                 # the loader refuses any file that loosens this

[[tier]]
name = "land-at-hub"
when = { kind = ["land_now"], site = "hub_or_landing_area" }
tier = "auto"
reason = "landing at a hub or approved site is planned"

[[tier]]
name = "land-elsewhere"
when = { kind = ["land_now"] }
tier = "human"
submitter_authorizes = true
reason = "landing outside a hub is an unplanned place"

[[tier]]
name = "abort-away-from-people"
when = { kind = ["abort"], over_people = false, final_approach = false }
tier = "auto"
reason = "aborting away from people is safe"

[[tier]]
name = "abort-otherwise"
when = { kind = ["abort"] }
tier = "human"
submitter_authorizes = true
reason = "over people or at the last moment, a person decides"

[[tier]]
name = "routine"
when = { kind = ["pickup", "deliver", "reroute", "swap_battery"], hub_or_confirmed = true }
tier = "auto"
reason = "routine order with every check clear"

[[tier]]
name = "everything-else"
tier = "human"
reason = "default: a person approves"

[approval]
expire_s = { default = 120, land_now = 30, abort = 30, confirm_drop_zone = 120, deliver = 300 }

[queue]
expire_s = 300
human_expire_s = 120
[queue.kind]
hold = { expire_s = 0 }
return_to_hub = { expire_s = 0 }
land_now = { expire_s = 0 }
abort = { expire_s = 0 }
reroute = { expire_s = 60 }
swap_battery = { expire_s = 900 }
```

Other knobs named above: `reroute.auto_added_pct = 25`, `reroute.join_m = 150`,
`abort.people_buffer_m = 100`, `abort.final_approach_s = 20`, `dropzone.confirm_valid_s = 3600`,
`hub.swap_s = 120`, `hub.charge_s = 2400`. The order of `[[tier]]` entries matters: first match
wins, and a hold outcome overrides an `auto` match.

From rules 2 (management by exception, docs/DECISIONS.md 2026-11-25):

- `[rules.delivery.dropzone] standing_approvals = true` (the default from rules 2): an approved
  delivery point needs no confirmation, with no time limit, and `hub_or_confirmed` means "to an
  approved delivery point". `false` is the strict procedure of rules 1, kept as a comparison
  setting in `sim/policy-strict.toml` (`make start SCENARIO=harbour-city
  POLICY=sim/policy-strict.toml`): every zone but a hub pad needs a person's confirmation within
  `confirm_valid_s`. The research question is the operator-attention cost of each design.
- What automation does before a person is told is the bridge's, not the policy's, and is no
  decision's input: `[automation]` in a scenario's dispatch file, `depart_wait_s = 120`,
  `hover_wait_s = 120`, `retry_after_s = 600`, `max_tries = 2`, `hold_retry_s = 15`,
  `hold_retry_max_s = 300`, `issue_show_s = 300`. The dispatcher dry-runs each of its orders and
  waits out a hold that is traffic near the hub or the route, a stale or unread fact, or the
  look at the drop zone (section 5.2), trying again every `hold_retry_s`; after
  `hold_retry_max_s` the order goes as it is and a person decides. No threshold moves.

## 10. Tests to write

- Each kind: a proposal with valid and invalid params (missing parcel, unknown hub, no vehicles).
- State machine: every legal path in section 4, and the illegal ones (approving a `rejected`
  intent, dispatching a `pending_approval` one) fail.
- Routing table: one test per tier rule, plus the override (a hold outcome on an `auto` kind goes to
  the inbox).
- Agents never approve: the existing `compile_fail` tests stay; add one that a proposal for
  `confirm_drop_zone` from an agent ends `pending_approval`; add one that a policy file setting it
  to `auto` fails to load.
- `abort` after release is refused; `abort` over people is held; `abort` with `then = land_now`
  outside a hub is held.
- `land_now` from an operator is approved at submission; from an agent waits.
- Approval re-check: weather worsens between proposal and approval, so approval voids.
- Queue: an order to an unreachable vehicle queues and, after the link returns, re-checks;
  `hold` and `land_now` never queue.
- Replay: a log with every kind round-trips with no mismatch; editing a logged reason code or a
  tier makes replay report it.
- Supersession: a new `deliver` voids a queued older one and supersedes a running one.

## 11. Open questions

1. Should `deliver` to a confirmed drop zone be `auto` at all in the first studies? Auto keeps the
   inbox small, which is what lets a fleet scale; always-human makes every delivery a decision and
   tests the other end. Make it a policy switch and run both.
2. Is `confirm_drop_zone` one order per delivery, or may one confirmation cover several parcels to
   the same zone within `confirm_valid_s`? Settled in rules 2: neither. Delivery points are
   approved ahead of time, as real operators approve theirs (a surveyed zone, the customer's own
   spot), and a person approves only a new point, once, after which it stands. The per-hour
   confirmation is kept as the strict comparison setting. Open: whether a point's approval should
   lapse (a survey that ages, a customer who moves), and an alternate zone to go to when the
   chosen one stays occupied.
3. How does a SITL vehicle get its battery swapped? A parameter write (`SIM_BATT_VOLTAGE`,
   `SIM_BATT_CAP_AH` reset) may need a reboot. Stand-ins can simply reset.
4. Should `land_now` over water for a boat or plane mean something? A boat holds station; a plane
   cannot land on water. The catalog field may need a per-platform "no action" with a hold outcome.
5. Does an agent get an `abort` of its own proposals only, or of any order? The draft allows any
   proposer, since routing decides who may act.
6. Parcel custody during `abort`: draft keeps the parcel aboard. Does a dropped-in-flight parcel
   (a fault) need its own event kind for the audit log?

## 12. As built (2026-11-25)

Built in `crates/fleetmesh-decide` (`intent.rs`, `delivery.rs`, `delivery_rules.rs`, `policy.rs`) and
the bridge's decision layer. Where it differs from the draft above:

- The knobs are in `sim/policy.toml`, not a separate `sim/rules.toml`: per-kind approval expiry is
  `[approval.kind.<kind>] expire_s`, the queue is `[queue.kind.<kind>]`, and the numbers of the
  delivery rules are `[rules.delivery.*]` (logged with the policy, so replay decides with them).
- Hubs and drop zones are in `sim/hubs.toml` (`delivery::Places`); the bridge resolves ids to
  points when it builds a proposal and the `order` rule checks that an id and its point agree.
- `rtl`, `orbit`, `patrol` and `transit` remain beside the nine kinds; they go with the cut.
- `swap_battery` and `confirm_drop_zone` have no vehicle op: the engine marks them dispatched and
  done. The hub's swap is instant and its pack count does not fall yet; a confirmation stands from
  its approval (`Engine::note_confirmation`, kept in memory) and a rejected or expired one voids
  the `deliver` that waited on it. `land_now` is flown as the task layer's hold (no land mode yet).
- Policy conditions added: `site`, `over_people`, `final_approach`, `unplanned_landing`,
  `hub_or_confirmed`. A tier marked `locked` stays human, and the loader refuses an auto tier that
  matches `confirm_drop_zone` or matches every kind.

### Rules 2 as built (2026-11-25)

- Standing approvals: `delivery::DropZone::approval` (`StandingApproval`), read by the drop-zone
  rule and `policy_facts` behind `RulesConfig::standing_approvals` (rules 2 unless the policy sets
  `standing_approvals = false`); the engine's confirmations (`Engine::drop_zone_confirmations`)
  approve a point with no time limit.
- The dispatcher (`crates/fleetmesh-bridge/src/decision/delivery_rt.rs`): parcels for a zone that
  is not approved are not assigned and the zone is asked about once; the look before departure;
  the dry run (`Decision::agent_dry_run`) and `WAIT_OUT`; parcels kept and tried again; the issue
  alerts (`Issue`, `Decision::issue_alerts`).
- The drop-time look: `tasks::DropLook`, given by `Decision::drop_look_hook` to every dispatched
  `deliver`, asked in the task layer's transit before the release (`look_at_zone`).
- Alerts carry `issue` (what happened, what automation did, what to do); the console words the
  same three parts for the derived alerts (`web/src/decisions.ts`, `ALERT_RESPONSE`).
- Routine noise automation clears: a pickup by a vehicle on the hub's pads loads in place
  (`Decision::pickup_in_place`); a failed return to a hub is proposed again up to 3 times, then
  `return_failed`; a delivery refused only for `SEP_ROUTE_CONFLICT` is proposed again after
  `hold_retry_s`; a parcel is offered only to vehicles that carry it inside the payload rule's
  hold band (heaviest first, each group its own logged plan; none free: all); the task layer's
  NOT MOVING watch counts a climb or a descent as moving.

