# fleetmesh-decide

The decision layer of FleetMesh: intents, fail-closed rules, risk-tier policy, an append-only
hash-chained audit log, and deterministic replay. A library with small `fleetmesh-replay` and `fleetmesh-policy` binaries. It does not depend on the bridge; the bridge calls it.

## The model

Every order, from a person or an AI agent, is an **intent**: what (pickup, deliver, reroute,
return_to_hub, land_now, abort, swap_battery, confirm_drop_zone, plus the plain flight kinds
orbit, patrol, hold, rtl, transit), to which vehicles, with what constraints, why (required for
agents), and from whom. It moves through one state machine:

```
proposed -> checked -> auto_approved | pending_approval
         -> approved | rejected | expired | voided -> dispatched -> done | failed
checked -> approved   (a person's own intent in a tier that lets the submitter authorize)
auto_approved | approved -> queued    (a vehicle cannot be reached now)
queued -> dispatched | pending_approval | voided | expired
```

The full table is `intent::next_state`; anything else is a typed `IllegalTransition`.

1. **Rules** (`rules.rs`, `delivery_rules.rs`) are pure functions of the intent and a
   `WorldSnapshot`: capability, readiness, platform limits, domain (boats on water, cars on land),
   geofence and keep-out areas, energy, payload, weather, separation, drop zones, link and
   navigation. They fail closed: a rule that cannot evaluate says why and blocks. The delivery
   rules and their reason codes are in `docs/design/delivery-rules.md`.
2. **Policy** (`policy.rs`) routes a checked intent to a tier: sent at once (auto) or held for a
   person (human), from `sim/policy.toml`. A file may tighten the floors, never loosen them.
   Consequential orders (the `deliver` kind, confirming a vehicle lost) always need a person, never
   the proposer, and two distinct approvers with a step-up of their own credential.
3. **Engine** (`engine.rs`) owns the state and logs every step. Agents get an `AgentPort`: they
   can propose and explain, never approve. Approval needs a `HumanAuthority`, which exists only
   inside an `OperatorSession`; a session with consequential authority is needed for the
   consequential decisions (`ConsequentialAuthority`).
4. **Audit** (`audit.rs`, `hash.rs`, `anchor.rs`) is a hash chain: each record carries the hash
   of the one before, with the exact inputs each decision read. It is tamper-evident, not
   tamper-proof; anchors outside the log make a rewrite detectable.
5. **Replay** (`replay.rs`) recomputes every decision from the log alone under the rules version
   logged in the run. New behaviour is gated on a named `*_SINCE` constant (see
   `docs/design/rules-versioning.md`) and added to replay in the same change.

Other modules: `queue.rs` (orders for vehicles that cannot be reached now), `lost.rs` and
`silent.rs` (what a silent vehicle may be doing), `nodes.rs` (confirming a vehicle lost),
`dispatch.rs` (batching), `route.rs`, `plan.rs`, `reach.rs`, `terrain.rs`, `areas.rs`, `group.rs`,
`catalog.rs` (platform facts), `world.rs`, `scenario.rs`.

## Tests

`cargo test -p fleetmesh-decide --offline`. Replay fixtures are in `tests/`.
