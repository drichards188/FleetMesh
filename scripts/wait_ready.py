#!/usr/bin/env python3
"""Wait until a FleetMesh stack started in the background is ready to watch, saying how far it got.

    .venv/bin/python scripts/wait_ready.py --port 8765     # the default stack (make up)

For an agent (or a script) that started a stack without the menu and needs to know when to tell
the person to look (AGENTS.md, "Helping someone use FleetMesh"). It reads the bridge's REST API
only, as the launcher does (eval/launcher.py): the stack is ready when every vehicle in
`GET /api/state` is navigation-ready with a position, or 180 s after the bridge answered with at
least one ready (the launcher's rule: the rest may follow). Prints a line each time the progress
changes and a last line saying what it found.

Exit 0 ready, 1 timed out, 2 a bad argument.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request

OPEN_WORLD_GRACE_S = 180.0  # eval/launcher.py READY_TIMEOUT_S


def get(port: int, path: str) -> dict | None:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=3) as r:
            return json.loads(r.read())
    except (OSError, ValueError, urllib.error.URLError):
        return None


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=8765, help="the bridge's HTTP port (default 8765)")
    ap.add_argument("--timeout", type=float, default=600.0, help="give up after this many seconds (default 600)")
    args = ap.parse_args(argv)
    start = time.monotonic()
    bridge_t: float | None = None
    said = ""

    def say(line: str) -> None:
        nonlocal said
        if line != said:
            print(f"[{time.monotonic() - start:4.0f} s] {line}", flush=True)
            said = line

    while time.monotonic() - start < args.timeout:
        state = get(args.port, "/api/state")
        if state is None:
            say(f"waiting for the bridge on :{args.port}")
            time.sleep(3)
            continue
        if bridge_t is None:
            bridge_t = time.monotonic()
        vehicles = state.get("vehicles") or []
        ready = sum(1 for v in vehicles if v.get("nav_ready") and v.get("position"))
        counted = f"{ready} of {len(vehicles)} vehicles ready"
        if vehicles and ready >= len(vehicles):
            print(f"ready: {counted}", flush=True)
            return 0
        if ready and time.monotonic() - bridge_t > OPEN_WORLD_GRACE_S:
            print(f"ready: {counted} after {OPEN_WORLD_GRACE_S:.0f} s; the rest may follow", flush=True)
            return 0
        say(f"bridge up; {counted}")
        time.sleep(3)
    print(f"timed out after {args.timeout:.0f} s: {said or 'nothing answered'}", flush=True)
    return 1


if __name__ == "__main__":
    sys.exit(main())
