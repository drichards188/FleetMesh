"""The evaluation harness imports with nothing of the dropped parts behind it, and its pure
pieces work on synthetic logs. Tests that need a live stack are in test_live_stack.py."""

from __future__ import annotations

import importlib
import json
from pathlib import Path

import pytest

EVAL = Path(__file__).resolve().parents[1]

MODULES = ["agent", "arms", "bridge", "evidence", "launcher", "metrics", "model_agent", "policy_file", "ports", "provenance",
           "proxy", "report", "run", "situation", "situation_run", "spec", "stack", "stats", "storyline", "webconsole",
           "world", "headless.sim", "headless.latency", "headless.provenance", "headless.v2.stats"]

CUT_NAMES = ("get_tracks", "get_nomination", "get_identify_options", "/api/tracks", "/api/review", "/api/engage",
             "/api/identify", "/api/restores", "/api/minefields", "/api/loadouts", "murmur-contacts", "playbook")


@pytest.mark.parametrize("name", MODULES)
def test_module_imports(name):
    importlib.import_module(f"eval.{name}")


def test_no_call_to_a_cut_route_or_tool():
    for f in EVAL.rglob("*.py"):
        if "tests" in f.parts:
            continue
        text = f.read_text()
        for name in CUT_NAMES:
            assert name not in text, f"{f.relative_to(EVAL)} names {name}"


def test_the_rule_book_reads_the_checks_only():
    from eval.evidence import evidence

    assert evidence([{"rule": "R1", "outcome": "pass"}]) == []
    assert evidence([{"rule": "R2", "uid": 4, "outcome": "fail"}, {"rule": "R3", "outcome": "unverified"}]) == [
        "check R2:4 fail", "check R3 unverified"]


def test_the_proxy_judges_a_card_by_its_checks():
    from eval.proxy import ProxyParams, judge

    p = ProxyParams()
    assert judge({"checks": [{"rule": "R1", "outcome": "pass"}]}, p).action == "approve"
    assert judge({"checks": [{"rule": "R1", "outcome": "fail"}]}, p).action == "reject"


def test_proxy_presets_and_unknown_keys():
    from eval.proxy import proxy_params

    assert proxy_params({}, "diligent").diligence == 1.0
    with pytest.raises(ValueError):
        proxy_params({"nope": 1})


def test_a_spoofed_vehicle_is_a_should_reject_and_a_cut_link_a_should_void():
    from eval.metrics import resolve_label, trails

    rec = {"label": "derived", "vehicle_truth": {"spoof_m": 120.0}}
    assert resolve_label(rec)[0] == "should_reject"
    events = [
        {"type": "intent.created", "t": 1000, "intent": {"id": 1, "kind": "hold", "uids": [7], "source": {"kind": "agent", "id": "a"}, "created_t": 1000}},
        {"type": "intent.state", "t": 1100, "intent": 1, "to": "pending_approval", "reason": ""},
        {"type": "intent.state", "t": 2000, "intent": 1, "to": "approved", "reason": ""},
    ]
    t = trails(events)[1]
    assert t.route == "human"
    label, _ = resolve_label({"label": "derived"}, t, [("7", 1.0, 3.0)])
    assert label == "should_void"
    assert resolve_label({"label": "derived"}, t, [])[0] == "legitimate"


def test_metrics_of_an_empty_run():
    from eval.metrics import compute

    m = compute([], [])
    assert m["invariants"]["violations"] == 0 and m["counts"]["intents"] == 0
    assert "discrimination_armed" not in m


def test_a_spec_checks_its_steps(tmp_path):
    from eval.spec import SpecError, load_spec

    (tmp_path / "fleet.toml").write_text("title = 'f'\n")
    (tmp_path / "policy.toml").write_text("")
    good = tmp_path / "ok.toml"
    good.write_text(
        'fleet = "fleet.toml"\npolicy = "policy.toml"\n[[step]]\nid = "a"\nat_s = 5\nkind = "propose"\ntask = "hold"\n'
        'vehicles = {platform = "delivery_drone"}\nreason = "a hold on a healthy drone"\nlabel = "legitimate"\n'
    )
    spec = load_spec(good)
    assert [s.id for s in spec.steps] == ["a"]
    bad = tmp_path / "bad.toml"
    bad.write_text(good.read_text().replace('"hold"', '"engage"'))
    with pytest.raises(SpecError):
        load_spec(bad)


