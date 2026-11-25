"""Situations: pre-built, pre-tasked scenarios started with one command (`make situation
NAME=<name>`), to watch the C2 work and an AI respond, with a person at the console or a
scripted operator on autoplay. The format is documented in `sim/situations/README.md`; the
runner is `eval/situation_run.py`. This module is the format and the runner's pure pieces
(the status document, the AI's command line and preflight, the port check, the audit tail),
so they can be tested without a stack.

A situation extends the evaluation's spec (`eval/spec.py`): the same fleet, events
and policy files, the same `[[step]]` kinds run by the same executor (`eval/storyline.py`),
plus what a watch needs: a brief for the operator and the AI, director's notes shown only when
revealed, the operator and AI modes, opening orders that put the fleet on station, and "what
to watch for" items for the console.

Everything a situation shows is display: the status document goes to the bridge's
`/api/sim/situation` and is never a decision input (crates/fleetmesh-bridge/src/situation.rs).
"""

from __future__ import annotations

import json
import os
import re
import shutil
import socket
import subprocess
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from . import REPO, TIDEWATER_HOME
from .spec import Step, _check_step

SITUATIONS = REPO / "sim" / "situations"
OPERATOR_MODES = ("live", "autoplay")
AI_MODES = ("claude", "codex", "none")
AI_LABEL = {
    "claude": "Claude",
    "codex": "ChatGPT (Codex)",
    "none": "None",
}
ORDER_TASKS = ("orbit", "patrol", "transit", "hold", "rtl")  # transit: rules 34
STORYLINE_PROPOSERS = ("agent", "operator")

# Who is who in a situation's dev run. op-1 is the console's operator: the person, in live
# mode, and never a script. Every scripted id is its own: op-setup issues the opening orders,
# op-auto decides in autoplay (the proxy model), op-planner is the timeline's second station.
# The AI is agent-watch (Claude or Codex); the timeline's MCP proposals come from
# agent-scripted, and an opening order that would be authorized by its own submission is
# proposed by agent-setup so a person (or op-auto) decides it. agent-planner is the person's own
# agent (the harness they opened in the checkout, AGENTS.md "Helping someone use FleetMesh"), as on
# the default stack: no script acts as it, and it is not the situation's AI.
CONSOLE_OPERATOR = "op-1"
SETUP_OPERATOR = "op-setup"
AUTOPLAY_OPERATOR = "op-auto"
STORYLINE_OPERATOR = "op-planner"
OPERATORS = [CONSOLE_OPERATOR, AUTOPLAY_OPERATOR, SETUP_OPERATOR, STORYLINE_OPERATOR]
WATCH_AGENT = "agent-watch"
STORYLINE_AGENT = "agent-scripted"
SETUP_AGENT = "agent-setup"
OWN_AGENT = "agent-planner"
AGENTS = [WATCH_AGENT, STORYLINE_AGENT, SETUP_AGENT, OWN_AGENT]
# how every scripted act announces itself in the audit log (notes, rationales)
SCRIPTED_SETUP = "situation setup (scripted, not a person)"
SCRIPTED_TIMELINE = "situation timeline (scripted, not a person)"
# how the console badges each scripted id (the status document's `scripted`)
SCRIPTED_BADGE = {
    SETUP_OPERATOR: "situation setup (scripted)",
    SETUP_AGENT: "situation setup (scripted)",
    STORYLINE_OPERATOR: "situation timeline (scripted)",
    STORYLINE_AGENT: "situation timeline (scripted)",
    AUTOPLAY_OPERATOR: "autoplay operator (scripted)",
}

# A situation's own port block by default (scripts/up.sh's variables override each): clear of
# the default stack (14560+, 15560+, 8765, 8766, 8767, 5173, 8770), a second stack (24560+,
# 25560+, 8865, 5273, 8866, 8867, 8870), the evaluation's blocks (UDP 35000-36999, HTTP
# 10600-10699) and the
# host's ephemeral range (39160-43255 here as of 2026-10-08; it moves between boots), so its
# fresh station never touches another station's restore or resume pointer. Its two UDP ranges
# hold up to 1000 vehicles each (`port_problems` refuses a larger fleet on them): a larger
# situation names its own block (`[ports]`, PORT_KEYS below).
# The evaluation's fixed UDP block (eval/ports.py): never a situation's.
EVAL_UDP = range(35000, 37000)

SITUATION_PORTS = {
    "FLEETMESH_BASE_PORT": "33000",
    "FLEETMESH_BRIDGE_BASE_PORT": "34000",
    "FLEETMESH_HTTP_PORT": "9265",
    "FLEETMESH_LINKEMU_PORT": "9266",
    "FLEETMESH_WEB_PORT": "5673",
    "FLEETMESH_TIDEWATER_PORT": "9270",
    "FLEETMESH_SITL_INSTANCE_BASE": "160",
}

# A situation may name its own port block (`[ports]`, each key one `scripts/up.sh` variable) in
# place of SITUATION_PORTS, for a fleet the common block can't carry (Saturation's 3,602 vehicles:
# its two UDP ranges hold 1,000 each). A key it leaves out is the common block's; an exported
# variable still overrides either. `standin_src_base` is the stand-ins' fixed source ports
# (FLEETMESH_STANDIN_SRC_BASE: one UDP port each from there, not from the host's ephemeral range).
# `link_host`, the one key that is not a port, moves the stack's UDP links to another loopback
# address (FLEETMESH_LINK_HOST): under WSL's mirrored networking 127.0.0.1 crosses a virtual NIC that
# drops packets at thousands of links, and 127.0.0.2 stays on `lo` (docs/load-test.md, Saturation).
PORT_KEYS = {
    "vehicle_base": "FLEETMESH_BASE_PORT",
    "bridge_base": "FLEETMESH_BRIDGE_BASE_PORT",
    "standin_src_base": "FLEETMESH_STANDIN_SRC_BASE",
    "http": "FLEETMESH_HTTP_PORT",
    "linkemu": "FLEETMESH_LINKEMU_PORT",
    "web": "FLEETMESH_WEB_PORT",
    "tidewater": "FLEETMESH_TIDEWATER_PORT",
    "sitl_instance_base": "FLEETMESH_SITL_INSTANCE_BASE",
    "link_host": "FLEETMESH_LINK_HOST",
}

