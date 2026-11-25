#!/usr/bin/env python3
"""The MCP session for a FleetMesh agent (the library the evaluation harness and the watch officer
use), and a small connect check. No model.

- As a library: `AgentSession` wraps the official MCP Python SDK session with the agent
  header and typed helpers (fleet, inbox, intents, preview, propose, withdraw, explain). The
  evaluation harness (`eval/agent.py`) and the watch officer propose through it. Proposing is all
  an agent can do: no tool approves anything.
- As a program: connects as an agent, lists the bridge's tools and the agent's own intents, and
  exits.

    .venv/bin/python agents/policy_agent.py --url http://127.0.0.1:8765/mcp

Needs `mcp>=2`. Nothing here calls an external model API.
"""

from __future__ import annotations

import argparse
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from scripted_agent import agent_headers, attr, call, content  # noqa: E402

from mcp import ClientSession, types  # noqa: E402
from mcp.client.streamable_http import streamable_http_client  # noqa: E402

try:  # the SDK's HTTP client (mcp >= 2 ships its own httpx fork)
    import httpx2 as httpx
except ImportError:  # pragma: no cover
    import httpx

class AgentSession:
    """An MCP session as one agent id. Use as `async with AgentSession(url, agent) as a:`."""

    def __init__(self, url: str = "http://127.0.0.1:8765/mcp", agent: str = "agent-planner", key: str | None = None,
                 client_version: str = "0.1"):
        self.url = url
        self.agent = agent
        # the agent's API key on a bridge that authenticates (FLEETMESH_AGENT_KEY by default)
        self.key = key
        # the MCP clientInfo.version sent at initialize: the bridge logs it in `agent.action
        # session.connected`
        self.client_version = client_version
        self.session: ClientSession | None = None
        self.tools: list[str] = []
        self._stack: list = []
        self._http = None

    async def __aenter__(self) -> "AgentSession":
        http = httpx.AsyncClient(headers=agent_headers(self.agent, self.key), timeout=httpx.Timeout(30, read=300))
        self._http = http
        try:
            transport = streamable_http_client(self.url, http_client=http)
            read, write = await transport.__aenter__()
            self._stack.append(transport)
            session = ClientSession(read, write, client_info=types.Implementation(name=self.agent, version=self.client_version))
            await session.__aenter__()
            self._stack.append(session)
            await session.initialize()
            self.session = session
            self.tools = [t.name for t in (await session.list_tools()).tools]
            if any("approve" in n for n in self.tools):
                raise RuntimeError(f"the server offers an approve tool: {self.tools}")
        except BaseException:
            # close what was opened (the transport ends the server's session): a caller that
            # retries every few seconds must not leave a session behind each time
            await self.__aexit__(None, None, None)
            raise
        return self

    async def __aexit__(self, *exc) -> None:
        for cm in reversed(self._stack):
            try:
                await cm.__aexit__(None, None, None)
            except Exception:  # noqa: BLE001 (closing is best effort)
                pass
        self._stack.clear()
        self.session = None
        # the HTTP client we made (the transport only borrows it): its connections close too
        http, self._http = self._http, None
        if http is not None:
            try:
                await http.aclose()
            except Exception:  # noqa: BLE001
                pass

    async def call(self, name: str, **args):
        if self.session is None:
            raise RuntimeError("session not open")
        return await call(self.session, name, **args)

    async def fleet(self) -> dict:
        return await self.call("get_fleet")

    async def inbox(self) -> dict:
        return await self.call("get_inbox")

    async def intents(self, mine: bool = True) -> list[dict]:
        return list((await self.call("get_intents", mine=mine)).get("intents", []))

    async def preview(self, **req) -> dict:
        return await self.call("preview_intent", **req)

    async def propose(self, rationale: str, **req) -> dict:
        """propose_intent. A refusal that still created an intent (its checks failed: it
        ends rejected by the rules) comes back as that intent with `refused` and the
        reason, not as an exception, so the caller can follow it in the audit log."""
        if self.session is None:
            raise RuntimeError("session not open")
        result = await self.session.call_tool("propose_intent", {"rationale": rationale, **req})
        body = content(result)
        if attr(result, "is_error", "isError"):
            intent = body.get("intent") if isinstance(body, dict) else None
            if isinstance(intent, dict) and intent.get("id") is not None:
                return {"id": intent["id"], "state": intent.get("state"), "dispatched": False,
                        "intent": intent, "refused": body.get("error")}
            raise RuntimeError(f"propose_intent: {body.get('error', body) if isinstance(body, dict) else body}")
        return body

    async def withdraw(self, id: int, reason: str) -> dict:
        return await self.call("withdraw_intent", id=id, reason=reason)

    async def explain(self, id: int) -> dict:
        return await self.call("explain_intent", id=id)


async def main_async(args) -> None:
    async with AgentSession(args.url, args.agent, args.key) as a:
        print(f"connected as {a.agent}; tools: {', '.join(a.tools)}")
        mine = await a.intents(mine=True)
        print("my intents: " + (", ".join(f"{i['id']}:{i['state']}" for i in mine) or "none"))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", default="http://127.0.0.1:8765/mcp")
    ap.add_argument("--agent", default="agent-planner")
    ap.add_argument("--key", help="the agent's API key (fleetmesh-auth agent-add; default FLEETMESH_AGENT_KEY)")
    asyncio.run(main_async(ap.parse_args()))


if __name__ == "__main__":
    main()
