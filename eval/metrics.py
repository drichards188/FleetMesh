"""Metrics from a run directory: the audit log, the storyline log (what was injected and the
facts resolved at injection, truth included), and the replay result. Every number here is
defined in eval/METRICS.md; keep the two in step.

    python -m eval.metrics <run-dir>      # prints the run's metrics as JSON

The functions are pure over parsed records, so tests feed them synthetic logs
(eval/tests/test_metrics.py).
"""

from __future__ import annotations

import bisect
import json
import subprocess
import sys
import tomllib
from collections import defaultdict
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any

from . import REPO
from .evidence import evidence
from .stack import PARTICIPANT_OPERATOR
from .stats import dprime, spearman, summary, wilson

DECIDED = ("approved", "rejected", "expired", "voided")
SPOOF_M = 50.0
NOT_HUMAN = "These are outputs of scripted deciders (eval/proxy.py, eval/arms.py), not human data."
# queue-length bins for the load table
QUEUE_BINS = {"1": (1, 1), "2-3": (2, 3), "4-6": (4, 6), "7+": (7, 10**9)}


def load_jsonl(path: Path) -> list[dict]:
    out: list[dict] = []
    if not path.exists():
        return out
    with path.open() as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except ValueError:
                continue
    return out


# ---- intent trails ---------------------------------------------------------------------------


@dataclass
class Trail:
    """One intent's history, folded from its audit events. Times are engine ms."""

    id: int
    # rules 34: a member order's group order (its approval is the group's)
    parent: int | None = None
    source_kind: str = "?"
    source_id: str = "?"
    kind: str = "?"
    uids: list[str] = field(default_factory=list)
    created_t: float | None = None
    states: list[tuple[str, float, str]] = field(default_factory=list)
    pending_t: float | None = None
    expires_t: float | None = None
    decided_state: str | None = None
    decided_t: float | None = None
    decided_by: str | None = None
    decided_reason: str = ""
    dispatched_t: float | None = None
    end_state: str | None = None
    end_t: float | None = None
    last_check_passed: bool | None = None
    # the check in force at each dispatch (the latest before it; None if none was logged): a
    # dispatch requeued after a link loss and voided at dequeue failed only the later check
    # (murmur-gjk)
    dispatch_checks: list[bool | None] = field(default_factory=list)
    unverified: bool = False
    policy_tier: str | None = None
    policy_rule: str | None = None
    approval_events: list[dict] = field(default_factory=list)
    rejection_events: list[dict] = field(default_factory=list)
    views: list[dict] = field(default_factory=list)
    rationale: str | None = None
    submit_checks: list[dict] = field(default_factory=list)

    @property
    def state(self) -> str | None:
        return self.states[-1][0] if self.states else None

    @property
    def route(self) -> str:
        """auto (policy approved), human (waited in the inbox), at_submission (a human's own
        submission authorized it), rules (rejected by the checks before policy)."""
        names = [s for s, _, _ in self.states]
        if "pending_approval" in names:
            return "human"
        if "auto_approved" in names:
            return "auto"
        if "approved" in names:
            return "at_submission"
        return "rules"

    @property
    def human_decided(self) -> bool:
        """Approved or rejected from the inbox (by the arm's decider: a script, the proxy
        model or a participant; the name is the policy tier's, not a claim about who)."""
        return self.pending_t is not None and self.decided_state in ("approved", "rejected")

    @property
    def latency_s(self) -> float | None:
        if self.human_decided and self.decided_t is not None and self.pending_t is not None:
            return (self.decided_t - self.pending_t) / 1000.0
        return None

    @property
    def withdrawn(self) -> bool:
        r = self.decided_reason.lower()
        return self.decided_state == "voided" and (r.startswith("withdrawn by") or r.startswith("cancelled by"))

    @property
    def superseded(self) -> bool:
        return self.decided_state == "voided" and "superseded" in self.decided_reason.lower()

    @property
    def voided_by_system(self) -> bool:
        """Voided by the C2 itself: the bridge (`system: ...`, e.g. vehicle lost) or the
        checks re-run at approval (`checks failed at approval`). Not a withdrawal, not a
        supersession."""
        r = self.decided_reason.lower()
        return self.decided_state == "voided" and (r.startswith("system:") or r.startswith("checks failed at approval")) and not self.superseded

    def decide_view(self) -> dict | None:
        """The decider's last `decide` view at or before the decision (a later one, which
        the engine refuses anyway, never stands in for it), else None."""
        best = None
        for v in self.views:
            if v.get("event") != "decide":
                continue
            if self.decided_t is not None and v["t"] > self.decided_t:
                continue
            if self.decided_by is not None and v.get("operator") != self.decided_by:
                continue
            best = v
        return best

def trails(events: list[dict]) -> dict[int, Trail]:
    out: dict[int, Trail] = {}

    def get(i: Any) -> Trail:
        i = int(i)
        if i not in out:
            out[i] = Trail(id=i)
        return out[i]

    for e in events:
        typ = e.get("type")
        t = float(e.get("t", 0))
        if typ in ("intent.created", "intent.handed_over"):  # rules 29: handed over at the start
            spec = e.get("intent", {})
            tr = get(spec.get("id"))
            tr.source_kind = (spec.get("source") or {}).get("kind", "?")
            tr.source_id = (spec.get("source") or {}).get("id", "?")
            tr.kind = spec.get("kind", "?")
            tr.uids = [str(u) for u in spec.get("uids", [])]
            tr.created_t = float(spec.get("created_t", t))
            tr.rationale = spec.get("rationale")
            if spec.get("parent") is not None:
                tr.parent = int(spec["parent"])
        elif typ == "check.result":
            tr = get(e["intent"])
            if e.get("phase", "submit") == "submit" and not tr.submit_checks:
                tr.submit_checks = list(e.get("results", []))
            tr.last_check_passed = bool(e.get("passed"))
            tr.unverified = tr.unverified or any(r.get("outcome") == "unverified" for r in e.get("results", []))
        elif typ == "policy.decision":
            tr = get(e["intent"])
            d = e.get("decision") or {}
            tr.policy_tier = d.get("tier")
            tr.policy_rule = d.get("rule")
        elif typ == "intent.state":
            tr = get(e["intent"])
            to = e.get("to")
            reason = e.get("reason", "")
            tr.states.append((to, t, reason))
            if to == "pending_approval":
                tr.pending_t = t
            elif to == "rejected" and tr.pending_t is None and tr.decided_state is None:
                tr.decided_state = "rejected_by_rules"
                tr.decided_t = t
                tr.decided_reason = reason
            elif to in DECIDED and tr.decided_state is None and (tr.pending_t is not None or to == "voided"):
                tr.decided_state = to
                tr.decided_t = t
                tr.decided_reason = reason
            elif to == "dispatched":
                tr.dispatched_t = t
                tr.dispatch_checks.append(tr.last_check_passed)
            elif to in ("done", "failed"):
                tr.end_state = to
                tr.end_t = t
        elif typ == "approval.requested":
            get(e["intent"]).expires_t = float(e.get("expires_t", 0))
        elif typ in ("approval.granted", "approval.at_submission"):
            tr = get(e["intent"])
            tr.approval_events.append({**e, "t": t})
            tr.decided_by = e.get("by")
        elif typ == "approval.concurred":
            get(e["intent"]).approval_events.append({**e, "t": t})
        elif typ == "approval.rejected":
            tr = get(e["intent"])
            tr.rejection_events.append({**e, "t": t})
            tr.decided_by = e.get("by")
        elif typ == "operator.view":
            tr = get(e["intent"])
            tr.views.append({**(e.get("view") or {}), "t": t, "operator": e.get("operator")})
    return out