TOP_KEYS = {
    "title", "brief", "summary", "director", "todo", "world", "fleet", "events", "policy", "duration_s",
    "rate", "at", "tidewater", "operator", "ai", "reveal", "seed", "ready_timeout_s", "ai_start_s",
    "order", "step", "watch", "storyline", "autoplay", "headless", "tutorial", "areas", "ports",
    "links",
}
# `[headless]`: what the headless simulator takes from the world (docs/HEADLESS.md, worlds); a
# live run reads tidewater itself and ignores the table. `domain_rule`: "grid", the world's baked
# land/water grid answers the bridge's domain rule (the default), or "not_evaluated", no terrain
# source, as every headless run before world v3; `currents`: "world", the world's baked current
# field moves drifting objects, and our boats and drifters get their water (current and depth)
# from the world (the default), or "none", still water, as before world v3; `conditions`:
# "world", each episode's conditions (its start, visibility, wind, sea state) drawn from the
# world's `[conditions]` ranges by the episode seed, or "none" (the default).
HEADLESS_KEYS = {"domain_rule", "currents", "conditions"}
DOMAIN_RULES = ("grid", "not_evaluated")
CURRENTS = ("world", "none")
CONDITIONS = ("world", "none")
ORDER_KEYS = {"id", "task", "vehicles", "center", "radius_m", "route", "reserve_pct", "duration_s", "note", "group"}
#: decision rules 34: an opening order to several vehicles is one group order; its `group` table
#: says how the members share it (crates/fleetmesh-decide/src/group.rs `GroupSpec`, which the bridge
#: checks: absent, the kind's defaults)
GROUP_KEYS = {"spread", "min_members", "alt_m", "wait_s", "reform", "lanes", "lane_spacing_m", "swarm"}
# what each step kind may carry (eval/storyline.py's docstring), besides id, at_s and kind
STEP_COMMON = {"text", "director"}
STEP_KEYS = {
    "note": set(),
    "propose": {"task", "vehicles", "center", "radius_m", "route", "reserve_pct", "duration_s",
                "rationale", "label", "reason", "premise", "withdraw_after_s", "retry_s"},
    "flood": {"count", "interval_s", "templates", "label"},
    "link": {"vehicles", "cut", "loss", "extra_latency_ms", "duration_s", "delay_s"},
}
# the faults only the headless simulator makes (docs/HEADLESS.md, faults and timelines; each
# modelled as the stand-ins or ArduPilot behave): the live runner refuses a situation with one
# (`resolve_modes`). Live, a GNSS jammer or spoofer is an events file's `gnss` table, which the
# headless simulator reads too.
#   gnss     effect = "jam" | "spoof", from at_s for duration_s (default: to the end), on the
#            vehicles selected (default all) and, if given, inside circle = {lat, lon, radius_m}
#            (a jammer needs one); jam: power_w (EIRP; none: no fix in the circle), height_m;
#            spoof: walk_mps, bearing_deg, max_offset_m (crates/fleetmesh-standins/src/gnss.rs)
#   battery  drop_pct: that share of the pack's capacity lost at once (a damaged cell); rate:
#            the draw times this from now, for duration_s (default: to the end)
#   reboot   the autopilot restarts: silent for down_s (default 3), then heard again
#   stop     the vehicle stops making way (its cruise and top speed set to zero)
#   current  a uniform current east_mps, north_mps in place of the world's for duration_s
#            (default: to the end)
HEADLESS_STEP_KEYS = {
    "gnss": {"effect", "vehicles", "circle", "power_w", "height_m", "walk_mps", "bearing_deg", "max_offset_m",
             "duration_s"},
    "battery": {"vehicles", "drop_pct", "rate", "duration_s"},
    "reboot": {"vehicles", "down_s"},
    "stop": {"vehicles"},
    "current": {"east_mps", "north_mps", "duration_s"},
}
STEP_KEYS.update(HEADLESS_STEP_KEYS)
WATCH_KEYS = {"at_s", "text", "director", "headline", "bullets"}


class SituationError(ValueError):
    pass


@dataclass
class Order:
    """An opening order: an ordinary operator order (op-1, over the operator API) issued once
    the fleet is ready, so the vehicles start on station."""

    id: str
    task: str
    vehicles: Any
    params: dict[str, Any]
    note: str = ""


@dataclass
class Watch:
    at_s: float
    text: str
    # shown only when revealed (the bridge strips it from a blinded status too)
    director: bool = False
    # optional glance at `text` (a few words, then 1 to 3 short bullets), shown first with the
    # time and `text` behind a click; display only (the AI never reads a watch item)
    headline: str = ""
    bullets: tuple[str, ...] = ()


@dataclass
class Situation:
    path: Path
    name: str
    title: str
    brief: str
    director: str
    todo: bool
    # the operator's glance at the brief: a headline and a few bullets (`[summary]`), shown first
    # with the whole brief behind a click. Display only: the AI is given the brief, never this.
    summary: dict[str, Any] | None = None
    # the world it runs in (sim/worlds/<world>.toml; eval/world.py): its fleet, `at` and
    # tidewater scenario are the situation's unless it names its own
    world: str = "golden-gate"
    fleet: Path | None = None
    events: list[Path] = field(default_factory=list)
    policy: Path | None = None
    # the operator's areas, a directory of *.geojson (the bridge's FLEETMESH_AREAS); None: sim/areas
    areas: Path | None = None
    # its own link profiles and ground stations, a links file in sim/links.toml's format, which
    # the runner hands every process that reads them as FLEETMESH_LINKS (the link emulator, the
    # bridge's coverage and estimates, the stand-ins, sim/swarm.py's lost-link timers); None:
    # whatever FLEETMESH_LINKS the runner was given, else sim/links.toml, as before. Live only: the
    # headless simulator reads its world's (`[links]` in sim/worlds/<world>.toml).
    links: Path | None = None
    # its own port block (`[ports]`: up.sh variable -> port), over SITUATION_PORTS
    ports: dict[str, str] = field(default_factory=dict)
    duration_s: float = 600.0
    rate: float = 1.0
    at: str = "2026-06-14T10:00:00Z"
    tidewater: Path | None = None
    tidewater_wanted: str = "none"
    operator: str = "live"
    ai: str = "claude"
    reveal: bool = False
    # the console's guided tutorial runs on it (web/src/Tutorial.tsx): its modes are fixed (live,
    # the file's AI, never revealed) and the menu offers it apart, at the top
    tutorial: bool = False
    seed: int = 1
    ready_timeout_s: float = 180.0
    ai_start_s: float = 30.0
    orders: list[Order] = field(default_factory=list)
    steps: list[Step] = field(default_factory=list)
    watch: list[Watch] = field(default_factory=list)
    storyline_proposer: str = "agent"
    autoplay: dict[str, Any] = field(default_factory=dict)
    # [headless] (HEADLESS_KEYS): the headless simulator's terrain source and water
    domain_rule: str = "grid"
    currents: str = "world"
    conditions: str = "none"
    raw: dict[str, Any] = field(default_factory=dict)


def _path(v: Any, base: Path, what: str, sit: Path) -> Path:
    if not isinstance(v, str) or not v.strip():
        raise SituationError(f"{sit}: {what} must be a file path")
    p = Path(v).expanduser()
    if not p.is_absolute():
        p = (REPO / p) if (REPO / p).exists() or not (base / p).exists() else base / p
    if not p.exists():
        raise SituationError(f"{sit}: {what} {v!r} not found")
    return p.resolve()


def find_situation(name: str) -> Path:
    """A situation by name (`sim/situations/<name>.toml`) or path."""
    p = Path(name).expanduser()
    if p.suffix == ".toml" and p.exists():
        return p.resolve()
    cand = SITUATIONS / f"{name}.toml"
    if cand.exists():
        return cand
    known = ", ".join(sorted(f.stem for f in SITUATIONS.glob("*.toml")))
    raise SituationError(f"no situation {name!r}; in {SITUATIONS}: {known}")


def _lonlat(v: Any) -> bool:
    return isinstance(v, list) and len(v) == 2 and all(isinstance(x, (int, float)) for x in v)


def _num(v: Any, what: str, sit: Path, kind: type = float) -> Any:
    """A number from the file, or a SituationError saying which key is wrong."""
    if isinstance(v, bool):
        raise SituationError(f"{sit}: {what} must be a number, not {v!r}")
    try:
        return kind(v)
    except (TypeError, ValueError):
        raise SituationError(f"{sit}: {what} must be a number, not {v!r}") from None


def _table(v: Any, what: str, sit: Path) -> dict[str, Any]:
    """A TOML table from the file, or a SituationError saying which one is not."""
    if v is None:
        return {}
    if not isinstance(v, dict):
        raise SituationError(f"{sit}: {what} must be a table, not {v!r}")
    return dict(v)


