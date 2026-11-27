#!/usr/bin/env python3
"""FleetMesh's main menu (`make start`), what a person opens first:

    make start                     # then open http://localhost:5173
    .venv/bin/python -m eval.launcher

A small local service. It owns the console's web server for the session (one Vite server on
FLEETMESH_WEB_PORT, 5173: the production build by default, or the dev server; eval/webconsole.py),
which shows the menu when nothing runs, and it starts and stops one scenario at a time when the
menu asks:

- a delivery scenario (sim/scenarios/<name>/scenario.toml): what `make start SCENARIO=<name>`
  runs (`scripts/scenario.sh` itself: its fleet on stand-ins, its hubs, demand, scripted events
  and traffic, tidewater's tide, current and wind when the data is in the checkout), with the
  options scenario.sh takes: a fraction of the fleet and demand (SCALE) and the strict drop-zone
  procedure (POLICY);
- either with the AI watch officer (AI=claude or codex: scripts/up.sh starts agents/watch_officer.py
  on the user's own CLI login, docs/design/watch-officer.md), offered only for a CLI installed here;
- the open world: what `make up` runs with tidewater's conditions (the delivery demo fleet in
  ArduPilot SITL, no demand, nothing scripted), offered only when SITL is built here.

Both run on the default stack's ports (8765 and the rest), exactly as their command runs from a
terminal, with two differences: `scripts/up.sh` starts no console of its own (FLEETMESH_WEB=0,
FLEETMESH_WEB_PORT the launcher's, so the bridge accepts the launcher's console as its origin),
and the console reaches the bridge through the launcher's bridge relay (a TCP relay on the
launcher's port + 1, which Vite proxies /api and /ws to).

Ending a scenario is `make down`'s: SIGTERM to up.sh, which stops tidewater, the link emulator,
the traffic simulator, the vehicles and the bridge; then the launcher waits until the stack's
ports are free. A scenario whose time is up (the bridge's elapsed scenario time past the
scenario's duration) stays up for inspection until it is ended. A launch that fails stops
whatever it started and says why, with its log.

The API, on 127.0.0.1 only (the console reaches it through Vite's proxy under /launcher):

    GET  /launcher/api/state   the scenarios, and the current launch with its progress
    POST /launcher/api/start   {"scenario": <id>, "scale": 1 | 0.5 | 0.25,
                               "policy": "standard" | "strict", "ai": "none" | "claude" | "codex",
                               "replace": true (end the running one first)}
    POST /launcher/api/end     end the running scenario
    POST /launcher/api/console {"mode": "build" | "dev"}: how the console is served (eval/webconsole.py)

A POST must come from the console's origin (`Origin`, as a browser sends it) as JSON, and may
name only a scenario of the launcher's own list and the options that scenario offers: nothing a
request carries reaches a command line but a name from that list.

Logs: logs/launcher/ (this launcher's, the console's web.log, one per launch with its command's
output); a run's audit log under logs/audit/ as always.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import tomllib
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

from . import REPO
from .bridge import Bridge
from .situation import port_problems, tcp_answers
from .stack import _swarm
from .webconsole import WebConsole

LAUNCHER_PORT = 8760  # the API (FLEETMESH_LAUNCHER_PORT); the bridge relay listens on the next port
CONSOLE_PORT = 5173  # FLEETMESH_WEB_PORT, as up.sh's
SCENARIOS = REPO / "sim" / "scenarios"
# the open world: `make up`'s default fleet with tidewater's conditions (the Makefile's FLEET)
OPEN_WORLD = "open-world"
OPEN_WORLD_FLEET = "delivery-demo"
OPEN_WORLD_TIDEWATER = "golden-gate-weather"
OPEN_WORLD_AT = "2026-06-14T10:00:00Z"
ARDUPILOT = Path(os.environ.get("ARDUPILOT_HOME") or Path.home() / "ardupilot")
# what a scenario offers (scripts/scenario.sh's settings): a fraction of its fleet and demand,
# and the drop-zone procedure (the strict one is the comparison setting of docs/DECISIONS.md,
# 2026-11-25). No faster clock: FLEETMESH_RATE speeds only the conditions' clock (tidewater), while
# the delivery runtime's parcels and scripted events keep real time (bd fleetmesh-uef).
SCALES = (1.0, 0.5, 0.25)
RATES: tuple[int, ...] = ()
POLICIES = {"standard": "sim/policy.toml", "strict": "sim/policy-strict.toml"}
# the default stack's ports (scripts/up.sh's defaults; each FLEETMESH_* variable overrides one)
DEFAULT_PORTS = {
    "FLEETMESH_BASE_PORT": 14560,
    "FLEETMESH_HTTP_PORT": 8765,
    "FLEETMESH_LINKEMU_PORT": 8766,
    "FLEETMESH_TIDEWATER_PORT": 8770,
    "FLEETMESH_SITL_INSTANCE_BASE": 0,
}
# make's own variables, the Makefile's and scenario.sh's settings, and what scenario_env.py sets:
# a shell may export one for something else, and it would change what the menu's choice runs
CHOICE_VARS = (
    "MAKEFLAGS", "MFLAGS", "MAKELEVEL", "MAKEOVERRIDES", "ARGS", "FLEET", "SCENARIO", "SCALE", "POLICY",
    "FLEETMESH_SCENARIO", "FLEETMESH_SCENARIO_NAME", "FLEETMESH_SCENARIO_DIR", "FLEETMESH_SCENARIO_SCALE",
    "FLEETMESH_AT", "FLEETMESH_RATE", "FLEETMESH_FLEET", "FLEETMESH_AREAS", "FLEETMESH_POLICY",
    "FLEETMESH_LINK_EVENTS", "FLEETMESH_TRAFFIC_SCENARIO", "AI", "FLEETMESH_OFFICER",
)
START_KEYS = {"scenario", "scale", "rate", "policy", "ai", "replace"}
# the AI watch officer's CLIs (agents/watch_officer.py backends), offered when installed here
OFFICER_CLIS = ("claude", "codex")


def officer_choices() -> list[str]:
    """The AI choices a launch offers: none, and each officer CLI installed here."""
    return ["none", *[c for c in OFFICER_CLIS if shutil.which(c)]]
DOMAINS = ("air", "ground", "surface")  # sim/platforms.toml's, in the menu's order
NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")
# how long vehicles usually take to be navigation-ready (the menu says so while it waits)
TYPICAL_SITL_S = (30, 60)
TYPICAL_STANDINS_S = (5, 20)
READY_TIMEOUT_S = 180.0  # a launch goes on with the vehicles that are ready by then
CLOCK_EVERY_S = 5.0  # how often a scenario's clock is read from the bridge (/api/parcels)
STACK_STOP_S = 90.0
PORTS_FREE_S = 30.0
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")

# a launch's stages, in order; `ended` (a scenario's time is up, its stack still up for
# inspection), `stopping`, `stopped` and `failed` come after any of them
STAGES = ("waiting", "starting", "bridge", "ready", "running")
ACTIVE = (*STAGES, "ended", "stopping")
STACK_UP = ("bridge", "ready", "running", "ended")


def say(msg: str) -> None:
    try:
        print(f"launcher: {msg}", flush=True)
    except OSError:
        pass


def utc_stamp() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def rel(p: Path) -> str:
    try:
        return str(p.relative_to(REPO))
    except ValueError:
        return str(p)


class BadRequest(ValueError):
    """A request the launcher refuses (400), saying why."""


class LaunchError(RuntimeError):
    """A launch that can't go on, saying why."""


