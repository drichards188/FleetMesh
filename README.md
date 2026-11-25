# FleetMesh

An open-source testbed for supervising large fleets of autonomous delivery vehicles: one
operator, many nodes, real conditions (tide, current, wind), and AI assistants that propose
orders. The question it studies is whether human judgment over consequential decisions stays
meaningful as the fleet scales.

**Status:** a working draft forked from an earlier project and reshaped for delivery. Nothing
here is connected to anything real.

- Every order is checked by a deterministic engine and routed by policy: sent at once, or held
  for a person.
- Agents propose and explain; they never approve.
- Every decision is logged in a hash-chained audit log that replay can re-derive.

## Set up and run

Linux or WSL2, with git, curl, uv, Rust and Node 22.18 or later.

```bash
make doctor                       # checks the machine
make setup                        # about 30 minutes; builds everything
make start SCENARIO=harbour-city  # or wind-event, or coastal-boat
make check                        # typecheck, lint, clippy and tests
make down                         # stops everything
```

`make start` alone opens the menu at http://localhost:5173; with `SCENARIO=` it starts that
scenario (add `SCALE=0.25` for a fraction of its fleet). The bridge answers at http://127.0.0.1:8765.
See `AGENTS.md` for more on using it, and `CLAUDE.md` for developers.
