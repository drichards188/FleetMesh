#!/usr/bin/env python3
"""FleetMesh's AI watch officer: a model as the operator's AI teammate, through the bridge's MCP
server, on the user's own Claude Code (`claude -p`, the default) or Codex CLI (`codex exec`).

    .venv/bin/python agents/watch_officer.py --backend claude --url http://127.0.0.1:8765/mcp \\
        --agent agent-watch --brief brief.md --situation-log logs/situation.jsonl --until <unix time>

It reads the picture as agent `--agent` (its key from --key or FLEETMESH_AGENT_KEY in a
credentials run), wakes the model when something worth a look changed (the trigger table in
agents/watch/wake.py) or on a slow heartbeat, gives the stronger model the consequential
moments (a suspected spoof, for one), reads the operator's tasking (`GET /api/tasking`:
a new request wakes the routine model, `Officer.tasking_loop`; `--no-tasking` reads none), which
the model carries out with its usual tools and answers (`reply_tasking`; agents/watch/tasking.md,
appended to its instructions), says back how far it has each item for the console (seen,
processed: `Officer.ack`), wakes the model once more for a request it had and left unanswered
(`Officer.unanswered`), enforces budgets, and writes every turn to the situation log (JSONL;
agents/watch/situation.py lists its lines). The model proposes through the same tools as any
agent: it cannot approve or sign anything, a request from the operator gives it no authority, and
humans sign every effect.

FleetMesh never stores, reads, proxies or prompts for model credentials: it runs the CLI you
installed and logged into yourself, with the API-key variables removed from its environment
so it uses your subscription. `--preflight` checks the CLI and its login and exits;
`--print-command` prints the exact locked-down command lines without running a model.

Stops at --until, on SIGTERM or SIGINT, when a budget is spent, or on a usage limit whose
reset comes after --until (with --on-limit wait, the default, it sleeps until an earlier
reset and resumes the session).
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import os
import signal
import sys
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from watch.backend import BackendCrashed, LimitReached, LockdownBreach, PreflightError  # noqa: E402
from watch.claude import FLEETMESH_PREFIX, ClaudeBackend, ClaudeConfig  # noqa: E402
from watch.codex import CodexBackend, CodexConfig  # noqa: E402
from watch.limits import LimitHit  # noqa: E402
from watch.picture import NoChangesTool, Poller, compose, render_mine  # noqa: E402
from watch.situation import SituationLog  # noqa: E402
from watch.wake import Budgets, Waker, Wake  # noqa: E402

PROMPT = HERE / "watch" / "prompt.md"
# what the model is told of the operator's tasking: only where the live driver reads it (the headless
# officer, which reads none, is told nothing of it, so its prompt is as before)
TASKING_PROMPT = HERE / "watch" / "tasking.md"
DEFAULTS = {
    "claude": {"model": "sonnet", "effort": "medium", "strong_model": "opus", "strong_effort": "high"},
    "codex": {"model": "gpt-6-luna", "effort": "high", "strong_model": "gpt-6.1-sol", "strong_effort": "high"},
}
CONSULT_HEAD = (
    "You are consulted as the senior watch officer for one consequential moment: {why}. The "
    "routine watch officer handles everything else and will be told what you did. Read the "
    "picture yourself before you act; your own earlier proposals (agent {agent}) are in "
    "get_intents with mine.\n\n")


class Stopped(Exception):
    pass


@dataclass
class Lane:
    """The back-off of one model: the routine session or the senior consult. Kept apart, so a
    failing routine model does not hold back an escalation, nor a failing consult a routine
    wake."""

    streak: int = 0  # failed turns in a row
    wait_s: float = 0.0  # the last wait after one
    until: float = 0.0  # no wake on this model before this

    def reset(self) -> None:
        self.streak, self.wait_s, self.until = 0, 0.0, 0.0


def sha256(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def build_prompt(brief: str | None, tasking: bool = False, tool_prefix: str = "") -> str:
    """The system prompt: agents/watch/prompt.md, the operator tasking section where the driver
    reads the operator's tasking (`tasking`), the names the CLI gives the FleetMesh tools where it
    prefixes them (`tool_prefix`), and the run's brief."""
    base = PROMPT.read_text().rstrip()
    if tasking:
        base += "\n\n" + TASKING_PROMPT.read_text().rstrip()
    if tool_prefix:
        base += "\n\n" + tool_names_section(tool_prefix)
    return base + "\n\n## Situation brief\n\n" + ((brief or "").strip() or "(no brief given)") + "\n"


def tool_names_section(prefix: str) -> str:
    """This prompt names the tools bare (`triage_item`); a CLI that prefixes them refuses a bare
    name, and a model refused once may decide the tool is not there (live, 2026-11-27: every
    verdict of a run refused, every item reaching the person at its deadline)."""
    return ("## Your tools' names\n\n"
            f"In this CLI every FleetMesh tool carries the server's prefix: call `get_inbox` as "
            f"`{prefix}get_inbox`, `triage_item` as `{prefix}triage_item`, and so on for each tool named "
            "here. A bare name is refused and does nothing; the tool is there under its full name.")


def misnamed(refused: list[dict], bridge_tools: set[str], prefix: str) -> list[str]:
    """The FleetMesh tools a turn called by their bare name where the CLI names them with its
    prefix: refused, so nothing it meant reached the bridge."""
    out: list[str] = []
    for r in refused:
        name = str(r.get("name") or "")
        if prefix and not name.startswith(prefix) and name in bridge_tools and name not in out:
            out.append(name)
    return out


def rename_note(names: list[str], prefix: str) -> str:
    """What the officer is told at once after calling a tool by its bare name."""
    calls = ", ".join(f"`{n}` is `{prefix}{n}`" for n in names)
    return (f"The CLI refused your call{'s' if len(names) > 1 else ''} by the bare name: {calls}. "
            "The tool is there under that full name. Nothing you meant reached the bridge: make those "
            "calls again now by the full name, with the same arguments, before the items' deadlines.")


class RunDirError(ValueError):
    pass