# ---- the scenarios ------------------------------------------------------------------------------


def fleet_counts(path: Path, scale: float = 1.0) -> dict[str, Any]:
    """A fleet as sim/swarm.py plans it, at a fraction of its size the way swarm.py takes
    FLEETMESH_SCENARIO_SCALE (each unit keeps at least one vehicle): how many vehicles, how many
    need SITL, its platform types and how many in each domain (which the menu draws)."""
    swarm = _swarm()
    fleet = swarm.load_fleet(str(path))
    if scale != 1.0:
        for unit in fleet.get("unit", []):
            unit["count"] = max(1, math.ceil(unit.get("count", 1) * scale))
    vehicles = swarm.plan(fleet, swarm.load_platforms())
    sitl = sum(1 for v in vehicles if v.get("sim") != "standin")
    return {
        "vehicles": len(vehicles),
        "sitl": sitl,
        "platform_types": len({v["platform"] for v in vehicles}),
        "domains": {d: sum(1 for v in vehicles if v["domain"] == d) for d in DOMAINS},
        "typical_s": list(TYPICAL_SITL_S if sitl else TYPICAL_STANDINS_S),
    }


def tidewater_ready(name: str | None, root: Path = REPO) -> bool:
    """Whether tidewater's conditions for `name` run here (scripts/scenario_env.py's test: its
    scenario file and tidewater's CLI are in the checkout)."""
    return bool(name) and (root / "tidewater" / "scenarios" / f"{name}.toml").exists() and (root / ".venv" / "bin" / "tidewater").exists()


def policy_id(path: str) -> str | None:
    return next((k for k, v in POLICIES.items() if v == path), None)


def scenario_entry(d: Path, root: Path = REPO) -> dict[str, Any] | None:
    """A delivery scenario as the menu shows it, or None (said in the launcher's output) for one
    that does not load: the menu offers only what runs."""
    name = d.name
    if not NAME_RE.match(name):
        return None
    try:
        sc = tomllib.loads((d / "scenario.toml").read_text())
        fleet = d / sc.get("fleet", "fleet.toml")
        scales = [{"scale": k, **fleet_counts(fleet, k)} for k in SCALES]
        at = str(sc["at"])
    except (OSError, KeyError, ValueError, SystemExit) as e:
        say(f"scenario {name} is left out of the menu: {e}")
        return None
    policies = [k for k, v in POLICIES.items() if (root / v).exists()]
    own_policy = policy_id(str(sc.get("policy", POLICIES["standard"])))
    if own_policy not in policies:
        say(f"scenario {name} is left out of the menu: its policy {sc.get('policy')!r} is not one the menu knows")
        return None
    rate = sc.get("rate", 1)
    rate = int(rate) if float(rate).is_integer() else float(rate)
    summary = sc.get("summary")
    tw = sc.get("tidewater")
    full = scales[0]
    return {
        "id": name,
        "kind": "scenario",
        # the menu's place for it (scenario.toml's `order`, as docs/design/scenarios.md numbers them)
        "order": int(sc.get("order") or 99),
        "default": False,
        "title": str(sc.get("title") or name),
        "brief": str(sc.get("brief") or "").strip(),
        "summary": {"headline": str(summary.get("headline", "")), "bullets": [str(b) for b in summary.get("bullets", [])]} if isinstance(summary, dict) else None,
        "runnable": True,
        "why_not": None,
        **{k: full[k] for k in ("vehicles", "sitl", "platform_types", "domains", "typical_s")},
        "scales": [{k: e[k] for k in ("scale", "vehicles", "domains")} for e in scales],
        "rate": rate,
        "rates": sorted({rate, *RATES}),
        "policy": own_policy,
        "policies": policies,
        "tidewater": tw if tidewater_ready(tw, root) else "none",
        "at": at,
        "duration_s": float(sc["duration_s"]) if sc.get("duration_s") else None,
        "traffic": bool(sc.get("traffic")) and (d / str(sc["traffic"])).exists(),
        "ai": "none",
        "ais": officer_choices(),
        "cli": f"make start SCENARIO={name}",
    }


