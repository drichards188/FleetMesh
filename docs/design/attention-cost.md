# Operator attention cost

Status: design draft, 2026-11-24. Nothing here is built yet. Citations are from memory of the
literature and must be checked before anything is published (section 9).

## 1. Purpose

FleetMesh asks whether human judgment over consequential decisions stays meaningful as the fleet
grows. "Meaningful" has to be turned into numbers the system can log and replay can reproduce.
This document defines the **operator attention cost**: a set of measures of how much of one
person's attention a fleet consumes, and of what happens to the quality of their decisions as that
cost approaches what a person has.

The measures are computed from the audit log alone, so a replay of a run gives the same numbers.
They are built to compare runs (fleet size, policy, dispatch rate, weather) and, when people take
part, to compare people. A development run (header ids, the red DEV pill) is never evidence about
people; the report says whether a run was a dev run.

## 2. Grounding in published work

Five bodies of work shape the design.

**Neglect tolerance, interaction effort, fan-out** (Olsen and Goodrich 2003; Olsen and Wood 2004;
Crandall, Goodrich, Olsen and Nielsen 2005). A vehicle can be left alone for some time before its
performance falls below an acceptable level (neglect tolerance, NT). Serving it takes time (interaction
time, IT). If one operator serves vehicles in turn, the number that can be supervised is about
`1 + NT / IT`, the fan-out. This gives a first estimate of how many vehicles one person can handle
and a way to say which design change raises the number (longer NT through more autonomy; shorter IT
through better display). FleetMesh uses NT and IT as defined here, measured from the log.

**Utilisation and wait time** (Cummings and Guerlain 2007; Cummings and Mitchell 2008). In
supervisory control of multiple vehicles, operator capacity can be predicted from how much of their
time is busy (utilisation) and from the time vehicles spend waiting for the operator. Their
experiments found performance degrading as utilisation rose above roughly 70 percent. FleetMesh
reports utilisation and wait time directly and marks a draft threshold at 0.7, to be tested, not
assumed.

**RESCHU** (the MIT Humans and Automation Laboratory's supervisory-control environment for
heterogeneous unmanned vehicles). It is an experimental paradigm: an operator allocates vehicles to
targets, handles re-planning and visual tasks, and the studies record operator-to-vehicle ratios,
wait times and workload. There is no maintained open repository (see
[RELATED-PROJECTS.md](../RELATED-PROJECTS.md)), so FleetMesh takes the paradigm, not code: task
arrivals the operator cannot control, a mix of routine and consequential decisions, a measured
operator-to-vehicle ratio.

**NASA MATB and OpenMATB.** The Multi-Attribute Task Battery (Comstock and Arnegard 1992; the
MATB-II software by Santiago-Espada and colleagues, NASA 2011) is the standard secondary-task
workload instrument: tracking, system monitoring, resource management and communications tasks,
with performance logged. OpenMATB (Cegarra and colleagues 2020) is an open reimplementation with
logging, listed in RELATED-PROJECTS.md; its licence text must be read before any code is copied.
FleetMesh uses the idea as an option: an embedded secondary task (a small monitoring or
communications task in the console) whose performance falls as the inbox demands more attention.
It is optional, and a study that uses it says so.

**Subjective workload, automation bias, complacency.** NASA-TLX (Hart and Staveland 1988) is the
common self-report; the log cannot contain it, so a study collects it separately and joins it by
session id. Automation bias and complacency (Parasuraman and Manzey 2010; Mosier and Skitka)
are the failure the research question is about: an operator who approves what the system proposes
without checking it. Bainbridge's "ironies of automation" (1983) is the argument that the more
reliable the automation, the less practised and less vigilant the person who must catch its
failures. Levels of automation (Sheridan and Verplank 1978; Parasuraman, Sheridan and Wickens
2000) provide the vocabulary for what the policy tiers do: an `auto` tier is a high level of
automation for that decision, a `human` tier is a low one.

What FleetMesh adds to this literature is a testbed in which the fleet size, the policy that routes
decisions, the dispatcher's rate, and the conditions that cause holds can each be varied while
every decision is logged with the information the operator was shown.

## 3. Concepts

**Decision item.** An inbox item that a person decided, or that expired. One item per order held
for a person. Group orders count once.

**Attention event.** A logged act showing the operator looked at or acted on something: opening an
item, selecting a vehicle, issuing an order, deciding an item. The console reports these. A
vehicle is **attended** at an attention event that names it.

**Window.** A time span of the run, default 5 minutes, sliding, over which rates are computed. The
report gives per-window series and whole-run summaries.

