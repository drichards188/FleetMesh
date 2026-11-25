#!/usr/bin/env bash
# Run the stack: swarm (SITL vehicles + the Rust stand-in engine), the link emulator, the Rust
# bridge and the web console. With FLEETMESH_SCENARIO (a tidewater scenario file) also tidewater:
# `tidewater serve` for the bridge, the stand-ins and the emulator's terrain line of sight,
# `tidewater drive` for the SITL vehicles. Ctrl-C (or make down) stops everything this script
# started.
#
# Every vehicle's link goes through fleetmesh-linkemu (crates/fleetmesh-linkemu) by default:
#   vehicle i -> UDP FLEETMESH_BASE_PORT+i -> linkemu -> UDP FLEETMESH_BRIDGE_BASE_PORT+i -> bridge
# degraded per link class (sim/links.toml): loss, latency, bandwidth, terrain (with a
# scenario), SATCOM (static numbers; real satellites when sim/space/cache exists), depth.
# FLEETMESH_LINKEMU=0 bypasses it (vehicles talk to the bridge on FLEETMESH_BASE_PORT directly).
# FLEETMESH_LINK_EVENTS (or FLEETMESH_EVENTS)=<file> adds scripted [[event]]s for comms and GNSS
# (e.g. sim/link-events/golden-gate.toml);
# FLEETMESH_LINKS=<file> replaces sim/links.toml (profiles and ground stations); every process here that reads the profiles inherits it. Emulator log:
# logs/linkemu.log.
# FLEETMESH_LINKEMU_SEED=<n> overrides the links file's seed (its random draws per vehicle and
# direction); unset, the file's seed (1) as before. an evaluation run sets it to the
# episode's seed so a run meets the draws the headless simulator gives that seed.
#
# Ports (env), so a second stack can run beside the default one:
#   FLEETMESH_BASE_PORT         14560  vehicle i sends UDP to base + i (the emulator listens there)
#   FLEETMESH_BRIDGE_BASE_PORT  base+1000  the bridge's UDP ports behind the emulator
#   FLEETMESH_LINKEMU_PORT      8766   emulator stats HTTP (/stats, /stats/<uid>, /health)
#   FLEETMESH_HTTP_PORT         8765   bridge REST + WebSocket
#   FLEETMESH_WEB_PORT          5173   console (vite)
#   FLEETMESH_TIDEWATER_PORT    8770   tidewater serve (reused if one already answers there)
#   FLEETMESH_SITL_INSTANCE_BASE 0     first SITL instance (TCP 5760 + 10*instance)
#   FLEETMESH_STANDIN_SRC_BASE  unset  stand-in i (in sim/run/standins.json's order) sends from UDP
#                                   base + i; unset, from an ephemeral port as before. Set it for
#                                   a fleet of thousands: the ephemeral range (~4,000 here, shared
#                                   by the whole host) is what capped one. The emulator takes no
#                                   port but its listening ones (it answers the bridge from them).
#   FLEETMESH_LINK_HOST         127.0.0.1  the address the vehicles, the emulator and the bridge's
#                                   links talk on (the emulator listens there; the bridge
#                                   listens on every address). Under WSL's mirrored networking
#                                   127.0.0.1 goes out through a virtual NIC (loopback0) whose
#                                   queues drop packets at a few thousand vehicles; another
#                                   127.x address (127.0.0.2) stays on lo.
# Keep every fixed range (vehicle, bridge, stand-in source; each as wide as the fleet) below
# `sysctl net.ipv4.ip_local_port_range` and clear of the other stacks' ranges.
# FLEETMESH_WEB=0 starts no console: the launcher's (`make start`, eval/launcher.py) already serves
# one on FLEETMESH_WEB_PORT, whose origin the bridge still accepts. FLEETMESH_WEB=preview builds the
# console and serves the production build instead of the dev server (make stress).
# FLEETMESH_WEB_MODE=build does the same where the dev server would run (default dev).
# FLEETMESH_TIDEWATER_URL overrides the port with any running `tidewater serve`. The scenario
# clock (FLEETMESH_AT, FLEETMESH_RATE) goes to the stand-ins, tidewater drive, the emulator;
# a FLEETMESH_RATE other than 1 needs FLEETMESH_AT (refused otherwise).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p logs
export PYTHONUNBUFFERED=1
export FLEETMESH_ROOT="$PWD"
export FLEETMESH_BASE_PORT="${FLEETMESH_BASE_PORT:-14560}"
export FLEETMESH_HTTP_PORT="${FLEETMESH_HTTP_PORT:-8765}"
web_port="${FLEETMESH_WEB_PORT:-5173}"
export FLEETMESH_BRIDGE_URL="http://127.0.0.1:$FLEETMESH_HTTP_PORT"  # vite proxies here
# the tasking tools for agents (MCP get_groups, make_group, reply_tasking: an agent's answers to
# the operator's requests): offered on a stack started here; an evaluation stack (eval/stack.py)
# starts its bridge itself and never sets it, so its tool list is as before
export FLEETMESH_TASKING_TOOLS="${FLEETMESH_TASKING_TOOLS:-1}"
case "${FLEETMESH_LINKEMU:-1}" in 0|off|false|no) linkemu=0 ;; *) linkemu=1 ;; esac
case "${FLEETMESH_WEB:-1}" in 0|off|false|no) web=0 ;; preview) web=preview ;; *) web=1 ;; esac
# FLEETMESH_WEB_MODE=build serves the production build where the dev server would be (dev, the default, is as before)
if [ "$web" = 1 ] && [ "${FLEETMESH_WEB_MODE:-dev}" = build ]; then web=preview; fi
vehicle_base="$FLEETMESH_BASE_PORT"
bridge_base="${FLEETMESH_BRIDGE_BASE_PORT:-$((vehicle_base + 1000))}"
emu_port="${FLEETMESH_LINKEMU_PORT:-8766}"
link_host="${FLEETMESH_LINK_HOST:-127.0.0.1}"
# one scripted-events file drives both the link emulator (comms) and the stand-ins (GNSS
# jamming/spoofing); either variable name works
FLEETMESH_LINK_EVENTS="${FLEETMESH_LINK_EVENTS:-${FLEETMESH_EVENTS:-}}"
export FLEETMESH_EVENTS="$FLEETMESH_LINK_EVENTS"
if [ -n "${FLEETMESH_LINK_EVENTS:-}" ] && [ ! -f "$FLEETMESH_LINK_EVENTS" ]; then
  echo "FLEETMESH_LINK_EVENTS: no file $FLEETMESH_LINK_EVENTS" >&2
  exit 1