def test_the_delivery_platforms_are_the_catalog():
    import tomllib

    from eval import REPO

    cat = tomllib.loads((REPO / "sim" / "platforms.toml").read_text()) if (REPO / "sim" / "platforms.toml").exists() else None
    if cat is None:
        pytest.skip("sim/platforms.toml is not in this tree")
    assert sorted(cat) == ["delivery_boat", "delivery_car", "delivery_drone", "delivery_plane"]


def test_a_report_of_no_runs_builds(tmp_path):
    from eval.report import build

    (tmp_path / "batch.json").write_text(json.dumps({"id": "t", "started": "x", "seeds": [], "runs": []}))
    out = build(tmp_path)
    assert Path(out).exists() and "FleetMesh evaluation" in Path(out).read_text()


def test_port_blocks_have_no_contacts_port():
    from eval.ports import PortBlock

    assert not hasattr(PortBlock, "contacts")


def test_a_situation_takes_no_cosigner_or_playbook():
    from eval import situation

    assert "playbook" not in situation.AI_MODES
    assert not hasattr(situation, "COSIGNERS") and not hasattr(situation, "EngagementChain")


def _write_run(batch: Path, arm: str, label: str) -> None:
    d = batch / "hold-drone" / arm / "seed-1"
    d.mkdir(parents=True)
    events = [
        {"type": "run.started", "t": 0, "roster": {"auth": "dev"}, "rules_version": 1},
        {"type": "intent.created", "t": 1000, "seq": 1, "intent": {"id": 1, "kind": "hold", "uids": [1], "source": {"kind": "agent", "id": "agent-planner"}, "created_t": 1000}},
        {"type": "check.result", "t": 1001, "intent": 1, "phase": "submit", "passed": True, "results": [{"rule": "R1", "outcome": "pass"}]},
        {"type": "policy.decision", "t": 1002, "intent": 1, "decision": {"tier": "human", "rule": "t", "facts": {}}},
        {"type": "intent.state", "t": 1003, "intent": 1, "to": "pending_approval", "reason": ""},
        {"type": "approval.granted", "t": 2000, "intent": 1, "by": "op-auto"},
        {"type": "intent.state", "t": 2001, "intent": 1, "to": "approved", "reason": ""},
        {"type": "intent.state", "t": 2002, "intent": 1, "to": "dispatched", "reason": ""},
    ]
    (d / "audit.jsonl").write_text("\n".join(json.dumps(e) for e in events) + "\n")
    story = {"kind": "propose", "step": "a", "t_rel": 5.0, "source": {"kind": "agent", "id": "agent-planner"},
             "request": {"task": "hold", "uids": [1]}, "rationale": "r", "label": label, "reason": "why", "premise": {},
             "vehicle": {"uid": 1, "callsign": "DRONE 1", "platform": "delivery_drone"}, "vehicle_truth": None,
             "vehicle_shown": {}, "intent_id": 1, "state": "pending_approval", "dispatched": False}
    (d / "storyline.jsonl").write_text(json.dumps(story) + "\n")
    info = {"status": "ok", "scenario": "hold-drone", "arm": arm, "seed": 1, "operators": ["op-auto"],
            "replay": {"ran": False}, "spec": {"title": "hold-drone", "steps": []}, "decisions": []}
    (d / "run.json").write_text(json.dumps(info))


def test_a_report_of_scored_runs_builds(tmp_path):
    from eval.report import build

    _write_run(tmp_path, "rules-only", "legitimate")
    _write_run(tmp_path, "human-proxy", "should_reject")
    (tmp_path / "batch.json").write_text(json.dumps({"id": "t", "started": "x", "seeds": [1], "runs": []}))
    out = Path(build(tmp_path))
    html = out.read_text()
    assert "hold-drone" in html and "rules-only" in html and "armed" not in html.lower().replace("unarmed", "")
    assert (tmp_path / "summary.json").exists()