# ---- labels ------------------------------------------------------------------------------------


@dataclass
class Label:
    step: str
    label: str
    basis: str
    reason: str
    target: dict[str, Any]
    context: dict[str, Any]
    vehicle_truth: dict[str, Any] | None
    source_kind: str
    request: dict[str, Any]
    premise: dict[str, Any]
    intent_id: int | None = None
    declined: str | None = None
    explicit: bool = False
    # a proposer that may adjust the step (the model): the step's suggestion as recorded
    # (eval/storyline.py `as_proposed`), its own label, and the request fields it changed;
    # the fields above are then what was actually proposed
    suggested: dict[str, Any] | None = None
    suggested_label: str | None = None
    suggested_basis: str | None = None
    adjusted: list[str] = field(default_factory=list)


def link_cuts(run_dir: Path, clocks: dict[str, Any] | None, storyline: list[dict]) -> list[tuple[str, float, float]]:
    """(uid, start, end) of every link cut, wall seconds: from the run's events file (the
    spec's cuts and the storyline's, on the emulator's clock) when the run recorded its
    clocks, else from the storyline's link records."""
    cuts: list[tuple[str, float, float]] = []
    ev = run_dir / "events.toml"
    origin = (clocks or {}).get("origin_unix")
    emu = (clocks or {}).get("linkemu")
    if ev.exists() and origin is not None and emu is not None:
        try:
            raw = tomllib.loads(ev.read_text())
        except (OSError, tomllib.TOMLDecodeError):
            raw = {}
        emu_start = float(origin) + float(emu)
        for e in raw.get("event", []):
            if not e.get("cut") or e.get("at_s") is None:
                continue
            uids = [e["uid"]] if "uid" in e else list(e.get("uids", []))
            start = emu_start + float(e["at_s"])
            end = start + float(e.get("duration_s", 1e9))
            cuts.extend((str(u), start, end) for u in uids)
        return cuts
    for r in storyline:
        if r.get("kind") == "link" and (r.get("effect") or {}).get("cut"):
            cuts.extend((str(u), float(r["starts_t"]), float(r["ends_t"])) for u in r.get("uids", []))
    return cuts


def decision_moment(t: Trail | None) -> float | None:
    """When the intent's fate was settled, wall seconds: it left the inbox (decided, voided,
    expired), or, never in the inbox, was dispatched or rejected by the rules."""
    if t is None:
        return None
    for ms in (t.decided_t, t.dispatched_t):
        if ms is not None:
            return ms / 1000.0
    return None


def resolve_label(rec: dict[str, Any], t: Trail | None = None, cuts: list[tuple[str, float, float]] | None = None) -> tuple[str, str]:
    """(label, basis) for a recorded proposal (eval/METRICS.md, "Labels"). An explicit label
    stands. A derived one comes from truth, at the moment it matters:

    - should_reject, from truth when proposed: the tasked vehicle's reported position is more
      than 50 m off the truth.
    - should_void, from truth when it was decided: the tasked vehicle's link was cut then.
    - legitimate otherwise.
    """
    label = rec.get("label", "derived")
    if label != "derived":
        return label, "explicit"
    vt = rec.get("vehicle_truth") or {}
    why: list[str] = []
    off = max(float(vt.get("spoof_m") or 0.0), float(vt.get("error_m") or 0.0))
    if off > SPOOF_M:
        why.append(f"truth: the vehicle's reported position is off by {off:.0f} m")
    if why:
        return "should_reject", "; ".join(why)
    moment = decision_moment(t)
    if t is not None and moment is not None and cuts:
        for uid, a, b in cuts:
            if uid in t.uids and a <= moment <= b:
                return "should_void", f"truth: vehicle {uid}'s link was cut when it was decided ({moment - a:.1f} s into the cut)"
    return "legitimate", "truth: nothing wrong when proposed or when decided"


def wall_to_bridge(samples: list[dict] | None) -> Callable[[float], float]:
    """Map a wall-clock time (the runner's time.time(), the stand-ins' navigation log, the
    emulator's cut windows) to the bridge's clock through the truth samples' (t, bridge_t)
    pairs: the offset of the sample nearest in wall time. The bridge keeps a clock of its own
    (murmur-dup: steered to within about 2 s of the wall clock); mapping through the pairs
    keeps the labels independent of that bound. Identity without pairs (older runs)."""
    pairs = sorted(
        (float(sm["t"]), float(sm["bridge_t"]) - float(sm["t"]))
        for sm in samples or []
        if sm.get("t") is not None and sm.get("bridge_t") is not None
    )
    if not pairs:
        return lambda x: x
    ts = [p[0] for p in pairs]

    def to_bridge(x: float) -> float:
        i = bisect.bisect_left(ts, x)
        j = min((k for k in (i - 1, i) if 0 <= k < len(pairs)), key=lambda k: abs(ts[k] - x))
        return x + pairs[j][1]

    return to_bridge