fi
# the scenario clock (murmur-drd), as every process reads it (fleetmesh_space::scenario_clock):
# FLEETMESH_AT is an RFC 3339 date-time and nothing else, so a start one process accepts every
# process accepts; a scenario rate needs a scenario start (without FLEETMESH_AT every clock is the
# wall clock, at 1x). The bridge, the stand-ins and the emulator refuse
# the same.
rfc3339='^[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?([Zz]|[+-][0-9]{2}:[0-9]{2})$'
if [ -n "${FLEETMESH_AT:-}" ] && ! [[ "$FLEETMESH_AT" =~ $rfc3339 ]]; then
  echo "FLEETMESH_AT=$FLEETMESH_AT is not an RFC 3339 date-time (e.g. 2026-06-14T10:00:00Z, or with an offset, 2026-06-14T03:00:00-07:00): every process reads the scenario start in that one format." >&2
  exit 1
fi
if [ -n "${FLEETMESH_RATE:-}" ] && [ -z "${FLEETMESH_AT:-}" ]; then
  case "$FLEETMESH_RATE" in
    1|1.0|1.00) ;;
    *)
      echo "FLEETMESH_RATE=$FLEETMESH_RATE needs FLEETMESH_AT (e.g. FLEETMESH_AT=2026-06-14T10:00:00Z): without a start the scenario clock is the wall clock, at 1x. Set FLEETMESH_AT, or unset FLEETMESH_RATE." >&2
      exit 1
      ;;
  esac
fi

# build first (a no-op when fresh), so vehicles don't wait on a bridge that is still compiling;
# only the crates this checkout has
pkgs=()
for c in bridge standins linkemu decide auth; do
  if [ -d "crates/fleetmesh-$c" ]; then pkgs+=(-p "fleetmesh-$c"); fi
