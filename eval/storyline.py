"""The storyline: injected situations, executed against a live stack on a wall clock.

Steps (`[[step]]` in a spec; `at_s` counts from the moment the fleet is ready):

    kind = "propose"            a proposal through the arm's proposer
      task = "orbit"            orbit | patrol | hold | rtl | transit
      vehicles = {...}          {uids = [..]} | {platform = "delivery_drone"} (first of that
                                platform, or all with all = true) | {callsign = "DRONE 1"}
      center = ...              [lon, lat] | {vehicle = "self" | uid, offset_m = [east, north]}
      radius_m = 600
      route = [[lon, lat], ...] or {from = "self", to = <center-like>, legs = 2}
      rationale = "..."         a template: {callsign} {platform} are filled from the
                                resolved vehicle (never from truth)
      label = "derived"         legitimate | should_reject | should_void | derived | none
      verify = "..."            vehicle_spoofed | none
      reason = "..."            why the label (for the report)
      withdraw_after_s = 40     the proposer withdraws it if still waiting (override metric)
      retry_s = 20              keep trying this long if a vehicle it names is not there yet
                                (default 20; 0 inside a flood)
    kind = "flood"              count proposals, interval_s apart, cycling templates
    kind = "link"               a link effect on named vehicles from now (delay_s) for
                                duration_s: cut = true | loss = 0.5 | extra_latency_ms = n,
                                appended to the run's events file for the link emulator
    kind = "note"               a marker in the storyline log

Everything resolved at injection time goes to `storyline.jsonl` (the vehicle as the C2 showed
it and its navigation truth), so the metrics can label each intent without re-reading the heavy
logs, and can cross-check against them. For a proposer that may adjust the step (`adjusts`, the
model), the record is what it actually proposed, read after the proposal, with the step's
request under `suggested` (`Storyline.as_proposed`).
"""

from __future__ import annotations

import asyncio
import json
import math
import time
from pathlib import Path
from typing import Any

from .agent import Proposer, ProposeError
from .bridge import Bridge
from .spec import Spec, Step

def haversine_m(lon1: float, lat1: float, lon2: float, lat2: float) -> float:
    r = 6371000.0
    p1, p2 = math.radians(lat1), math.radians(lat2)
    dp = p2 - p1
    dl = math.radians(lon2 - lon1)
    a = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * r * math.asin(math.sqrt(a))


def offset(lonlat: list[float], east_m: float, north_m: float) -> list[float]:
    lon, lat = lonlat
    dlat = north_m / 111_320.0
    dlon = east_m / (111_320.0 * math.cos(math.radians(lat)))
    return [round(lon + dlon, 6), round(lat + dlat, 6)]


class StepError(RuntimeError):
    pass


class Log:
    def __init__(self, path: Path):
        self.path = path
        self.path.write_text("")

    def write(self, rec: dict[str, Any]) -> None:
        rec = {"t": round(time.time(), 3), **rec}
        with self.path.open("a") as f:
            f.write(json.dumps(rec) + "\n")


class World:
    """Reads at injection time: the fleet from the API, truth from the evaluation hook."""

    def __init__(self, bridge: Bridge, run_dir: Path):
        self.bridge = bridge
        self.run_dir = run_dir

    async def vehicles(self) -> list[dict]:
        return await asyncio.to_thread(self.bridge.vehicles)

    async def truth(self) -> dict:
        """The bridge's clock (`/api/state`'s `t`), for putting wall times on it."""
        return {"t": (await asyncio.to_thread(self.bridge.try_get, "/api/state") or {}).get("t")}

    async def vehicle_truth(self, uid: int) -> dict | None:
        """The stand-in truth log's latest line for a vehicle (only vehicles that lost GPS
        or were spoofed write lines; absent means healthy)."""
        path = self.run_dir / "standins-truth.jsonl"
        if not path.exists():
            return None

        def read() -> dict | None:
            last = None
            with path.open() as f:
                for line in f:
                    try:
                        rec = json.loads(line)
                    except ValueError:
                        continue
                    if rec.get("uid") == uid:
                        last = rec
            return last

        return await asyncio.to_thread(read)


