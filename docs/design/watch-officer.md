# The AI watch officer: triage before the person

Status: design draft, 2026-11-26. The maintainer's direction: deterministic code raises the
alerts and holds; the AI watch officer triages them; only what it cannot confidently handle
reaches the person's Inbox, with its recommendation; the person audits everything else in the
order log. Section 11 is the build order.

## 1. Purpose

FleetMesh asks whether human judgment over consequential decisions stays meaningful as one
person supervises more and more vehicles. Under rules 2 to 6 the answer so far is management by
exception: routine deliveries run with no person, automation waits out what it can, and the rest
reaches the person as an Inbox item (a held order, or an issue alert). At 54 vehicles that is
already a steady stream; at thousands it is more than one person can read.

The watch officer is the second filter. It reads every item the engine and automation raise,
handles the ones it can within fixed limits, and passes on the rest with a recommendation. The
person sees fewer items, each with a pre-read. The cost to measure (phase 8) is the officer's
misses: what it handled that a person should have decided.

This changes what reaches the person; it does not change who may approve. The officer never
approves anything.

## 2. The flow

```
rules and policy (fleetmesh-decide)      automation (delivery_rt.rs)
  held order: tier "human"                 issue: drop_not_clear, delivery_failed, ...
            \                                /
             +--------- triage queue -------+
                            |
                    the watch officer
             /              |                 \
        handled         escalated            not triaged in time
   (logged, in the    (Inbox, with the      (Inbox, marked untriaged:
    order log only)    officer's note)       the deadline is deterministic)
```

Every item a person would have seen enters the triage queue first. The officer takes it, then
either handles it (section 4), or escalates it with a recommendation. An item the officer has not
triaged by its deadline goes to the Inbox anyway (section 5). With no officer on the run (the
menu's AI: none), there is no triage queue and the Inbox works as it does today; the two are the
arms phase 8 compares.

## 3. What the officer may handle

A deviation is the officer's to handle only when it passes all four tests; failing any one sends
it to the person:

1. **The response is already decided.** The policy or a standing instruction covers the case, or
   the answer is the conservative default (hold, keep the parcel, return to the hub). Handling it
   carries out a decision already made.
2. **It is cheap to undo.** A parcel delayed is recoverable; a boat over a shoal at low water,
   a release over people, a vehicle landed off-site are not.
3. **It accepts no new risk**, for the fleet or for anyone outside it. A held order is the
   engine saying this order crosses the line the policy pre-accepted; going ahead with it is
   someone accepting that risk, which is the person's.
4. **Its inputs can be trusted.** With navigation untrusted, a spoof suspected, sensors that
   disagree or a degraded traffic picture, the officer reasons from the same bad data, so its
   confidence is worth nothing.

The policy sets the most the officer may handle (which item kinds and reason codes it may take at
all, section 6). The officer's own confidence can only lower that, never raise it: an officer
unsure of an item escalates it.

How the current items sort (to be settled per reason code in the policy file):

| The officer handles | The person decides |
| --- | --- |
| `drop_not_clear`, `delivery_failed`: a retry later, or another approved delivery point | `TIDE_SHALLOW`, `TIDE_DOCK_LEVEL`: send the boat or wait |
| `parcel_no_vehicle`: wait, or offer the parcel again | `ENERGY_MARGIN_THIN` on an urgent parcel: service against safety |
| many alerts with one cause (a tower out, a gust front): one incident | `nav_monitor`, a suspected spoof or jam: the inputs are in doubt |
| a traffic conflict automation is already waiting out | `traffic_noncooperative_near`: risk to people outside the fleet |
| a held order it rejects or replaces with a safer one (section 4) | a `land_now` away from a hub or landing area; an `abort` |
| | anything where rules conflict, or its own proposal was refused |

## 4. What the officer may do

- **Act through orders.** It proposes orders (`propose_intent`, as today), which go through the
  rules and the policy like anyone's: an order in an auto tier runs, one in a human tier waits for
  the person like any agent's proposal.
- **Group.** It turns many held orders with one cause into one group order (one decision covering
  many vehicles: the 30 drones held for gusts become "return all 30 to the hub"), which reaches
  the person as one item. A group order is one ordinary order naming several vehicles
  (`Engine::grouping`, rules 34); MCP's `make_group` only makes the console's groups and is not
  this.
- **Turn down the dispatcher's held order** that adds exposure (a `deliver`, a `pickup`, a
  `reroute`), with its reason, optionally proposing the safer order that replaces it. The officer
  gains no authority over another's order: the dispatcher (the bridge's own automation, agent
  `dispatcher`) withdraws its own proposal, which it may already do (`AgentPort::withdraw`, owner
  only), with the officer's reason, and holds the parcel back from offering the same order again
  until something material changes. Only the dispatcher's orders, never an operator's or another
  agent's, and never a safety order (`land_now`, `abort`, `return_to_hub`, `hold`): withdrawing
  one can raise exposure, so those always reach the person. The policy grants it per kind and
  reason code (section 6).
- **Mark an issue handled**, with what it did (the orders it proposed, or "no action, because")
  and why.
- **Escalate**, with a recommendation the person can check in seconds: bottom line first, the
  facts with their ages, what it would do and why it did not.
- **Never approve.** The engine refuses an approval from any agent, as it does today.

## 5. When the officer is slow, stalled or gone