done
cargo="$(command -v cargo || echo "$HOME/.cargo/bin/cargo")"
if [ -x "$cargo" ] && [ ${#pkgs[@]} -gt 0 ]; then
  "$cargo" build --release "${pkgs[@]}"
fi
for b in fleetmesh-standins fleetmesh-linkemu fleetmesh-bridge; do
  if [ "$b" = fleetmesh-linkemu ] && [ "$linkemu" != 1 ]; then continue; fi
  if [ ! -d "crates/$b" ]; then
    echo "up: crates/$b is not in this checkout yet (not ported); nothing to run" >&2
    exit 0
  fi
  [ -x "target/release/$b" ] || { echo "no target/release/$b and no cargo; run make setup" >&2; exit 1; }
done
if [ ! -d web ] && [ "$web" != 0 ]; then
  echo "web: no web/ in this checkout yet; starting without the console"
  web=0
fi

# who may act (rules 8): a dev run by default (FLEETMESH_AUTH unset or dev: operator ids asserted
# by header, the console's "Acting as" switcher, logged as dev, no human-control claim, no
# credential file needed). FLEETMESH_AUTH=credentials: operators log in with their own password
# from the credential file (fleetmesh-auth; FLEETMESH_CREDENTIALS, else
# ~/.config/fleetmesh/credentials.json), agents with their own key.
export FLEETMESH_WEB_PORT="$web_port"  # the console's origin, which the bridge accepts requests from
case "${FLEETMESH_AUTH:-dev}" in
  dev) echo "auth: DEV run (operator ids asserted by header; FLEETMESH_AUTH=credentials signs operators in)" ;;
  credentials)
    if [ -x target/release/fleetmesh-auth ]; then
      creds="$(target/release/fleetmesh-auth path)"
      if [ ! -s "$creds" ]; then
        echo "FLEETMESH_AUTH=credentials, but no operator credentials at $creds. Add one (password on stdin), e.g." >&2
        echo "  target/release/fleetmesh-auth add op-1" >&2
        echo "or leave FLEETMESH_AUTH unset for a dev run (header ids, logged as dev, no human-control claim)." >&2
        exit 1
      fi
    fi
    ;;
  *) echo "FLEETMESH_AUTH=${FLEETMESH_AUTH}: dev (the default) or credentials" >&2; exit 1 ;;
esac