# What may not be in the run directory or above it: instructions a CLI reads by walking up
# (CLAUDE.md, AGENTS.md), project settings with their hooks (.claude), a repository root
# (.git). The user's own ~/.claude is their configuration directory, which
# `--setting-sources project` keeps out, so it is allowed above a run directory.
FORBIDDEN_ABOVE = ("CLAUDE.md", "AGENTS.md", ".claude", ".git")


def run_dir_problems(d: Path) -> list[str]:
    # both resolved: a home or a run directory reached by a symlink is the same directory
    home = Path.home().resolve()
    d = d.expanduser().resolve()
    found = []
    for p in [d, *d.parents]:
        for name in FORBIDDEN_ABOVE:
            if name == ".claude" and p == home:
                continue
            if (p / name).exists():
                found.append(str(p / name))
    return found


def make_run_dir(given: Path | None) -> Path:
    """An empty directory with nothing at or above it that a CLI would read. A directory the
    driver creates is made private (0700); one given that already exists must be empty, and
    is left as it is."""
    if given is None:
        # 0700, new, empty; resolved, since TMPDIR (or a home) may be reached by a symlink
        d = Path(tempfile.mkdtemp(prefix="fleetmesh-watch-")).resolve()
        problems = run_dir_problems(d)
        if problems:
            d.rmdir()
            raise RunDirError(f"the temporary directory {d.parent} has {', '.join(problems)} at or above it: "
                              "pass --run-dir somewhere clear of them (or set TMPDIR)")
        return d
    d = given.expanduser().resolve()
    problems = run_dir_problems(d)
    if problems:
        raise RunDirError(f"--run-dir {d} has {', '.join(problems)} at or above it: the CLI would read "
                          "them; pick a directory outside any repository or project")
    if d.exists():
        if not d.is_dir() or any(d.iterdir()):
            raise RunDirError(f"--run-dir {d} must be a new or empty directory")
        return d
    d.mkdir(parents=True)
    os.chmod(d, 0o700)
    return d


def active_tasking(doc) -> list[dict]:
    """The active items of a `GET /api/tasking` answer (newest first, as served), pure."""
    items = doc.get("items") if isinstance(doc, dict) else None
    return [i for i in items or [] if isinstance(i, dict) and i.get("active") and isinstance(i.get("text"), str)
            and i["text"].strip()]


def replied_by(item: dict, agent: str) -> bool:
    """Whether `agent` has answered the item (its `replies`, as the bridge serves them)."""
    return any(isinstance(r, dict) and r.get("agent") == agent for r in item.get("replies") or [])


def tasking_key(item: dict) -> tuple:
    """One posting of guidance: its id and when it was posted (a restarted bridge, whose tasking
    is in memory, numbers its items again from tk-1)."""
    return (str(item.get("id")), item.get("at_t"))


def tasking_changes(doc, known: dict, now: float) -> list[dict]:
    """What changed in the operator's tasking since the last read (`known`: key -> the active item
    as read, updated in place to this read's): `tasking.retired` for an item read before that is
    no longer active (`gone` when the bridge no longer lists it at all, as after a restart), then
    `tasking.posted` for an active item not read before, oldest first. An answer without its item
    list is refused (ValueError) and changes nothing: a garbled read never withdraws guidance."""
    items = doc.get("items") if isinstance(doc, dict) else None
    if not isinstance(items, list):
        raise ValueError("a tasking answer without its items")
    active = {tasking_key(i): i for i in active_tasking(doc)}
    listed = {tasking_key(i) for i in items if isinstance(i, dict)}

    def order(kv):
        at = kv[0][1]
        return (at if isinstance(at, (int, float)) else 0, kv[0][0])

    out: list[dict] = []
    for key, i in sorted(known.items(), key=order):
        if key not in active:
            out.append({"kind": "tasking.retired", "t": now, "tasking": key[0], "at_t": key[1], "text": i.get("text"),
                        "by": i.get("by"), **({"gone": True} if key not in listed else {})})
    for key, i in sorted(active.items(), key=order):
        if key not in known:
            out.append({"kind": "tasking.posted", "t": now, "tasking": key[0], "at_t": key[1], "text": i.get("text"),
                        "by": i.get("by")})
    known.clear()
    known.update(active)
    return out