def _order(i: int, raw: Any, sit: Path) -> Order:
    if not isinstance(raw, dict):
        raise SituationError(f"{sit}: order {i + 1} is not a table")
    unknown = set(raw) - ORDER_KEYS
    if unknown:
        raise SituationError(f"{sit}: order {i + 1}: unknown keys {sorted(unknown)}")
    oid = str(raw.get("id", f"order-{i + 1}"))
    task = raw.get("task")
    if task not in ORDER_TASKS:
        raise SituationError(f"{sit}: order {oid!r}: task {task!r} not in {ORDER_TASKS}")
    if "vehicles" not in raw:
        raise SituationError(
            f"{sit}: order {oid!r}: needs vehicles ({{callsign = ...}}, {{platform = ...}}, {{uids = [...]}} or {{swarm = ...}})"
        )
    params = {k: raw[k] for k in ("center", "radius_m", "route", "reserve_pct", "duration_s") if k in raw}
    if "group" in raw:
        g = raw["group"]
        if not isinstance(g, dict) or set(g) - GROUP_KEYS:
            raise SituationError(f"{sit}: order {oid!r}: group is a table of {sorted(GROUP_KEYS)}")
        # passed on as the bridge's own key (the operator API's and MCP's `group`)
        params["extra"] = {"group": dict(g)}
    if task == "orbit":
        if not _lonlat(params.get("center")) or not isinstance(params.get("radius_m"), (int, float)) or params["radius_m"] <= 0:
            raise SituationError(f"{sit}: order {oid!r}: an orbit needs center = [lon, lat] and radius_m > 0")
    if task == "patrol":
        route = params.get("route")
        if not isinstance(route, list) or len(route) < 2 or not all(_lonlat(p) for p in route):
            raise SituationError(f"{sit}: order {oid!r}: a patrol needs route = [[lon, lat], ...] (two points or more)")
    if task == "transit":
        route = params.get("route")
        if not isinstance(route, list) or not route or not all(_lonlat(p) for p in route):
            raise SituationError(f"{sit}: order {oid!r}: a transit needs route = [[lon, lat], ...] (one point or more)")
    if "center" in params and not _lonlat(params["center"]):
        raise SituationError(f"{sit}: order {oid!r}: center is [lon, lat] (opening orders name places)")
    return Order(id=oid, task=str(task), vehicles=raw["vehicles"], params=params, note=str(raw.get("note", "")))


def _steps(raw: list[Any], sit: Path, duration_s: float) -> list[Step]:
    """The timeline: the storyline's steps (eval/storyline.py), checked as the evaluation
    checks them, each kind with its own keys only. A situation proposal carries no truth
    label, so the evaluation's `reason` may be left out; `text` (what the panel says when the
    step is done) and `director` (shown only when revealed) are the situation's own."""
    steps: list[Step] = []
    seen: set[str] = set()
    for i, s in enumerate(raw):
        if not isinstance(s, dict):
            raise SituationError(f"{sit}: step {i + 1} is not a table")
        s = dict(s)
        sid = str(s.pop("id", f"step-{i + 1}"))
        if sid in seen:
            raise SituationError(f"{sit}: duplicate step id {sid!r}")
        seen.add(sid)
        kind = str(s.pop("kind", "note"))
        at_s = _num(s.pop("at_s", 0), f"step {sid!r}: at_s", sit)
        if kind not in STEP_KEYS:
            raise SituationError(f"{sit}: step {sid!r}: kind {kind!r} not in {sorted(STEP_KEYS)}")
        unknown = set(s) - STEP_KEYS[kind] - STEP_COMMON
        if unknown:
            raise SituationError(f"{sit}: step {sid!r}: a {kind} step takes no {sorted(unknown)}")
        if not 0 <= at_s < duration_s:
            raise SituationError(f"{sit}: step {sid!r}: at_s {at_s:g} is outside the situation (0 to {duration_s:g} s)")
        if "director" in s and not isinstance(s["director"], bool):
            raise SituationError(f"{sit}: step {sid!r}: director is true or false")
        if kind == "flood":
            for k, t in enumerate(s.get("templates") or []):
                if not isinstance(t, dict):
                    raise SituationError(f"{sit}: step {sid!r}: template {k + 1} is not a table")
                extra = set(t) - STEP_KEYS["propose"]
                if extra:
                    raise SituationError(f"{sit}: step {sid!r}: template {k + 1} takes no {sorted(extra)} (a proposal's keys)")
        check = dict(s)
        if kind == "propose":
            check.setdefault("reason", "a situation step: no truth label")
        if kind == "flood":
            check["templates"] = [{"reason": "a situation step: no truth label", **t} for t in s.get("templates", [])]
        if kind in HEADLESS_STEP_KEYS:
            _headless_step(sid, kind, s, sit)
            steps.append(Step(id=sid, kind=kind, at_s=at_s, params=s))
            continue
        try:
            _check_step(Step(id=sid, kind=kind, at_s=at_s, params=check))
        except ValueError as e:
            raise SituationError(f"{sit}: {e}") from e
        steps.append(Step(id=sid, kind=kind, at_s=at_s, params=s))
    steps.sort(key=lambda s: s.at_s)
    return steps


def _headless_step(sid: str, kind: str, s: dict[str, Any], sit: Path) -> None:
    """A headless fault step's values (HEADLESS_STEP_KEYS), each of the right type and range."""
    def num(k: str, lo: float | None = None, hi: float | None = None) -> float | None:
        if k not in s:
            return None
        v = _num(s[k], f"step {sid!r}: {k}", sit)
        if lo is not None and v < lo:
            raise SituationError(f"{sit}: step {sid!r}: {k} {v:g} is below {lo:g}")
        if hi is not None and v > hi:
            raise SituationError(f"{sit}: step {sid!r}: {k} {v:g} is above {hi:g}")
        return v

    if "vehicles" in s and not (isinstance(s["vehicles"], dict) and {"uids", "callsign", "platform"} & set(s["vehicles"])):
        raise SituationError(f"{sit}: step {sid!r}: vehicles is {{callsign = ...}}, {{platform = ...}} or {{uids = [...]}}")
    if kind in ("battery", "reboot", "stop") and "vehicles" not in s:
        raise SituationError(f"{sit}: step {sid!r}: a {kind} step needs vehicles")
    if num("duration_s", 0.0) == 0.0:
        raise SituationError(f"{sit}: step {sid!r}: duration_s > 0 (leave it out for the rest of the run)")
    if kind == "gnss":
        effect = s.get("effect")
        if effect not in ("jam", "spoof"):
            raise SituationError(f"{sit}: step {sid!r}: effect is jam or spoof")
        c = s.get("circle")
        if c is not None and not (isinstance(c, dict) and set(c) == {"lat", "lon", "radius_m"}):
            raise SituationError(f"{sit}: step {sid!r}: circle is {{lat, lon, radius_m}}")
        if effect == "jam" and c is None:
            raise SituationError(f"{sit}: step {sid!r}: a jammer needs a circle (its footprint's outer limit)")
        jam, spoof = {"power_w", "height_m"}, {"walk_mps", "bearing_deg", "max_offset_m"}
        wrong = set(s) & (spoof if effect == "jam" else jam)
        if wrong:
            raise SituationError(f"{sit}: step {sid!r}: {sorted(wrong)} are not {effect} fields")
        num("power_w", 1e-9)
        num("height_m", 0.0)
        num("walk_mps", 0.0)
        num("bearing_deg", 0.0, 360.0)
        num("max_offset_m", 0.0)
    elif kind == "battery":
        if num("drop_pct", 0.0, 100.0) is None and num("rate", 0.0) is None:
            raise SituationError(f"{sit}: step {sid!r}: a battery step needs drop_pct (a step) or rate (a new draw)")
        if "duration_s" in s and "rate" not in s:
            raise SituationError(f"{sit}: step {sid!r}: duration_s is the rate's (a drop is at once)")
    elif kind == "reboot":
        num("down_s", 0.0, 600.0)
    elif kind == "current":
        if num("east_mps", -10.0, 10.0) is None or num("north_mps", -10.0, 10.0) is None:
            raise SituationError(f"{sit}: step {sid!r}: a current step needs east_mps and north_mps")