def sitl_built(home: Path = ARDUPILOT) -> bool:
    """Whether ArduPilot's SITL firmwares are built here (make setup builds them)."""
    return all((home / "build" / "sitl" / "bin" / b).exists() for b in ("arducopter", "arduplane", "ardurover"))


def open_world_entry(root: Path = REPO) -> dict[str, Any] | None:
    """The open world (`make up` with tidewater's conditions), or None when it can't run here:
    its fleet needs ArduPilot SITL."""
    try:
        fleet = fleet_counts(root / "sim" / "fleets" / f"{OPEN_WORLD_FLEET}.toml")
    except (OSError, ValueError, SystemExit) as e:
        say(f"the open world is left out of the menu: {e}")
        return None
    if fleet["sitl"] and not sitl_built():
        say(f"the open world is left out of the menu: its fleet runs in ArduPilot SITL, not built in {ARDUPILOT} (make setup)")
        return None
    tw = OPEN_WORLD_TIDEWATER if tidewater_ready(OPEN_WORLD_TIDEWATER, root) else "none"
    return {
        "id": OPEN_WORLD,
        "kind": "open_world",
        "default": False,
        "title": "Open world: the delivery demo fleet",
        "brief": (
            f"The delivery demo fleet: {fleet['vehicles']} vehicles of the {fleet['platform_types']} delivery platforms around the Golden Gate, "
            "flown in ArduPilot SITL, with real tide, current and wind from tidewater.\n\n"
            "Nothing is scripted and no parcels arrive: you give the orders, and every one goes through the decision engine and the Inbox as in a scenario."
        ),
        "summary": None,
        "runnable": True,
        "why_not": None,
        **fleet,
        "tidewater": tw,
        "at": OPEN_WORLD_AT,
        "duration_s": None,
        "ai": "none",
        "ais": officer_choices(),
        "cli": f"FLEETMESH_SCENARIO=tidewater/scenarios/{OPEN_WORLD_TIDEWATER}.toml FLEETMESH_AT={OPEN_WORLD_AT} make up" if tw != "none" else "make up",
    }


def catalog(root: Path = REPO, scenarios: Path = SCENARIOS) -> list[dict[str, Any]]:
    """The menu's list: the delivery scenarios in their order (the first is the default), then
    the open world when it can run here. Only what runs."""
    entries = [e for e in (scenario_entry(p.parent, root) for p in sorted(scenarios.glob("*/scenario.toml"))) if e is not None]
    entries.sort(key=lambda e: (e["order"], e["id"]))
    world = open_world_entry(root)
    out = [*entries, *([world] if world else [])]
    if out:
        out[0]["default"] = True
    return out


# ---- a choice, its command and its environment -------------------------------------------------


@dataclass(frozen=True)
class Choice:
    scenario: str
    kind: str  # scenario | open_world
    scale: float = 1.0
    rate: float | None = None
    policy: str | None = None
    ai: str = "none"
    replace: bool = False


def validate_start(body: Any, entries: list[dict[str, Any]]) -> Choice:
    """A start request checked against the menu's own list and what the scenario offers;
    BadRequest says what is wrong. Nothing else of the body is used."""
    if not isinstance(body, dict):
        raise BadRequest("a JSON object: scenario, and for a delivery scenario scale, rate and policy")
    extra = sorted(set(body) - START_KEYS)
    if extra:
        raise BadRequest(f"unknown keys {extra}: only {sorted(START_KEYS)}")
    by_id = {e["id"]: e for e in entries}
    sid = body.get("scenario")
    if not isinstance(sid, str) or sid not in by_id:
        raise BadRequest(f"no scenario {sid!r} in the menu")
    entry = by_id[sid]
    replace = body.get("replace", False)
    if not isinstance(replace, bool):
        raise BadRequest("replace is true or false")
    ai = body.get("ai", entry.get("ai", "none"))
    if ai not in entry.get("ais", ["none"]):
        raise BadRequest(f"ai {ai!r}: one of {', '.join(entry.get('ais', ['none']))} (an officer's CLI must be installed here)")
    if entry["kind"] == "open_world":
        if any(body.get(k) is not None for k in ("scale", "rate", "policy")):
            raise BadRequest("the open world takes no options but the AI: its fleet, conditions and policy are make up's")
        return Choice(sid, "open_world", ai=ai, replace=replace)
    scale = body.get("scale", 1.0)
    if isinstance(scale, bool) or not isinstance(scale, (int, float)) or float(scale) not in SCALES:
        raise BadRequest(f"scale {scale!r}: one of {', '.join(f'{k:g}' for k in SCALES)}")
    rate = body.get("rate", entry["rate"])
    if isinstance(rate, bool) or not isinstance(rate, (int, float)) or rate not in entry["rates"]:
        raise BadRequest(f"rate {rate!r}: one of {', '.join(f'{r:g}' for r in entry['rates'])}")
    policy = body.get("policy", entry["policy"])
    if policy not in entry["policies"]:
        raise BadRequest(f"policy {policy!r}: one of {', '.join(entry['policies'])}")
    return Choice(sid, "scenario", scale=float(scale), rate=rate, policy=policy, ai=ai, replace=replace)