echo $$ > logs/up.pid
pids=()
cleanup() {
  trap - INT TERM EXIT
  [ ${#pids[@]} -gt 0 ] && kill "${pids[@]}" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -f logs/up.pid logs/tidewater-serve.pid logs/linkemu.pid
}
trap cleanup EXIT
trap 'cleanup; exit 0' INT TERM  # a requested stop (Ctrl-C, make down) is a clean exit

# real-world conditions from a tidewater scenario (tidewater/scenarios), if one is set; the
# tidewater CLI is installed editable into .venv (make setup)
TIDEWATER=.venv/bin/tidewater
if [ -n "${FLEETMESH_SCENARIO:-}" ]; then
  : "${FLEETMESH_AT:?set FLEETMESH_AT, e.g. 2026-06-14T10:00:00Z}"
  [ -x "$TIDEWATER" ] || { echo "no $TIDEWATER: run make setup (it installs tidewater/ into .venv)" >&2; exit 1; }
  tw_port="${FLEETMESH_TIDEWATER_PORT:-8770}"
  export FLEETMESH_TIDEWATER_URL="${FLEETMESH_TIDEWATER_URL:-http://127.0.0.1:$tw_port}"
  if curl -sf -m 2 "$FLEETMESH_TIDEWATER_URL/v1/info" > /dev/null; then
    echo "tidewater serve: using the one at $FLEETMESH_TIDEWATER_URL"
  else
    # bridge and stand-ins query it from their first tick, so it must answer before they start
    "$TIDEWATER" serve "$FLEETMESH_SCENARIO" --port "$tw_port" > logs/tidewater-serve.log 2>&1 & pids+=($!)
    echo "${pids[-1]}" > logs/tidewater-serve.pid
    echo -n "tidewater serve: starting on :$tw_port"
    for _ in $(seq 120); do
      curl -sf -m 2 "$FLEETMESH_TIDEWATER_URL/v1/info" > /dev/null && break
      kill -0 "${pids[-1]}" 2>/dev/null || { echo; echo "tidewater serve exited; see logs/tidewater-serve.log" >&2; exit 1; }
      echo -n .; sleep 1
    done
    curl -sf -m 2 "$FLEETMESH_TIDEWATER_URL/v1/info" > /dev/null \
      || { echo; echo "tidewater serve did not answer in 120 s" >&2; exit 1; }
    echo " up"
  fi
fi

# This run's fleet manifest first, in both modes: the emulator binds every vehicle port from
# it before any vehicle sends, and the bridge reads it as it starts (its carry-over check
# compares the previous run's open orders against this run's fleet; started beside swarm.py
# it could read the previous run's sim/run/fleet.json, murmur-0u7). swarm.py leaves it as it
# is when it launches the vehicles. The bridge logs which manifest it read and how old it was.
.venv/bin/python sim/swarm.py --base-port "$vehicle_base" --bridge-host "$link_host" "$@" --manifest-only
bridge_env=()
if [ "$linkemu" = 1 ]; then
  emu_args=(--fleet sim/run/fleet.json --config "${FLEETMESH_LINKS:-sim/links.toml}" --vehicle-base "$vehicle_base"
            --bridge-base "$bridge_base" --manifest-out sim/run/bridge-fleet.json
            --listen-host "$link_host" --bridge-host "$link_host"
            --http "127.0.0.1:$emu_port" --log-interval 10)
  [ -n "${FLEETMESH_LINK_EVENTS:-}" ] && emu_args+=(--scenario "$FLEETMESH_LINK_EVENTS")
  [ -n "${FLEETMESH_LINKEMU_SEED:-}" ] && emu_args+=(--seed "$FLEETMESH_LINKEMU_SEED")
  # terrain line of sight needs tidewater (FLEETMESH_TIDEWATER_URL, set above with a scenario);
  # SATCOM follows real satellites when sim/space/cache exists, else static numbers
  [ -d sim/space/cache ] || echo "linkemu: no sim/space/cache: SATCOM uses static numbers"
  rm -f sim/run/bridge-fleet.json
  target/release/fleetmesh-linkemu "${emu_args[@]}" > logs/linkemu.log 2>&1 & pids+=($!)
  echo "${pids[-1]}" > logs/linkemu.pid
  echo -n "linkemu: starting (vehicles :$vehicle_base+ -> bridge :$bridge_base+, stats :$emu_port)"
  for _ in $(seq 120); do
    curl -sf -m 2 "http://127.0.0.1:$emu_port/health" > /dev/null && break
    kill -0 "${pids[-1]}" 2>/dev/null || { echo; echo "linkemu exited; see logs/linkemu.log" >&2; exit 1; }
    echo -n .; sleep 1
  done
  curl -sf -m 2 "http://127.0.0.1:$emu_port/health" > /dev/null \
    || { echo; echo "linkemu did not answer in 120 s" >&2; exit 1; }
  echo " up"
  bridge_env=(FLEETMESH_BASE_PORT="$bridge_base" FLEETMESH_FLEET_MANIFEST="$PWD/sim/run/bridge-fleet.json"
              FLEETMESH_LINKEMU_URL="http://127.0.0.1:$emu_port")
else
  echo "linkemu: off (FLEETMESH_LINKEMU=0); vehicles talk to the bridge directly"
fi

# exec in each subshell so the tracked pid is the real process and cleanup reaches it
.venv/bin/python sim/swarm.py --base-port "$vehicle_base" --bridge-host "$link_host" "$@" > logs/sim.log 2>&1 & pids+=($!)
# two glibc malloc arenas, not one per thread: at 3,602 vehicles the bridge
# held 3.4 GB after 13 minutes with the default and 0.8 GB with two, its loops as fast
# (docs/load-test.md); MALLOC_ARENA_MAX in the environment wins
env "${bridge_env[@]}" MALLOC_ARENA_MAX="${MALLOC_ARENA_MAX:-2}" target/release/fleetmesh-bridge > logs/bridge.log 2>&1 & pids+=($!)
if [ "$web" = 1 ]; then
  (cd web && exec node_modules/.bin/vite --host 127.0.0.1 --port "$web_port" --strictPort) > logs/web.log 2>&1 & pids+=($!)
  starting="swarm, bridge (:$FLEETMESH_HTTP_PORT) and web"
elif [ "$web" = preview ]; then
  # the production build (what load measurements use: React's development build is slower)
  (cd web && node_modules/.bin/vite build --logLevel warn \
     && exec node_modules/.bin/vite preview --host 127.0.0.1 --port "$web_port" --strictPort) > logs/web.log 2>&1 & pids+=($!)
  starting="swarm, bridge (:$FLEETMESH_HTTP_PORT) and the production console (built first)"
else
  echo "web: not started (FLEETMESH_WEB=0 or no web/): a console already on :$web_port can serve this stack"
  starting="swarm and bridge (:$FLEETMESH_HTTP_PORT)"
fi
if [ -n "${FLEETMESH_SCENARIO:-}" ]; then
  # tidewater drive pushes conditions into the SITL vehicles over their SERIAL1 ports
  instances=$(.venv/bin/python sim/swarm.py "$@" --sitl-instances)
  if [ -n "$instances" ]; then
    "$TIDEWATER" drive "$FLEETMESH_SCENARIO" --at "$FLEETMESH_AT" \
      --rate "${FLEETMESH_RATE:-1}" --instances "$instances" > logs/tidewater.log 2>&1 & pids+=($!)
  fi
  echo "tidewater: $FLEETMESH_SCENARIO from $FLEETMESH_AT (logs/tidewater*.log)"
fi

echo "$starting starting; logs in ./logs"
echo "open http://127.0.0.1:$web_port  (vehicles get a GPS fix ~20-30s after start)"
if [ "${FLEETMESH_AUTH:-dev}" = credentials ]; then
  # rules 10: WebAuthn takes no IP address as a relying party: passkeys need the localhost name
  echo "passkeys (sign-in, step-ups, registering one): open http://localhost:$web_port instead"
fi
wait -n "${pids[@]}"
echo "a process exited; see logs/. stopping the rest."
