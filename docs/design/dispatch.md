# Dispatch: parcels, hubs, assignment

Status: design draft, 2026-11-24. Nothing here is built yet.

## 1. Purpose

Something has to decide which vehicle carries which parcel, from which hub, in which order. That
something proposes orders; it never approves them. Dispatch is therefore an agent in the engine's
sense: it holds an `AgentPort`, its proposals pass the same checks as anyone's, and policy routes
them ([delivery-orders.md](delivery-orders.md)).

v1 is a deterministic greedy assigner with energy and wind costs, inside the repository. v2 is an
optional external solver (VROOM or OR-Tools, see [RELATED-PROJECTS.md](../RELATED-PROJECTS.md)) used as
another proposer under the same rules.

## 2. Concepts

**Hub.** A place vehicles start and end, with pads, parcels waiting, charged battery packs and
opening hours.

**Parcel.** A thing to move from a hub (or pickup point) to a drop zone, with a mass, a deadline
and a priority.

**Drop zone.** Defined in [delivery-rules.md](delivery-rules.md), section 10.

**Assignment.** A pairing of one vehicle with one or more parcels in a sequence, with a route,
an estimated time and cost. An assignment becomes orders: `pickup` (if the vehicle is not at the
parcel's origin), then `deliver` for each parcel, then `return_to_hub`.

**Dispatcher.** The agent id `dispatcher`, a deterministic program in the bridge. Further agents
(an LLM planner, an external solver adapter) get their own ids and propose through the same port.

## 3. Data shapes

`sim/dispatch/<scenario>.toml` holds the world the dispatcher knows. Parcels may be listed or
generated from a seeded demand model.

```toml
seed = 7                       # logged; same seed, same parcels

[[hub]]
id = "hub-marina"
lon = -122.4350
lat = 37.8070
pads = 6
packs_charged = 8
charge_s = 2400
swap_s = 120
open = "00:00-24:00"
platforms = ["delivery_drone", "delivery_car"]

[[hub]]
id = "hub-pier"
lon = -122.4100
lat = 37.8090
pads = 4
packs_charged = 4
platforms = ["delivery_boat", "delivery_drone"]

[[parcel]]
id = "p-0001"
from = "hub-marina"
to = "dz-fort-mason-3"
mass_kg = 1.2
ready_s = 0                    # seconds since scenario start
deadline_s = 1800
priority = 2                   # 0 low, 1 normal, 2 high, 3 urgent
class = "general"              # general | fragile | hazmat (v1 refuses hazmat)

[demand]                       # optional generator
rate_per_min = 4
mass_kg = { min = 0.2, max = 4.0 }
priority_mix = [0.6, 0.3, 0.08, 0.02]
zones = ["dz-fort-mason-3", "dz-pier-39-1", "dz-aquatic-2"]
```

Vehicle availability, which the dispatcher reads from the snapshot, is derived, not stored:
`idle_at_hub` (landed, disarmed, pad known, battery known), `loaded` (parcel aboard, not
yet dispatched), `en_route`, `returning`, `unavailable` (fault, navigation untrusted, charging,
reserved by a pending order).

## 4. v1: the greedy assigner

A pure function, `assign(snapshot, params) -> Plan`, in its own module (`dispatch`) with no I/O.
It runs on a schedule (`dispatch.tick_s`, default 10 s of scenario time), and also on an event
(a new parcel, a vehicle becoming idle, an order voided or rejected, a drop zone closed).

**Algorithm.**

1. Open parcels are those `ready` and unassigned. Sort by priority descending, then deadline
   ascending, then id (all stable).
2. For each parcel in that order, build candidates: vehicles available now whose platform serves
   the parcel's origin hub and carries the parcel (payload rule, ignoring the weather derate for
   a cheap first filter), and which are within `dispatch.max_range_frac` (0.7) of range.