def headless_steps(sit: Situation) -> list[Step]:
    """The steps only the headless simulator can run (HEADLESS_STEP_KEYS)."""
    return [s for s in sit.steps if s.kind in HEADLESS_STEP_KEYS]


def _links_file(p: Path, sit: Path) -> None:
    """`links`: a links file in sim/links.toml's format, read as the link emulator reads it: its
    profiles (`[class.*]`) and its radio sites, one `[ground_station]` table or several
    `[[ground_station]]` tables, each with `lat`, `lon` and `alt_m` (metres, or "dem"). The
    emulator checks the rest when it starts."""
    try:
        raw = tomllib.loads(p.read_text())
    except (OSError, tomllib.TOMLDecodeError) as e:
        raise SituationError(f"{sit}: links {p}: {e}") from None
    if not isinstance(raw.get("class"), dict) or not raw["class"]:
        raise SituationError(f"{sit}: links {p}: no [class.*] profiles (sim/links.toml's format)")
    gs = raw.get("ground_station")
    sites = gs if isinstance(gs, list) else [gs] if isinstance(gs, dict) else []
    if not sites:
        raise SituationError(f"{sit}: links {p}: no ground station ([ground_station] or [[ground_station]])")
    for i, g in enumerate(sites):
        alt = g.get("alt_m") if isinstance(g, dict) else None
        ok = isinstance(g, dict) and all(isinstance(g.get(k), (int, float)) and not isinstance(g.get(k), bool) for k in ("lat", "lon"))
        if not ok or not (alt == "dem" or (isinstance(alt, (int, float)) and not isinstance(alt, bool))):
            raise SituationError(f"{sit}: links {p}: ground station {i + 1} needs lat, lon and alt_m (metres or \"dem\")")


def tidewater_scenario(v: Any) -> tuple[str, Path | None]:
    """`tidewater = "<scenario>"` (a name under tidewater/scenarios, or a path: a relative one
    is this repository's, as San Diego's sim/worlds/san-diego.tidewater.toml) or "none": what
    was asked and the file, when it exists here."""
    if v is None or v == "none":
        return "none", None
    name = str(v)
    p = Path(name).expanduser()
    if p.suffix != ".toml":
        p = TIDEWATER_HOME / "scenarios" / f"{name}.toml"
    elif not p.is_absolute() and not p.exists():
        p = REPO / p
    return name, (p if p.exists() else None)


WATCH_BULLETS_MAX = 3
SUMMARY_KEYS = {"headline", "bullets"}
SUMMARY_BULLETS_MAX = 5


def _summary(raw: Any, path: Path) -> dict[str, Any] | None:
    """`[summary]`: `headline` (one line) and `bullets` (1 to 5 short lines), what the panel and
    the menu show first. Optional; display only (the AI is given the brief, byte for byte)."""
    if raw is None:
        return None
    if not isinstance(raw, dict) or set(raw) - SUMMARY_KEYS:
        raise SituationError(f"{path}: summary is a table with headline and bullets only")
    headline = raw.get("headline")
    bullets = raw.get("bullets")
    if not isinstance(headline, str) or not headline.strip() or "\n" in headline.strip():
        raise SituationError(f"{path}: summary.headline is one line of text")
    if not isinstance(bullets, list) or not 1 <= len(bullets) <= SUMMARY_BULLETS_MAX or not all(isinstance(b, str) and b.strip() for b in bullets):
        raise SituationError(f"{path}: summary.bullets is 1 to {SUMMARY_BULLETS_MAX} lines of text")
    return {"headline": headline.strip(), "bullets": [b.strip() for b in bullets]}


def _watch_doc(w: Watch) -> dict[str, Any]:
    """A watch item as the status document carries it: the glance (`headline`, `bullets`) only
    when the file wrote one."""
    d: dict[str, Any] = {"at_s": w.at_s, "text": w.text, "director": w.director}
    if w.headline:
        d["headline"] = w.headline
        d["bullets"] = list(w.bullets)
    return d