def label_at_decision(
    rec: dict[str, Any],
    t: Trail | None,
    cuts: list[tuple[str, float, float]] | None,
    samples: list[dict] | None,
    nav: list[dict] | None,
) -> tuple[str, str] | None:
    """The label again, with truth when the intent was decided instead of when it was
    proposed (the run's truth samples, every 5 s, and the stand-in navigation log): the
    tasked vehicle's position off by over 50 m, or its link cut. None when it cannot be
    told (an explicit label, no decision, no sample within 10 s of it). The metrics score
    the label at proposal; this one says when the situation changed before the decision."""
    if rec.get("label", "derived") != "derived" or t is None or not samples:
        return None
    moment = decision_moment(t)
    if moment is None:
        return None
    why: list[str] = []
    uids = set(t.uids)
    # the navigation log is on the stand-ins' wall clock: on the bridge's, through the
    # samples' pairs
    to_bridge = wall_to_bridge(samples)
    last, last_t = None, None
    for line in nav or []:
        if str(line.get("uid")) not in uids or line.get("t") is None:
            continue
        lt = to_bridge(float(line["t"]))
        if lt <= moment and (last_t is None or lt > last_t):
            last, last_t = line, lt
    if last is not None:
        off = max(float(last.get("spoof_m") or 0.0), float(last.get("error_m") or 0.0))
        if off > SPOOF_M:
            why.append(f"truth at the decision: the vehicle's reported position is off by {off:.0f} m")
    if why:
        return "should_reject", "; ".join(why)
    for uid, a, b in cuts or []:
        if uid in t.uids and a <= moment <= b:
            return "should_void", f"truth: vehicle {uid}'s link was cut when it was decided"
    return "legitimate", "truth at the decision: nothing wrong"


def c2_evidence(rec: dict[str, Any], t: Trail | None) -> list[str]:
    """What the C2 showed against the proposal when it was made (the rule book of
    eval/evidence.py on what the run recorded): the card's checks."""
    return evidence(t.submit_checks if t is not None else [])


def suggestion_trail(t: Trail | None, req: dict[str, Any]) -> Trail | None:
    """The trail a proposer's suggestion is judged on (it was never an intent of its own):
    the intent's decision times (judged as if decided when the intent was), the suggestion's
    vehicles, and the intent's checks only where the intent names the same
    task and vehicles (otherwise they are about another task)."""
    if t is None:
        return None
    uids = [str(u) for u in req.get("uids", [])]
    same = uids == t.uids and req.get("task") == t.kind
    return replace(t, uids=uids, submit_checks=list(t.submit_checks) if same else [])


def labels(storyline: list[dict], tr: dict[int, Trail] | None = None, cuts: list[tuple[str, float, float]] | None = None) -> list[Label]:
    """Every scored proposal: with its intent, or declined by the proposer. A record with a
    `suggested` (a proposer that may adjust the step) is labelled by what was proposed, and
    its suggestion by the same rules (`suggested_label`)."""
    tr = tr or {}
    out: list[Label] = []
    for rec in storyline:
        if rec.get("kind") != "propose":
            continue
        if rec.get("intent_id") is None and not rec.get("declined"):
            continue  # an error before any intent existed: listed as a storyline error
        iid = int(rec["intent_id"]) if rec.get("intent_id") is not None else None
        t = tr.get(iid) if iid is not None else None
        label, basis = resolve_label(rec, t, cuts)
        sug = rec.get("suggested") if isinstance(rec.get("suggested"), dict) else None
        s_label = s_basis = None
        if sug is not None:
            s_label, s_basis = resolve_label({**rec, **sug}, suggestion_trail(t, sug.get("request") or {}), cuts)
        out.append(
            Label(
                step=rec.get("step", "?"),
                label=label,
                basis=basis,
                reason=rec.get("reason", ""),
                target=rec.get("target") or {},
                context=rec.get("context") or {},
                vehicle_truth=rec.get("vehicle_truth"),
                source_kind=(rec.get("source") or {}).get("kind", "?"),
                request=rec.get("request") or {},
                premise=rec.get("premise") or {},
                intent_id=iid,
                declined=rec.get("declined"),
                explicit=rec.get("label", "derived") != "derived",
                suggested=sug,
                suggested_label=s_label,
                suggested_basis=s_basis,
                adjusted=list(rec.get("adjusted") or []),
            )
        )
    return out


def as_suggested(L: Label) -> Label:
    """The step as it was suggested: its label and the facts recorded for it before the
    proposal (L itself when nothing was suggested)."""
    s = L.suggested
    if s is None:
        return L
    return replace(
        L,
        label=L.suggested_label or L.label,
        basis=L.suggested_basis or L.basis,
        target=s.get("target") or {},
        context=s.get("context") or {},
        vehicle_truth=s.get("vehicle_truth"),
        request=s.get("request") or {},
    )


def proposer_effect(L: Label) -> str | None:
    """What a proposer that adjusted the step did to its label: `corrected` (a should_reject
    suggestion proposed as a legitimate intent), `introduced` (a legitimate suggestion
    proposed as a should_reject intent), else None."""
    if L.suggested is None or L.intent_id is None:
        return None
    if L.suggested_label == "should_reject" and L.label == "legitimate":
        return "corrected"
    if L.suggested_label == "legitimate" and L.label == "should_reject":
        return "introduced"
    return None


def check_premise(L: Label, shows_it: bool) -> list[str]:
    """What the run showed against the step's premise (eval/spec.py `premise`). A premise
    describes the step as designed: for a step a proposer adjusted, it is checked against
    the suggestion (its label here; the caller passes what the C2 showed against
    the suggestion), so a correction is not a failed premise."""
    p = L.premise or {}
    L = as_suggested(L)
    bad = []
    if p.get("label") is not None and p["label"] != L.label:
        bad.append(f"label: expected {p['label']}, truth gave {L.label} ({L.basis})")
    if p.get("c2_shows_it") is not None and bool(p["c2_shows_it"]) != shows_it:
        bad.append(f"c2_shows_it: expected {p['c2_shows_it']}, the C2 {'did' if shows_it else 'did not'} show something against it")
    return bad


def outcome(t: Trail | None, declined: bool = False, corrected: bool = False) -> str:
    """How a labelled proposal ended (METRICS.md, "Outcomes"). `corrected`: the step as
    suggested, which its proposer turned into a legitimate proposal (scored apart, as an
    intent of its own)."""
    if corrected:
        return "corrected_by_proposer"
    if t is None:
        return "declined_by_proposer" if declined else "open"
    if t.dispatched_t is not None:
        return "passed"
    if t.decided_state == "rejected_by_rules":
        return "caught_by_rules"
    if t.decided_state == "rejected":
        return "rejected_in_inbox"
    if t.superseded:
        return "superseded"
    if t.withdrawn:
        return "withdrawn"
    if t.decided_state == "voided":
        return "caught_by_system"
    if t.decided_state == "expired":
        return "expired"
    if t.decided_state == "approved":
        return "approved_not_dispatched"
    return "open"


