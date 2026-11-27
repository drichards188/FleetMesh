# Rules versioning

Status: design draft, 2026-11-24. Describes a convention the engine already follows and how
FleetMesh applies it from the first commit.

## 1. Purpose

Every decision the engine makes is logged with the exact inputs it read, so that replay can
recompute it from the log alone and compare. That promise has a catch: the code that recomputes the
decision is today's code, and the decision was made by the code that ran then. If the rules change
and old logs are replayed with the new rules, replay reports mismatches that are not tampering,
only history. If replay instead tolerated mismatches, it could no longer detect tampering. Rules
versioning resolves this. It says which behaviour a log was made under, and lets the current code
reproduce that behaviour exactly.

## 2. The rule

1. The engine has one integer, `RULES_VERSION`. A run writes it into `run.started`.
2. **FleetMesh starts at rules version 1.** The earlier project this one was cut from had reached
   a rules version in the thirties; those numbers described behaviour (non-delivery task kinds,
   collection tasking, identification) that no longer exists, and FleetMesh's history starts clean
   with no first commit until the cut is done. Logs from the earlier project are not FleetMesh logs
   and do not replay here. Resetting to 1 says so in the log itself.
3. The delivery behaviour built before the first commit is version 1, with no `*_SINCE`
   constant, because there was no older behaviour to reproduce. The first gated change is rules
   2, `STANDING_APPROVALS_SINCE` (standing approved delivery points, docs/DECISIONS.md
   2026-11-25); rules 3 is `TRAFFIC_SEPARATION_SINCE` (separation from the traffic the fleet does
   not control, the picture of it logged by value; policy schema 3); rules 4 is `HOME_BASE_SINCE`
   (every vehicle has a home base, and the assigner costs a trip's way back to it; the policy
   schema stays 3); rules 5 is `HUB_SEQUENCING_SINCE` (a hub sequences its own arrivals and
   departures: the separation rule predicts the fleet's own vehicles along their dispatched routes,
   or at rest when they are still, leaves a vehicle parked on a hub's stand to the hub for an order
   on, leaving or reaching that hub, and words a hub-zone meeting as the hub's to sequence; a
   pickup or a return may name a point within the hub's radius; the policy schema stays 3); rules 6
   is `FLOOD_ALONG_ROUTE_SINCE` (the weather rule reads the ground and the water, a car's flood
   and a boat's depth, where the order takes the vehicle, not anywhere; the policy schema stays 3).
4. **After the first commit, any change that can alter a decision's outcome, a check's inputs, an
   event's shape or a replay check is gated.** The change adds a named constant
   `<NAME>_SINCE: u32 = N`, where `N` is the new `RULES_VERSION`, and code branches on
   `rules >= NAME_SINCE`. It never branches on a literal version, and never on a date or a build.
5. **The same change teaches replay the gate.** Replay already runs under the version in the log.
   The change adds the behaviour under the constant in the replay path, adds the marker (below),
   and adds a test that a log made before the change replays without a mismatch and a log made
   after it replays without a mismatch.
6. `RULES_VERSION` moves to `N` in that change. One change, one bump; a change that touches two
   independent behaviours is two changes.

## 3. Why

**Replay must be able to reproduce history.** A replay under rules 3 must give the answers rules 3
gave, even when the code is at rules 7. Gating on a constant keeps the old branch alive and tested.

**Named constants can be found.** A literal `if rules >= 9` says nothing. `ROUTE_PLAN_SINCE` can be
searched, documented and deleted when the version it protects is retired. A reader of the code
finds every behaviour that depends on the version by searching for `_SINCE`.

**A change without a gate is invisible and a change with a gate is auditable.** The log says
which rules ran. A change that alters decisions silently looks, in a replay of an old log, like
tampering. A change made through a gate shows up as a new version in the logs made after it.

**Tamper-evidence stays honest.** Replay's job is to say that nothing in the log was edited. If it
could accept mismatches for old logs, an edited log could pass as old. With versions, a mismatch is
always a finding.

**Studies need it.** A study compares runs. Two runs under different rules are not comparable
unless the difference is known. The version in the report header says what changed between them.

**It is cheap.** The cost is one constant, one branch, and a test per change. The benefit is that
nobody has to remember what the rules used to say.

## 4. What counts as a rules change

Gated (bumps the version):

- any check's threshold, formula, outcome mapping or reason code that changes what it returns for
  some input;
- a new check, or a check applied to more order kinds;
- a change to routing that moves an order between tiers by default (policy files are inputs and are
  logged, so changing a policy file is not a rules change; changing how the loader interprets it is);
- a new audit event type that replay checks, or a new field that replay reads;
- a change to what is logged that decision inputs depend on;
- the dispatcher's assignment function, since the plan is replayed;
- the keep-out monitor's stage thresholds, and the attention report's definitions (which carry
  their own version in the report header, bumped with the rules).

Not gated:

- the console, API shapes the engine does not read, documentation, logging of diagnostics that
  replay ignores, performance changes that give the same answer, bug fixes that make the code agree
  with a version's documented behaviour (the fix is the new baseline, and the log text says so).

A bug fix that changes a decision's outcome is gated, not silent. If logs made under the old
behaviour exist, replay needs to reproduce the bug. In the first weeks, before any logs worth
keeping exist, the maintainer may choose to bump without keeping the old branch; that choice is a
decision recorded in DECISIONS.md and the old branch is deleted on purpose.

## 5. Mechanics

In `crates/fleetmesh-decide/src/rules.rs` (renamed with the crate):

```rust
/// The rules version this build writes into new logs.
pub const RULES_VERSION: u32 = 1;
/// The oldest rules version this build can replay. Raising it retires logs.
pub const RULES_OLDEST: u32 = 1;

/// Rules 2: the energy rule counts the hover at the drop zone (the version that added it).
pub const DROP_HOVER_SINCE: u32 = 2;
```

`RulesConfig` carries the version a run (or a replayed log) uses:

```rust
impl RulesConfig {
    pub fn at_least(&self, since: u32) -> bool { self.version >= since }
}
// in the rule
let hover_s = if cfg.at_least(DROP_HOVER_SINCE) { cfg.energy.drop_hover_s } else { 0.0 };
```

**Markers.** Each version that adds an event type or field records it in `RULES_MARKERS`: a list of
(since, name). Rules 2 marks `[rules.delivery.dropzone] standing_approvals` in the logged policy
and a drop zone's `approval` in a decision's delivery inputs; replay's `check_markers` and the
`run.started` check flag either in a log that claims rules 1. Rules 3 marks
`[rules.delivery.separation.traffic]` in the policy and `delivery.picture` in a decision's inputs,
flagged in a log that claims rules 2 or older. Rules 4 marks a candidate's `home` in a
`dispatch.plan`'s input (`dispatch.plan.input.vehicles.home`), flagged in a log that claims rules 3
or older; replay re-runs the assigner under the log's rules (`dispatch::assign_under`). Rules 5
marks a vehicle's `route` in the separation rule's traffic inputs
(`check.result.inputs.delivery.traffic.route`: the bridge fills the fleet's dispatched routes from
rules 5), flagged in a log that claims rules 4 or older. Rules 6 adds no field: the samples were
logged before, and only which ones the rule reads changed, so a rules 5 log given a rules 6 header
is found by its recomputed checks. Rules 7 marks `[officer]` in the logged policy and the
`triage.*` events (the watch officer's triage, docs/design/watch-officer.md), flagged in a log that
claims rules 6 or older; a run under older rules logs no `[officer]` (`Policy::as_logged`). Rules
8 marks `[plays]` in the logged policy and `play.adopted` (a person's play, docs/design/incidents.md),
flagged in a log that claims rules 7 or older; replay checks each decision read exactly the plays in
force. Replay
flags a log whose version is older than a
marker it contains (an event that
cannot have existed then), which catches a log edited to include newer events under an older
version number, and a log of a newer version that is missing an event the version requires.

**Replay.** `replay --rules N` replays a log under version `N` when the log's header is damaged or
a study wants to see a log under other rules (the earlier project used this after each bump);
without the flag it uses the version in `run.started`. Replay refuses a version outside
`RULES_OLDEST..=RULES_VERSION`.

**Policy and rules config are inputs.** `run.started` logs the policy file, the rules config
(`sim/rules.toml` thresholds), the catalog facts, the area set hash and the roster. Changing a
threshold in a config file does not bump the version, because the log carries the value it used
and replay reads it from the log. Changing how a threshold is applied does.

**The golden episode.** A short recorded run, made by the headless simulator, is stored with its
digest. A change that moves the digest bumps the version, and the golden episode is re-recorded in
the same change with the new digest in the commit message. A digest that moves without a bump is a
failed test.

## 6. A worked change

Suppose, after the first commit, the energy rule should add the hover at the drop zone for
copters (its cost was missed in version 1). The change contains:

1. `pub const DROP_HOVER_SINCE: u32 = 2;` and `RULES_VERSION = 2`.
2. In `energy.rs`: the hover term, applied only when `cfg.at_least(DROP_HOVER_SINCE)`.
3. In `replay.rs`: nothing special if replay calls the same function with the logged config; if
   replay has its own copy of an input (a derived number), the same gate there.
4. In `RULES_MARKERS`: if the change added a logged field (`hover_s` in the energy inputs), the
   marker `(2, "check.result.energy.hover_s")`.
5. Tests: a version-1 log made before the change replays clean under 1 and shows no hover term;
   a fresh log under 2 replays clean; a version-2 log given a version-1 header is reported by the
   marker check.
6. The golden episode is re-recorded; the commit message says so.
7. The design document for the rule is updated in the same change, with the version noted:
   "From rules 2, the energy rule adds `energy.drop_hover_s` per delivery stop."
8. DECISIONS.md gets an entry if the change was a choice, not a fix.

## 7. Tests to write

- `RULES_VERSION` is at least `RULES_OLDEST`, and every `*_SINCE` constant is at most
  `RULES_VERSION` (a test that scans the constants).
- Every `*_SINCE` constant is used in at least one `at_least` call and appears in a replay test
  (a grep-based test fails on a constant that is defined but unused).
- Every marker's version is at most `RULES_VERSION`.
- A log with a marker newer than its header version is flagged.
- Replay under each supported version of a stored fixture log for that version gives no mismatch.
- No source file other than the constants module compares `rules` or `version` against a numeric
  literal (a lint-style test that greps for `version >= <digit>`).
- The golden episode digest matches, and the test fails with a clear message when it moves
  without a version change.

## 8. Open questions

1. Retirement. How long are old versions kept? The draft says: while any log the project still
   cares about uses them, and `RULES_OLDEST` is raised in a commit that says which logs it
   retires.
2. Is a bump per change too fine? One bump per merged change is simple and keeps blame clear.
   Several small changes per release could share a bump if they land together; the draft says no.
3. Should a version bump be allowed to change the attention report's definitions, or should that
   report have a version of its own? The draft ties them together and also records the report
   version in its header.
4. Does the policy loader (its schema) get its own `*_SCHEMA_SINCE` constants, as the earlier
   project had? The draft says yes, since a loader that reads a file differently is a decision change.