def command(choice: Choice) -> list[str]:
    """What a choice runs: the command line's own command, never a shell string."""
    if choice.kind == "open_world":
        return ["scripts/up.sh", "--fleet", OPEN_WORLD_FLEET]
    if not NAME_RE.match(choice.scenario):
        raise BadRequest(f"not a choice the menu offers: {choice}")
    return ["scripts/scenario.sh", choice.scenario]


def cli_text(choice: Choice, entry: dict[str, Any]) -> str:
    """The same choice as a person types it."""
    officer = f" AI={choice.ai}" if choice.ai != "none" else ""
    if choice.kind == "open_world":
        return entry["cli"] + officer
    out = [f"make start SCENARIO={choice.scenario}"]
    if choice.scale != 1.0:
        out.append(f"SCALE={choice.scale:g}")
    if choice.rate != entry["rate"]:
        out.append(f"FLEETMESH_RATE={choice.rate:g}")
    if choice.policy != entry["policy"]:
        out.append(f"POLICY={POLICIES[choice.policy or 'standard']}")
    return " ".join(out) + officer


def launch_env(base: dict[str, str], console_port: int, choice: Choice | None = None, entry: dict[str, Any] | None = None) -> dict[str, str]:
    """The environment a launch runs in: the launcher's own (so any FLEETMESH_* port variable
    works as from the user's shell) without make's variables or a scenario's settings, then the
    choice's own, with up.sh told the console is the launcher's."""
    env = {k: v for k, v in base.items() if k not in CHOICE_VARS}
    env.update({"FLEETMESH_WEB": "0", "FLEETMESH_WEB_PORT": str(console_port), "PYTHONUNBUFFERED": "1"})
    if choice is None or entry is None:
        return env
    if choice.ai != "none":
        env["FLEETMESH_OFFICER"] = choice.ai
    if choice.kind == "open_world":
        if entry.get("tidewater") not in (None, "none"):
            env.update({"FLEETMESH_SCENARIO": f"tidewater/scenarios/{entry['tidewater']}.toml", "FLEETMESH_AT": entry["at"]})
        return env
    if choice.scale != 1.0:
        env["SCALE"] = f"{choice.scale:g}"
    env["FLEETMESH_RATE"] = f"{choice.rate:g}"
    env["POLICY"] = POLICIES[choice.policy or "standard"]
    return env


def stack_ports_for(env: dict[str, str]) -> dict[str, int]:
    """The ports a launch's stack uses: the default stack's (up.sh's defaults); each FLEETMESH_*
    variable overrides one."""

    def port(k: str) -> int:
        return int((env.get(k) or "").strip() or DEFAULT_PORTS[k])

    base = port("FLEETMESH_BASE_PORT")
    return {
        "vehicle_base": base,
        "bridge_base": int((env.get("FLEETMESH_BRIDGE_BASE_PORT") or "").strip() or base + 1000),
        "http": port("FLEETMESH_HTTP_PORT"),
        "web": int(env["FLEETMESH_WEB_PORT"]),
        "linkemu": port("FLEETMESH_LINKEMU_PORT"),
        "tidewater": port("FLEETMESH_TIDEWATER_PORT"),
        "sitl_instance_base": port("FLEETMESH_SITL_INSTANCE_BASE"),
    }


def refusal(method: str, headers: Any, console_port: int) -> str | None:
    """Why a request is refused: a POST from another origin than the console's (a browser always
    sends Origin with one), or not JSON. A GET only reads the state."""
    if method != "POST":
        return None
    allowed = {f"http://127.0.0.1:{console_port}", f"http://localhost:{console_port}"}
    origin = (headers.get("Origin") or "").rstrip("/")
    if origin not in allowed:
        return f"refused: a request from another origin ({origin or 'no Origin'}); the menu is at http://localhost:{console_port}"
    if not (headers.get("Content-Type") or "").lower().startswith("application/json"):
        return "refused: the body must be JSON (Content-Type: application/json)"
    return None


# ---- the logs -------------------------------------------------------------------------------------


def tail_lines(path: Path | None, size: int = 16384) -> list[str]:
    """The last lines of a log, without colour codes or blank lines."""
    if path is None:
        return []
    try:
        with path.open("rb") as f:
            f.seek(0, os.SEEK_END)
            f.seek(max(0, f.tell() - size))
            text = f.read().decode(errors="replace")
    except OSError:
        return []
    out = []
    for line in text.replace("\r", "\n").splitlines():
        line = ANSI.sub("", line).strip()
        if line:
            out.append(line)
    return out