def select_vehicles(sel: Any, vehicles: list[dict]) -> list[dict]:
    if isinstance(sel, list):
        want = {int(u) for u in sel}
        out = [v for v in vehicles if v.get("uid") in want]
    elif isinstance(sel, dict) and "uids" in sel:
        want = {int(u) for u in sel["uids"]}
        out = [v for v in vehicles if v.get("uid") in want]
    elif isinstance(sel, dict) and "platform" in sel:
        out = [v for v in vehicles if (v.get("identity") or {}).get("platform") == sel["platform"]]
        if not sel.get("all"):
            idx = int(sel.get("index", 0))
            out = out[idx : idx + 1]
    elif isinstance(sel, dict) and "callsign" in sel:
        out = [v for v in vehicles if (v.get("identity") or {}).get("callsign") == sel["callsign"]]
    else:
        raise StepError(f"bad vehicle selector {sel!r}")
    if not out:
        raise StepError(f"no vehicle matches {sel!r}")
    return out


# ---- what a proposer that adjusts actually proposed ---------------------------------------------

# a centre or route point that moved less than this is the suggestion's
SAME_POINT_M = 1.0
# the request fields each task kind carries in the intent (`params`); a field the actual task
# does not carry (a radius on a patrol) is not an adjustment
CARRIES = {"orbit": ("center", "radius_m"), "patrol": ("route",), "transit": ("route",), "hold": (), "rtl": ()}
REQUEST_FIELDS = ("task", "uids", "center", "radius_m", "route", "reserve_pct", "duration_s", "extra")


def _uid(u: Any) -> Any:
    return int(u) if isinstance(u, (int, str)) and str(u).isdigit() else u


def _point(p: Any) -> list[float]:
    if isinstance(p, dict):
        return [float(p["lon"]), float(p["lat"])]
    return [float(p[0]), float(p[1])]


def request_of(intent: dict[str, Any]) -> dict[str, Any]:
    """The request an intent the bridge holds stands for, in the storyline's terms: its
    `kind`, `params`, `uids` and `constraints` as `{task, uids, center, radius_m, route,
    reserve_pct, duration_s}`, any other parameter under `extra`."""
    params = dict(intent.get("params") or {})
    req: dict[str, Any] = {"task": intent.get("kind"), "uids": [_uid(u) for u in intent.get("uids") or []]}
    if params.get("center") is not None:
        req["center"] = _point(params["center"])
    if params.get("radius_m") is not None:
        req["radius_m"] = float(params["radius_m"])
    if params.get("route") is not None:
        req["route"] = [_point(p) for p in params["route"]]
    for k in ("reserve_pct", "duration_s"):
        if (intent.get("constraints") or {}).get(k) is not None:
            req[k] = intent["constraints"][k]
    extra = {k: v for k, v in params.items() if k not in ("center", "radius_m", "route")}
    if extra:
        req["extra"] = extra
    return req


def _same(k: str, a: Any, b: Any) -> bool:
    if a is None or b is None:
        return a is None and b is None
    try:
        if k == "center":
            a, b = _point(a), _point(b)
            return haversine_m(a[0], a[1], b[0], b[1]) <= SAME_POINT_M
        if k == "route":
            pa, pb = [_point(p) for p in a], [_point(p) for p in b]
            return len(pa) == len(pb) and all(haversine_m(p[0], p[1], q[0], q[1]) <= SAME_POINT_M for p, q in zip(pa, pb))
        if k in ("radius_m", "reserve_pct", "duration_s"):
            return abs(float(a) - float(b)) <= 1e-6
        if k == "uids":
            return [str(u) for u in a] == [str(u) for u in b]
    except (TypeError, ValueError, KeyError, IndexError):
        return False
    return bool(a == b)


