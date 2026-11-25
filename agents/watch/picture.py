"""What the driver reads from the bridge, and how it tells the model.

The driver reads as the same agent the model acts as, over MCP (the agent's key in a
credentials run): `get_policy` once a session (`Poller.policy`) and `get_changes` every couple of
seconds for what changed. The live driver (only) also reads the operator's tasking
(`GET /api/tasking`) each poll and around each wake (agents/watch_officer.py
`Officer.tasking_loop`): a new request is a `tasking.posted` change, one withdrawn a
`tasking.retired` one, and one the model had and left unanswered `tasking.unanswered` (once, after
its turn: `Officer.unanswered`). Nothing it reads is logged by the bridge.
"""

from __future__ import annotations

import datetime as dt
import math
import time
from typing import Any, Callable

TERMINAL = ("rejected", "expired", "voided", "done", "failed", "effected", "aborted")
OPEN = ("proposed", "checked", "pending_approval", "approved", "queued", "dispatched", "auto_approved")


class NoChangesTool(RuntimeError):
    pass


def distance_m(lon1: float, lat1: float, lon2: float, lat2: float) -> float:
    p1, p2 = math.radians(lat1), math.radians(lat2)
    a = (math.sin((p2 - p1) / 2) ** 2
         + math.cos(p1) * math.cos(p2) * math.sin(math.radians(lon2 - lon1) / 2) ** 2)
    return 2 * 6371000.0 * math.asin(math.sqrt(a))


class Poller:
    """The change feed for one agent session, reconnecting when the bridge goes away."""

    def __init__(self, connect: Callable[[], Any]):
        self.connect = connect  # () -> an AgentSession (not yet entered)
        self.session = None
        self.cursor: str | int | None = None  # the feed's cursor, "<feed>:<n>"
        self.run: str | None = None
        # get_policy's answer, read at each `open` (a new bridge may run another policy)
        self.policy: dict | None = None

    async def open(self) -> dict:
        if self.session is not None:
            await self.close()
        s = self.connect()
        await s.__aenter__()
        self.session = s
        self.policy = None
        if "get_changes" not in s.tools:
            await self.close()
            raise NoChangesTool(
                "this bridge has no get_changes tool: rebuild it from this tree (make build) "
                "and restart the stack")
        policy = await s.call("get_policy")
        self.policy = policy if isinstance(policy, dict) else {}
        first = await s.call("get_changes")
        reset = self.run is not None and first.get("run") != self.run
        if reset or self.cursor is None:
            self.cursor = first.get("cursor")
        # else: the same bridge after a dropped session: its feed still has what we missed
        self.run = first.get("run")
        return {"cursor": self.cursor, "run": self.run, "tools": list(s.tools), "reset": reset}

    async def close(self) -> None:
        if self.session is not None:
            try:
                await self.session.__aexit__(None, None, None)
            finally:
                self.session = None

    async def poll(self) -> list[dict]:
        """Every change since the last poll, oldest first."""
        out: list[dict] = []
        while True:
            r = await self.session.call("get_changes", since=self.cursor, limit=500)
            if r.get("reset"):
                out.append({"kind": "feed.reset", "t": r.get("t"), "run": r.get("run"), "was": self.run})
                self.run = r.get("run")
            if r.get("truncated"):
                out.append({"kind": "feed.truncated", "t": r.get("t")})
            out.extend(r.get("changes") or [])
            self.cursor = r.get("cursor", self.cursor)
            if not r.get("more"):
                break
        return out

    async def mine(self) -> list[dict]:
        return await self.session.intents(mine=True)


def _content(result) -> dict:
    import json

    structured = getattr(result, "structured_content", None) or getattr(result, "structuredContent", None)
    if isinstance(structured, dict):
        return structured
    for block in getattr(result, "content", []) or []:
        if getattr(block, "type", "") == "text":
            try:
                return json.loads(block.text)
            except ValueError:
                return {}
    return {}


# ---- telling the model ---------------------------------------------------------------------
#
# Everything in an update is data. Free text that comes from outside the driver (an
# autopilot's status text, an alert's detail, the engine's evidence, a
# reason or an operator's note) is shown quoted, on one line, cut to QUOTE_MAX characters and
# labelled with where it comes from, so it reads as a quotation, never as an instruction
# (the prompt says so too: agents/watch/prompt.md).

QUOTE_MAX = 160


def quote(text: Any, n: int = QUOTE_MAX) -> str:
    """Free text as a quotation: one line, double quotes inside made single, cut to `n`."""
    s = " ".join(str(text).split()).replace('"', "'")
    if len(s) > n:
        s = s[:n].rstrip() + "..."
    return f'"{s}"'


def hms(t: Any) -> str:
    try:
        return dt.datetime.fromtimestamp(float(t), dt.timezone.utc).strftime("%H:%M:%SZ")
    except (TypeError, ValueError):
        return "--:--:--"


