# SPDX-License-Identifier: AGPL-3.0-or-later
"""Template: start a profile, do the job, always stop it.

Copy ../../fury.py next to this file (or import it from there) rather than
writing a client: it reads the token from disk, which is the rule.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
from fury import Fury, running  # noqa: E402


def job(ws_endpoint: str) -> None:
    """The part that is yours. ws_endpoint is the profile's DevTools address."""
    print("connected to", ws_endpoint)


def main() -> int:
    # `running` stops the profile whatever happens, so a failure in job()
    # never leaves the browser open holding the profile's lock.
    with running(Fury(), sys.argv[1]) as session:
        if not session.get("ws_endpoint"):
            print("no DevTools endpoint: a team role may forbid CDP for this profile")
            return 1
        job(session["ws_endpoint"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
