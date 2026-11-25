"""The agents import with nothing of the dropped parts behind them, and their pure logic works."""

from __future__ import annotations

import importlib
import time
from pathlib import Path

import pytest

AGENTS = Path(__file__).resolve().parents[1]

# bridge tools and routes cut with the dropped parts: nothing in the agents may call them
CUT_NAMES = (
    "get_identify_options", "get_nomination_options", "get_nominations", "get_satellite_tasking_options",
    "get_sensor_reach", "get_tracks", "/api/loadouts", "/api/minefields", "/api/restores", "/api/sensing/reach",
    "/api/space/runs", "/api/engage/options", "/api/identify/options", "/api/tracks", "/api/review",
)


@pytest.mark.parametrize("name", ["scripted_agent", "policy_agent", "watch_officer", "watch.backend", "watch.claude",
                                  "watch.codex", "watch.limits", "watch.picture", "watch.situation", "watch.wake"])
def test_module_imports(name):
    importlib.import_module(name)


def test_no_call_to_a_cut_tool_or_route():
    for f in AGENTS.rglob("*"):
        if f.suffix not in (".py", ".md") or "tests" in f.parts:
            continue
        text = f.read_text()
        for name in CUT_NAMES:
            assert name not in text, f"{f.relative_to(AGENTS)} names {name}"


def test_a_lost_link_wakes_the_model():
    from watch.wake import Budgets, Waker

    now = time.time()
    w2 = Waker(Budgets(coalesce_s=0.0, min_gap_s=0.0), escalation=True)
    w2.started = True
    w2.add([{"kind": "vehicle.link_lost", "t": now, "uid": 2, "callsign": "DRONE 1"}], now)
    wake = w2.due(now + 1)
    assert wake is not None and wake.why and not wake.escalate


def test_gps_glitch_escalates_once_per_subject():
    from watch.wake import Budgets, Waker

    w = Waker(Budgets(coalesce_s=0.0, min_gap_s=0.0), escalation=True)
    w.started = True
    now = time.time()
    w.add([{"kind": "vehicle.gps_glitch", "t": now, "uid": 3, "glitch": True}], now)
    wake = w.due(now + 1)
    assert wake is not None and wake.escalate


def test_update_text_names_changes_and_tasking():
    from watch.picture import compose, render
    from watch.wake import Wake

    wake = Wake(1, "changes", [{"kind": "vehicle.link_lost", "t": 1.0, "uid": 2, "callsign": "CAR 1", "platform": "delivery_car"}],
                ["a vehicle lost its link"], False, [], 1.0)
    text = compose(wake, [], "Budget: x", [], 2.0, tasking=[{"id": "tk-1", "text": "hold the boat", "by": "op-1", "at_t": 1}])
    assert "CAR 1" in text and "tk-1" in text and "hostile" not in text.lower()
    assert render({"kind": "feed.reset", "t": 1.0, "run": "r"}).endswith("read the picture again")


def test_tasking_changes_baseline_and_news():
    import watch_officer as wo

    known: dict = {}
    doc = {"items": [{"id": "tk-1", "active": True, "text": "a", "at_t": 1, "by": "op-1"}]}
    assert [c["kind"] for c in wo.tasking_changes(doc, known, 1.0)] == ["tasking.posted"]
    assert wo.tasking_changes(doc, known, 2.0) == []
    gone = {"items": [{"id": "tk-1", "active": False, "text": "a", "at_t": 1}]}
    assert [c["kind"] for c in wo.tasking_changes(gone, known, 3.0)] == ["tasking.retired"]


def test_prompt_has_no_old_domain_text():
    for f in (AGENTS / "watch" / "prompt.md", AGENTS / "watch" / "tasking.md"):
        text = f.read_text().lower()
        for word in ("hostile", "engage", "nominat", "armed", "weapon", "strike", "ais "):
            assert word not in text, f"{f.name}: {word}"


def test_the_agent_session_has_no_track_reader():
    from policy_agent import AgentSession

    assert not hasattr(AgentSession, "tracks")