def _noise(line: str) -> bool:
    """make's own words, the launch log's header."""
    return line.startswith(("make: ***", "make[", "===", "# "))


def failure_reason(lines: list[str]) -> str:
    """Why a launch's command stopped, from its output: up.sh's last words."""
    for line in reversed(lines):
        if not _noise(line):
            return line
    return "it stopped without saying why"


def progress_line(lines: list[str]) -> str:
    """The latest line of a starting stack's output, short, for the menu's progress."""
    for line in reversed(lines):
        if not _noise(line):
            return line if len(line) <= 160 else line[:157] + "..."
    return ""


# ---- processes --------------------------------------------------------------------------------------


def proc_stat(pid: int) -> tuple[str, int, int, int] | None:
    """(state, parent, session, start time in clock ticks since boot) from /proc."""
    try:
        rest = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return rest[0], int(rest[1]), int(rest[3]), int(rest[19])
    except (OSError, IndexError, ValueError):
        return None


def alive(pid: int, start: int | None) -> bool:
    st = proc_stat(pid)
    return st is not None and st[0] != "Z" and (start is None or st[3] == start)


def terminate(proc: subprocess.Popen, grace: float) -> bool:
    """SIGTERM, then SIGKILL after `grace` seconds; whether it stopped on SIGTERM."""
    if proc.poll() is not None:
        return True
    try:
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=grace)
        return True
    except subprocess.TimeoutExpired:
        proc.kill()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        return False


# ---- the bridge relay -------------------------------------------------------------------------------


def _pipe(src: socket.socket, dst: socket.socket) -> None:
    try:
        while data := src.recv(65536):
            dst.sendall(data)
    except OSError:
        pass
    finally:
        try:
            dst.shutdown(socket.SHUT_WR)
        except OSError:
            pass


class Relay:
    """A TCP relay from a fixed local port to whichever bridge runs (Vite proxies the console's
    /api and /ws here), so the console's origin stays the same whichever launch runs. Bytes only,
    both ways, so HTTP and the WebSocket pass as they are; with no bridge it closes at once."""

    def __init__(self, port: int):
        self.port = port
        self.target: int | None = None
        self.server = socket.create_server(("127.0.0.1", port))

    def serve(self) -> None:
        while True:
            try:
                conn, _ = self.server.accept()
            except OSError:
                return  # closed
            threading.Thread(target=self.handle, args=(conn,), daemon=True).start()

    def handle(self, conn: socket.socket) -> None:
        target = self.target
        try:
            if target is None:
                return
            try:
                up = socket.create_connection(("127.0.0.1", target), timeout=3)
            except OSError:
                return
            up.settimeout(None)
            back = threading.Thread(target=_pipe, args=(up, conn), daemon=True)
            back.start()
            _pipe(conn, up)
            back.join()
            up.close()
        finally:
            conn.close()

    def close(self) -> None:
        self.server.close()


# ---- a launch ---------------------------------------------------------------------------------------