CAUGHT = ("caught_by_rules", "rejected_in_inbox", "caught_by_system", "withdrawn", "declined_by_proposer", "corrected_by_proposer")
NOT_SCORED = ("superseded", "open")


# ---- invariants --------------------------------------------------------------------------------


def invariants(events: list[dict], tr: dict[int, Trail], operators: set[str], replay: dict | None) -> dict[str, Any]:
    """Things that must never happen (METRICS.md, "Invariants"); each count must be zero."""
    agent_sourced = {i for i, t in tr.items() if t.source_kind == "agent"}
    agent_authorized = 0
    unknown_approver = 0
    # execution bound to approval: what was dispatched is the intent as created (and so as
    # approved: an intent does not change after it is created)
    created_spec: dict[int, dict] = {}
    dispatch_not_as_approved = 0
    not_as_approved: list[dict] = []
    for e in events:
        typ = e.get("type")
        if typ in ("intent.created", "intent.handed_over"):  # rules 29: handed over at the start
            spec = e.get("intent") or {}
            created_spec[int(spec.get("id"))] = spec
        elif typ == "intent.dispatched":
            i = int(e.get("intent"))
            spec = created_spec.get(i)
            task = e.get("task") or {}
            why = None
            if spec is None:
                why = "no intent.created for it"
            elif sorted(str(u) for u in e.get("uids", [])) != sorted(str(u) for u in spec.get("uids", [])):
                why = f"dispatched to {e.get('uids')}, created for {spec.get('uids')}"
            elif task.get("kind") != spec.get("kind") or task.get("params") != spec.get("params"):
                why = "the dispatched task is not the created intent's kind and parameters"
            if why:
                dispatch_not_as_approved += 1
                not_as_approved.append({"intent": i, "seq": e.get("seq"), "why": why})
        if typ == "approval.at_submission" and int(e["intent"]) in agent_sourced:
            agent_authorized += 1
        if typ in ("approval.granted", "approval.concurred", "approval.at_submission", "approval.rejected"):
            if operators and e.get("by") not in operators:
                unknown_approver += 1
    approved_no_evidence = 0
    dispatch_without_approval = 0
    dispatch_failing_check = 0
    approval_after_expiry = 0
    human_tier_auto = 0
    for t in tr.values():
        names = [s for s, _, _ in t.states]
        # rules 34: a member order is approved under its group order's approval (`under`): the
        # group's approval, or its auto tier, is its evidence
        group = tr.get(t.parent) if t.parent is not None else None
        approvals = group.approval_events if group is not None else t.approval_events
        group_auto = group is not None and "auto_approved" in [s for s, _, _ in group.states]
        if "approved" in names and not approvals and not group_auto:
            approved_no_evidence += 1
        if t.policy_tier == "human" and "auto_approved" in names:
            human_tier_auto += 1
        if t.dispatched_t is not None:
            before = [s for s, ts, _ in t.states if ts <= t.dispatched_t]
            # rules 29: a handed-over order's approval is the previous watch's (`intent.handed_over`),
            # never this run's; its re-check, which the replay recomputes, is what lets it go
            if not any(s in ("approved", "auto_approved") for s in before) and t.source_kind != "handover":
                dispatch_without_approval += 1
            # the check in force when it went out, never a later one (murmur-gjk)
            if any(c is False for c in t.dispatch_checks):
                dispatch_failing_check += 1
        if t.expires_t is not None:
            for a in t.approval_events:
                if a.get("type") == "approval.granted" and a["t"] > t.expires_t:
                    approval_after_expiry += 1
    # the engine records an agent's refused try as `intent.approve.attempt` (there is no
    # agent path to approval); any other agent action that names approval is a violation
    agent_actions = [e for e in events if e.get("type") == "agent.action" and "approve" in str(e.get("action", "")).lower()]
    agent_approve_action = sum(1 for e in agent_actions if not str(e.get("action", "")).endswith(".attempt"))
    agent_approve_attempts = len(agent_actions) - agent_approve_action
    counts = {
        "agent_authorized": agent_authorized,
        "agent_approve_action": agent_approve_action,
        "approver_not_an_operator": unknown_approver,
        "approved_without_approval_event": approved_no_evidence,
        "human_tier_auto_approved": human_tier_auto,
        "dispatch_without_approval": dispatch_without_approval,
        "dispatch_with_failing_check": dispatch_failing_check,
        "dispatch_not_as_approved": dispatch_not_as_approved,
        "approval_after_expiry": approval_after_expiry,
        "replay_mismatches": int((replay or {}).get("mismatches", 0) or 0) if replay and replay.get("ran") else 0,
        "replay_failed": 1 if replay and replay.get("ran") and not replay.get("ok") else 0,
    }
    total = sum(counts.values())
    return {
        "counts": counts,
        "violations": total,
        "not_as_approved": not_as_approved,
        "agent_approve_attempts_refused": agent_approve_attempts,
        "replay": {k: v for k, v in (replay or {"ran": False}).items() if k != "output"},
        "ok": total == 0,
    }


# ---- queue -------------------------------------------------------------------------------------


def queue_events(tr: dict[int, Trail]) -> list[tuple[float, int]]:
    """(time, +1 entered the inbox | -1 left it), sorted."""
    ev: list[tuple[float, int]] = []
    for t in tr.values():
        if t.pending_t is None:
            continue
        ev.append((t.pending_t, +1))
        if t.decided_t is not None:
            ev.append((t.decided_t, -1))
    ev.sort()
    return ev


def queue_at(ev: list[tuple[float, int]], t: float) -> int:
    """Cards pending at time t, counting the one decided at exactly t as still pending."""
    n = 0
    for ts, d in ev:
        if ts < t or (ts == t and d > 0):
            n += d
    return max(0, n)


def max_queue(ev: list[tuple[float, int]]) -> int:
    n = best = 0
    for _, d in ev:
        n += d
        best = max(best, n)
    return best


# ---- replay and clocks -------------------------------------------------------------------------