def load_situation(path_or_name: str | Path) -> Situation:
    path = find_situation(str(path_or_name))
    try:
        raw = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as e:
        raise SituationError(f"{path}: {e}") from e
    unknown = set(raw) - TOP_KEYS
    if unknown:
        raise SituationError(f"{path}: unknown keys {sorted(unknown)} (sim/situations/README.md lists them)")
    title = str(raw.get("title", "")).strip()
    brief = str(raw.get("brief", "")).strip()
    if not title or not brief:
        raise SituationError(f"{path}: needs a title and a brief")
    summary = _summary(raw.get("summary"), path)
    base = path.parent
    sit = Situation(
        path=path,
        name=path.stem,
        title=title,
        brief=brief,
        director=str(raw.get("director", "")).strip(),
        todo=bool(raw.get("todo", False)),
        summary=summary,
        raw=raw,
    )
    if sit.todo:
        # a stub: title and brief only, written later; nothing else is read
        return sit
    from .world import WorldError, load_world

    try:
        world = load_world(str(raw.get("world", sit.world)))
    except WorldError as e:
        raise SituationError(f"{path}: world: {e}") from None
    sit.world = world.name
    # the world's fleet, scenario time and tidewater scenario unless the situation names its own
    sit.fleet = _path(raw["fleet"], base, "fleet", path) if "fleet" in raw else world.fleet
    raw_at = raw.get("at", world.at or sit.at)
    raw_tidewater = raw.get("tidewater", world.tidewater)
    hl = _table(raw.get("headless"), "[headless]", path)
    if set(hl) - HEADLESS_KEYS:
        raise SituationError(f"{path}: [headless] takes {sorted(HEADLESS_KEYS)}, not {sorted(set(hl) - HEADLESS_KEYS)}")
    sit.domain_rule = str(hl.get("domain_rule", sit.domain_rule))
    if sit.domain_rule not in DOMAIN_RULES:
        raise SituationError(f"{path}: [headless] domain_rule is one of {DOMAIN_RULES}")
    sit.currents = str(hl.get("currents", sit.currents))
    if sit.currents not in CURRENTS:
        raise SituationError(f"{path}: [headless] currents is one of {CURRENTS}")
    sit.conditions = str(hl.get("conditions", sit.conditions))
    if sit.conditions not in CONDITIONS:
        raise SituationError(f"{path}: [headless] conditions is one of {CONDITIONS}")
    if sit.conditions == "world" and not world.conditions:
        raise SituationError(f"{path}: [headless] conditions world, but {world.name} has no [conditions]")
    events = raw.get("events", [])
    if isinstance(events, str):
        events = [events]
    if not isinstance(events, list):
        raise SituationError(f"{path}: events is a file or a list of files")
    sit.events = [_path(e, base, "events", path) for e in events]
    sit.policy = _path(raw.get("policy", "sim/policy.toml"), base, "policy", path)
    if "areas" in raw:
        sit.areas = _path(raw["areas"], base, "areas", path)
        if not sit.areas.is_dir() or not any(sit.areas.glob("*.geojson")):
            raise SituationError(f"{path}: areas is a directory of *.geojson (the bridge's FLEETMESH_AREAS)")
    if "links" in raw:
        sit.links = _path(raw["links"], base, "links", path)
        _links_file(sit.links, path)
    ports = _table(raw.get("ports"), "[ports]", path)
    if set(ports) - set(PORT_KEYS):
        raise SituationError(f"{path}: [ports] takes {sorted(PORT_KEYS)}, not {sorted(set(ports) - set(PORT_KEYS))}")
    for k, v in ports.items():
        if k == "link_host":
            if not isinstance(v, str) or not re.fullmatch(r"127(\.\d{1,3}){3}", v):
                raise SituationError(f"{path}: [ports] link_host is a loopback address (127.x.y.z), not {v!r}")
        elif isinstance(v, bool) or not isinstance(v, int) or not 0 < v < 65536:
            raise SituationError(f"{path}: [ports] {k} is a port number, not {v!r}")
    sit.ports = {PORT_KEYS[k]: str(v) for k, v in ports.items()}
    sit.duration_s = _num(raw.get("duration_s", 600), "duration_s", path)
    if not sit.duration_s > 0:
        raise SituationError(f"{path}: duration_s must be > 0")
    sit.rate = _num(raw.get("rate", 1), "rate", path)
    if not sit.rate > 0:
        raise SituationError(f"{path}: rate must be > 0")
    sit.at = str(raw_at)
    sit.tidewater_wanted, sit.tidewater = tidewater_scenario(raw_tidewater)
    sit.operator = str(raw.get("operator", "live"))
    if sit.operator not in OPERATOR_MODES:
        raise SituationError(f"{path}: operator {sit.operator!r} not in {OPERATOR_MODES}")
    sit.ai = str(raw.get("ai", "claude"))
    if sit.ai not in AI_MODES:
        raise SituationError(f"{path}: ai {sit.ai!r} not in {AI_MODES}")
    if not isinstance(raw.get("reveal", False), bool):
        raise SituationError(f"{path}: reveal is true or false")
    sit.reveal = bool(raw.get("reveal", False))
    if not isinstance(raw.get("tutorial", False), bool):
        raise SituationError(f"{path}: tutorial is true or false")
    sit.tutorial = bool(raw.get("tutorial", False))
    if sit.tutorial and (sit.operator != "live" or sit.reveal):
        raise SituationError(f"{path}: a tutorial is live (the person makes every human decision) and never revealed")
    sit.seed = _num(raw.get("seed", 1), "seed", path, int)
    sit.ready_timeout_s = _num(raw.get("ready_timeout_s", 180), "ready_timeout_s", path)
    sit.ai_start_s = _num(raw.get("ai_start_s", 30), "ai_start_s", path)
    if sit.ai_start_s < 0 or sit.ai_start_s >= sit.duration_s:
        raise SituationError(f"{path}: ai_start_s must be >= 0 and before the end")
    orders = raw.get("order", [])
    if not isinstance(orders, list):
        raise SituationError(f"{path}: [[order]] is a list of tables")
    sit.orders = [_order(i, o, path) for i, o in enumerate(orders)]
    ids = [o.id for o in sit.orders]
    if len(set(ids)) != len(ids):
        raise SituationError(f"{path}: duplicate order ids")
    _orders_against_fleet(sit)
    steps = raw.get("step", [])
    if not isinstance(steps, list):
        raise SituationError(f"{path}: [[step]] is a list of tables")
    sit.steps = _steps(steps, path, sit.duration_s)
    _steps_against_fleet(sit)
    watch = raw.get("watch", [])
    if not isinstance(watch, list):
        raise SituationError(f"{path}: [[watch]] is a list of tables")
    for i, w in enumerate(watch):
        if not isinstance(w, dict) or set(w) - WATCH_KEYS:
            raise SituationError(f"{path}: watch {i + 1}: a table of {sorted(WATCH_KEYS)}")
        at_s = _num(w.get("at_s", -1), f"watch {i + 1}: at_s", path)
        if not str(w.get("text", "")).strip() or not 0 <= at_s < sit.duration_s:
            raise SituationError(f"{path}: watch {i + 1} needs text and at_s from 0 to before duration_s")
        if not isinstance(w.get("director", False), bool):
            raise SituationError(f"{path}: watch {i + 1}: director is true or false")
        headline = w.get("headline", "")
        bullets = w.get("bullets", [])
        if not isinstance(headline, str) or "\n" in headline.strip():
            raise SituationError(f"{path}: watch {i + 1}: headline is one line of text")
        if not isinstance(bullets, list) or len(bullets) > WATCH_BULLETS_MAX or not all(isinstance(b, str) and b.strip() for b in bullets):
            raise SituationError(f"{path}: watch {i + 1}: bullets is up to {WATCH_BULLETS_MAX} lines of text")
        if bullets and not headline.strip():
            raise SituationError(f"{path}: watch {i + 1}: bullets need a headline")
        sit.watch.append(
            Watch(
                at_s=at_s,
                text=str(w["text"]).strip(),
                director=bool(w.get("director", False)),
                headline=headline.strip(),
                bullets=tuple(b.strip() for b in bullets),
            )
        )
    sit.watch.sort(key=lambda w: w.at_s)
    story = _table(raw.get("storyline"), "[storyline]", path)
    sit.storyline_proposer = str(story.get("proposer", "agent"))
    if sit.storyline_proposer not in STORYLINE_PROPOSERS or set(story) - {"proposer"}:
        raise SituationError(f"{path}: [storyline] proposer is one of {STORYLINE_PROPOSERS}")
    sit.autoplay = _table(raw.get("autoplay"), "[autoplay]", path)
    from .proxy import proxy_params

    try:
        proxy_params(sit.autoplay)
    except (TypeError, ValueError) as e:
        raise SituationError(f"{path}: [autoplay]: {e}") from None
    return sit


def _planned_fleet(fleet: Path) -> list[dict[str, Any]]:
    """The fleet as sim/swarm.py plans it: callsign, platform, uid in unit order."""
    from .stack import _swarm

    swarm = _swarm()
    vehicles = swarm.plan(swarm.load_fleet(str(fleet)), swarm.load_platforms())
    for i, v in enumerate(vehicles):
        v["uid"] = i + 1
    return vehicles


def swarm_uids(sel: Any, vehicles: list[dict[str, Any]]) -> list[int] | None:
    """`{swarm = "<id>"}`: the uids of that swarm's members as the fleet plans them (sim/swarm.py;
    every member, in uid order), so an opening order goes to the whole swarm as one intent. None
    for any other selector."""
    if not (isinstance(sel, dict) and set(sel) == {"swarm"}):
        return None
    return [int(v["uid"]) for v in vehicles if v.get("swarm") == sel["swarm"]]


def _select_planned(sel: Any, vehicles: list[dict[str, Any]]) -> list[dict[str, Any]]:
    swarm = swarm_uids(sel, vehicles)
    if swarm is not None:
        return [v for v in vehicles if v["uid"] in set(swarm)]
    if isinstance(sel, dict) and "uids" in sel:
        want = {int(u) for u in sel["uids"]}
        return [v for v in vehicles if v["uid"] in want]
    if isinstance(sel, dict) and "callsign" in sel:
        return [v for v in vehicles if v.get("callsign") == sel["callsign"]]
    if isinstance(sel, dict) and "platform" in sel:
        out = [v for v in vehicles if v.get("platform") == sel["platform"]]
        return out if sel.get("all") else out[int(sel.get("index", 0)) : int(sel.get("index", 0)) + 1]
    return []


def _orders_against_fleet(sit: Situation) -> None:
    """Every opening order names vehicles of the fleet (refused at load otherwise)."""
    if not sit.orders:
        return
    assert sit.fleet is not None
    vehicles = _planned_fleet(sit.fleet)
    for o in sit.orders:
        try:
            chosen = _select_planned(o.vehicles, vehicles)
        except (TypeError, ValueError, KeyError) as e:
            raise SituationError(f"{sit.path}: order {o.id!r}: vehicles {o.vehicles!r}: {e}") from None
        if not chosen:
            raise SituationError(f"{sit.path}: order {o.id!r}: no vehicle of {sit.fleet.name} matches {o.vehicles!r}")