class Launch:
    """One scenario from start to end, run by its own thread: its command, its progress read
    from the bridge (and a delivery scenario's clock), and its end."""

    def __init__(self, launcher: "Launcher", choice: Choice, entry: dict[str, Any], previous: "Launch | None"):
        self.launcher = launcher
        self.choice = choice
        self.entry = entry
        self.previous = previous
        self.id = f"{choice.scenario}-{utc_stamp()}"
        self.title = entry["title"]
        self.env = launch_env(dict(os.environ), launcher.console_port, choice, entry)
        self.ports = stack_ports_for(self.env)
        self.argv = command(choice)
        self.cli = cli_text(choice, entry)
        self.log = launcher.log_dir / f"{self.id}.log"
        scaled = next((e for e in entry.get("scales", []) if e["scale"] == choice.scale), None)
        self.expected = int((scaled or entry).get("vehicles") or 0)
        self.sitl = int(entry.get("sitl") or 0)
        self.stage = "waiting" if previous is not None else "starting"
        self.detail = f"ending {previous.title} first" if previous is not None else "starting"
        self.error: str | None = None
        self.leftovers: list[str] = []
        self.started_t = time.time()
        self.stage_t = self.started_t
        self.history: list[dict[str, Any]] = [{"stage": self.stage, "t": round(self.started_t, 3)}]
        self.ready: tuple[int, int] | None = None
        # a delivery scenario's clock as the bridge last said it (elapsed and duration, s)
        self.clock: dict[str, float] | None = None
        self.clock_t = 0.0
        self.proc: subprocess.Popen | None = None
        self.up_pid: tuple[int, int] | None = None  # up.sh: (pid, start ticks), once identified
        self.proc_start: int | None = None  # the command's start, clock ticks since boot
        self.tidewater_reused = False
        # whether a stack came up at all (up.sh identified, or the bridge answered): only then are
        # its ports waited for at the end
        self.stack_started = False
        self.bridge = Bridge(f"http://127.0.0.1:{self.ports['http']}", timeout=2.0)
        self.end_requested = threading.Event()
        self.thread = threading.Thread(target=self.run, name=f"launch {self.id}", daemon=True)

    # ---- state ------------------------------------------------------------------------------------

    def active(self) -> bool:
        return self.stage in ACTIVE

    def set_stage(self, stage: str, detail: str) -> None:
        with self.launcher.lock:
            if self.end_requested.is_set() and stage not in ("stopping", "stopped", "failed"):
                return  # asked to end: it only goes down from here
            if stage != self.stage:
                self.stage = stage
                self.stage_t = time.time()
                self.history.append({"stage": stage, "t": round(self.stage_t, 3)})
                self.launcher.note(f"{self.id}: {stage}: {detail}")
            self.detail = detail

    def public(self) -> dict[str, Any]:
        c = self.choice
        options = {"ai": c.ai} if c.kind == "open_world" else {"scale": c.scale, "rate": c.rate, "policy": c.policy, "ai": c.ai}
        return {
            "id": self.id,
            "scenario": c.scenario,
            "kind": c.kind,
            "title": self.title,
            "options": options,
            "cli": self.cli,
            "stage": self.stage,
            "detail": self.detail,
            "error": self.error,
            "leftovers": self.leftovers,
            "started_t": round(self.started_t, 3),
            "stage_t": round(self.stage_t, 3),
            "history": self.history,
            "ready": None if self.ready is None else {"ready": self.ready[0], "of": self.ready[1]},
            "typical_s": list(TYPICAL_SITL_S if self.sitl else TYPICAL_STANDINS_S),
            "log": rel(self.log),
            "bridge_port": self.ports["http"],
            "clock": self.clock,
            "stack_up": self.stage in STACK_UP,
        }

    def request_end(self) -> None:
        """End it: its thread stops what it started within a second; the console is told at once
        (it stops asking the bridge)."""
        self.end_requested.set()
        self.set_stage("stopping", "ending: stopping what it started")

    # ---- the run ----------------------------------------------------------------------------------

    def run(self) -> None:
        try:
            if self.previous is not None:
                self.previous.thread.join()
                if self.previous.leftovers:
                    raise LaunchError(f"{self.previous.title} did not stop cleanly ({'; '.join(self.previous.leftovers)}): `make down`, then start again")
            if self.end_requested.is_set():
                self.set_stage("stopped", "ended before it started")
                return
            self.preflight()
            if self.end_requested.is_set():
                self.set_stage("stopped", "ended before it started")
                return
            self.start()
            self.monitor()
        except LaunchError as e:
            self.fail(str(e))
        except Exception as e:  # noqa: BLE001 (said in the menu and the launcher's log)
            self.fail(f"the launcher failed ({type(e).__name__}: {e})")

    def preflight(self) -> None:
        """Refuse to start beside a stack on the same ports (a `make up` or `make start
        SCENARIO=` from a terminal). A tidewater already answering is reused, as up.sh does."""
        self.tidewater_reused = tcp_answers(self.ports["tidewater"])
        self.set_stage("starting", "checking that no other stack holds the stack's ports")
        problems = port_problems(self.ports, self.expected, self.sitl, skip=("web", "tidewater"))
        if problems:
            raise LaunchError("refusing to start: " + "; ".join(problems) + ". If that is a stack of yours, `make down` stops it.")

    def start(self) -> None:
        self.log.parent.mkdir(parents=True, exist_ok=True)
        with self.log.open("w") as f:
            f.write(f"# {self.cli}\n# {' '.join(self.argv)}\n")
            f.flush()
            self.proc = subprocess.Popen(
                self.argv, cwd=str(REPO), env=self.env, stdout=f, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, start_new_session=True
            )
        st = proc_stat(self.proc.pid)
        self.proc_start = st[3] if st is not None else None
        self.launcher.relay.target = self.ports["http"]
        self.launcher.note(f"{self.id}: started {' '.join(self.argv)} (pid {self.proc.pid}; output {rel(self.log)})")
        self.set_stage("starting", "starting the stack")

    def note_up_pid(self) -> None:
        """Identify this launch's up.sh from logs/up.pid (up.sh writes it once its build is done):
        it runs in the session the launcher started (scenario.sh execs it). Only that process is
        ever signalled."""
        if self.up_pid is not None or self.proc is None:
            return
        try:
            pid = int((REPO / "logs" / "up.pid").read_text().strip())
        except (OSError, ValueError):
            return
        st = proc_stat(pid)
        if st is None or st[0] == "Z":
            return
        if st[2] == self.proc.pid:
            self.up_pid = (pid, st[3])
            self.stack_started = True

    def up_alive(self) -> bool:
        return self.up_pid is not None and alive(*self.up_pid)

    def read_clock(self) -> None:
        """A delivery scenario's clock from the bridge (its elapsed scenario time and duration),
        every few seconds; once the time is up the stack stays up for inspection."""
        if self.choice.kind != "scenario" or time.monotonic() - self.clock_t < CLOCK_EVERY_S:
            return
        self.clock_t = time.monotonic()
        body = self.bridge.try_get("/api/parcels")
        if not isinstance(body, dict) or not isinstance(body.get("elapsed_s"), (int, float)):
            return
        duration = (body.get("scenario") or {}).get("duration_s") or self.entry.get("duration_s")
        with self.launcher.lock:
            self.clock = {"elapsed_s": round(float(body["elapsed_s"]), 1), "duration_s": float(duration) if duration else None}
        if self.stage == "running" and duration and body["elapsed_s"] >= duration:
            self.set_stage("ended", "the scenario's time is up; its stack stays up for inspection until you end it")

    def monitor(self) -> None:
        bridge_t: float | None = None
        while not self.end_requested.wait(1.0):
            self.note_up_pid()
            assert self.proc is not None
            code = self.proc.poll()
            if code is not None:
                lines = tail_lines(self.log)
                if code == 0 and not any("a process exited" in x for x in lines[-5:]):
                    # up.sh stopped on a request from outside the menu (make down)
                    self.stop_all()
                    self.set_stage("stopped", "the stack was stopped outside the menu (make down)")
                    return
                raise LaunchError(f"{failure_reason(lines)} (exit {code}; see {rel(self.log)})")
            state = self.bridge.try_get("/api/state")
            if state is None:
                if self.stage == "starting":
                    self.set_stage("starting", progress_line(tail_lines(self.log)) or "starting the stack")
                elif self.stage in STACK_UP and not self.up_alive():
                    raise LaunchError(f"the stack stopped (see {rel(self.log)})")
                continue
            self.stack_started = True
            vehicles = state.get("vehicles") or []
            ready = sum(1 for v in vehicles if v.get("nav_ready") and v.get("position"))
            of = max(self.expected, len(vehicles))
            with self.launcher.lock:
                self.ready = (ready, of)
            if bridge_t is None:
                bridge_t = time.time()
            counted = f"{ready} of {of} vehicles ready"
            if self.stage in ("starting", "bridge"):
                if ready >= of:
                    self.set_stage("running", counted)
                elif time.time() - bridge_t > READY_TIMEOUT_S:
                    self.set_stage("running", f"{counted} after {READY_TIMEOUT_S:.0f} s; the rest may follow")
                else:
                    self.set_stage("bridge", f"bridge up; {counted}")
            self.read_clock()
        self.stop_all()
        self.set_stage("stopped", "ended: everything it started has stopped" if not self.leftovers else "ended, but still held: " + "; ".join(self.leftovers))

    def fail(self, reason: str) -> None:
        with self.launcher.lock:
            self.error = reason
        self.launcher.note(f"{self.id}: failed: {reason}")
        if self.proc is not None:
            self.stop_all()
        self.set_stage("failed", reason if not self.leftovers else f"{reason}; still held: {'; '.join(self.leftovers)}")

    # ---- the end ----------------------------------------------------------------------------------

    def stop_all(self) -> None:
        """`make down`, for this launch only: the stack, then its ports must be free."""
        self.set_stage("stopping", "ending: stopping what it started")
        proc = self.proc
        self.note_up_pid()
        if self.up_pid is not None and alive(*self.up_pid):
            self.set_stage("stopping", "ending: stopping the stack (as make down)")
            try:
                os.kill(self.up_pid[0], signal.SIGTERM)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + STACK_STOP_S
            while alive(*self.up_pid) and time.monotonic() < deadline:
                time.sleep(0.5)
            if alive(*self.up_pid):
                self.leftovers.append(f"up.sh (pid {self.up_pid[0]}) did not stop in {STACK_STOP_S:.0f} s: `make down`")
        if proc is not None and proc.poll() is None:
            # still building, before up.sh wrote its pid: the command itself
            terminate(proc, 15)
        if self.stack_started:
            # the stand-ins and SITL instances stop a moment after up.sh (swarm.py signals them)
            self.set_stage("stopping", "ending: waiting for the stack's ports to be free")
            skip = ("web", "tidewater") if self.tidewater_reused else ("web",)
            deadline = time.monotonic() + PORTS_FREE_S
            held = port_problems(self.ports, self.expected, self.sitl, skip=skip)
            while held and time.monotonic() < deadline:
                time.sleep(1.0)
                held = port_problems(self.ports, self.expected, self.sitl, skip=skip)
            self.leftovers += held
        with self.launcher.lock:
            if self.launcher.relay.target == self.ports["http"] and self.launcher.current is self:
                self.launcher.relay.target = None