def adjusted_fields(suggested: dict[str, Any], actual: dict[str, Any]) -> list[str]:
    """The request fields the proposal changed from the suggestion (a point within
    `SAME_POINT_M` is unchanged)."""
    carried = CARRIES.get(str(actual.get("task")))
    out = []
    for k in REQUEST_FIELDS:
        if actual.get(k) is None and carried is not None and k in ("center", "radius_m", "route") and k not in carried:
            continue
        if not _same(k, suggested.get(k), actual.get(k)):
            out.append(k)
    return out


def facts_at(req: dict[str, Any], lead: dict, vehicle_truth: dict | None) -> dict[str, Any]:
    """A proposal record's facts: the lead vehicle as the C2 showed it and as truth had it."""
    return {
        "vehicle": {
            "uid": lead.get("uid"),
            "callsign": (lead.get("identity") or {}).get("callsign"),
            "platform": (lead.get("identity") or {}).get("platform"),
            "position": lead.get("position"),
        },
        "vehicle_truth": (
            {k: vehicle_truth.get(k) for k in ("t", "error_m", "spoof_m", "ekf", "fix", "dead_reckoning_s")}
            if vehicle_truth
            else None
        ),
        "vehicle_shown": {k: lead.get(k) for k in ("nav_ready", "nav_source", "gps_fix", "satellites")},
    }