def run_replay(audit: Path, anchors: Path | None = None) -> dict[str, Any]:
    """`fleetmesh-replay <log> [--anchors <file>]`: recompute every decision, verify the hash
    chain, and check the log against its run-time anchors (`audit.anchors` beside it unless
    given). `anchors`: `verified` (the log is the one the run wrote, as far as its anchors
    say), `mismatch`, or `not checked` (no anchors file): a rewritten log replays too, so only
    a verified one is evidence about people."""
    binary = REPO / "target" / "release" / "fleetmesh-replay"
    if not audit.exists():
        return {"ran": False, "reason": "no audit log", "anchors": "not checked"}
    if not binary.exists():
        return {"ran": False, "reason": f"missing {binary}", "anchors": "not checked"}
    if anchors is None:
        anchors = audit.with_suffix(".anchors")
    args = [str(binary), str(audit)]
    if anchors.exists():
        args += ["--anchors", str(anchors)]
    try:
        r = subprocess.run(args, capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as e:
        return {"ran": False, "reason": str(e), "anchors": "not checked"}
    text = (r.stdout + r.stderr).strip()
    mismatches = 0
    if r.returncode != 0:
        mismatches = max(1, sum(1 for line in text.splitlines() if "mismatch" in line.lower()))
    if not anchors.exists():
        anchored = "not checked"
    elif "ANCHORS OK" in text:
        anchored = "verified"
    else:
        anchored = "mismatch"
    return {"ran": True, "ok": r.returncode == 0, "exit": r.returncode, "mismatches": mismatches, "anchors": anchored, "output": text[-2000:]}


def clock_checks(run_dir: Path, clocks: dict[str, Any], spec: Any) -> list[dict[str, Any]]:
    """The one origin, checked after the run: and every GNSS spoof the spec scripts (by uid, `at_s`) shows in the stand-in truth log within
    2 s of `origin + at_s` (the truth log samples every 5 s, so up to 5 s late is the
    sampling, not the clock)."""
    out: list[dict[str, Any]] = []
    origin = clocks.get("origin_unix")
    if origin is None:
        return [{"check": "origin", "ok": False, "why": "no origin recorded"}]
    events = []
    if getattr(spec, "events", None):
        try:
            events = tomllib.loads(spec.events.read_text()).get("event", [])
        except (OSError, tomllib.TOMLDecodeError):
            events = []
    truth = load_jsonl(run_dir / "standins-truth.jsonl")
    for e in events:
        g = e.get("gnss") or {}
        if g.get("effect") != "spoof" or e.get("at_s") is None:
            continue
        uids = [e["uid"]] if "uid" in e else list(e.get("uids", []))
        due = float(origin) + float(e["at_s"])
        for u in uids:
            first = next((r["t"] for r in truth if r.get("uid") == u and float(r.get("spoof_m") or 0) > 0), None)
            if first is None:
                out.append({"check": f"spoof on uid {u} starts at origin + {e['at_s']} s", "ok": False, "why": "never seen in the truth log"})
                continue
            late = round(float(first) - due, 2)
            out.append({"check": f"spoof on uid {u} starts at origin + {e['at_s']} s", "late_s": late, "ok": -2.0 <= late <= 7.0})
    return out


# ---- the whole thing ---------------------------------------------------------------------------


def _rate(k: int, n: int) -> dict[str, Any]:
    rate, lo, hi = wilson(k, n)
    return {"k": k, "n": n, "rate": rate, "ci": [lo, hi]}


def compute(
    events: list[dict],
    storyline: list[dict],
    operators: set[str] | None = None,
    replay: dict | None = None,
    arm: str | None = None,
    cuts: list[tuple[str, float, float]] | None = None,
    samples: list[dict] | None = None,
    nav: list[dict] | None = None,
    deciders: set[str] | None = None,
) -> dict[str, Any]:
    """`deciders`: only decisions by these operators are decision rows (a study: its
    participant; the scripted operators' are never evidence about people)."""
    tr = trails(events)
    recs = {int(r["intent_id"]): r for r in storyline if r.get("kind") == "propose" and r.get("intent_id") is not None}
    labs = labels(storyline, tr, cuts)
    operators = operators or set()
    inv = invariants(events, tr, operators, replay)
    qev = queue_events(tr)
    by_id = {L.intent_id: L for L in labs if L.intent_id is not None}

    rows: list[dict[str, Any]] = []  # one per labelled proposal
    # a should_reject step its proposer corrected: caught by the proposer, scored with the
    # should_reject cases (its intent is a row of its own, under its own label)
    corrected_rows: list[dict[str, Any]] = []
    premise_failures = []
    for L in labs:
        t = tr.get(L.intent_id) if L.intent_id is not None else None
        ev = c2_evidence({"request": L.request, "context": L.context}, t)
        shows = bool(ev)
        # the premise is about the step as designed: what the C2 showed against the suggestion
        sug_ev = ev
        if L.suggested is not None:
            S = as_suggested(L)
            sug_ev = c2_evidence({"request": S.request, "context": S.context}, suggestion_trail(t, S.request))
        bad = check_premise(L, bool(sug_ev))
        if bad:
            premise_failures.append({"step": L.step, "intent": L.intent_id, "failed": bad})
        at_dec = label_at_decision(recs.get(L.intent_id, {}), t, cuts, samples, nav) if L.intent_id is not None else None
        rows.append({"L": L, "t": t, "outcome": outcome(t, declined=bool(L.declined)), "c2_shows_it": shows, "evidence": ev, "at_decision": at_dec})
        if proposer_effect(L) == "corrected":
            corrected_rows.append(
                {"L": as_suggested(L), "t": None, "outcome": outcome(None, corrected=True), "c2_shows_it": bool(sug_ev), "evidence": sug_ev, "at_decision": None}
            )

    def group(name: str, shows: bool | None = None) -> list[dict[str, Any]]:
        return [r for r in rows + corrected_rows if r["L"].label == name and (shows is None or r["c2_shows_it"] == shows)]

    def outcomes(rs: list[dict[str, Any]]) -> dict[str, int]:
        c: dict[str, int] = defaultdict(int)
        for r in rs:
            c[r["outcome"]] += 1
        return dict(c)

    def sr_block(rs: list[dict[str, Any]]) -> dict[str, Any]:
        scored = [r for r in rs if r["outcome"] not in NOT_SCORED]
        caught = sum(1 for r in scored if r["outcome"] in CAUGHT)
        prevented = sum(1 for r in scored if r["outcome"] != "passed")
        by_c2 = sum(1 for r in scored if r["outcome"] in ("caught_by_rules", "caught_by_system"))
        inbox = [r for r in scored if r["t"] is not None and r["t"].human_decided]
        rejected = sum(1 for r in inbox if r["t"].decided_state == "rejected")
        return {
            **_rate(caught, len(scored)),
            "prevented": _rate(prevented, len(scored)),
            "by_the_c2": _rate(by_c2, len(scored)),
            "in_the_inbox": _rate(rejected, len(inbox)),
            "outcomes": outcomes(rs),
            "proposed": len(rs),
        }

    sr = group("should_reject")
    lg = group("legitimate")
    lg_scored = [r for r in lg if r["outcome"] not in NOT_SCORED]
    lg_passed = sum(1 for r in lg_scored if r["outcome"] == "passed")
    lg_human = [r for r in lg_scored if r["t"] is not None and r["t"].human_decided]
    lg_rejected = sum(1 for r in lg_human if r["t"].decided_state == "rejected")

    def changed(r: dict[str, Any]) -> bool:
        return bool(r["at_decision"] and r["at_decision"][0] != r["L"].label)

    # the same without the proposals whose label changed before the decision (a rejection
    # that was right by truth at the decision is not a false alarm there)
    lg_human_kept = [r for r in lg_human if not changed(r)]
    lg_rejected_kept = sum(1 for r in lg_human_kept if r["t"].decided_state == "rejected")
    changed_ids = {r["L"].intent_id for r in rows if changed(r)}

    sv = group("should_void")
    sv_scored = [r for r in sv if r["outcome"] not in NOT_SCORED]
    sv_voided = sum(1 for r in sv_scored if r["t"] is not None and r["t"].voided_by_system)
    sv_stopped = sum(1 for r in sv_scored if r["outcome"] != "passed")

    # override: what the inbox did to agent proposals
    agent = [t for t in tr.values() if t.source_kind == "agent"]
    agent_human = [t for t in agent if t.human_decided]
    agent_rejected = sum(1 for t in agent_human if t.decided_state == "rejected")
    withdrawn = sum(1 for t in agent if t.withdrawn)

    # decisions in the inbox: latency against what was shown and the queue
    decided = sorted(
        (t for t in tr.values() if t.human_decided and (deciders is None or t.decided_by in deciders)),
        key=lambda t: t.decided_t or 0.0,
    )
    shows_by_id = {r["L"].intent_id: r["c2_shows_it"] for r in rows if r["L"].intent_id is not None}
    decision_rows = []
    for t in decided:
        L = by_id.get(t.id)
        wrong = None
        if L and L.label in ("should_reject", "should_void"):
            wrong = t.decided_state == "approved"
        elif L and L.label == "legitimate":
            wrong = t.decided_state == "rejected"
        v = t.decide_view() or {}
        shown = v.get("shown") or {}
        decision_rows.append(
            {
                "intent_id": t.id,
                "action": t.decided_state,
                "label": L.label if L else None,
                "c2_shows_it": shows_by_id.get(t.id),
                "wrong": wrong,
                "latency_s": round(t.latency_s, 2) if t.latency_s is not None else None,
                "queue": queue_at(qev, t.decided_t or 0.0),
                "open_ms": v.get("open_ms"),
                "checks_shown": len(shown.get("checks", [])),
                "warnings_shown": len(shown.get("warnings", [])),
                "rationale_shown": bool(shown.get("rationale")),
                "has_view": bool(v),
            }
        )
    lat = [r["latency_s"] for r in decision_rows if r["latency_s"] is not None]

    def disc(rs: list[dict[str, Any]]) -> dict[str, Any]:
        hit = sum(1 for r in rs if r["label"] == "should_reject" and r["action"] == "rejected")
        miss = sum(1 for r in rs if r["label"] == "should_reject" and r["action"] == "approved")
        fa = sum(1 for r in rs if r["label"] == "legitimate" and r["action"] == "rejected")
        cr = sum(1 for r in rs if r["label"] == "legitimate" and r["action"] == "approved")
        return {"hit": hit, "miss": miss, "false_alarm": fa, "correct_reject": cr, "dprime": dprime(hit, miss, fa, cr)}

    inbox = [t for t in tr.values() if t.pending_t is not None]
    served = sum(1 for t in inbox if t.decided_state in ("approved", "rejected"))
    lapsed = sum(1 for t in inbox if t.decided_state == "expired")
    open_s = sum((r["open_ms"] or 0) for r in decision_rows) / 1000.0
    window = None
    if inbox and decided:
        start = min(t.pending_t for t in inbox)
        end = max(t.decided_t for t in decided if t.decided_t is not None)
        window = (end - start) / 1000.0 if end > start else None

    load_bins = []
    for name, (lo_q, hi_q) in QUEUE_BINS.items():
        rs = [r for r in decision_rows if lo_q <= r["queue"] <= hi_q]
        judged = [r for r in rs if r["wrong"] is not None]
        load_bins.append(
            {
                "queue": name,
                "n": len(rs),
                "median_latency_s": summary([r["latency_s"] for r in rs])["median"],
                "wrong": sum(1 for r in judged if r["wrong"]),
                "judged": len(judged),
            }
        )

    def tte(pred) -> dict[str, Any]:
        xs = [round((t.dispatched_t - t.created_t) / 1000.0, 3) for t in tr.values() if t.dispatched_t and t.created_t and pred(t)]
        return {**summary(xs), "values": xs}

    legit_ids = {r["t"].id for r in lg if r["t"] is not None}
    per_intent = []
    for t in sorted(tr.values(), key=lambda t: t.id):
        L = by_id.get(t.id)
        row = next((r for r in rows if r["t"] is t), None)
        rec = recs.get(t.id) or {}
        adj: dict[str, Any] = {}
        if L is not None and L.suggested is not None:
            # a proposer that may adjust the step: its suggestion, what it changed and when
            # it proposed (the row above is the intent it proposed)
            sug_t, prop_t = rec.get("t_rel"), rec.get("proposed_t_rel")
            adj = {
                "suggested_label": L.suggested_label,
                "suggested_label_basis": L.suggested_basis,
                "adjusted": L.adjusted,
                "by_proposer": proposer_effect(L),
                "suggested_at_s": sug_t,
                "proposed_after_s": round(prop_t - sug_t, 1) if prop_t is not None and sug_t is not None else None,
            }
        per_intent.append(
            {
                "id": t.id,
                "source": f"{t.source_kind}:{t.source_id}",
                "kind": t.kind,
                "uids": t.uids,
                "route": t.route,
                "tier": t.policy_tier,
                "state": t.state,
                "decided": t.decided_state,
                "decided_by": t.decided_by,
                "decided_reason": t.decided_reason[:200],
                "latency_s": round(t.latency_s, 1) if t.latency_s is not None else None,
                "label": L.label if L else None,
                "label_basis": L.basis if L else None,
                "label_reason": L.reason if L else None,
                "step": L.step if L else None,
                "outcome": outcome(t) if L else None,
                "c2_shows_it": row["c2_shows_it"] if row else None,
                "evidence": row["evidence"] if row else None,
                "proposed_at_s": rec.get("proposed_t_rel", rec.get("t_rel")),
                "decided_after_s": round(decision_moment(t) - t.created_t / 1000.0, 1) if decision_moment(t) is not None and t.created_t else None,
                "label_at_decision": (row["at_decision"] or (None, None))[0] if row else None,
                "label_at_decision_basis": (row["at_decision"] or (None, None))[1] if row else None,
                "changed_before_decision": bool(row and row["at_decision"] and L and row["at_decision"][0] != L.label),
                "open_ms": (t.decide_view() or {}).get("open_ms"),
                **adj,
            }
        )
    for L in labs:
        if L.intent_id is None:
            per_intent.append({"id": None, "step": L.step, "label": L.label, "label_basis": L.basis, "outcome": "declined_by_proposer", "declined": L.declined})
    # the step a proposer corrected, under the suggestion's label (no intent of its own: `id`
    # None; the intent it proposed instead is `intent_id`, listed above under its own label)
    for r in corrected_rows:
        L = r["L"]
        per_intent.append(
            {"id": None, "intent_id": L.intent_id, "step": L.step, "label": L.label, "label_basis": L.basis, "outcome": r["outcome"],
             "c2_shows_it": r["c2_shows_it"], "evidence": r["evidence"], "adjusted": L.adjusted, "corrected_to": by_id[L.intent_id].label}
        )

    counts = {
        "events": len(events),
        "intents": len(tr),
        "labelled": len(labs),
        "agent_intents": len(agent),
        "human_decisions": len(decided),
        "views": sum(len(t.views) for t in tr.values()),
        "by_route": {r: sum(1 for t in tr.values() if t.route == r) for r in ("auto", "human", "at_submission", "rules")},
        "storyline_errors": sum(1 for r in storyline if r.get("kind") == "error"),
        "storyline_skipped": sum(1 for r in storyline if r.get("kind") == "skipped" and r.get("reason") != "in this arm the agent makes its own proposals from the world model"),
        # what a proposer that may adjust the step did with it (the agent+model arm)
        "declined_by_proposer": sum(1 for L in labs if L.intent_id is None),
        "adjusted_by_proposer": sum(1 for L in labs if L.adjusted),
        "corrected_by_proposer": len(corrected_rows),
        "introduced_by_proposer": sum(1 for L in labs if proposer_effect(L) == "introduced"),
    }
    return {
        "arm": arm,
        "not_human_data": NOT_HUMAN,
        "counts": counts,
        "invariants": inv,
        "premises": {"checked": sum(1 for L in labs if L.premise), "failed": premise_failures},
        # the label again with truth at the decision; the scored label is the one at proposal
        "changed_before_decision": [
            {"step": r["L"].step, "intent": r["L"].intent_id, "at_proposal": r["L"].label, "at_decision": r["at_decision"][0], "why": r["at_decision"][1], "outcome": r["outcome"]}
            for r in rows
            if r["at_decision"] and r["at_decision"][0] != r["L"].label
        ],
        "should_reject": {**sr_block(sr), "c2_shows_it": sr_block(group("should_reject", True)), "picture_wrong": sr_block(group("should_reject", False))},
        "legitimate": {
            "passed": _rate(lg_passed, len(lg_scored)),
            "false_reject": _rate(lg_rejected, len(lg_human)),
            "false_reject_without_changed": _rate(lg_rejected_kept, len(lg_human_kept)),
            "outcomes": outcomes(lg),
            "proposed": len(lg),
        },
        "should_void": {
            **_rate(sv_voided, len(sv_scored)),
            "stopped": _rate(sv_stopped, len(sv_scored)),
            "outcomes": outcomes(sv),
            "proposed": len(sv),
        },
        "override": {
            **_rate(agent_rejected, len(agent_human)),
            "agent_intents": len(agent),
            "withdrawn": withdrawn,
            "withdraw_rate": (withdrawn / len(agent)) if agent else None,
        },
        "discrimination": disc(decision_rows),
        "discrimination_without_changed": disc([r for r in decision_rows if r["intent_id"] not in changed_ids]),
        # SandTable's operator_goodput: decided in time, of every card that reached the inbox
        "goodput": {**_rate(served, len(inbox)), "expired": lapsed},
        "utilization": {"card_open_s": round(open_s, 1), "window_s": round(window, 1) if window else None, "rate": (open_s / window) if window else None},
        "latency": {**summary(lat), "open_ms": summary([r["open_ms"] for r in decision_rows if r["open_ms"] is not None]), "with_view_event": sum(1 for r in decision_rows if r["has_view"])},
        "load": {
            "max_queue": max_queue(qev),
            "bins": load_bins,
            "spearman_latency_vs_queue": spearman([r["queue"] for r in decision_rows], [r["latency_s"] for r in decision_rows]) if len(decision_rows) >= 3 else None,
            "expired": lapsed,
            "pending_total": len(inbox),
        },
        "time_to_effect": {
            "auto": tte(lambda t: t.route == "auto"),
            "human_legitimate": tte(lambda t: t.route == "human" and t.id in legit_ids),
            "at_submission": tte(lambda t: t.route == "at_submission"),
        },
        "decisions": decision_rows,
        "per_intent": per_intent,
    }


def compute_run(run_dir: Path, rerun_replay: bool = False, info: dict[str, Any] | None = None) -> dict[str, Any]:
    """Metrics for a run directory, from its raw logs (so a fix to this module applies to
    old runs too). The replay result in run.json is reused unless `rerun_replay`."""
    run_dir = Path(run_dir)
    events = load_jsonl(run_dir / "audit.jsonl")
    story = load_jsonl(run_dir / "storyline.jsonl")
    if info is None:
        info = {}
        if (run_dir / "run.json").exists():
            try:
                info = json.loads((run_dir / "run.json").read_text())
            except ValueError:
                info = {}
    operators = set(info.get("operators") or [])
    replay = None if rerun_replay else info.get("replay")
    if replay is None:
        replay = run_replay(run_dir / "audit.jsonl")
    # a study's evidence is its participant's decisions only (the other operators are scripts)
    participant = PARTICIPANT_OPERATOR if info.get("arm") == "study" else None
    samples = load_jsonl(run_dir / "truth.jsonl")
    nav = load_jsonl(run_dir / "standins-truth.jsonl")
    # the emulator's cut windows are on the wall clock, a decision's time on the bridge's
    to_bridge = wall_to_bridge(samples)
    cuts = [(u, to_bridge(a), to_bridge(b)) for u, a, b in link_cuts(run_dir, info.get("clocks"), story)]
    m = compute(
        events, story, operators, replay, arm=info.get("arm"), cuts=cuts, samples=samples, nav=nav,
        deciders={participant} if participant else None,
    )
    m["run"] = {k: info.get(k) for k in ("run_id", "scenario", "arm", "seed", "status", "error", "ready", "ready_at_s", "warnings", "clocks", "clock_checks")}
    m["auth"] = auth_of(events, replay, participant)
    return m


# the acts of an operator's that are decisions (by `by`)
HUMAN_ACTS = ("approval.granted", "approval.concurred", "approval.at_submission", "approval.rejected")


def auth_of(events: list[dict], replay: dict | None, participant: str | None = None) -> dict[str, Any]:
    """How the run's operators proved who they were (rules 8: the roster's `auth` in
    `run.started`, each session's `auth`). `credentials`: each signed in with a checked
    credential; `dev`: ids asserted by request
    headers (the proxy batches, scripted runs); `none recorded`: a log of earlier rules, ids
    asserted too. In a credentials run, `people` logged in with their own password and
    `scripted` operators with a credential the harness holds (never a person's). Evidence
    about people (`human_control_evidence`) needs all of: a credentials run, a log that
    replayed, its anchors verified (a rewritten log replays too), and (a study,
    `participant`) the participant signed in as a person; only the participant's acts count
    (`participant_acts`), the scripted operators' never. Rules 10: a study also needs the log
    to name it (`run.started`'s roster `study`, its participant), without the password
    override, and every consequential act of the participant's stepped up by passkey
    (`participant_steps`; a passkey registration's password step-up is not a decision). A
    log of earlier rules names no study: a pilot. `why_not` says what is missing."""
    first = events[0] if events else {}
    started = first if first.get("type") == "run.started" else {}
    roster = started.get("roster") or {}
    mode = roster.get("auth")
    rules = started.get("rules_version") or 1
    study = roster.get("study")
    replayed = bool(replay and replay.get("ran") and replay.get("ok"))
    anchors = (replay or {}).get("anchors") or "not checked"
    methods: dict[str, set[str]] = defaultdict(set)
    for e in events:
        if e.get("type") == "operator.action" and e.get("action") == "session.opened":
            methods[str(e.get("operator"))].add(str(e.get("auth")))
    # a person signs in with their own password or (rules 10) their own passkey
    people = sorted(op for op, m in methods.items() if m and m <= {"password", "passkey"})
    scripted = sorted(op for op, m in methods.items() if "scripted" in m)
    acts: dict[str, int] = defaultdict(int)
    for e in events:
        if e.get("type") in HUMAN_ACTS and e.get("by"):
            acts[str(e["by"])] += 1
    # rules 10: each consequential act's step-up, by its factor (the `operator.action step_up`
    # event its `step_up` names): a person's by passkey or by password
    factor_of = {
        e.get("seq"): str(e.get("auth"))
        for e in events
        if e.get("type") == "operator.action" and e.get("action") == "step_up"
    }
    stepped: dict[str, dict[str, int]] = defaultdict(lambda: {"passkey": 0, "password": 0, "other": 0})
    for e in events:
        if e.get("type") == "passkey.registered":
            continue  # a registration re-enters the password by design: not a decision
        ref = e.get("step_up")
        by = e.get("by") or e.get("operator")
        declarer = (e.get("declaration") or {}).get("by") or {}
        if ref is None and declarer.get("kind") == "operator":
            ref, by = declarer.get("step_up"), declarer.get("operator")
        if not isinstance(ref, dict) or by is None:
            continue
        f = factor_of.get(ref.get("seq"), "other")
        stepped[str(by)][f if f in ("passkey", "password") else "other"] += 1
    why_not = []
    if mode != "credentials":
        why_not.append(f"operators not authenticated (auth {mode or 'none recorded'})")
    if not replayed:
        why_not.append("the log did not replay")
    if anchors != "verified":
        why_not.append(f"anchors {anchors}")
    if participant is not None and participant not in people:
        why_not.append(f"{participant} did not sign in as a person (own password or passkey)")
    if participant is not None:
        # rules 10: a study counts only with the participant's passkey at every step-up (a log
        # of earlier rules names no study: a pilot)
        if not study or study.get("participant") != participant:
            why_not.append(f"the log names no study of {participant} (rules {rules}; a study's bridge, FLEETMESH_STUDY, from rules 10)")
        elif study.get("password_step_up"):
            why_not.append(f"password step-ups allowed by the study's override ({study['password_step_up']}): a pilot")
        mine = stepped.get(participant, {"passkey": 0, "password": 0, "other": 0})
        if mine["password"] or mine["other"]:
            why_not.append(f"{mine['password'] + mine['other']} of {participant}'s consequential acts stepped up without a passkey")
    out: dict[str, Any] = {
        "mode": mode or "none recorded",
        "anchors": anchors,
        "people": people,
        "scripted": scripted,
        "acts_by_people": sum(acts[p] for p in people),
        "acts_scripted": sum(acts[s] for s in scripted),
        "human_control_evidence": not why_not,
        "why_not": why_not,
    }
    if participant is not None:
        out["participant"] = participant
        out["participant_acts"] = acts[participant] if participant in people else 0
        mine = stepped.get(participant, {"passkey": 0, "password": 0, "other": 0})
        out["participant_steps"] = {"passkey": mine["passkey"], "password": mine["password"] + mine["other"]}
        out["study"] = study
    return out


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit("usage: python -m eval.metrics <run-dir>")
    m = compute_run(Path(sys.argv[1]))
    print(json.dumps(m, indent=1))


if __name__ == "__main__":
    main()
