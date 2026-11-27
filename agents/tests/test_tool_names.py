"""The watch officer and its tools' names (agents/watch_officer.py): Claude Code names the bridge's
tools mcp__fleetmesh__<tool>, the prompt names them bare, and a model sometimes calls the bare name.
The CLI refuses it (nothing reaches the bridge); a run on 2026-11-27 had every verdict refused and
every item reach the person at its deadline. The prompt says the full names, and a turn that
called a FleetMesh tool by its bare name is told at once to call it again by the full one."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import watch_officer as w  # noqa: E402

PREFIX = "mcp__fleetmesh__"
TOOLS = {"get_inbox", "triage_item", "explain_intent"}


def test_the_prompt_says_the_full_names_only_for_a_cli_that_prefixes_them():
    claude = w.build_prompt("brief", tool_prefix=PREFIX)
    assert f"`{PREFIX}triage_item`" in claude and "## Situation brief" in claude
    assert claude.index("## Your tools' names") < claude.index("## Situation brief")
    assert "## Your tools' names" not in w.build_prompt("brief")


def test_a_bare_fleetmesh_name_is_called_again_by_its_full_name():
    refused = [{"name": "triage_item"}, {"name": "triage_item"}, {"name": "Bash"}, {"name": f"{PREFIX}get_inbox"}]
    assert w.misnamed(refused, TOOLS, PREFIX) == ["triage_item"], "once each; not a tool outside the server"
    assert w.misnamed(refused, TOOLS, "") == [], "a CLI without a prefix (Codex): nothing to rename"
    note = w.rename_note(["triage_item"], PREFIX)
    assert f"`triage_item` is `{PREFIX}triage_item`" in note and "again" in note
    assert "—" not in note
