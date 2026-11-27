.PHONY: doctor setup build start scenarios up down sim bridge web eval headless stress check

FLEET ?= delivery-demo   # any sim/fleets/*.toml
CARGO ?= $(shell command -v cargo || echo $(HOME)/.cargo/bin/cargo)
PY := .venv/bin/python

# the release binaries a run starts: only the crates this checkout has (the rest are not ported yet)
RUN_PKGS := $(foreach c,bridge standins linkemu decide auth,$(if $(wildcard crates/fleetmesh-$(c)),-p fleetmesh-$(c)))
# a step for a part that is not in the checkout yet says so and moves on
skip = echo "skipped: $(1) is not in this checkout yet"

doctor:  ## is this machine ready? prerequisites, setup state, ports, optional AI CLIs (OK / MISSING / WARN; exit 1 if anything is missing)
	@./scripts/doctor.sh

setup:   ## one-time: venv, Rust release build, ArduPilot SITL build, npm install
	./scripts/setup.sh

build:   ## release builds of the Rust crates a run starts: bridge, stand-ins, link emulator, decision engine (fleetmesh-replay) and fleetmesh-auth (up.sh does this too)
	@if [ -n "$(RUN_PKGS)" ]; then $(CARGO) build --release $(RUN_PKGS); else $(call skip,crates/); fi

DELIVERY_SCENARIOS := $(notdir $(wildcard sim/scenarios/*))

start:   ## the main menu (eval/launcher.py): open http://localhost:5173 and start a scenario there; Ctrl-C ends the running one and stops the launcher. SCENARIO=harbour-city (or wind-event, coastal-boat; make scenarios lists them) starts that delivery scenario on stand-ins instead (SCALE=0.25 for a fraction of its fleet and demand; AI=claude or codex adds the AI watch officer on your own CLI login)
	@if [ -n "$(SCENARIO)" ]; then $(if $(AI),FLEETMESH_OFFICER=$(AI)) ./scripts/scenario.sh $(SCENARIO) $(ARGS); \
	elif [ -f eval/launcher.py ]; then $(PY) -m eval.launcher; else $(call skip,eval/launcher.py); fi

scenarios: ## list the delivery scenarios (sim/scenarios): make start SCENARIO=name
	@for s in $(DELIVERY_SCENARIOS); do echo "$$s"; done

up:      ## fleet + link emulator + bridge + web (FLEET=, ARGS=; FLEETMESH_LINKEMU=0 bypasses the emulator; FLEETMESH_TRAFFIC=0 starts no traffic simulator; ports: scripts/up.sh; FLEETMESH_SCENARIO=<tidewater scenario> with FLEETMESH_AT adds real tide, current and wind; AI=claude or codex adds the AI watch officer)
	$(if $(AI),FLEETMESH_OFFICER=$(AI)) ./scripts/up.sh --fleet $(FLEET) $(ARGS)

# a setting that comes from make's command line only: a shell may export N for something else
cmd_arg = $(if $(filter command line,$(origin $(1))),$($(1)))
stress:  ## the console under load, to watch: N=3600 stand-ins over the Bay (default), SWARM=1 a dense swarm, N=0 the map with no vehicles; production console, no link emulator or tidewater, default ports; open http://localhost:5173/?perf; make down stops it
	./scripts/stress.sh $(or $(call cmd_arg,N),3600) $(if $(filter 1,$(call cmd_arg,SWARM)),--swarm)

down:    ## stop a stack started with make up (e.g. from another terminal)
	@# TERM, not INT: a shell started in the background ignores SIGINT and can't trap it
	@if [ -f logs/up.pid ]; then kill -TERM $$(cat logs/up.pid) && echo stopping; else echo "not running"; fi

sim:     ## the fleet alone: SITL vehicles + Rust stand-ins (FLEET=, ARGS= for swarm.py)
	$(PY) sim/swarm.py --fleet $(FLEET) $(ARGS)

bridge:  ## MAVLink <-> WebSocket bridge on :8765 (crates/fleetmesh-bridge)
	@if [ -d crates/fleetmesh-bridge ]; then $(CARGO) run --release -p fleetmesh-bridge; else $(call skip,crates/fleetmesh-bridge); fi

web:     ## console on http://127.0.0.1:5173
	@if [ -f web/package.json ]; then cd web && npx vite --host 127.0.0.1; else $(call skip,web/); fi

eval: build ## evaluation batch on isolated stacks (eval/README.md): SCENARIOS=, ARMS=, SEEDS=, PARALLEL=
	@if [ -f eval/run.py ]; then \
	  $(PY) -m eval.run batch $(if $(SCENARIOS),--scenarios $(SCENARIOS)) $(if $(ARMS),--arms $(ARMS)) --seeds $(or $(SEEDS),1 2 3) $(if $(PARALLEL),--parallel $(PARALLEL)); \
	else $(call skip,eval/run.py); fi

headless: ## headless simulator batch: FAMILIES=, SEEDS=1-10, WORKERS=, AGENT=
	@if [ -d crates/fleetmesh-headless ] && [ -f eval/headless/batch.py ]; then \
	  $(CARGO) build --release -p fleetmesh-headless && \
	  $(PY) -m eval.headless.batch --no-build $(if $(FAMILIES),--families $(FAMILIES)) --seeds $(or $(SEEDS),1-10) $(if $(WORKERS),--workers $(WORKERS)) $(if $(AGENT),--agent $(AGENT)); \
	else $(call skip,crates/fleetmesh-headless or eval/headless/); fi

# Python test directories that exist (a part not ported yet has none)
PY_TESTS := $(wildcard eval/tests agents/tests tidewater/tests sim/tests eval/headless/tests)
# Python sources to byte-compile: only the directories that exist
PY_SRC := $(wildcard sim/swarm.py sim/tests/*.py scripts/*.py agents/*.py agents/watch/*.py eval/*.py eval/tests/*.py eval/headless/*.py eval/headless/tests/*.py agents/tests/*.py)

check:   ## typecheck + lint + every web/src *.check.ts (node 22.18+: it runs the .ts directly) + Python compile and tests (swarm generator, evaluation, agents, tidewater) + cargo clippy and test; a part not in the checkout yet is skipped
	@if [ -f web/package.json ] && [ -d web/node_modules ]; then \
	  cd web && npx tsc -b && npx oxlint src && \
	  for f in $$(find src -name '*.check.ts' | sort); do echo "node $$f"; node $$f || exit 1; done; \
	else $(call skip,web/ (or its node_modules: make setup)); fi
	$(PY) -m py_compile $(PY_SRC)
	@for d in $(PY_TESTS); do echo "pytest $$d"; $(PY) -m pytest -q $$d || exit 1; done
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) test --workspace
	@if [ -d crates/fleetmesh-headless ] && [ -d crates/fleetmesh-decide ]; then $(CARGO) build --release -p fleetmesh-headless -p fleetmesh-decide; else $(call skip,crates/fleetmesh-headless or crates/fleetmesh-decide); fi