class Storyline:
    """Runs a spec's steps against a live stack. Each step starts at `t_ready + at_s` on its
    own task, so a long step (a flood, a retry) never delays the next one."""

    def __init__(
        self,
        spec: Spec,
        bridge: Bridge,
        proposer: Proposer,
        world: World,
        log: Log,
        t_ready: float,
        events_file: Path | None = None,
        emu_url: str | None = None,
    ):
        self.spec = spec
        self.bridge = bridge
        self.proposer = proposer
        self.world = world
        self.log = log
        self.t_ready = t_ready
        self.events_file = events_file
        self.emu_url = emu_url
        self.tasks: list[asyncio.Task] = []
        self.injected: list[dict[str, Any]] = []
        self._events_lock = asyncio.Lock()

    async def run(self, stop: asyncio.Event) -> None:
        steps = [asyncio.create_task(self._at(step, stop)) for step in self.spec.steps]
        await asyncio.gather(*steps, return_exceptions=True)
        if self.tasks:
            await asyncio.gather(*self.tasks, return_exceptions=True)

    async def _at(self, step: Step, stop: asyncio.Event) -> None:
        due = self.t_ready + step.at_s
        while not stop.is_set():
            left = due - time.time()
            if left <= 0:
                break
            try:
                await asyncio.wait_for(stop.wait(), timeout=min(left, 1.0))
            except asyncio.TimeoutError:
                pass
        if stop.is_set():
            self.log.write({"kind": "skipped", "step": step.id, "reason": "run ended"})
            return
        try:
            await self.execute(step)
        except (StepError, ProposeError) as e:
            self.log.write({"kind": "error", "step": step.id, "t_rel": self.t_rel(), "error": str(e)})
        except Exception as e:  # noqa: BLE001 (a broken step is recorded, the run goes on)
            self.log.write({"kind": "error", "step": step.id, "t_rel": self.t_rel(), "error": repr(e)})

    def t_rel(self, t: float | None = None) -> float:
        return round((t if t is not None else time.time()) - self.t_ready, 1)

    async def execute(self, step: Step) -> None:
        if step.kind == "note":
            self.log.write({"kind": "note", "step": step.id, "t_rel": self.t_rel(), "text": step.params.get("text", "")})
        elif step.kind == "propose":
            await self.propose(step.id, step.params)
        elif step.kind == "flood":
            await self.flood(step)
        elif step.kind == "link":
            await self.link(step)
        else:
            raise StepError(f"unknown step kind {step.kind}")

    # ---- resolution ----------------------------------------------------------------------

    async def resolve_center(self, c: Any, vehicle: dict | None, now: float) -> list[float]:
        if isinstance(c, list) and len(c) == 2:
            return [float(c[0]), float(c[1])]
        if not isinstance(c, dict):
            raise StepError(f"bad center {c!r}")
        off = c.get("offset_m", [0, 0])
        if "vehicle" in c:
            if c["vehicle"] == "self":
                if vehicle is None or not vehicle.get("position"):
                    raise StepError("self center: the vehicle has no position")
                pos = list(vehicle["position"])
            else:
                vs = await self.world.vehicles()
                v = next((v for v in vs if v.get("uid") == int(c["vehicle"])), None)
                if v is None or not v.get("position"):
                    raise StepError(f"vehicle {c['vehicle']} has no position")
                pos = list(v["position"])
            return offset(pos, off[0], off[1])
        raise StepError(f"bad center {c!r}")

    async def resolve_route(self, r: Any, vehicle: dict | None, now: float) -> list[list[float]]:
        if isinstance(r, list):
            return [[float(p[0]), float(p[1])] for p in r]
        if isinstance(r, dict) and "to" in r:
            start = await self.resolve_center(r.get("from", {"vehicle": "self"}), vehicle, now)
            end = await self.resolve_center(r["to"], vehicle, now)
            legs = max(1, int(r.get("legs", 1)))
            pts = [start]
            for k in range(1, legs + 1):
                f = k / legs
                pts.append([round(start[0] + (end[0] - start[0]) * f, 6), round(start[1] + (end[1] - start[1]) * f, 6)])
            return pts
        raise StepError(f"bad route {r!r}")

    def fill(self, template: str, vehicle: dict) -> str:
        ident = vehicle.get("identity") or {}
        fields = {
            "callsign": ident.get("callsign", f"vehicle {vehicle.get('uid')}"),
            "platform": ident.get("platform", "?"),
        }
        try:
            return template.format_map(fields)
        except (KeyError, ValueError) as e:
            raise StepError(f"rationale template: {e}") from e

    async def prepare(self, p: dict[str, Any]) -> tuple[dict[str, Any], str, dict]:
        """Resolve a proposal step against the world now: (request, rationale, lead
        vehicle). Raises StepError if something it names is not there (yet)."""
        now = time.time()
        vehicles = await self.world.vehicles()
        chosen = select_vehicles(p["vehicles"], vehicles)
        lead = chosen[0]
        req: dict[str, Any] = {"task": p["task"], "uids": [v["uid"] for v in chosen]}
        if p.get("center") is not None:
            req["center"] = await self.resolve_center(p["center"], lead, now)
        if p.get("radius_m") is not None:
            req["radius_m"] = float(p["radius_m"])
        if p.get("route") is not None:
            req["route"] = await self.resolve_route(p["route"], lead, now)
        for k in ("reserve_pct", "duration_s"):
            if p.get(k) is not None:
                req[k] = p[k]
        rationale = self.fill(str(p.get("rationale", "")), lead) or f"{p['task']} on {lead.get('uid')}"
        return req, rationale, lead

    async def prepare_with_retry(self, step_id: str, p: dict[str, Any]) -> tuple[dict[str, Any], str, dict]:
        """`prepare`, retried every 2 s for `retry_s` (default 20 s): a vehicle the step
        names may not be held yet a few seconds after the fleet is ready."""
        deadline = time.monotonic() + float(p.get("retry_s", 20))
        while True:
            try:
                return await self.prepare(p)
            except StepError as e:
                if time.monotonic() >= deadline:
                    raise
                last = str(e)
            self.log.write({"kind": "retry", "step": step_id, "t_rel": self.t_rel(), "error": last})
            await asyncio.sleep(2.0)

    # ---- steps ---------------------------------------------------------------------------

    async def propose(self, step_id: str, p: dict[str, Any]) -> dict[str, Any]:
        req, rationale, lead = await self.prepare_with_retry(step_id, p)
        return await self.submit(step_id, p, req, rationale, lead)

    async def submit(
        self, step_id: str, p: dict[str, Any], req: dict[str, Any], rationale: str, lead: dict
    ) -> dict[str, Any]:
        """Record what the C2 showed and what truth had for the vehicle, then propose. A
        proposer that may adjust the request (`adjusts`, the model) is then recorded by what
        it proposed (`as_proposed`)."""
        now = time.time()
        vehicle_truth = await self.world.vehicle_truth(int(lead["uid"]))
        label = p.get("label", "derived")
        rec: dict[str, Any] = {
            "kind": "propose",
            "step": step_id,
            "t_rel": self.t_rel(now),
            "source": {"kind": self.proposer.kind, "id": self.proposer.source_id},
            "request": req,
            "rationale": rationale,
            "label": label,
            "reason": p.get("reason", ""),
            "premise": p.get("premise") or {},
            **facts_at(req, lead, vehicle_truth),
        }
        try:
            out = await self.proposer.propose(req, rationale)
            rec["intent_id"] = out.get("id")
            rec["state"] = out.get("state")
            rec["dispatched"] = out.get("dispatched")
            for k in ("refused", "declined", "model_note"):
                if out.get(k) is not None:
                    rec[k] = out[k]
        except ProposeError as e:
            rec["error"] = str(e)
        if getattr(self.proposer, "adjusts", False) and rec.get("intent_id") is not None:
            await self.as_proposed(rec)
        self.log.write(rec)
        self.injected.append(rec)
        if rec.get("intent_id") is not None and p.get("withdraw_after_s") is not None:
            self.tasks.append(asyncio.create_task(self.withdraw_later(step_id, int(rec["intent_id"]), float(p["withdraw_after_s"]))))
        return rec

    async def as_proposed(self, rec: dict[str, Any]) -> None:
        """For a proposer that may adjust the step (`adjusts`): the record follows what it
        actually proposed, when it proposed it. The intent the bridge holds is the request
        (`request_of`), its facts are read again after the proposal (`facts_at`), and the suggestion with the facts read before it go under
        `suggested`, with the request fields that changed (`adjusted`), when the intent was
        created (`proposed_t` on the bridge's clock, `proposed_t_rel` on the storyline's)
        and when the facts were read again (`read_t_rel`). If the intent cannot be read, the
        record keeps the suggestion and says why (`follow_error`)."""
        iid = int(rec["intent_id"])
        try:
            cur = await asyncio.to_thread(self.bridge.intent, iid)
        except Exception as e:  # noqa: BLE001 (recorded; the report lists it)
            rec["follow_error"] = f"could not read intent {iid}: {e}"
            return
        intent = (cur.get("intent") or cur) if isinstance(cur, dict) else {}
        actual = request_of(intent)
        suggested = {k: rec[k] for k in ("request", "vehicle", "vehicle_truth", "vehicle_shown")}
        now = time.time()
        vehicles = await self.world.vehicles()
        uid = actual["uids"][0] if actual["uids"] else rec["vehicle"]["uid"]
        lead = next((v for v in vehicles if v.get("uid") == uid), {"uid": uid})
        vehicle_truth = await self.world.vehicle_truth(uid)
        truth = await self.world.truth()
        rec["request"] = actual
        rec.update(facts_at(actual, lead, vehicle_truth))
        rec["suggested"] = suggested
        rec["adjusted"] = adjusted_fields(suggested["request"], actual)
        if intent.get("created_t") is not None:
            rec["proposed_t"] = round(float(intent["created_t"]) / 1000.0, 3)
            # onto the wall clock through the bridge's own time in the truth just read
            skew = float(truth["t"]) - now if truth.get("t") is not None else 0.0
            rec["proposed_t_rel"] = self.t_rel(rec["proposed_t"] - skew)
        rec["read_t_rel"] = self.t_rel(now)

    async def withdraw_later(self, step_id: str, intent_id: int, after_s: float) -> None:
        await asyncio.sleep(after_s)
        try:
            cur = await asyncio.to_thread(self.bridge.intent, intent_id)
        except Exception as e:  # noqa: BLE001
            self.log.write({"kind": "withdraw", "step": step_id, "intent_id": intent_id, "error": str(e)})
            return
        state = (cur.get("intent") or cur).get("state")
        if state not in ("pending_approval", "approved"):
            self.log.write({"kind": "withdraw", "step": step_id, "intent_id": intent_id, "skipped": state})
            return
        try:
            out = await self.proposer.withdraw(intent_id, "the picture changed; the proposer withdraws")
            self.log.write({"kind": "withdraw", "step": step_id, "intent_id": intent_id, "state": (out or {}).get("state")})
        except ProposeError as e:
            self.log.write({"kind": "withdraw", "step": step_id, "intent_id": intent_id, "error": str(e)})

    async def flood(self, step: Step) -> None:
        p = step.params
        count = int(p.get("count", 6))
        interval = float(p.get("interval_s", 8))
        templates = p["templates"]
        for k in range(count):
            t = dict(templates[k % len(templates)])
            t.setdefault("label", p.get("label", "derived"))
            t.setdefault("retry_s", 0)
            try:
                await self.propose(f"{step.id}-{k + 1}", t)
            except (StepError, ProposeError) as e:
                self.log.write({"kind": "error", "step": f"{step.id}-{k + 1}", "t_rel": self.t_rel(), "error": str(e)})
            if k + 1 < count:
                await asyncio.sleep(interval)

    async def link(self, step: Step) -> None:
        """A scripted link effect on named vehicles, appended to the run's events file as
        an `[[event]]` the link emulator picks up within a second (`at_s` on the emulator's
        own clock, from its `uptime_s`, so the effect starts `delay_s` from now to within
        that second)."""
        p = step.params
        if self.events_file is None or self.emu_url is None:
            raise StepError("link steps need the run's events file and the emulator")
        vehicles = await self.world.vehicles()
        chosen = select_vehicles(p["vehicles"], vehicles)
        uids = [int(v["uid"]) for v in chosen]
        summary = await asyncio.to_thread(Bridge(self.emu_url).try_get, "/stats/summary")
        if not summary or summary.get("uptime_s") is None:
            raise StepError("the link emulator did not answer /stats/summary")
        now = time.time()
        lead = max(1.5, float(p.get("delay_s", 0)))  # the emulator polls the file once a second
        at_s = round(float(summary["uptime_s"]) + lead, 1)
        duration = float(p["duration_s"])
        lines = ["", "[[event]]", f'name = "storyline {step.id}"', f"at_s = {at_s}", f"duration_s = {duration}"]
        lines.append(f"uids = [{', '.join(str(u) for u in uids)}]")
        effect: dict[str, Any] = {}
        if p.get("cut"):
            lines.append("cut = true")
            effect["cut"] = True
        if p.get("loss") is not None:
            lines.append(f"loss = {float(p['loss'])}")
            effect["loss"] = float(p["loss"])
        if p.get("extra_latency_ms") is not None:
            lines.append(f"extra_latency_ms = {float(p['extra_latency_ms'])}")
            effect["extra_latency_ms"] = float(p["extra_latency_ms"])
        async with self._events_lock:
            with self.events_file.open("a") as f:
                f.write("\n".join(lines) + "\n")
        self.log.write(
            {
                "kind": "link",
                "step": step.id,
                "t_rel": self.t_rel(now),
                "uids": uids,
                "callsigns": [(v.get("identity") or {}).get("callsign") for v in chosen],
                "emu_at_s": at_s,
                "starts_t": round(now + lead, 2),
                "ends_t": round(now + lead + duration, 2),
                "effect": effect,
            }
        )


class TruthSampler:
    """Every `every_s`, the bridge's clock beside the wall clock to
    `truth.jsonl`, so a time on the bridge's clock maps to the wall clock (eval/metrics.py
    `wall_to_bridge`)."""

    def __init__(self, world: World, path: Path, every_s: float = 5.0):
        self.world = world
        self.path = path
        self.every_s = every_s
        self.path.write_text("")

    async def run(self, stop: asyncio.Event) -> None:
        while not stop.is_set():
            snap = await self.world.truth()
            if snap:
                rec = {"t": round(time.time(), 2), "bridge_t": snap.get("t")}
                with self.path.open("a") as f:
                    f.write(json.dumps(rec) + "\n")
            try:
                await asyncio.wait_for(stop.wait(), timeout=self.every_s)
            except asyncio.TimeoutError:
                pass