# ---- the launcher -----------------------------------------------------------------------------------


class Launcher:
    def __init__(self, console_port: int, api_port: int, log_dir: Path):
        self.console_port = console_port
        self.api_port = api_port
        self.log_dir = log_dir
        self.lock = threading.RLock()
        self.current: Launch | None = None
        self.relay = Relay(api_port + 1)
        self.entries = catalog()
        self.notes = log_dir / "launcher.log"
        self.web = WebConsole(console_port, api_port, log_dir, lambda: self.current is not None and self.current.active(), self.note)

    def note(self, text: str) -> None:
        line = f"{dt.datetime.now(dt.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')} {text}"
        try:
            with self.notes.open("a") as f:
                f.write(line + "\n")
        except OSError:
            pass
        say(text)

    def state(self) -> dict[str, Any]:
        with self.lock:
            return {
                "launcher": {"console_port": self.console_port, "api_port": self.api_port},
                "console": self.web.public(),
                "scenarios": self.entries,
                "current": self.current.public() if self.current is not None else None,
                "now": round(time.time(), 3),
            }

    def start(self, body: Any) -> tuple[int, dict[str, Any]]:
        choice = validate_start(body, self.entries)
        entry = next(e for e in self.entries if e["id"] == choice.scenario)
        with self.lock:
            cur = self.current
            previous = None
            if cur is not None and cur.active():
                if not choice.replace:
                    return 409, {"detail": f"{cur.title} is running: end it first (or start with replace)", "current": cur.public()}
                cur.request_end()
                previous = cur
            launch = Launch(self, choice, entry, previous)
            self.current = launch
        launch.thread.start()
        return 202, {"current": launch.public()}

    def end(self) -> tuple[int, dict[str, Any]]:
        with self.lock:
            cur = self.current
            if cur is None or not cur.active():
                return 409, {"detail": "nothing is running"}
            cur.request_end()
            return 202, {"current": cur.public()}

    def shutdown(self) -> None:
        cur = self.current
        if cur is not None and cur.active():
            say(f"ending {cur.title} before the launcher stops (as make down)")
            cur.request_end()
            cur.thread.join(timeout=STACK_STOP_S + PORTS_FREE_S + 30)