def tasking_client(mcp_url: str, agent: str, key: str | None):
    """The bridge's operator tasking for the live runner, on the host the MCP endpoint is on:
    `fetch()` reads `GET /api/tasking`, `ack(id, body)` posts `POST /api/tasking/{id}/ack` and
    answers its HTTP status. Both as the agent, as `/mcp` takes it (its key, else its id header in
    a dev run): the read so the console can say an officer is connected, the acknowledgement
    because only an agent may make one. Stdlib, a thread each, a short timeout."""
    import json
    import urllib.error
    import urllib.request
    from urllib.parse import quote as q, urlsplit

    u = urlsplit(mcp_url)
    url = f"{u.scheme}://{u.netloc}/api/tasking"
    who = {"Authorization": f"Bearer {key}"} if key else {"X-FleetMesh-Agent": agent}

    def read() -> dict:
        req = urllib.request.Request(url, headers=who)
        with urllib.request.urlopen(req, timeout=3) as r:  # noqa: S310 (the bridge's own http URL)
            return json.load(r)

    def post(tid: str, body: dict) -> int:
        req = urllib.request.Request(f"{url}/{q(tid, safe='')}/ack", data=json.dumps(body).encode(), method="POST",
                                     headers={**who, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=3) as r:  # noqa: S310
                return r.status
        except urllib.error.HTTPError as e:
            return e.code

    async def fetch() -> dict:
        return await asyncio.to_thread(read)

    async def ack(tid: str, body: dict) -> int:
        return await asyncio.to_thread(post, tid, body)

    return fetch, ack


# How far an acknowledgement takes an item (the bridge's `delivery`): waiting and failed are
# notes on a posted and a seen item, never a step
STAGE = {"posted": 0, "waiting": 0, "seen": 1, "failed": 1, "processed": 2}
# a turn that ended so finished its update (`processed`); any other end failed it
FINISHED = ("success", "max_turns", "budget")


class Officer:
    def __init__(self, args, backend, log: SituationLog, waker: Waker, poller: Poller,
                 until: float, tasking_fetch=None, tasking_ack=None):
        self.args = args
        # operator tasking: `async () -> dict` reading the bridge's GET /api/tasking, given only by
        # the live runner (`main`); None (the headless design, never a live channel) tells nothing
        self.tasking_fetch = tasking_fetch
        # the active tasking read so far (`tasking_key` -> item; None until a read succeeds): a poll
        # tells what is new or withdrawn against it (`tasking_loop`), an update adds what it lists
        self.tasking_known: dict | None = None
        self.tasking_lock = asyncio.Lock()  # one read at a time, each applied in the order it was made
        self.tasking_in_feed = False  # the bridge's own change feed carries tasking: the loop stops
        # how far the officer has each item, said back to the bridge for the console (`ack`):
        # `async (id, body) -> HTTP status`, or None (no live channel: nothing is said)
        self.tasking_ack = tasking_ack
        self.acked: dict[tuple, str] = {}  # tasking_key -> the last said: seen, processed, failed:<n>, waiting:<why>
        self.ack_queue: asyncio.Queue = asyncio.Queue()
        self.ack_errors = 0
        # requests the model had and left unanswered, told once more (`unanswered`): by tasking_key
        self.nudged: set[tuple] = set()
        self.bridge_tools: set[str] = set()  # the bridge's MCP tools, as listed at connect
        self.backend = backend
        self.log = log
        self.waker = waker
        self.poller = poller
        self.until = until
        self.stop = asyncio.Event()
        self.stop_reason: str | None = None
        self.consult_notes: list[str] = []
        self.crashes: list[float] = []
        self.strong_blocked_until = 0.0
        self.init_logged = False
        self.poll_errors = 0
        self.turns = 0
        self.proposed: list[int] = []
        self._deferred_logged = False
        self.bridge_restarted = False
        self.limits: list[float] = []  # usage limits hit in the last hour
        self.lanes = {"routine": Lane(), "consult": Lane()}

    def signal(self, name: str) -> None:
        if not self.stop.is_set():
            self.stop_reason = f"signal {name}"
            self.stop.set()

    async def sleep(self, seconds: float) -> None:
        if seconds <= 0:
            return
        try:
            await asyncio.wait_for(self.stop.wait(), seconds)
        except asyncio.TimeoutError:
            pass

    async def race(self, coro):
        """Run `coro` unless a stop comes first (then the backend is stopped and Stopped raised)."""
        task = asyncio.ensure_future(coro)
        stopper = asyncio.ensure_future(self.stop.wait())
        done, _ = await asyncio.wait({task, stopper}, return_when=asyncio.FIRST_COMPLETED)
        if task in done:
            stopper.cancel()
            return task.result()
        await self.backend.stop()
        task.cancel()
        try:
            await task
        except BaseException:  # noqa: BLE001 (the turn was cut short on purpose)
            pass
        raise Stopped()

    # ---- the bridge ------------------------------------------------------------------------

    async def connect(self) -> dict | None:
        delay = 2.0
        while not self.stop.is_set() and time.time() < self.until:
            try:
                return await self.poller.open()
            except NoChangesTool:
                raise
            except Exception as e:  # noqa: BLE001 (the bridge may still be starting)
                self.log.write("poll.error", error=f"{type(e).__name__}: {e}", retry_in_s=delay)
                await self.sleep(delay)
                delay = min(delay * 2, 30.0)
        return None

    async def poll_loop(self) -> None:
        every = self.args.poll_s
        while not self.stop.is_set():
            try:
                changes = await self.poller.poll()
                if not self.tasking_in_feed and any(isinstance(c, dict) and str(c.get("kind", "")).startswith("tasking.")
                                                    for c in changes):
                    self.tasking_in_feed = True  # told by the bridge's feed: the driver's own read stops
                self.poll_errors = 0
                if any(isinstance(c, dict) and c.get("kind") == "feed.reset" for c in changes):
                    self.bridge_restarted = True
                if changes:
                    self.waker.add(changes, time.time())
                    self.log_dropped()
            except Exception as e:  # noqa: BLE001 (keep polling; reconnect after a few misses)
                self.poll_errors += 1
                if self.poll_errors in (1, 5) or self.poll_errors % 30 == 0:
                    self.log.write("poll.error", error=f"{type(e).__name__}: {e}", misses=self.poll_errors)
                if self.poll_errors >= 3:
                    try:
                        info = await self.poller.open()
                        self.log.write("bridge.reconnected", cursor=info["cursor"], bridge_run=info["run"],
                                       reset=info["reset"])
                        if info.get("reset"):
                            self.bridge_restarted = True
                            self.waker.add([{"kind": "feed.reset", "t": time.time(), "run": info.get("run")}],
                                           time.time())
                    except Exception:  # noqa: BLE001
                        pass
            await self.sleep(every)

    async def tasking_poll(self, now: float) -> list[dict]:
        """One poll's read of the operator's tasking and what changed since (`tasking_changes`).
        The first read that succeeds, a poll's or an update's, is the baseline and tells nothing."""
        async with self.tasking_lock:
            doc = await self.tasking_fetch()
            if self.tasking_known is None:
                known: dict = {}
                tasking_changes(doc, known, now)
                self.tasking_known = known
                return []
            return tasking_changes(doc, self.tasking_known, now)

    async def tasking_loop(self) -> None:
        """The operator's tasking every `poll_s` (`GET /api/tasking`, read-only, beside the change
        feed so a slow read never holds it up): new guidance is a `tasking.posted` change, which
        wakes the routine model within tasking's share of the wakes (agents/watch/wake.py), and
        guidance withdrawn a `tasking.retired` note. Guidance already posted when the officer
        connects is the baseline: its opening update lists it, so it is not told again as new. A
        read that fails is tried again at the next poll (logged now and then); a bridge without
        the channel (404) is read no more, and one whose own change feed tells tasking is left to it."""
        errors = 0
        while not self.stop.is_set() and not self.tasking_in_feed:
            try:
                changes = await self.tasking_poll(time.time())
                errors = 0
                if changes and not self.tasking_in_feed:
                    now = time.time()
                    self.waker.add(changes, now)
                    self.log_dropped()
                    # past tasking's share of the wakes: a note, given with the next update
                    held = [c for c in changes if c.get("kind") == "tasking.posted" and not self.waker.wakes_now(c, now)]
                    self.ack([(self.tasking_known or {}).get((c.get("tasking"), c.get("at_t"))) for c in held], "waiting",
                             why="tasking's share of the officer's wakes is spent for now: it goes with the next update")
            except Exception as e:  # noqa: BLE001 (the watch goes on without it)
                if getattr(e, "code", None) == 404:
                    self.log.write("tasking.none", error="the bridge has no /api/tasking: no operator tasking this run")
                    return
                errors += 1
                if errors in (1, 5) or errors % 30 == 0:
                    self.log.write("tasking.error", error=f"{type(e).__name__}: {e}", misses=errors)
            await self.sleep(self.args.poll_s)

    # ---- how far the officer has the tasking, said back for the console (display only) -----

    def ack(self, items, state: str, wake: int | None = None, why: str | None = None) -> None:
        """Say how far the officer has each of `items` (as read from the bridge; one without
        `delivery`, an older bridge's, is never said): `seen` when an update listing it goes to the
        model, `processed` when the turn on that update finished, `failed` when it did not (the next
        update lists it again), `waiting` (read, not yet given to the model, and why). Each only
        forward from what the bridge's read or this run last said, `failed` once a wake and
        `waiting` once a reason; queued in order for `ack_loop`, so a slow bridge never holds a wake."""
        if self.tasking_ack is None:
            return
        for i in items or []:
            if not isinstance(i, dict) or "delivery" not in i or not i.get("id"):
                continue
            key = tasking_key(i)
            said = self.acked.get(key)
            stage = max(STAGE.get(str(i.get("delivery")), 0), STAGE.get((said or "posted").split(":")[0], 0))
            mark = {"failed": f"failed:{wake}", "waiting": f"waiting:{why}"}.get(state, state)
            if (said == mark or (state == "waiting" and stage > 0) or (state == "failed" and stage != 1)
                    or (state in ("seen", "processed") and stage >= STAGE[state])):
                continue
            self.acked[key] = mark
            body = {"state": state, **({"wake": wake} if wake is not None else {}), **({"why": why} if why else {})}
            self.ack_queue.put_nowait((str(i["id"]), body))

    def tasking_waiting(self, why: str) -> None:
        """Every item read and not yet given to the model waits, and why (the hour's wakes spent,
        the routine model backing off)."""
        self.ack(list((self.tasking_known or {}).values()), "waiting", why=why)

    async def ack_loop(self) -> None:
        """Send the queued acknowledgements, in order. A 404 (the item gone, as after a bridge
        restart) or a 409 (the bridge has it further already) needs nothing more; another answer,
        or none, is logged now and then and not tried again: it is for display only."""
        while True:
            tid, body = await self.ack_queue.get()
            bad = None
            try:
                status = await self.tasking_ack(tid, body)
                if status not in (200, 404, 409):
                    bad = f"HTTP {status}"
            except Exception as e:  # noqa: BLE001 (display only: the watch goes on)
                bad = f"{type(e).__name__}: {e}"
            finally:
                self.ack_queue.task_done()
            if bad:
                self.ack_errors += 1
                if self.ack_errors in (1, 5) or self.ack_errors % 30 == 0:
                    self.log.write("tasking.ack_error", error=bad, tasking=tid, state=body.get("state"),
                                   misses=self.ack_errors)

    # ---- the run ---------------------------------------------------------------------------

    async def run(self) -> str:
        try:
            info = await self.connect()
        except NoChangesTool as e:
            self.log.write("error", error=str(e))
            return "error"
        if info is None:
            return self.stop_reason or "until"
        # bridge_run: the bridge's audit-log run (a line's own run is this driver's)
        self.log.write("bridge.connected", cursor=info["cursor"], bridge_run=info["run"], tools=info["tools"])
        self.bridge_tools = set(info.get("tools") or [])
        if hasattr(self.backend, "fleetmesh_tools"):
            self.backend.fleetmesh_tools = set(info["tools"])
        started = await self.backend.start()
        self.log.write("backend.start", **{k: v for k, v in started.items() if k != "argv"})
        poller = asyncio.ensure_future(self.poll_loop())
        tasking = asyncio.ensure_future(self.tasking_loop()) if self.tasking_fetch is not None else None
        acks = asyncio.ensure_future(self.ack_loop()) if self.tasking_ack is not None else None
        try:
            while True:
                if self.stop.is_set():
                    return self.stop_reason or "signal"
                now = time.time()
                if now >= self.until:
                    return "until"
                if self.waker.exhausted():
                    self.log.write("budget.exhausted", what="wakes per run", max_wakes=self.waker.budgets.max_wakes)
                    return "budget"
                routine = self.lanes["routine"]
                if now < routine.until:
                    # the routine model failed and waits; an escalating wake still goes to the
                    # senior model, unless that one waits too (or is limited)
                    wake = None
                    if now >= self.lanes["consult"].until and now >= self.strong_blocked_until:
                        wake = self.waker.due(now, escalating_only=True)
                    if wake is None:
                        self.log_dropped()
                        self.tasking_waiting("the officer's model waits after a failed turn: it goes with the next wake")
                        await self.sleep(min(routine.until - now, self.args.poll_s))
                        continue
                else:
                    wake = self.waker.due(now)
                self.log_dropped()
                if wake is None:
                    if self.waker.deferred and not self._deferred_logged:
                        self._deferred_logged = True
                        self.log.write("budget.deferred", what="wakes per hour",
                                       max_wakes_per_hour=self.waker.budgets.max_wakes_per_hour,
                                       pending=len(self.waker.pending))
                        self.tasking_waiting("the officer's wakes for this hour are spent: it goes with the next wake "
                                             "the budget allows")
                    # changes arrive with every poll: look again at least that often
                    nxt = min(self.waker.next_check(now), self.until, now + self.args.poll_s)
                    await self.sleep(max(0.05, nxt - now))
                    continue
                self._deferred_logged = False
                try:
                    outcome = await self.do_wake(wake)
                except Stopped:
                    return self.stop_reason or "signal"
                if outcome:
                    return outcome
        finally:
            for task in (poller, tasking):
                if task is None:
                    continue
                task.cancel()
                try:
                    await task
                except BaseException:  # noqa: BLE001
                    pass
            if acks is not None:
                try:  # what the last turn did, said before the officer goes (a few seconds at most)
                    await asyncio.wait_for(self.ack_queue.join(), 3.0)
                except asyncio.TimeoutError:
                    pass
                acks.cancel()
                try:
                    await acks
                except BaseException:  # noqa: BLE001
                    pass
            await self.backend.stop()
            await self.poller.close()

    def budget_line(self, wake: Wake) -> str:
        b = self.waker.budgets
        left = max(0, b.max_escalations - self.waker.escalations)
        return (f"Budget: this is wake {wake.n} of at most {b.max_wakes} this run ({b.max_wakes_per_hour} an "
                f"hour); you have up to {b.max_turns_per_wake} tool turns for this update; "
                f"{left} escalations to the senior model left.")

    def failed(self, now: float, crashed: bool, lane: str = "routine") -> str | None:
        """A failed turn (an error result, or a crash) on one model (`lane`: the routine
        session or the consult): that model waits before its next wake, doubling with each
        failure in a row (`--error-backoff-s`, up to `--error-backoff-max-s`), so an API
        overload of a minute costs a few waits, not the watch. The run stops only on repeated
        crashes (more than three in ten minutes, either model) or on routine turns that go on
        failing after a wait at the ceiling; the reason says which. A consult that keeps
        failing never stops the run: escalations go to the routine model while it waits."""
        if crashed:
            self.crashes = [t for t in self.crashes if now - t < 600] + [now]
            if len(self.crashes) > 3:
                return "crashes: the CLI died more than three times in ten minutes"
        state = self.lanes[lane]
        ceiling = self.args.error_backoff_max_s
        if lane == "routine" and state.streak and state.wait_s >= ceiling:
            return (f"errors: turns kept failing after a {ceiling:.0f} s wait "
                    f"({state.streak + 1} in a row)")
        state.streak += 1
        state.wait_s = min(self.args.error_backoff_s * 2 ** (state.streak - 1), ceiling)
        state.until = now + state.wait_s
        self.log.write("backoff", model=lane, wait_s=round(state.wait_s, 1), failures_in_a_row=state.streak,
                       crashed=crashed)
        return None

    def log_dropped(self) -> None:
        for change, error in self.waker.take_dropped():
            self.log.write("change.dropped", error=error, change=str(change)[:500])

    async def do_wake(self, wake: Wake) -> str | None:
        now = time.time()
        if self.bridge_restarted:
            # the bridge restarted: a long-lived CLI holds a dead MCP session; a new process
            # (the same conversation) connects afresh, and its MCP status is checked again
            self.bridge_restarted = False
            info = await self.backend.restart()
            self.init_logged = False
            self.log.write("backend.restart", why="the bridge restarted", resume=info.get("resume"))
        escalate = wake.escalate and now >= self.strong_blocked_until and now >= self.lanes["consult"].until
        if wake.escalate and not escalate:
            # the stronger model is limited, or waits after failed consults: the routine one
            # takes it, and nothing is spent
            self.waker.unescalate(wake)
        try:
            mine = render_mine(await self.poller.mine(), now)
        except Exception:  # noqa: BLE001 (the update goes out without the list)
            mine = ["(could not read your intents: use get_intents with mine)"]
        notes = [] if escalate else list(self.consult_notes)
        tasking = await self.tasking()
        text = compose(wake, mine, self.budget_line(wake), notes, now, first=wake.reason == "start",
                       tasking=tasking, me=self.args.agent, can_reply=self.can_reply())
        if escalate:
            text = CONSULT_HEAD.format(why="; ".join(wake.escalate_why), agent=self.args.agent) + text
        cfg = self.backend.cfg
        model = cfg.strong_model if escalate else cfg.model
        effort = cfg.strong_effort if escalate else cfg.effort
        self.log.write("wake", n=wake.n, reason=wake.reason, why=wake.why, escalated=escalate,
                       escalate_why=wake.escalate_why if escalate else [], model=model, effort=effort,
                       update=text, changes=wake.changes, cursor=self.poller.cursor)
        # the update listing the operator's tasking goes to the model now: seen, by this wake
        self.ack(tasking, "seen", wake.n)
        deadline = min(now + self.args.turn_timeout_s, self.until)
        try:
            if escalate:
                turn = await self.race(self.backend.escalate(text, deadline))
            else:
                turn = await self.race(self.backend.turn(text, deadline))
        except Stopped:
            self.ack(tasking, "failed", wake.n, why="the officer stopped during the turn")
            raise
        except LimitReached as e:
            self.ack(tasking, "failed", wake.n, why="a usage limit")
            if e.turn is not None:
                self.log.write("turn", n=wake.n, escalated=escalate, **e.turn.as_dict())
            return await self.on_limit(e.hit, escalate, wake)
        except LockdownBreach as e:
            self.ack(tasking, "failed", wake.n, why="a lockdown breach stopped the officer")
            if e.turn is not None:
                self.log.write("turn", n=wake.n, escalated=escalate, **e.turn.as_dict())
            self.log.write("lockdown.breach", n=wake.n, error=str(e))
            self.log.write("error", n=wake.n, error=f"lockdown breach: {e}; the run stops")
            return "lockdown"
        except BackendCrashed as e:
            self.ack(tasking, "failed", wake.n, why="the CLI crashed")
            # a crashed turn reports no cost: what it spent is missing from the budget's
            # count (the CLI's own estimate comes only with a turn's result)
            self.log.write("backend.crashed", n=wake.n, escalated=escalate, error=str(e), code=e.code,
                           stderr=e.stderr[-2000:], cost="unknown: a crashed turn reports none")
            self.waker.give_back(wake, time.time())  # the next wake tells it again
            stop = self.failed(now, crashed=True, lane="consult" if escalate else "routine")
            if stop:
                self.log.write("error", error=stop)
                return stop.split(":")[0]
            if not escalate:
                info = await self.backend.restart()
                self.log.write("backend.restart", why="crashed", resume=info.get("resume"))
            return None
        self.turns += 1
        self.proposed += turn.proposals()
        self.log.write("turn", n=wake.n, escalated=escalate, **turn.as_dict())
        for r in turn.refused:
            self.log.write("lockdown.refused", n=wake.n, tool=r.get("name"), input=r.get("input"),
                           text=f"the model asked for {r.get('name')}, outside the FleetMesh server; the CLI refused it")
        for u in turn.unknown_items:
            self.log.write("lockdown.unknown_item", n=wake.n, item_type=u.get("type"), item=u.get("item"))
        prefix = getattr(self.backend, "tool_prefix", "")
        bad = misnamed(turn.refused, self.bridge_tools, prefix)
        if bad and turn.stop != "error":
            # told at once, it calls them again by the full name, so its verdicts land before their
            # deadlines instead of every item reaching the person untriaged
            self.log.write("tools.renamed", n=wake.n, tools=bad)
            try:
                again = await self.race((self.backend.escalate if escalate else self.backend.turn)(rename_note(bad, prefix), deadline))
            except Stopped:
                raise
            except LockdownBreach as e:
                if e.turn is not None:
                    self.log.write("turn", n=wake.n, escalated=escalate, retry="renamed", **e.turn.as_dict())
                self.log.write("lockdown.breach", n=wake.n, error=str(e))
                self.log.write("error", n=wake.n, error=f"lockdown breach: {e}; the run stops")
                return "lockdown"
            except (LimitReached, BackendCrashed) as e:
                self.log.write("error", n=wake.n, error=f"calling them again by the full name did not run: {e}")
            else:
                self.turns += 1
                self.proposed += again.proposals()
                self.log.write("turn", n=wake.n, escalated=escalate, retry="renamed", **again.as_dict())
                for r in again.refused:
                    self.log.write("lockdown.refused", n=wake.n, tool=r.get("name"), input=r.get("input"),
                                   text=f"the model asked for {r.get('name')}, outside the FleetMesh server; the CLI refused it")
        if turn.stop == "error":
            # an error turn (overloaded after the CLI's retries, say): what it was told
            # comes back for the next wake, an escalation it spent is returned
            self.ack(tasking, "failed", wake.n, why="the turn ended in an error")
            self.waker.give_back(wake, time.time())
            stop = self.failed(now, crashed=False, lane="consult" if escalate else "routine")
            if stop:
                self.log.write("error", error=stop)
                return stop.split(":")[0]
            return None
        self.lanes["consult" if escalate else "routine"].reset()
        # the turn on the update finished (or ended short, timed out or interrupted: told again)
        finished = turn.stop in FINISHED
        self.ack(tasking, "processed" if finished else "failed", wake.n,
                 why=None if finished else f"the turn ended: {turn.stop}")
        if finished:
            await self.unanswered(tasking, wake.n)
        if not escalate and not self.init_logged and getattr(self.backend, "init", None):
            self.init_logged = True
            self.log.write("backend.init", **self.backend.init)
            if not self.mcp_ok():
                self.log.write("error", error="the model cannot reach the bridge's MCP tools",
                               init=self.backend.init)
                return "error"
        if escalate:
            calls = ", ".join(c.name.removeprefix("mcp__fleetmesh__") for c in turn.tool_calls) or "none"
            ids = ", ".join(f"#{i}" for i in turn.proposals()) or "none"
            self.consult_notes.append(
                f"While you were not woken, the senior watch officer ({turn.model or model}) handled update "
                f"{wake.n} ({'; '.join(wake.escalate_why)}). Its tool calls: {calls}. It proposed: {ids}. "
                f"It said: {(turn.text or '').strip()[:800]}")
        else:
            self.consult_notes = []
        if turn.stop == "budget":
            self.log.write("budget.exhausted", what="claude --max-budget-usd (the CLI's estimate)")
            return "budget"
        cap = getattr(cfg, "max_budget_usd", None)
        spent = getattr(self.backend, "session_cost_usd", 0.0) + getattr(self.backend, "consult_cost_usd", 0.0)
        if cap is not None and spent >= cap:
            self.log.write("budget.exhausted", what="max budget (the CLI's estimate)", spent_usd=round(spent, 4),
                           max_budget_usd=cap)
            return "budget"
        return None

    async def tasking(self) -> list[dict]:
        """The operator's active tasking now (`GET /api/tasking`, read-only), or none when this
        runner has no live channel or the read fails: an update goes out without it. What it lists
        is read (`tasking_known`), so a poll after it does not tell it again as new."""
        if self.tasking_fetch is None:
            return []
        try:
            async with self.tasking_lock:
                doc = await self.tasking_fetch()
                if not isinstance(doc, dict) or not isinstance(doc.get("items"), list):
                    return []  # garbled: neither told nor a baseline
                items = active_tasking(doc)
                if self.tasking_known is None:
                    self.tasking_known = {}
                self.tasking_known.update({tasking_key(i): i for i in items})
        except Exception:  # noqa: BLE001
            return []
        return items

    def can_reply(self) -> bool:
        """Whether the model can answer the operator's tasking with its own tool: the bridge lists
        `reply_tasking` (a bridge without it: the update says to answer in the note)."""
        return "reply_tasking" in self.bridge_tools

    async def unanswered(self, told: list[dict], n: int) -> None:
        """After a turn that finished on an update listing the operator's tasking: each request it
        listed that is still in force and has no answer from this agent wakes the model once more
        (`tasking.unanswered`, within tasking's share of the wakes), so a request is never dropped
        unanswered in silence; the console shows it as not answered meanwhile. Once a request: after
        that its update says it is not answered, every time."""
        if not told or not self.can_reply():
            return
        now_items = {tasking_key(i): i for i in await self.tasking()}
        out = []
        for i in told:
            key = tasking_key(i)
            cur = now_items.get(key)
            if cur is None or key in self.nudged or replied_by(cur, self.args.agent):
                continue
            self.nudged.add(key)
            out.append({"kind": "tasking.unanswered", "t": time.time(), "tasking": key[0], "at_t": key[1],
                        "text": cur.get("text"), "by": cur.get("by")})
        if out:
            self.log.write("tasking.unanswered", n=n, tasking=[c["tasking"] for c in out],
                           text=f"update {n} ended with no answer to {', '.join(c['tasking'] for c in out)}: told once more")
            self.waker.add(out, time.time())
            self.log_dropped()

    def mcp_ok(self) -> bool:
        init = self.backend.init or {}
        if "mcp_servers" not in init:  # codex: `required = true` fails the turn instead
            return True
        servers = {s.get("name"): s.get("status") for s in init.get("mcp_servers") or []}
        tools = init.get("tools") or []
        return servers.get("fleetmesh") == "connected" and any(t.startswith("mcp__fleetmesh__") for t in tools)

    def retry_at(self, hit: LimitHit, now: float) -> float:
        """When to try again after a limit: its reset (plus the margin), or the retry time when
        it gives none, never sooner than a floor that doubles with each limit in the last hour
        (60 s, 120 s, ... 30 min): a reset already past cannot loop restart, turn, limit."""
        self.limits = [t for t in self.limits if now - t < 3600] + [now]
        floor = now + min(self.args.limit_floor_s * 2 ** (len(self.limits) - 1), 1800.0)
        wanted = hit.reset_at + self.args.limit_margin_s if hit.reset_at else now + self.args.limit_retry_s
        return max(wanted, floor)

    async def on_limit(self, hit: LimitHit, escalated: bool, wake: Wake) -> str | None:
        now = time.time()
        retry = self.retry_at(hit, now)
        if escalated:
            # the stronger model's limit: the routine one carries on, told what it missed
            self.strong_blocked_until = retry
            self.lanes["consult"].reset()  # a limit, not a failure: its streak starts again
            self.log.write("limit.hit", **hit.as_dict(), escalated=True, action="routine model only",
                           until=retry)
            self.waker.give_back(wake, now)
            return None
        if self.args.on_limit == "wait" and retry < self.until:
            self.log.write("limit.hit", **hit.as_dict(), action="wait", resume_at=retry)
            await self.backend.stop()
            await self.sleep(retry - time.time())
            if self.stop.is_set():
                return self.stop_reason or "signal"
            info = await self.backend.restart()
            self.lanes["routine"].reset()  # failures before the limit are not counted after it
            self.log.write("limit.resume", resume=info.get("resume"))
            self.waker.give_back(wake, time.time())
            return None
        self.log.write("limit.hit", **hit.as_dict(), action="stop",
                       why="--on-limit stop" if self.args.on_limit == "stop" else "the reset comes after --until")
        return "limit"

def make_backend(args, run_dir: Path, prompt_path: Path, prompt_text: str):
    d = DEFAULTS[args.backend]
    model = args.model or d["model"]
    effort = args.effort or d["effort"]
    strong_model = args.strong_model or d["strong_model"]
    strong_effort = args.strong_effort or d["strong_effort"]
    key = args.key or os.environ.get("FLEETMESH_AGENT_KEY") or None
    if args.backend == "claude":
        cfg = ClaudeConfig(bin=args.claude_bin, model=model, effort=effort, strong_model=strong_model,
                           strong_effort=strong_effort, max_budget_usd=args.max_budget_usd,
                           allow_other_auth=args.allow_other_auth)
        return ClaudeBackend(cfg, run_dir, args.url, args.agent, key, prompt_path,
                             max_turns=args.max_turns_per_wake)
    cfg = CodexConfig(bin=args.codex_bin, model=model, effort=effort, strong_model=strong_model,
                      strong_effort=strong_effort, home=args.codex_home,
                      allow_items=tuple(args.codex_allow_item or ()))
    return CodexBackend(cfg, run_dir, args.url, args.agent, key, prompt_text, max_turns=args.max_turns_per_wake)


async def main_async(args) -> int:
    run_id = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + f"-{os.getpid()}"
    log = SituationLog(args.situation_log, run_id, echo=not args.quiet)
    until = args.until if args.until else time.time() + args.minutes * 60
    brief = args.brief.read_text() if args.brief else None
    # the operator tasking section only where the driver reads the operator's tasking
    # Claude Code names an MCP server's tools mcp__<server>__<tool>; Codex's are its own
    prompt_text = build_prompt(brief, tasking=not args.no_tasking, tool_prefix=FLEETMESH_PREFIX if args.backend == "claude" else "")
    try:
        run_dir = make_run_dir(args.run_dir)
    except RunDirError as e:
        log.write("error", error=str(e))
        print(f"watch officer: {e}", file=sys.stderr)
        return 2
    prompt_path = run_dir / "system-prompt.md"
    prompt_path.write_text(prompt_text)
    try:
        backend = make_backend(args, run_dir, prompt_path, prompt_text)
    except ValueError as e:
        log.write("error", error=str(e))
        return 2
    if args.print_command:
        import json

        print(json.dumps(backend.describe(), indent=1))
        return 0
    try:
        pre = await backend.preflight()
    except PreflightError as e:
        log.write("preflight.failed", backend=args.backend, problem=e.problem, fix=e.fix)
        print(f"watch officer: {e.problem}.\n  fix: {e.fix}", file=sys.stderr)
        return 2
    log.write("preflight", backend=args.backend, **pre)
    if args.preflight:
        return 0
    budgets = Budgets(max_wakes_per_hour=args.max_wakes_per_hour, max_turns_per_wake=args.max_turns_per_wake,
                      max_escalations=args.max_escalations, max_wakes=args.max_wakes,
                      heartbeat_s=args.heartbeat_s, coalesce_s=args.coalesce_s, min_gap_s=args.min_gap_s)
    waker = Waker(budgets, escalation=not args.no_escalation)
    key = args.key or os.environ.get("FLEETMESH_AGENT_KEY") or None

    def connect():
        from policy_agent import AgentSession

        return AgentSession(args.url, args.agent, key)

    poller = Poller(connect)
    log.write("run.start", pid=os.getpid(), backend=backend.describe(), budgets=asdict(budgets), until=until,
              until_iso=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(until)), url=args.url, agent=args.agent,
              auth="agent key" if key else "dev header", run_dir=str(run_dir),
              brief=str(args.brief) if args.brief else None, brief_sha256=sha256(brief) if brief else None,
              prompt_sha256=sha256(prompt_text), escalation=not args.no_escalation,
              on_limit=args.on_limit)
    fetch, ack = (None, None) if args.no_tasking else tasking_client(args.url, args.agent, key)
    officer = Officer(args, backend, log, waker, poller, until, tasking_fetch=fetch, tasking_ack=ack)
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, officer.signal, sig.name)
    try:
        reason = await officer.run()
    except Exception as e:  # noqa: BLE001 (logged, then the run ends)
        log.write("error", error=f"{type(e).__name__}: {e}")
        reason = "error"
    log.write("run.stop", reason=reason, wakes=waker.wakes, escalations=waker.escalations, turns=officer.turns,
              proposed=officer.proposed,
              session=getattr(backend, "session_id", None) or getattr(backend, "thread_id", None),
              cost_usd_estimate=round(getattr(backend, "session_cost_usd", 0.0)
                                      + getattr(backend, "consult_cost_usd", 0.0), 4) if args.backend == "claude" else None,
              run_dir=str(run_dir))
    log.close()
    return 1 if reason in ("error", "crashes", "errors", "lockdown") else 0


