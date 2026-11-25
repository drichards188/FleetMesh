"""The headless simulator's Python side (evaluation only).

    sim.py          the `fleetmesh-headless` process client and the session adapter for agents
    latency.py      the latency-charged clock's constants (latency.toml)
    provenance.py   what an episode was made from, beyond the code at a commit
    v2/stats.py     the statistics of the scoring (situations as the unit, bootstrap, sign tests)

MODEL RESULTS ONLY: an operator here is a scripted model with stated parameters. Nothing here is
human data or evidence about people. The episode runner, its families of situations and the
scoring's labels were cut with the dropped parts they were built on and are to be written fresh
for delivery.
"""

from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
