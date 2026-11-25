"""Tests that need a live stack (a bridge answering, the release binaries built): skipped when
there is none, never deleted."""

from __future__ import annotations

import pytest

from eval import REPO
from eval.bridge import Bridge

BRIDGE = "http://127.0.0.1:8765"
BINARIES = [REPO / "target" / "release" / n for n in ("fleetmesh-bridge", "fleetmesh-standins", "fleetmesh-linkemu")]


def _alive() -> bool:
    try:
        return Bridge(BRIDGE).try_get("/api/state") is not None
    except Exception:  # noqa: BLE001
        return False


live = pytest.mark.skipif(not _alive(), reason="no bridge answering on 127.0.0.1:8765 (make start)")
built = pytest.mark.skipif(not all(b.exists() for b in BINARIES), reason="release binaries not built (make setup)")


@live
def test_the_bridge_state_has_a_clock_and_vehicles():
    state = Bridge(BRIDGE).state()
    assert isinstance(state.get("t"), (int, float)) and isinstance(state.get("vehicles"), list)


@live
def test_the_inbox_and_intents_read():
    b = Bridge(BRIDGE)
    assert "pending" in b.inbox()
    assert isinstance(b.intents(), list)


@live
def test_the_cut_routes_are_gone():
    status, _ = Bridge(BRIDGE)._call("GET", "/api/tracks")
    assert status == 404


@built
def test_a_one_run_batch_scores_and_reports(tmp_path):
    pytest.skip("a full-stack run needs a delivery scenario spec (eval/scenarios is to be written fresh)")