def _steps_against_fleet(sit: Situation) -> None:
    """A timeline proposal or link step may name a swarm (`vehicles = { swarm = ... }`, as an
    opening order may): it names one of the fleet's, with members. The runner hands the storyline
    its members' uids (`with_swarm_uids`)."""
    named = [s for s in sit.steps if s.kind in ("propose", "link") and swarm_uids(s.params.get("vehicles"), []) is not None]
    if not named:
        return
    if sit.fleet is None:
        raise SituationError(f"{sit.path}: step {named[0].id!r} names a swarm, but the situation has no fleet")
    vehicles = _planned_fleet(sit.fleet)
    for s in named:
        if not swarm_uids(s.params["vehicles"], vehicles):
            raise SituationError(f"{sit.path}: step {s.id!r}: no swarm of {sit.fleet.name} is {s.params['vehicles']['swarm']!r}")


def with_swarm_uids(step: Step, vehicles: list[dict[str, Any]]) -> Step:
    """The step with a swarm selector (`{swarm = ...}`) replaced by its members' uids as the fleet
    plans them (`swarm_uids`), which the storyline selects by; any other step as it is."""
    uids = swarm_uids(step.params.get("vehicles"), vehicles)
    if uids is None:
        return step
    return Step(id=step.id, kind=step.kind, at_s=step.at_s, params={**step.params, "vehicles": {"uids": uids}})


def uses_planner(sit: Situation) -> bool:
    """Whether the timeline acts as op-planner: the operator proposer."""
    kinds = {s.kind for s in sit.steps}
    return sit.storyline_proposer == "operator" and bool(kinds & {"propose", "flood"})


def resolve_modes(sit: Situation, operator: str | None = None, ai: str | None = None, reveal: bool | None = None) -> dict[str, Any]:
    """The run's modes: the file's, with the command line's overrides."""
    op = operator or sit.operator
    a = ai or sit.ai
    if op not in OPERATOR_MODES:
        raise SituationError(f"operator {op!r}: one of {', '.join(OPERATOR_MODES)}")
    if a not in AI_MODES:
        raise SituationError(f"AI {a!r}: one of {', '.join(AI_MODES)}")
    rv = sit.reveal if reveal is None else bool(reveal)
    if sit.tutorial and (op, a, rv) != (sit.operator, sit.ai, sit.reveal):
        raise SituationError(
            f"{sit.name}: a tutorial runs with its own modes (operator {sit.operator}, AI {sit.ai}, "
            "not revealed), which the console's walk-through follows. Start it without OPERATOR, AI or REVEAL."
        )
    faults = sorted({s.kind for s in headless_steps(sit)})
    if faults:
        raise SituationError(
            f"{sit.name}: its timeline has {', '.join(faults)} steps, faults only the headless simulator makes "
            f"(docs/HEADLESS.md): run it with `.venv/bin/python -m eval.headless.batch --situation {sit.name}`. A live "
            "GNSS jammer or spoofer is an events file's gnss table."
        )
    if sit.conditions == "world":
        raise SituationError(
            f"{sit.name}: its conditions are drawn per episode from its world's ranges, which only the headless simulator "
            f"does (docs/HEADLESS.md, the three worlds): run it with `.venv/bin/python -m eval.headless.batch --situation "
            f"{sit.name}`."
        )
    # who acts as whom: op-1 is never a script; every scripted id the run uses is listed
    scripted: list[str] = []
    if sit.orders:
        scripted.append(SETUP_OPERATOR)
    if op == "autoplay":
        scripted += [AUTOPLAY_OPERATOR]
    if uses_planner(sit):
        scripted.append(STORYLINE_OPERATOR)
    # every scripted id, operators and agents, as the console badges it
    ids = list(scripted)
    if sit.orders:
        ids.append(SETUP_AGENT)
    if sit.storyline_proposer == "agent" and {s.kind for s in sit.steps} & {"propose", "flood"}:
        ids.append(STORYLINE_AGENT)
    return {
        "operator": op,
        "ai": a,
        "ai_label": AI_LABEL[a],
        "reveal": rv,
        # the console's guided walk-through runs over this situation (web/src/Tutorial.tsx)
        "tutorial": sit.tutorial,
        "console_operator": CONSOLE_OPERATOR,
        "autoplay_operator": AUTOPLAY_OPERATOR if op == "autoplay" else None,
        "scripted_operators": scripted,
        "scripted_badges": {i: SCRIPTED_BADGE[i] for i in ids},
        "agent": {"claude": WATCH_AGENT, "codex": WATCH_AGENT}.get(a),
        # a model watching with the director's notes served on the bridge: what it did is no
        # evidence about it (it could have read the truth)
        "evidence_note": (
            "not evidence about the AI (truth was served on the bridge)" if rv and a in ("claude", "codex") else None
        ),
    }


# ---- the status document ---------------------------------------------------------------------


def step_text(rec: dict[str, Any], step: Step | None) -> str:
    """One line for the panel from a storyline record (eval/storyline.py) and its step."""
    kind = rec.get("kind")
    headline = str((step.params.get("text") if step else "") or "")
    if kind == "note":
        detail = str(rec.get("text", ""))
    elif kind == "propose":
        req = rec.get("request") or {}
        who = (rec.get("vehicle") or {}).get("callsign") or ", ".join(str(u) for u in req.get("uids", []))
        state = f"error: {rec['error']}" if rec.get("error") else rec.get("state")
        detail = f"{req.get('task', '?')} for {who} proposed by {(rec.get('source') or {}).get('id', '?')}: intent {rec.get('intent_id')} {state}"
    elif kind == "link":
        eff = rec.get("effect") or {}
        what = "link cut" if eff.get("cut") else ("loss " + str(eff.get("loss")) if eff.get("loss") is not None else f"+{eff.get('extra_latency_ms')} ms")
        detail = f"{what} on {', '.join(str(c) for c in rec.get('callsigns') or [])}"
    elif kind == "skipped":
        detail = f"step {rec.get('step')} skipped: {rec.get('reason')}"
    elif kind == "error":
        detail = f"step {rec.get('step')} failed: {rec.get('error')}"
    else:
        detail = json.dumps({k: v for k, v in rec.items() if k not in ("t",)})[:200]
    if headline and detail and detail != headline:
        return f"{headline} ({detail})"
    return headline or detail


def status_doc(
    sit: Situation,
    modes: dict[str, Any],
    *,
    phase: str,
    phase_detail: str = "",
    elapsed_s: float | None,
    now: float,
    entries: list[dict[str, Any]],
    done_steps: set[str],
    orders: list[dict[str, Any]],
    agent: dict[str, Any],
    autoplay: dict[str, Any] | None,
    run: dict[str, Any],
    problems: list[str] | None = None,
) -> dict[str, Any]:
    """The document the runner posts to `/api/sim/situation` (display only). With `reveal`
    false it carries no director's notes, no timeline still to come and no step or watch item
    marked `director = true`: nothing a blinded operator should not see (the bridge strips the
    same again, whatever it is sent)."""
    reveal = bool(modes.get("reveal"))
    el = None if elapsed_s is None else round(max(0.0, elapsed_s), 1)
    shown = [e for e in entries if reveal or not e.get("director")]
    watch = [w for w in sit.watch if reveal or not w.director]
    doc: dict[str, Any] = {
        "schema": 1,
        "name": sit.name,
        "title": sit.title,
        "brief": sit.brief,
        **({"summary": sit.summary} if sit.summary else {}),
        "reveal": reveal,
        "modes": {
            k: modes.get(k)
            for k in ("operator", "ai", "ai_label", "console_operator", "autoplay_operator", "scripted_operators", "evidence_note", "tutorial")
        },
        "phase": phase,
        "phase_detail": phase_detail,
        "clock": {
            "t": round(now, 3),
            "elapsed_s": el,
            "duration_s": sit.duration_s,
            "remaining_s": None if el is None else round(max(0.0, sit.duration_s - el), 1),
        },
        "orders": orders,
        "timeline": {"done": shown[-40:]},
        "watch": [_watch_doc(w) for w in watch if el is not None and w.at_s <= el],
        "agent": agent,
        "autoplay": autoplay,
        "run": run,
        # every scripted id of the run and its badge (the Inbox shows it on their cards)
        "scripted": dict(modes.get("scripted_badges") or {}),
        # what went wrong in the runner itself (a task that failed): its own
        # fixed messages, never a step's or the director's text
        "problems": list(problems or [])[-8:],
    }
    if reveal:
        doc["director"] = sit.director
        doc["timeline"]["upcoming"] = [
            {"at_s": s.at_s, "id": s.id, "kind": s.kind, "text": str(s.params.get("text") or s.kind)}
            for s in sit.steps
            if s.id not in done_steps
        ]
        doc["watch_upcoming"] = [_watch_doc(w) for w in watch if el is None or w.at_s > el]
    return doc