class Api(BaseHTTPRequestHandler):
    server_version = "fleetmesh-launcher"
    launcher: Launcher  # set on the class by serve()

    def reply(self, code: int, body: dict[str, Any]) -> None:
        data = json.dumps(body, default=str).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802 (the stdlib's name)
        if self.path.split("?", 1)[0] == "/launcher/api/state":
            self.reply(200, self.launcher.state())
        else:
            self.reply(404, {"detail": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        path = self.path.split("?", 1)[0]
        if path not in ("/launcher/api/start", "/launcher/api/end", "/launcher/api/console"):
            return self.reply(404, {"detail": "not found"})
        why = refusal("POST", self.headers, self.launcher.console_port)
        if why:
            return self.reply(403, {"detail": why})
        try:
            length = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            length = -1
        if not 0 <= length <= 4096:
            return self.reply(413 if length > 4096 else 400, {"detail": "a small JSON body"})
        try:
            body = json.loads(self.rfile.read(length) or b"{}")
        except ValueError:
            return self.reply(400, {"detail": "the body is not JSON"})
        try:
            if path.endswith("/console"):
                code, out = self.launcher.web.request(body.get("mode") if isinstance(body, dict) else None)
            elif path.endswith("/start"):
                code, out = self.launcher.start(body)
            else:
                code, out = self.launcher.end()
        except BadRequest as e:
            code, out = 400, {"detail": str(e)}
        except Exception as e:  # noqa: BLE001 (said to the menu, and in the launcher's log)
            self.launcher.note(f"{path} failed: {e!r}")
            code, out = 500, {"detail": f"the launcher failed ({type(e).__name__}); see logs/launcher/launcher.log"}
        self.reply(code, out)

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002 (the stdlib's name)
        pass


# ---- main -------------------------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m eval.launcher", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=int(os.environ.get("FLEETMESH_LAUNCHER_PORT") or LAUNCHER_PORT),
                    help="the launcher's API (default 8760, FLEETMESH_LAUNCHER_PORT); its bridge relay is the next port")
    args = ap.parse_args(argv)
    console_port = int(os.environ.get("FLEETMESH_WEB_PORT") or CONSOLE_PORT)
    taken = [f"{what} :{p}" for what, p in (("the console", console_port), ("the launcher", args.port), ("its bridge relay", args.port + 1)) if tcp_answers(p)]
    if taken:
        print(
            f"launcher: something already answers on {', '.join(taken)}: a launcher already running (open "
            f"http://localhost:{console_port}), or a stack started with make up or make start SCENARIO= (`make down` stops it)",
            file=sys.stderr,
        )
        return 2
    log_dir = REPO / "logs" / "launcher"
    log_dir.mkdir(parents=True, exist_ok=True)
    # sim/swarm.py reads it as it loads a fleet: the menu counts each scenario at its own scales
    os.environ.pop("FLEETMESH_SCENARIO_SCALE", None)
    say("reading the scenarios")
    launcher = Launcher(console_port, args.port, log_dir)
    stop = threading.Event()

    def on_signal(sig: int, _frame: Any) -> None:
        stop.set()

    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, on_signal)
    Api.launcher = launcher
    api = ThreadingHTTPServer(("127.0.0.1", args.port), Api)
    api.daemon_threads = True
    threading.Thread(target=api.serve_forever, name="api", daemon=True).start()
    threading.Thread(target=launcher.relay.serve, name="relay", daemon=True).start()
    pidfile = REPO / "logs" / "launcher.pid"
    pidfile.write_text(f"{os.getpid()}\n")
    try:
        say(f"serving the console as the {'production build' if launcher.web.mode == 'build' else 'development server'} (built first if the sources changed)")
        launcher.web.start()
        runnable = [e["title"] for e in launcher.entries if e["runnable"]]
        say(f"{len(runnable)} scenarios: {'; '.join(runnable)}")
        say(f"open http://localhost:{console_port} for the menu (Ctrl-C ends the running scenario and stops the launcher)")
        while not stop.wait(0.5):
            if not launcher.web.alive():
                say(f"the console's server stopped; see {rel(log_dir / 'web.log')}")
                break
    except (LaunchError, RuntimeError) as e:
        print(f"launcher: {e}", file=sys.stderr)
        return 1
    finally:
        launcher.shutdown()
        launcher.web.close()
        api.shutdown()
        launcher.relay.close()
        try:
            pidfile.unlink()
        except OSError:
            pass
        say("stopped")
    return 0


if __name__ == "__main__":
    sys.exit(main())