The triage deadline is deterministic and the bridge's, never the officer's: an item not triaged
within `triage_deadline_s` of entering the queue (a policy value; a first draft of 60 s for an
issue, 30 s for a held order, less than `approval.expire_s`) goes to the Inbox marked
"untriaged". The same holds for every item at once when the officer's session drops, its budget
is spent or it hits a usage limit: nothing waits on a silent officer. An urgent item (a safety
order, a high-severity alert) bypasses triage and goes straight to the person.

## 6. The policy

The policy file says what the officer may take, so the limit is the maintainer's, logged at the
start of every run and replayed like every other routing decision. A draft shape:

```toml
[officer]
enabled = true                 # with no officer on the run, nothing is queued for triage
triage_deadline_s = { issue = 60, held = 30 }
# the issues it may handle (the rest escalate untouched)
issues = ["drop_not_clear", "delivery_failed", "parcel_no_vehicle", "return_failed", "traffic_conflict"]
# held orders it may reject or replace, by kind and reason code; never a safety kind
reject = { kinds = ["deliver", "pickup", "reroute"], codes = ["ENERGY_MARGIN_THIN", "PAYLOAD_NEAR_MAX", "WX_GUST_LIMIT"] }
# straight to the person, never queued
bypass = { kinds = ["land_now", "abort"], severity = ["high"] }
```

## 7. What the person sees

- **The Inbox** holds only what was escalated or not triaged in time. An escalated card leads
  with the officer's recommendation (bottom line, facts with ages), then the engine's checks as
  today. An untriaged card says so.
- **The order log** lists every order and every triaged item, with a filter for "handled by the
  officer": what the item was, what the officer did (the orders it proposed or had withdrawn, with
  their ids), its reason, and the time it took. That is the person's audit, by hand, whenever they
  choose. Nothing the officer handled is sampled into the Inbox (the maintainer's choice); phase 8
  measures the officer's misses against the scenarios' reference decisions. Today the Inbox's
  Decisions tab shows only the latest 50 orders (`PUSH_INTENTS`) with no filter, and the Log is
  the command log of dispatched orders, with no proposer: the audit view is new work (a
  `GET /api/triage` history and the full `GET /api/intents`, filtered by who handled what).
- **The Officer drawer** (TaskingPanel) stays the person's channel to the officer: requests, and
  its answers.

## 8. The log and replay

Each triage step is a logged event, so replay re-derives what reached the person and when:
the item entering the queue, the officer's verdict (handled, escalated, turned down) with its
reason, and the deadline passing. Alerts are derived from state on every read and never logged
today (only an issue's raising is, as a `delivery.event`), so the bridge keeps a triage record of
its own, keyed by the alert's id (stable while its condition lasts) or the order's id. The routing (queued, bypassed, deadline) is deterministic
from the logged policy and the logged times; the officer's verdicts are inputs, logged by value,
as an operator's decisions are. The behaviour is gated on a named constant
(`OFFICER_TRIAGE_SINCE`) and replay learns it in the same change (docs/design/rules-versioning.md).

## 9. The officer's side

`agents/watch_officer.py` as ported, with:

- **Its wake table** (`agents/watch/wake.py`) extended to the triage queue: a new item wakes it;
  items that arrive together are given to it together, so it can see one cause behind many.
- **Its prompt** (`agents/watch/prompt.md`) given section 3's tests, the handle-or-escalate duty,
  and the shape of an escalation.
- **New tools** for the verdicts (handle an issue, escalate an item, reject or replace a held
  order), each refused by the bridge outside what the policy grants.
- **Its model:** the routine model triages; the stronger one takes what the wake table escalates
  (a suspected spoof), as today.

It runs on the person's own Claude Code or Codex login, as before; FleetMesh never sees the
credentials.

## 10. Measurement (phase 8)

The two arms on the same scenario and seed: the person alone (AI: none), and the person with the
officer. At each fleet size:

- items reaching the person, per hour, and the attention cost of each;
- items the officer handled, and its misses: handled items whose reference decision
  (`reference.toml`) was the person's, or whose outcome went wrong;
- time from an item entering the queue to the person seeing it, and how many hit the deadline;
- the person's decisions on escalated items against the reference.

## 11. Build order

Step 1 is built (rules 7, 2026-11-26), with the MCP tool `triage_item` for `handled` and
`escalated`; `turned_down` is refused until step 2 gives the dispatcher its side.

1. Policy `[officer]` and the triage queue in the bridge (queue, deadline, bypass), logged, with
   replay, behind `OFFICER_TRIAGE_SINCE`; Inbox filtering. With no officer, no change.
2. The officer's verdict tools over MCP (handle, escalate, reject, replace), refused outside the
   policy; the engine's narrow reject authority.
3. The officer: wake table, prompt, tools.
4. The launcher: an AI choice for each scenario (Claude, Codex, none), starting the officer
   against the scenario's bridge.
5. The console: the escalated card, the untriaged mark, the order log's officer filter.
6. Phase 8 measures for both arms.

## 12. Open questions

1. Which reason codes the officer may reject at first. The table in section 3 is a starting
   point; the policy file is where it is decided.
2. Whether the officer may reject the operator's own held submissions, or only agents' (the
   dispatcher's). The draft: agents' only.
3. Whether a grouped order counts as one item or as many for the attention measure
   (attention-cost.md, open question 6).
4. The deadlines: long enough for a model turn (tens of seconds with tools), short enough that a
   held order does not expire waiting.