**Informed decision.** A decision made after the item's card was shown for at least the card's
minimum read time. The minimum read time is a proxy, not a measurement of reading: `min_view_s =
2 s + words_in_card / 4` (about 240 words per minute) with a floor of 3 s. A decision before that
is a **quick decision**. A decision on an item never shown is a **blind decision**. The point is
not to blame the operator but to count how often the system's design leaves them time to read.

## 4. The measures

Each measure has a definition, the events it comes from, and what it says about scale.

### 4.1 Inbox load

- **Arrival rate** `lambda(t)`: items entering `pending_approval` per minute, in each window, by
  order kind and by cause (the reason codes that held the item).
- **Queue length** `Q(t)`: items pending at an instant. Report mean, p95 and max per window, and
  the **oldest age**.
- **Service demand**: the sum, over decided items in a window, of the operator's handling time
  (from the card being shown to the decision), divided by the window length. This is the
  **utilisation** `rho`. When the log has no view event for an item, handling time is the decision
  latency (an upper bound).
- **Saturation**: fraction of windows with `rho` above 0.7 (draft threshold), and the fleet size
  at which `rho` first exceeds it.

Scale reading: how `lambda` grows with fleet size is the first result. If holds scale linearly with
vehicles and handling time is constant, `rho` grows linearly and capacity is a fixed number of
vehicles per person.

### 4.2 Decision latency

For each decided item: `latency = t_decision - t_pending`. Report p50, p90, p99 per order kind and
per window. Also:

- **Expiry rate**: expired items over items that entered pending. An expiry is a decision nobody
  made; its consequence is logged by the engine (the vehicle's own failsafe or the order voided).
- **Time to first view**: from pending to the card being shown. This is the part of latency spent
  before the operator is even looking.
- **Handling time**: from first view to decision.
- **Age at decision**: how stale the facts on the card were when the person acted, from the
  facts' ages in the decision's inputs. A slow decision on a card whose battery reading is now
  two minutes old is a different thing from a fast one.

### 4.3 Neglect time

For each vehicle: the time since it was last attended, at every instant. Report:

- **Neglect time distribution** across vehicles per window (mean, p90, max).
- **Neglect tolerance, NT** (Olsen): per vehicle, the longest time it can run without attention
  before it needs it. Measured in the log as the interval from a vehicle's last attention (or the
  start of its current order) to the instant it **first needed a person**: an item that held for
  it, a navigation trust fall, a link loss, a failsafe. The mean of these over the run, by order
  kind, is `NT`. A scenario in which most orders go out automatically has a large NT.
- **Interaction time, IT**: the mean handling time per decision plus the mean time per
  operator-initiated order (from the first attention event of an interaction to its last, with
  gaps over 10 s ending an interaction).
- **Fan-out estimate**: `FO = 1 + NT / IT`, reported with a bootstrap interval. It is an estimate
  of how many vehicles one person can serve if neglect and interaction were the only demands.
- **Wait time due to the operator** (Cummings): for each vehicle, time spent in a state where
  nothing can proceed until a person decides (an order pending, a vehicle holding for a
  confirmation). Report the fleet total and the fraction of vehicle-time.
- **Unattended exceptions**: events that needed a person and were never attended before they
  resolved (a failsafe ran with no operator contact).

### 4.4 Interruptions

An **interruption** is an inbox arrival or an alert shown while the operator is mid-interaction
(between the card opening for one item and its decision, or between the first and last attention
event of an operator-initiated order). Report:

- interruptions per hour, split by whether the interruption was for the same vehicle as the
  interaction (related) or another (unrelated);
- **task switches**: consecutive operator acts on different vehicles or items, per hour;
- **resumption lag**: the time from an interruption ending to the next operator act on the
  interrupted task;
- **unsolicited alerts** per hour: items shown that asked for no decision (link loss badges,
  failover notices). The console shows them, so they cost attention even though they are not
  decisions.

### 4.5 Errors and override rates

Errors need a notion of right. In a simulation the truth is known, so for each decided item the
**reference outcome** is defined, in two ways:

1. **Scripted**: scenarios mark items where the right answer is built in (an occupied drop zone
   that should be rejected, an interference-spoofed vehicle that should be held). The scenario file
   lists them by cause and time.
2. **Counterfactual**: for the rest, the reference is what the engine's checks would say under
   ground truth (true position, true weather, true occupancy), run on the truth log by the
   evaluation tool. An approved order whose truth-check would have refused it is a **false
   approval**; a rejected order whose truth-check would have allowed it and would have been safe
   is a **false rejection**.

Report, per 100 decisions and per window:

- **false approval rate** (the harmful one) and **false rejection rate** (the costly one, lost
  deliveries and lateness);