def parse_args(argv: list[str] | None = None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--backend", choices=("claude", "codex"), default="claude")
    ap.add_argument("--url", default="http://127.0.0.1:8765/mcp", help="the bridge's MCP endpoint")
    ap.add_argument("--agent", default="agent-watch", help="the agent id (a dev bridge: listed in FLEETMESH_AGENTS)")
    ap.add_argument("--key", help="the agent's key in a credentials run (default FLEETMESH_AGENT_KEY); never logged")
    ap.add_argument("--no-tasking", action="store_true",
                    help="do not read the operator's tasking (GET /api/tasking, each poll and at each wake; a new "
                         "request wakes the routine model, which carries it out and answers it): the officer "
                         "runs on its brief alone, and its prompt has no tasking section")
    ap.add_argument("--brief", type=Path, help="the situation brief, appended to the system prompt")
    ap.add_argument("--situation-log", type=Path, help="JSONL log of every wake and turn (appended)")
    ap.add_argument("--until", type=float, help="stop at this Unix time")
    ap.add_argument("--minutes", type=float, default=60.0, help="without --until: stop after this long")
    g = ap.add_argument_group("budgets")
    g.add_argument("--max-wakes-per-hour", type=int, default=60)
    g.add_argument("--max-turns-per-wake", type=int, default=12, help="tool turns the model gets per update")
    g.add_argument("--max-escalations", type=int, default=6, help="consults of the stronger model per run")
    g.add_argument("--max-wakes", type=int, default=400, help="wakes per run, all told")
    g.add_argument("--max-budget-usd", type=float, help="claude: stop when the CLI's own cost estimate reaches this")
    g.add_argument("--heartbeat-s", type=float, default=60.0, help="wake this often with no trigger (0: never); "
                   "doubles while nothing changes, up to 300 s")
    g.add_argument("--poll-s", type=float, default=2.0)
    g.add_argument("--coalesce-s", type=float, default=3.0)
    g.add_argument("--min-gap-s", type=float, default=10.0)
    g.add_argument("--turn-timeout-s", type=float, default=300.0)
    g.add_argument("--on-limit", choices=("wait", "stop"), default="wait",
                   help="on a usage limit: wait for the reset if it comes before --until, or stop")
    g.add_argument("--limit-retry-s", type=float, default=900.0, help="a limit with no reset time: retry after this")
    g.add_argument("--limit-margin-s", type=float, default=60.0, help="resume this long after a limit's reset")
    g.add_argument("--limit-floor-s", type=float, default=60.0,
                   help="never retry sooner than this after a limit; doubles with each limit in the last hour")
    g.add_argument("--error-backoff-s", type=float, default=30.0,
                   help="wait this long after a failed turn; doubles with each failure in a row")
    g.add_argument("--error-backoff-max-s", type=float, default=900.0,
                   help="the longest wait; a failure after a wait this long ends the run")
    m = ap.add_argument_group("models")
    m.add_argument("--model", help="routine model (claude: sonnet; codex: gpt-6-luna)")
    m.add_argument("--effort", help="routine effort (claude: medium; codex: high)")
    m.add_argument("--strong-model", help="the escalation model (claude: opus; codex: gpt-6.1-sol)")
    m.add_argument("--strong-effort", help="the escalation effort (high)")
    m.add_argument("--no-escalation", action="store_true", help="never use the stronger model")
    c = ap.add_argument_group("CLIs")
    c.add_argument("--claude-bin", default=os.environ.get("FLEETMESH_CLAUDE_BIN", "claude"))
    c.add_argument("--codex-bin", default=os.environ.get("FLEETMESH_CODEX_BIN", "codex"))
    c.add_argument("--codex-home", default=os.environ.get("FLEETMESH_CODEX_HOME") or None,
                   help="CODEX_HOME for codex (default FLEETMESH_CODEX_HOME; unset: your normal home)")
    c.add_argument("--codex-allow-item", action="append", metavar="TYPE",
                   help="codex: let an item type the backend does not know through (repeatable; logged at "
                        "start and at each use). Unknown types stop the run otherwise. Never a command, file "
                        "change, web search, patch, image view or tool call")
    c.add_argument("--allow-other-auth", action="store_true",
                   help="claude: accept a login other than a Claude subscription (it may bill an API account)")
    c.add_argument("--run-dir", type=Path,
                   help="the CLI's working directory: new or empty, with no CLAUDE.md, AGENTS.md, .claude or "
                        ".git at or above it (default: a fresh temporary one)")
    c.add_argument("--preflight", action="store_true", help="check the CLI and its login, then exit")
    c.add_argument("--print-command", action="store_true", help="print the backend's command lines and exit")
    ap.add_argument("--quiet", action="store_true", help="no lines on stderr")
    return ap.parse_args(argv)


def _quiet_session_close() -> None:
    """The MCP client warns "Session termination failed: 202" when the bridge (rmcp) accepts a
    session's close with 202: not a failure, so not said in the agent's output."""
    import logging

    class Accepted(logging.Filter):
        def filter(self, record: logging.LogRecord) -> bool:
            return record.getMessage() != "Session termination failed: 202"

    logging.getLogger("mcp.client.streamable_http").addFilter(Accepted())


def main() -> None:
    _quiet_session_close()
    sys.exit(asyncio.run(main_async(parse_args())))


if __name__ == "__main__":
    main()
