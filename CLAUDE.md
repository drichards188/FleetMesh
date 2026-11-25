# FleetMesh

Open-source testbed for supervising large fleets of autonomous delivery vehicles (drones first,
then ground and surface). One operator oversees many nodes; the first question is whether human
judgment over consequential decisions stays meaningful as the fleet scales. Every order is
checked by a deterministic engine and routed by policy (sent at once, or held for a person);
agents propose and explain but never approve; every decision is logged in a hash-chained audit
log that replay can re-derive.

Status: forked from an earlier project (Murmur) and still being cut down. The rename to `fleetmesh-`
crates, `FLEETMESH_*` env vars and paths is done (see docs/DECISIONS.md). Treat any module that
has no delivery meaning as scheduled for removal, not as design to extend.

@AGENTS.md

## Start of a session

1. Read `docs/PLAN.md` and `docs/DECISIONS.md` when they exist for the delivery plan; until
   then the plan is in bd.
2. `bd ready` for open work (bd usage in AGENTS.md).

## Run and verify

- `make doctor` checks the machine; `make setup` builds everything (about 30 minutes, mostly
  downloading real-world conditions and building ArduPilot's simulator in `~/ardupilot`).
- `make check` runs typecheck, lint, `cargo clippy`, `cargo test` and tidewater's tests.
  Tidewater alone: `.venv/bin/python -m pytest -q tidewater/tests`.
- Console http://localhost:5173, bridge http://127.0.0.1:8765 (`/api/state`, `/api/intents`,
  `/api/inbox`, MCP at `/mcp`). A run is dev by default (header ids, red DEV pill, never
  evidence); `FLEETMESH_AUTH=credentials` signs operators in with their own password or passkey.
- Live path is Rust: `crates/fleetmesh-bridge` (REST, MCP, tasks), `crates/fleetmesh-standins`
  (stand-in vehicles), `crates/fleetmesh-linkemu` (link emulator), `crates/fleetmesh-decide` (the
  decision engine, audit, replay). `tidewater/` supplies real terrain, tide, current and wind.

## Gotchas

- Never `pkill -f <pattern>` that appears in your own command line (kills your shell). Use
  `make down`.
- Background tasks here stop after 2 hours: start long runs detached
  (`setsid nohup make <target> > logs/x.out 2>&1 &`). Foreground `sleep` is blocked: wait with
  `until <check>; do sleep 2; done`.
- Global git `core.autocrlf=true` (WSL): rely on `.gitattributes` (LF).
- WSL mirrored networking drops UDP and stalls loopback TCP at a few thousand vehicles; keep
  fixed port bases below the ephemeral range (`sysctl net.ipv4.ip_local_port_range`); a stack
  uses two UDP ranges (vehicle and bridge), pick them clear of other stacks.
- A checkout runs one stack at a time (`logs/up.pid`). Worktrees need their own `.venv` and a
  symlink to `tidewater/data/cache`.
- After changing SITL code or params, rebuild all firmwares in `~/ardupilot` with `waf`.
- Stream-rate params are `MAV1_*`; pass SITL one defaults file, not a list.
- Stop SITL vehicles with forces the IMU feels, never by zeroing velocity or clamping position.
- MapLibre is pinned to v5 (deck.gl 9.4 breaks on v6). Do not upgrade.
- The scenario clock is `FLEETMESH_AT` (RFC 3339 only) plus elapsed x `FLEETMESH_RATE`.
- Decisions are replayed under the rules version logged in each run: gate new behaviour on a
  named `*_SINCE` constant (never a literal version) and add it to replay in the same change.

## Conventions

- bd for all task tracking (`bd ready`, `bd create`, `bd close`); no markdown TODO lists.
- No AI attribution in commits; no em dashes in prose (global CLAUDE.md).
- Commit subjects: imperative, 72 characters at most, no trailing period; a body saying what
  was wrong or missing, what changed, and what it was checked against. One coherent change per
  commit; squash branch work onto a linear main.