- **expiry rate** (from 4.2), counted separately, since nobody judged;
- **agreement rate with proposals**: the fraction of agent-proposed items the operator approved
  unchanged. A rate near 1 with a high load is the signature of rubber-stamping, not proof of it;
- **override rate**: the fraction of items where the operator did something other than accept the
  proposal as given: reject, amend, or enter an order that supersedes it within 60 s;
- **intervention rate on automatic orders**: operator-initiated orders (hold, abort, reroute)
  against vehicles whose current order went out automatically, per 100 automatic orders. This is
  the operator catching what policy let through. It rises when oversight is meaningful and falls
  when it is not.
- **quick and blind decision fractions** (section 3).
- **approve streak**: the longest run of consecutive approvals, and the mean latency within runs
  versus the mean outside them.

### 4.6 The summary: meaningful decision fraction

One headline number per window and per run:

```text
meaningful = (decisions that are informed AND before expiry AND not part of a streak flagged as
              rubber-stamped) / (items that entered pending)
```

where a streak is flagged when its length is at least 8 and its median handling time is under
`min_view_s`. This is a proxy and the report always shows the components, so a reader can see which
failure is driving it (expiry, blind decisions, streaks). The research question becomes: how does
this fraction change with fleet size, and where is the knee.

## 5. What the bridge must log

The engine already logs the decisions (`intent.*`, `approval.*`, `policy.decision`, `check.result`)
and `operator.view` events. The following are added (all as audit events, all in world time, which
equals wall time at the 1x that a study requires):

| event | when | fields |
| --- | --- | --- |
| `inbox.snapshot` | every 5 s, and on each change if more than 1 s since the last | operator id, queue length, oldest age, count by kind, ids |
| `operator.card_shown` | the console renders an item's card | operator, session, item id, `card_sha256`, word count, time |
| `operator.card_closed` | the card leaves view (closed, or another opened) | same, with the reason |
| `operator.focus` | the operator selects a vehicle, zone, or area | operator, session, subject, how (click, search, keyboard) |
| `operator.order_started` | the operator begins an order of their own | operator, session, order draft id |
| `alert.shown` | the console shows an alert or badge change | alert id, class, vehicle, whether it asks for an act, `card_sha256` |
| `alert.dismissed` | dismissed or resolved | alert id, by whom or by what |
| `operator.idle` | no console input for 30 s, and again when input resumes | operator, session |
| `secondary.task` | optional: MATB-style secondary task events and responses | task, stimulus, response, latency |
| `study.marker` | scenario file marks a scripted item | item id or cause, reference outcome |

The card hash lets replay prove what the operator was shown without re-rendering: the card's content
is a function of the logged check results, so the hash is checked against a recomputation.
Console events are signed into the log by the bridge on receipt; the console is not trusted to
timestamp. The `view` and `focus` events are best effort (a console can be closed, a tab hidden),
and the report says which hours had a connected console.

Not logged: keystrokes, mouse paths, screen contents. Eye tracking is out of scope.

Privacy: in a credentials run the operator id is the account. For a study, the account is a
pseudonym and the pseudonym-to-person mapping is held outside the log.

## 6. How replay reproduces it

A new tool (`fleetmesh-replay --attention <log>`)
computes every measure above as a pure function of the log and, for the counterfactual error
reference, of the truth log of the same run. Rules:

- No wall clock and no randomness. Window boundaries are multiples of the window length from the
  run's first event. Bootstrap intervals use a fixed seed recorded in the report.
- Every measure's inputs are events a replayed log contains. The tool refuses a log whose hash
  chain does not verify.
- Output is JSON and CSV with a header block: log hash, rules version, authentication mode, dev or
  not, window length, thresholds (`min_view_s` formula, saturation threshold, streak length),
  tool version.
- The same log gives the same bytes. A test replays a recorded log twice and compares.

What replay can show: all of the above from the events as logged. What it cannot show: what the
person attended to beyond the logged view and focus events, whether they understood the card, or
their workload (self-report).

An illustrative report fragment:

```json
{
  "run": {"log_sha256": "...", "rules_version": 1, "auth": "credentials", "dev": false,
          "fleet": {"delivery_drone": 120, "delivery_car": 30, "delivery_boat": 20, "delivery_plane": 10}},
  "window_s": 300,
  "summary": {
    "arrivals_per_min": 3.4, "queue_p95": 6, "utilisation": 0.62, "saturated_windows": 0.18,
    "latency_s": {"p50": 11.2, "p90": 41.0, "p99": 118.0}, "expiry_rate": 0.031,
    "nt_s": 412, "it_s": 14.6, "fan_out": 29.2,
    "interruptions_per_h": 18, "task_switches_per_h": 74,
    "false_approval_per_100": 1.4, "false_rejection_per_100": 2.1,
    "agreement_rate": 0.93, "override_rate": 0.06,
    "quick_fraction": 0.22, "blind_fraction": 0.04, "meaningful": 0.68
  }
}
```

