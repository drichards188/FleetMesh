"""What the C2 shows against a task: the generic rule book.

One function, used twice: the proxy operator applies it when it reads a card
(eval/proxy.py), and the metrics apply it to what was recorded at proposal time to sort each
wrong proposal into "the C2 showed something against it" (catchable from the picture) or
"the C2 showed nothing against it" (the picture itself was wrong) (eval/metrics.py).

It reads only the card's checks: any check that failed or could not be verified. Nothing here
reads the rationale, truth, or anything a scenario scripts.
"""

from __future__ import annotations

from typing import Any

RULEBOOK_VERSION = "v3, checks only"


def evidence(checks: list[dict[str, Any]]) -> list[str]:
    """Every reason what the C2 shows speaks against the task; empty when nothing does."""
    return [
        f"check {c.get('rule')}{':' + str(c['uid']) if c.get('uid') is not None else ''} {c.get('outcome')}"
        for c in checks
        if c.get("outcome") in ("fail", "unverified")
    ]