# ---- the AI ----------------------------------------------------------------------------------

FIX_LOGIN = {
    "claude": "run `claude`, then `/login` in it (your own Claude subscription or API key; FleetMesh never sees it)",
    "codex": "run `codex login` (your own ChatGPT plan or API key; FleetMesh never sees it)",
}
INSTALL = {
    "claude": "install Claude Code (https://docs.anthropic.com/en/docs/claude-code), then run `claude` and `/login`",
    "codex": "install the Codex CLI (`npm install -g @openai/codex`), then run `codex login`",
}


def ai_preflight(ai: str, run: Callable[..., Any] = subprocess.run, which: Callable[[str], str | None] = shutil.which) -> dict[str, Any]:
    """Before the stack starts: the chosen AI's CLI is installed and logged in, or a clear
    refusal saying how to fix it (and that AI=none is the free option). FleetMesh reads no
    credential: it asks the user's own CLI whether it is logged in and keeps only yes or no
    and the kind of login (never an account, an email or a key)."""
    if ai == "none":
        return {"ai": ai, "checked": False}
    free = "or run AI=none (no account needed)"
    exe = which(ai)
    if not exe:
        raise SituationError(f"AI={ai}: the `{ai}` command is not installed: {INSTALL[ai]}; {free}.")
    if ai == "claude":
        try:
            r = run([exe, "auth", "status"], capture_output=True, text=True, timeout=30)
        except (OSError, subprocess.SubprocessError) as e:
            raise SituationError(f"AI=claude: `claude auth status` failed ({e}): {FIX_LOGIN['claude']}; {free}.") from None
        try:
            status = json.loads(r.stdout) if (r.stdout or "").strip() else None
        except ValueError:
            status = None
        if status is None or not isinstance(status, dict):
            # no JSON: a CLI too old for `claude auth status`, or something else wrong
            old = claude_too_old(exe, run)
            if old:
                raise SituationError(f"AI=claude: {old}; {free}.")
            status = {}
        method = status.get("authMethod")
        if not status.get("loggedIn") or method in (None, "", "none"):
            raise SituationError(f"AI=claude: Claude Code is not logged in: {FIX_LOGIN['claude']}; {free}.")
        return {"ai": ai, "checked": True, "cli": exe, "logged_in": True, "auth_method": str(method)}
    # codex: `codex login status` where the installed CLI has it
    try:
        r = run([exe, "login", "status"], capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError) as e:
        raise SituationError(f"AI=codex: `codex login status` failed ({e}): {FIX_LOGIN['codex']}; {free}.") from None
    out = f"{r.stdout}\n{r.stderr}".lower()
    if "unrecognized subcommand" in out or "unexpected argument" in out:
        return {"ai": ai, "checked": False, "cli": exe, "note": "this codex has no `login status`: the login was not checked"}
    if r.returncode != 0 or "not logged in" in out or "logged in" not in out:
        raise SituationError(f"AI=codex: the Codex CLI is not logged in: {FIX_LOGIN['codex']}; {free}.")
    method = "chatgpt" if "chatgpt" in out else ("api key" if "api key" in out else "logged in")
    return {"ai": ai, "checked": True, "cli": exe, "logged_in": True, "auth_method": method}


CLAUDE_MIN = (2, 1, 294)