3. Cost each candidate with the shared model (`energy::trip_cost`, the same function the energy
   rule calls, wind and current taken from the snapshot's samples):

   ```text
   cost = w_energy * energy_pct
        + w_time * eta_s / 60
        + w_late * max(0, eta_s - slack_s) / 60
        + w_hold * expected_hold_penalty
        + w_idle * battery_wear_pct
   ```

   with first-draft weights `w_energy = 1.0`, `w_time = 0.5`, `w_late = 8.0`, `w_hold = 4.0`,
   `w_idle = 0.2`. `slack_s` is the time to deadline. `expected_hold_penalty` is 1 when a
   dry-run check (the `AgentPort::preview` call) returns any hold outcome, else 0, so the
   assigner prefers options that do not wait for a person when there is a comparable one.
4. Drop candidates for which a preview returns refuse. Keep the refuse reasons for the log.
5. Pick the lowest cost; break ties by vehicle uid. Mark the vehicle and the parcel reserved so
   the next parcel in the loop sees them taken.
6. **Batching** (car and boat only, as drones carry one parcel). After choosing a car or boat for
   a parcel, try adding further open parcels from the same hub whose drop zones lie within
   `dispatch.batch_detour_pct` (30) of the route and whose total mass fits. Insert at the cheapest
   position (cheapest insertion). Stop when no insertion lowers the cost per parcel.
7. If a parcel has no candidate, it stays open with the reasons recorded; the dispatcher raises a
   `dispatch.unassigned` note, never an order.

The result is a `Plan`: a list of assignments, each with vehicle, parcels in order, route, ETA,
cost terms, and the candidates it beat with their costs and rejection reasons.

**From plan to orders.** For each assignment the dispatcher calls `AgentPort::propose` with, in
sequence: `pickup` (skipped when the vehicle is at the origin with the parcel loaded), `deliver`
for each parcel, `return_to_hub`. A rationale is generated from the cost terms: "p-0001 to
dz-fort-mason-3: drone-3, 61% battery, 12 min, 4 m/s headwind adds 6% energy; next best drone-5
cost 41.2". Each order is proposed when the one before it is dispatched, not all at once, so
rejected or voided orders do not leave a chain of dead proposals. A rejected, expired or voided
order triggers a new tick for its parcel.

The engine then checks and routes each proposal as in delivery-orders. If a person holds an order
in the inbox, the dispatcher marks the vehicle reserved until the order ends, and reconsiders at
`dispatch.hold_reassign_s` (default 120 s) if the order is still waiting: it may withdraw its
proposal and assign another vehicle. Withdrawing is the agent's own `withdraw`, logged.

**Rate limit.** At most `dispatch.max_proposals_per_min` (default 60) proposals per minute. This
is a control on the demand the dispatcher can place on the inbox, and a knob for the study (see
[attention-cost.md](attention-cost.md)).

**Hub model.** Pads are a resource: an arriving vehicle takes a pad from `eta - 30 s` until it is
swapped or sent out. Packs: `swap_battery` takes one charged pack and returns a spent one to the
charging queue, which refills at `charge_s`. The dispatcher plans a `swap_battery` for any vehicle
returning below `dispatch.swap_below_pct` (default 45) when it will be needed for the next parcel.
All hub state is part of the snapshot and is logged with each plan.

**Re-dispatch triggers.** A vehicle faults or goes untrusted on navigation; a drop zone closes
or its confirmation is rejected; the keep-out monitor holds a vehicle for a restriction; weather
moves the energy cost enough that the rule would now hold (the dispatcher uses preview for this
before each tick, so it learns from the rules without knowing them).

**Home bases (rules 4).** Every vehicle belongs to a home base, the hub it returns to
([fleet-organisation.md](fleet-organisation.md)). From rules 4 (`HOME_BASE_SINCE`) the assigner
costs a trip's way back to the candidate's home base (or to the hub nearest the zone when the
battery would not cover the way home), so a vehicle based where the parcel waits wins over a
visitor; the dispatcher sends a vehicle home after a delivery (the nearest hub only when the way
home would not go at once), and sends a visitor idle for 60 s at another hub home. Before rules 4
the way back, and the return, went to the hub nearest the vehicle.

## 5. What is deterministic and logged

`assign` is deterministic: all sorting is stable, ties break by id, no randomness, no wall clock,
no hash-map iteration. Parcel generation uses a seeded PRNG (ChaCha) whose seed and stream
position are logged.

Logged, so replay re-derives the plan (under the rules the log was made under: from rules 4 each
candidate in `input.vehicles` names its `home` base):

```jsonc
{"type": "dispatch.plan", "t": 1796132410000, "tick": 142,
 "input_sha256": "...",              // hash of the logged snapshot slice the assigner read
 "params": {"w_energy": 1.0, "w_time": 0.5, "w_late": 8.0, "w_hold": 4.0, "w_idle": 0.2},
 "assignments": [
   {"vehicle": "drone-3", "parcels": ["p-0001"], "eta_s": 702, "cost": 38.1,
    "terms": {"energy_pct": 24.3, "time": 5.9, "late": 0.0, "hold": 0.0, "wear": 0.0},
    "beat": [{"vehicle": "drone-5", "cost": 41.2}], "rejected": [{"vehicle": "drone-7", "reason": "PAYLOAD_OVER_DERATED"}]}],
 "unassigned": [{"parcel": "p-0003", "reasons": ["no idle vehicle with range"]}]}
```

The snapshot slice is logged once per tick (or by hash with a reference to the earlier event that
holds it) so that replay can re-run `assign` and compare. The proposals the dispatcher then makes
are ordinary `intent.created` events; replay checks that each corresponds to an assignment in the
logged plan with the same vehicle, parcel and route (agent text is not compared).

Preview calls inside `assign` read the same engine rules as real checks. They are logged in the
plan as the outcomes they returned, since replay cannot call the engine with a hypothetical world;
replay instead re-runs the rules on the logged snapshot.

The operator's later decision on an order is not in the plan and is not recomputed.

## 6. v2: external solver

An adapter in the bridge builds a routing problem from the snapshot, sends it to a solver, reads
back routes, and turns them into candidate assignments. The adapter is an agent with its own id
(`solver-vroom`, `solver-ortools`). Its output is a set of proposals subject to the same rules and
policy as any other. Nothing about the solver is trusted: a solution that breaks a rule is refused
by the engine, and the adapter logs the refusals.

**Problem mapping (VROOM).** VROOM's JSON has `vehicles`, `jobs` or `shipments`, and optional custom
`matrices`.

- Each idle vehicle becomes a `vehicle` with `start` and `end` at its hub, a `capacity` of
  `[payload_kg * 10]` (VROOM wants integers), `skills` for its platform id, a `time_window` from now
  to the end of the planning horizon, and `max_travel_time` set to a conservative endurance
  (endurance seconds times `(1 - reserve)`).
- Each parcel becomes a `shipment`: a pickup step at the origin and a delivery step at the drop
  zone, with `amount`, `priority` (0 to 100), `skills` listing the platforms that may serve it, and
  the delivery's `time_windows`.
- The cost matrix is ours, not the road network's: for each platform profile, a duration matrix and
  a cost matrix between all stops from `energy::trip_cost` with the current wind and current. VROOM
  accepts custom matrices per profile, so the solver sees wind and energy even though it is not
  drone-aware.
- Forbidden legs (through a no-go area, beyond range) get a very large cost.

**OR-Tools.** The same mapping in a Python sidecar using the routing library: capacity and time
dimensions, a disjunction penalty per parcel for dropping it, and per-vehicle arc costs from the
matrices. It is more flexible (energy as its own dimension, reserve as a cumul bound) and more
work.

**Determinism and replay.** A solver run is not part of the deterministic core. The adapter logs
the request (hash and body) and the full response (`dispatch.solver` event) before it proposes
anything. Replay does not run the solver. It checks that the proposals match the logged response
and that each proposal's checks re-derive on the logged snapshot. Where the solver supports a
fixed seed and a solution limit rather than a time limit, set them, so reruns are identical, but
do not depend on it.

**Where v2 helps.** Many parcels and many hubs (hundreds of parcels, batching on the car and the
boat), where cheapest-insertion greedy is visibly worse. For drones, one parcel per sortie, greedy
is probably close to optimal; the benefit shows up in the cars and boats. Run both on the same
generated demand and compare cost and lateness; that comparison is one of the first eval tasks.

## 7. Policy knobs and first-draft defaults

```toml
[dispatch]
tick_s = 10
max_range_frac = 0.7
batch_detour_pct = 30
swap_below_pct = 45
hold_reassign_s = 120
max_proposals_per_min = 60
weights = { energy = 1.0, time = 0.5, late = 8.0, hold = 4.0, idle = 0.2 }
solver = "greedy"              # greedy | vroom | ortools
```

All logged in `run.started`. The dispatcher's agent id and the solver ids are fixed names on the
agent roster.

## 8. Tests to write

- Greedy: one parcel, one vehicle; nearest by cost wins; ties by uid; priority ordering; deadline
  ordering; reservation stops a vehicle being chosen twice in a tick.
- Wind: a strong headwind changes the winner between two vehicles in a constructed case.
- Payload and platform filter: heavy parcel never goes to a drone; hazmat unassigned.
- Batching: car picks up three parcels along a route; detour bound respected; mass bound respected.
- Hub: pad contention, pack stock, swap plan below threshold.
- Plan to orders: orders are proposed in sequence; a rejected order triggers a re-tick; withdrawn
  proposal on `hold_reassign_s`.
- Rate limit.
- Replay: re-running `assign` on the logged snapshot gives the logged plan byte for byte; a
  tampered plan is reported.
- Solver adapter (v2): problem JSON for a golden snapshot; a canned VROOM response becomes proposals;
  a response that breaks a rule gets those proposals refused and logged.
- Agents never approve: the dispatcher's port has no approve method (type test).
- Scale: 1,000 vehicles and 2,000 parcels assigned in under `dispatch.tick_s`.

## 9. Open questions

1. Where does demand come from for studies: scripted lists, a seeded generator, or replay of a real
   delivery trace? The draft supports the first two.
2. Should the dispatcher chain `pickup`, `deliver` and `return_to_hub` as one group order so a
   person approves a delivery, not three orders? The engine already has group orders (one approval
   covering member orders). It would cut inbox load, and it also changes what is being measured.
3. Is `w_hold` (prefer options that do not need a person) legitimate? It makes the dispatcher steer
   around the inbox, which shrinks the attention load and also hides marginal calls from people.
   Make it a switch and say so in any result that uses it.
4. Hub pack logistics may be too detailed for v1; a simple "swap takes 120 s, always available" is an
   alternative.
5. Multi-hub rebalancing (moving idle vehicles between hubs) is not in v1.
6. VROOM or OR-Tools first? VROOM is a service with a small JSON interface and licence-compatible
   as a separate process; OR-Tools handles energy as a dimension natively. Start with VROOM.