(The numbers are made up to show the shape.)

## 7. Using it: the scaling study

1. **Without people.** A scripted operator (the autoplay proxy, with a fixed service-time
   distribution) exercises the pipeline at fleet sizes 10, 30, 100, 300, 1,000 and 3,000. It gives
   the theoretical shape: `lambda(N)`, `rho(N)`, and the knee under an ideal operator. It also
   finds measurement bugs before a person is involved. This is a software test, not a finding.
2. **Policy sweeps.** At each size, vary one thing: the hold band in the weather and energy rules,
   the dispatcher's proposal rate, whether `deliver` to a confirmed zone is `auto`, the batch
   size. Report how inbox load and the meaningful fraction respond.
3. **With people.** Credentials runs, the Rust console, a pseudonymous operator, fast forward
   refused, an agreed protocol. Compare to the scripted curve to see where people depart from it.
   Self-report workload (NASA-TLX) collected separately and joined by session.
4. **Perturbations.** Scenario 2 (wind event) and scenario 3 (interference) are built to create
   bursts of holds and scripted wrong answers. Report the measures through and after each burst
   (recovery time).

## 8. Policy knobs and first-draft defaults

```toml
[attention]
window_s = 300
min_view_floor_s = 3
read_words_per_s = 4
saturation_rho = 0.7
streak_len = 8
interaction_gap_s = 10
idle_s = 30
snapshot_s = 5
override_window_s = 60
bootstrap_seed = 1
bootstrap_n = 1000
```

All are logged in the report header. A change of definition after the first commit is a replay
change and is gated like a rule ([rules-versioning.md](rules-versioning.md)); the attention tool
carries its own version in the report so a published number says which definition made it.

## 9. Tests to write

- A synthetic log with known arrivals, views and decisions gives the hand-computed rates, latencies
  and utilisation.
- NT and IT from a constructed timeline; `FO = 1 + NT / IT` equals the hand value.
- Blind, quick and informed classification at the boundary of `min_view_s`.
- Streak detection: a run of 8 fast approvals flags; 7 does not; slow approvals do not.
- Interruption classification: related versus unrelated; arrival during a decision versus between.
- Error reference: a scripted item approved is a false approval; the counterfactual check on a
  small truth log matches the hand result.
- Determinism: two runs of the tool on one log give identical bytes; an altered log fails the hash
  chain.
- Missing console events: a log without view events still produces latency-based measures and
  flags the others as unavailable.
- Scale: a 10-million-event log processes in under a minute and bounded memory (stream the log).
- Card hash: the hash of a card computed by replay from check results equals the logged one.

## 10. Open questions

1. Is `min_view_s` too crude to call a decision informed? It could be replaced by a calibration
   from pilot sessions (how long people who then answer correctly look). Keep it, label it a proxy,
   and calibrate in the first human pilots.
2. What counts as "attended" for NT when the operator watches a map with a hundred vehicles on it?
   Selecting a vehicle is clear; scanning is not. The draft counts only selections and acts.
3. Should the saturation threshold of 0.7 come from the literature or from the first scripted-operator
   curve? The draft uses 0.7 as a placeholder.
4. Counterfactual error needs the truth log joined to the audit log by time. Which component
   owns that join: the replay tool or a separate evaluation tool in `eval/`?
5. Whether to embed an OpenMATB-style secondary task in the console, given the licence uncertainty
   and the risk that it changes the operator's behaviour.
6. How to treat group orders: one decision covering many vehicles is one item here, but it carries
   more risk per decision. A weighted measure (decisions weighted by vehicles covered) may be
   needed.
7. Citations. Olsen and Goodrich 2003 (Metrics for evaluating human-robot interactions, PerMIS),
   Olsen and Wood 2004 (Fan-out, CHI), Crandall and colleagues 2005 (IEEE Trans. SMC-A),
   Cummings and Guerlain 2007 (Human Factors), Cummings and Mitchell 2008 (IEEE Trans. SMC-A),
   Comstock and Arnegard 1992 (NASA TM 104174), Santiago-Espada and colleagues 2011 (NASA TM
   2011-217164), Cegarra and colleagues 2020 (Behavior Research Methods), Hart and Staveland 1988,
   Parasuraman and Manzey 2010 (Human Factors), Bainbridge 1983 (Automatica), Sheridan and Verplank
   1978, Parasuraman, Sheridan and Wickens 2000 (IEEE Trans. SMC-A). These are given from memory;
   check each before citing.