def claude_too_old(exe: str, run: Callable[..., Any] = subprocess.run) -> str | None:
    """Why the installed claude can't answer `claude auth status` (too old), or None."""
    try:
        r = run([exe, "--version"], capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return None
    import re

    m = re.search(r"(\d+)\.(\d+)\.(\d+)", r.stdout or "")
    if not m:
        return None
    version = tuple(int(x) for x in m.groups())
    if version < CLAUDE_MIN:
        want = ".".join(str(x) for x in CLAUDE_MIN)
        have = ".".join(str(x) for x in version)
        return f"the claude CLI is too old ({have}; needs {want} or later for `claude auth status`): update Claude Code"
    return None


def ai_command(
    ai: str,
    sit: Situation,
    *,
    python: str,
    mcp_url: str,
    brief_file: Path,
    agent_log: Path,
    until: float,
    seconds: float,
    codex_home: str | None = None,
) -> list[str] | None:
    """The AI's command line (the launch contract in agents/README.md), or None for none.
    `codex_home`: the user's FLEETMESH_CODEX_HOME (the AI's environment has no FLEETMESH_*
    variable, so it goes on the command line)."""
    if ai in ("claude", "codex"):
        cmd = [
            python, str(REPO / "agents" / "watch_officer.py"),
            "--backend", ai,
            "--url", mcp_url,
            "--agent", WATCH_AGENT,
            "--brief", str(brief_file),
            "--situation-log", str(agent_log),
            "--until", f"{until:.0f}",
        ]
        if ai == "codex" and codex_home:
            cmd += ["--codex-home", codex_home]
        return cmd
    return None


def brief_text(sit: Situation) -> str:
    """What the AI is told: the title and the operator's brief, never the director's notes,
    the timeline or the watch items."""
    return f"# {sit.title}\n\n{sit.brief}\n"


# ---- the stack's ports -----------------------------------------------------------------------


def situation_env(env: dict[str, str], sit: Situation | None = None) -> dict[str, str]:
    """The environment with the situation's port block filled in where a `FLEETMESH_*` port
    variable is not set (each one set overrides its port): the situation's own `[ports]` where it
    names them, else the common block (SITUATION_PORTS)."""
    out = dict(env)
    block = {**SITUATION_PORTS, **(sit.ports if sit is not None else {})}
    for k, v in block.items():
        if not (out.get(k) or "").strip():
            out[k] = v
    return out


def stack_ports(env: dict[str, str], sit: Situation | None = None) -> dict[str, int]:
    """The ports `scripts/up.sh` will use, from the same variables (the situation's block
    where one is not set, `situation_env`). `standin_src_base` only when one is set."""
    env = situation_env(env, sit)
    base = int(env["FLEETMESH_BASE_PORT"])
    out = {
        "vehicle_base": base,
        "bridge_base": int(env.get("FLEETMESH_BRIDGE_BASE_PORT") or base + 1000),
        "http": int(env["FLEETMESH_HTTP_PORT"]),
        "web": int(env["FLEETMESH_WEB_PORT"]),
        "linkemu": int(env["FLEETMESH_LINKEMU_PORT"]),
        "tidewater": int(env["FLEETMESH_TIDEWATER_PORT"]),
        "sitl_instance_base": int(env["FLEETMESH_SITL_INSTANCE_BASE"]),
    }
    if (env.get("FLEETMESH_STANDIN_SRC_BASE") or "").strip():
        out["standin_src_base"] = int(env["FLEETMESH_STANDIN_SRC_BASE"])
    return out


# the UDP ranges a stack takes, one port per vehicle from each base (`stack_ports`' keys)
UDP_RANGES = (
    ("vehicle_base", "FLEETMESH_BASE_PORT"),
    ("bridge_base", "FLEETMESH_BRIDGE_BASE_PORT"),
    ("standin_src_base", "FLEETMESH_STANDIN_SRC_BASE"),
)


def up_starts_console(env: dict[str, str]) -> bool:
    """Whether `scripts/up.sh` starts a console of its own on FLEETMESH_WEB_PORT: not with
    FLEETMESH_WEB=0, when the launcher's (`make start`, eval/launcher.py) serves it there."""
    return (env.get("FLEETMESH_WEB") or "1").strip() not in ("0", "off", "false", "no")


def tcp_answers(port: int, host: str = "127.0.0.1", timeout: float = 0.5) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def udp_taken(port: int) -> bool:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.bind(("0.0.0.0", port))
        return False
    except OSError:
        return True
    finally:
        s.close()


def pid_alive(pidfile: Path) -> int | None:
    try:
        pid = int(pidfile.read_text().strip())
    except (OSError, ValueError):
        return None
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return None
    except PermissionError:
        return pid
    return pid


def range_problems(ports: dict[str, int], vehicles: int) -> list[str]:
    """Why these UDP ranges can't carry this fleet: the vehicle and bridge ranges overlap (a
    fleet larger than the gap between their bases), or one reaches into the evaluation's block."""
    out: list[str] = []
    n = max(1, vehicles)
    ranges = [(var, ports[key]) for key, var in UDP_RANGES if key in ports]
    for i, (va, a) in enumerate(ranges):
        for vb_, b in ranges[i + 1:]:
            if abs(b - a) < n:
                out.append(
                    f"{vehicles} vehicles do not fit: the {va} range {a}-{a + n - 1} and the {vb_} range {b}-{b + n - 1} "
                    "overlap"
                )
    for name, base in ranges:
        if base < EVAL_UDP.stop and base + n > EVAL_UDP.start:
            out.append(f"{name} range {base}-{base + n - 1} reaches into the evaluation's UDP {EVAL_UDP.start}-{EVAL_UDP.stop - 1}")
    return out


def port_problems(ports: dict[str, int], vehicles: int, sitl: int, root: Path = REPO, skip: tuple[str, ...] = ()) -> list[str]:
    """Why a stack can't start on these ports now: its UDP ranges can't carry the fleet
    (`range_problems`), one already answers (or a port is held), or this checkout's stack is
    up. Never touches what holds them. `skip` names TCP ports not to look at: the console's
    ("web") when the launcher serves it (`up_starts_console`), tidewater's when one there is
    reused."""
    out = range_problems(ports, vehicles)
    if out:
        return out
    pid = pid_alive(root / "logs" / "up.pid")
    if pid is not None:
        out.append(f"a stack started from this checkout is running (logs/up.pid, pid {pid})")
    for key, var in (
        ("http", "FLEETMESH_HTTP_PORT"),
        ("web", "FLEETMESH_WEB_PORT"),
        ("linkemu", "FLEETMESH_LINKEMU_PORT"),
        ("tidewater", "FLEETMESH_TIDEWATER_PORT"),
    ):
        if key in ports and key not in skip and tcp_answers(ports[key]):
            out.append(f"something answers on {var}={ports[key]}")
    for key, var in UDP_RANGES:
        if key not in ports:
            continue
        taken = [p for p in range(ports[key], ports[key] + max(1, vehicles)) if udp_taken(p)]
        if taken:
            out.append(f"UDP {', '.join(str(p) for p in taken[:5])}{' ...' if len(taken) > 5 else ''} in the {var} range ({ports[key]}+{vehicles}) is held")
    for k in range(sitl):
        p = 5760 + 10 * (ports["sitl_instance_base"] + k)
        if tcp_answers(p):
            out.append(f"SITL instance port {p} answers (FLEETMESH_SITL_INSTANCE_BASE={ports['sitl_instance_base']})")
    return out


def pointer_names_situation(p: Path) -> bool | None:
    """Whether a restore pointer (`.last-run-<port>`: a log's path) or resume pointer
    (`.resume-<port>.json`: `{"log": ...}`) names a situation's log (`situation-*`); None when
    it can't be read."""
    try:
        text = p.read_text().strip()
    except (OSError, ValueError):
        return None
    if p.name.endswith(".json"):
        try:
            text = str((json.loads(text) or {}).get("log") or "")
        except (ValueError, AttributeError):
            return None
    return Path(text).name.startswith("situation-") if text else None


def pointer_problems(http_port: int, root: Path = REPO) -> list[str]:
    """The fresh station deletes this port's restore and resume pointers: only those naming a
    situation's log. One naming any other log is another station's (an exported
    `FLEETMESH_HTTP_PORT` that is another stack's), and the runner refuses rather than delete it."""
    out = []
    for name in (f".last-run-{http_port}", f".resume-{http_port}.json"):
        p = root / "logs" / "audit" / name
        if not (p.exists() or p.is_symlink()):
            continue
        if pointer_names_situation(p) is not True:
            out.append(
                f"logs/audit/{name} names a log that is not a situation's (or can't be read): it is another station's "
                f"pointer on HTTP port {http_port}, which a situation's fresh station would delete. Use another "
                "FLEETMESH_HTTP_PORT (or unset it for the situation's own), or remove that file yourself if it is stale"
            )
    return out


# ---- the audit log, read-only ----------------------------------------------------------------


class AuditTail:
    """New complete lines of a growing JSON-lines file (the bridge's audit log, the AI's own
    log), read-only, from where the last read stopped."""

    def __init__(self, path: Path):
        self.path = path
        self.offset = 0
        self.partial = b""

    def read(self) -> list[dict[str, Any]]:
        try:
            with self.path.open("rb") as f:
                f.seek(self.offset)
                data = f.read()
        except OSError:
            return []
        self.offset += len(data)
        data = self.partial + data
        lines = data.split(b"\n")
        self.partial = lines.pop()
        out = []
        for line in lines:
            try:
                rec = json.loads(line)
            except ValueError:
                continue
            if isinstance(rec, dict):
                out.append(rec)
        return out


def agent_activity(events: list[dict[str, Any]], agent: str | None) -> list[dict[str, Any]]:
    """What the AI did, as the audit log has it: its `agent.action` events (connected,
    previewed, proposed, withdrew), each with the bridge's time (s) and the engine's detail."""
    out = []
    for e in events:
        if e.get("type") != "agent.action" or (agent and e.get("agent") != agent):
            continue
        out.append({"t": round(float(e.get("t", 0)) / 1000.0, 3), "seq": e.get("seq"), "action": e.get("action"), "detail": str(e.get("detail", ""))[:400]})
    return out


def decision_text(d: dict[str, Any]) -> str:
    """One scripted decision in words: an intent's approval or rejection."""
    return f"{d.get('by')} {d.get('action')} intent {d.get('intent_id')}: {d.get('result')}"


def merge_events(files: list[Path]) -> str:
    """One events file from several (link and GNSS `[[event]]` tables), for `up.sh`'s single
    `FLEETMESH_LINK_EVENTS`; storyline `link` steps append to it while the run goes."""
    from .stack import event_toml

    head = "# This situation run's scripted events (eval/situation_run.py): the situation's events files\n"
    head += "# merged, on the stack's clock (at_s from the link emulator's start), then storyline link steps.\n\n"
    body = []
    for f in files:
        raw = tomllib.loads(f.read_text())
        body.append(f"# from {f}\n")
        for e in raw.get("event", []):
            body.append(event_toml(dict(e)))
    return head + "".join(body)