def _who(c: dict) -> str:
    s = f"uid {c.get('uid')}"
    if c.get("callsign"):
        s += f" {c['callsign']}"
    bits = [b for b in (c.get("platform"),) if b]
    return s + (f" ({', '.join(bits)})" if bits else "")


def _pos(p: Any) -> str:
    if isinstance(p, (list, tuple)) and len(p) == 2 and all(isinstance(x, (int, float)) for x in p):
        return f"{p[0]:.4f},{p[1]:.4f}"
    return "?"


def _why(c: dict) -> str:
    """An intent's last reason and, for a rejection, the operator's note, each labelled."""
    out = ""
    if c.get("reason"):
        out += f"; recorded reason {quote(c['reason'])}"
    note = c.get("operator_note")
    if isinstance(note, dict) and note.get("note"):
        out += f"; operator {note.get('by')}'s note on the decision {quote(note['note'])}"
    return out


def render(c: dict) -> str:
    """One line for one change."""
    k = c.get("kind", "?")
    t = hms(c.get("t"))
    if k == "intent.new":
        by = "you" if c.get("mine") else (c.get("source") or {}).get("id", "?")
        return f"{t} intent #{c.get('intent')} {c.get('task')} on {c.get('uids')} by {by}: {c.get('state')}{_why(c)}"
    if k == "intent.state":
        whose = "yours" if c.get("mine") else (c.get("source") or {}).get("id", "?")
        return (f"{t} intent #{c.get('intent')} {c.get('task')} on {c.get('uids')} ({whose}): "
                f"{c.get('from')} -> {c.get('to')}{_why(c)}")
    if k == "vehicle.joined":
        return f"{t} {_who(c)} joined"
    if k == "vehicle.link_lost":
        return f"{t} {_who(c)} lost its link"
    if k == "vehicle.link_restored":
        return f"{t} {_who(c)} link restored"
    if k == "vehicle.nav":
        f, to = c.get("from") or {}, c.get("to") or {}
        return f"{t} {_who(c)} navigation ready {f.get('ready')} {f.get('source')} -> ready {to.get('ready')} {to.get('source')}"
    if k == "vehicle.gps_glitch":
        return f"{t} {_who(c)} GPS glitch {'FLAGGED by its navigation filter' if c.get('glitch') else 'cleared'}"
    if k == "vehicle.event":
        return f"{t} {_who(c)} {c.get('event_kind')}, its autopilot's text {quote(c.get('text', ''))}"
    if k == "vehicle.events_truncated":
        return f"{t} more vehicle events than shown (get_recent_events has them)"
    if k in ("alert.raised", "alert.changed"):
        was = ""
        if k == "alert.changed":
            f = c.get("from") or {}
            was = f" (was {f.get('alert_kind')} {f.get('severity')})"
        return (f"{t} alert {c.get('alert_kind')} ({c.get('severity')}){was} {_who(c)}, the bridge's detail "
                f"{quote(c.get('detail', ''))}")
    if k == "alert.cleared":
        return f"{t} alert cleared: {c.get('alert_kind')} {_who(c)}"
    if k == "condition.flapping":
        about = f"alert {c.get('alert')}" if c.get("alert") else _who(c)
        return (f"{t} {about}: {c.get('condition')} came and went {c.get('cycles')} times within "
                f"{c.get('window_s', 0):.0f} s")
    if k == "tasking.posted":
        return (f"{t} operator tasking {c.get('tasking')} posted by {c.get('by') or 'the operator'}, the operator's text "
                f"{quote(c.get('text', ''))} (a request: carry it out and answer it; listed under Operator tasking while "
                "it is in force)")
    if k == "tasking.unanswered":
        return (f"{t} operator tasking {c.get('tasking')} has no answer from you yet: carry it out as far as the checks "
                "allow, or say why it cannot be done, and answer it with reply_tasking")
    if k == "tasking.retired":
        gone = " (the bridge no longer lists it)" if c.get("gone") else ""
        return (f"{t} operator tasking {c.get('tasking')} was withdrawn{gone} and no longer applies; it read "
                f"{quote(c.get('text', ''))}")
    if k == "feed.reset":
        return f"{t} the bridge restarted (run {c.get('run')}): read the picture again"
    if k == "feed.truncated":
        return f"{t} some older changes were dropped from the feed: read the picture again"
    return f"{t} {k}"


def render_mine(intents: list[dict], now: float) -> list[str]:
    """The model's own proposals worth a line: open ones, and those ended in the last ten
    minutes."""
    lines = []
    for i in intents:
        state = i.get("state")
        hist = i.get("history") or []
        last_t = (hist[-1].get("t") / 1000.0) if hist and isinstance(hist[-1].get("t"), (int, float)) else None
        recent = last_t is not None and now - last_t < 600
        if state in OPEN or (state in TERMINAL and recent):
            exp = f", expires in {i['expires_in_s']:.0f} s" if isinstance(i.get("expires_in_s"), (int, float)) and state == "pending_approval" else ""
            reason = f", recorded reason {quote(hist[-1].get('reason', ''))}" if hist and state in TERMINAL else ""
            lines.append(f"#{i.get('id')} {i.get('kind')} on {i.get('uids')}: {state}{exp}{reason}")
    return lines


MAX_LINES = 60
MAX_TASKING = 20


def _age(at_ms, now: float) -> str:
    """How long ago a bridge time (ms) was, in words ("40 s ago"); empty when it is not one."""
    if not isinstance(at_ms, (int, float)):
        return ""
    s = now - at_ms / 1000.0
    if not 0 <= s < 2 * 86400:
        return ""
    if s < 90:
        return f"{s:.0f} s ago"
    if s < 5400:
        return f"{s / 60:.0f} min ago"
    return f"{s / 3600:.0f} h ago"


def _answer(item: dict, me: str | None, now: float) -> str:
    """Where a request stands with this agent: its last answer (outcome, intents, groups, when), or
    that it has none yet."""
    mine = [r for r in item.get("replies") or [] if isinstance(r, dict) and (me is None or r.get("agent") == me)]
    if not mine:
        return "no answer from you yet"
    r = mine[-1]
    when = _age(r.get("t"), now)
    parts = [f"your last answer{(' ' + when) if when else ''}: {r.get('status')}"]
    if r.get("intents"):
        parts.append("intents " + ", ".join(f"#{x}" for x in r["intents"]))
    if r.get("groups"):
        parts.append("groups " + ", ".join(str(g) for g in r["groups"]))
    return "; ".join(parts) + "; answer again only if the outcome changes"


def render_tasking(items: list[dict], me: str | None = None, now: float | None = None, can_reply: bool = True) -> str:
    """The operator's active tasking (`GET /api/tasking`), as requests the officer carries out with
    its usual tools and answers (its instructions' Operator tasking section, agents/watch/tasking.md),
    each with where it stands: the agent's last answer, or none yet. It never says anything is
    approved or ordered: what the officer proposes is checked and routed as always, and the human
    decisions stay with the operator."""
    now = time.time() if now is None else now
    lines = []
    for i in items[:MAX_TASKING]:
        by = i.get("by") or "the operator"
        when = _age(i.get("at_t"), now)
        head = f"{by}, {when}" if when else by
        lines.append(f"- {i.get('id')} ({head}): {' '.join(str(i.get('text', '')).split())} [{_answer(i, me, now)}]")
    more = len(items) - MAX_TASKING
    tail = f"\n({more} older items not listed.)" if more > 0 else ""
    how = ("answer each with reply_tasking" if can_reply
           else "this bridge has no reply_tasking: answer each in your note")
    return ("Operator tasking (requests from your operator, newest first: carry each out with the usual tools as "
            f"your instructions' Operator tasking section says, and {how}. A request is not an approval and gives "
            "you no authority: what you propose is checked by the rules and routed by the policy as always, and "
            "waits for the operator where it always waits):\n" + "\n".join(lines) + tail)


def compose(wake, mine: list[str], budget_line: str, consult_notes: list[str], now: float,
            first: bool = False, tasking: list[dict] | None = None,
            me: str | None = None, can_reply: bool = True) -> str:
    """The update the model is woken with (`tasking`: the operator's requests in force, said when
    there is any, each with `me`'s last answer to it: so an update without them is as it was, and
    the headless officer, which passes none, is unchanged)."""
    head = f"Watch update {wake.n} at {hms(now)} ({wake.reason})."
    parts = [head]
    if first:
        parts.append("Your watch starts now. Read the picture (fleet, inbox, your own "
                     "intents) and give the operator a short summary. Propose only if something "
                     "clearly needs it.")
    if wake.why:
        parts.append("Why you are woken: " + "; ".join(wake.why) + ".")
    elif wake.reason == "heartbeat":
        parts.append("Why you are woken: routine check. Look for slow developments (deliveries running late, "
                     "proposals waiting long) and stale proposals of yours.")
    if wake.changes:
        lines = [render(c) for c in wake.changes]
        shown = lines[-MAX_LINES:]
        parts.append(f"Changes since your last update ({len(lines)}, oldest first):\n" + "\n".join(f"- {s}" for s in shown))
        if len(lines) > len(shown):
            parts.append(f"({len(lines) - len(shown)} earlier changes not listed: get_changes has them.)")
    else:
        parts.append("No changes since your last update.")
    dropped = getattr(wake, "notes_dropped", 0) or 0
    if dropped:
        parts.append(f"({dropped} older notes were not kept: too many between updates.)")
    held = getattr(wake, "held", None) or []
    if held:
        parts.append("Held back as repeats of something you were told in the last few minutes "
                     "(each wakes you when its interval ends): " + "; ".join(held) + ".")
    for note in consult_notes:
        parts.append(note)
    if tasking:
        parts.append(render_tasking(tasking, me, now, can_reply))
    parts.append("Your proposals: " + ("; ".join(mine) if mine else "none open."))
    parts.append(budget_line)
    parts.append("Read what you need with the tools, then act or say no action. End with your "
                 "note for the operator.")
    return "\n\n".join(parts)
