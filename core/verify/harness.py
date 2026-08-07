# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Launching the built core and asking it questions.

Every verify script needs the same forty lines: a throwaway profile directory,
a config on descriptor 3, a debugging port read out of `DevToolsActivePort`,
a real page to evaluate in, and a tidy shutdown. The first five scripts each
carried their own copy, and four of them had already drifted — different waits,
different window positions, one that read the port before the file existed.

Twenty-two more copies would be twenty-two more chances to be separately wrong,
and the failure mode is the bad one: a verify script that launches slightly
differently from the others tests a browser nobody ships.

So it lives here. What each script then contains is only what it is about.

## Two things this insists on, because both have cost an afternoon

The config MUST carry `"schema_version": 1`. Without it the core aborts on
startup with SIGABRT and no message that mentions the config — the symptom is a
verify script that reports "core exited -6" and sends you to look at the patch.
`launch` adds it if the caller forgot.

Every script needs a CONTROL: an unconfigured build must answer DIFFERENTLY.
A check that also passes when nothing is wired up is not a check, and this
repository has shipped one. `Claims.control` exists to make writing it the easy
path.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cdp import browser  # noqa: E402

# Somewhere a real page loads from. `about:blank` is a special case in more
# than one part of Blink — layout, storage and permissions all treat it
# differently — so a value measured there is not the value a site sees.
PAGE = "https://example.com"


class Session:
    """One running browser, attached and ready to evaluate."""

    def __init__(self, core, config, page=PAGE, extra_args=(),
                 os_crypt_key=None, user_data_dir=None):
        self.dir = user_data_dir or tempfile.mkdtemp(prefix="fury-verify-")
        self.owns_dir = user_data_dir is None
        # A reused profile still holds the PREVIOUS launch's DevToolsActivePort,
        # and _port() would read that stale number and connect to nothing.
        stale = os.path.join(self.dir, "DevToolsActivePort")
        if os.path.exists(stale):
            os.remove(stale)
        args = [
            core,
            f"--user-data-dir={self.dir}",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            # Off-screen rather than headless: --headless=new denies notification
            # permission by policy and reports navigator.webdriver, so a browser
            # configured the way the product never runs it answers questions
            # nobody asked.
            "--window-position=-4000,-4000",
            "--window-size=900,700",
            *extra_args,
        ]

        # The os_crypt key rides its own descriptor, one past the config's:
        # --fury-oscrypt-fd names it, and the core reads exactly 32 bytes.
        key_fd = None
        if os_crypt_key is not None:
            if len(os_crypt_key) != 32:
                raise ValueError("the os_crypt key is 32 bytes")
            r, w = os.pipe()
            os.write(w, os_crypt_key)
            os.close(w)
            key_fd = r
            os.set_inheritable(key_fd, True)
            args.append("--fury-oscrypt-fd=4")

        fd = None
        if config is not None:
            config = dict(config)
            config.setdefault("schema_version", 1)
            path = os.path.join(self.dir, "config.json")
            with open(path, "w") as f:
                json.dump(config, f)
            fd = os.open(path, os.O_RDONLY)
            os.set_inheritable(fd, True)
            args.append("--fury-fp-fd=3")

        def place_descriptors():
            # dup2 into fixed slots, because the switches name numbers rather
            # than passing the descriptors themselves.
            if fd is not None:
                os.dup2(fd, 3)
            if key_fd is not None:
                os.dup2(key_fd, 4)

        self.proc = subprocess.Popen(
            args,
            preexec_fn=place_descriptors if (fd or key_fd) else None,
            close_fds=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if fd is not None:
            os.close(fd)
        if key_fd is not None:
            os.close(key_fd)

        port = self._port()
        time.sleep(1)
        self.ws = browser(port)
        target = self.ws.call("Target.createTarget", {"url": page})["targetId"]
        self.session = self.ws.call(
            "Target.attachToTarget", {"targetId": target, "flatten": True}
        )["sessionId"]
        self.ws.call("Runtime.enable", session=self.session)
        time.sleep(2.5)

    def _port(self):
        marker = os.path.join(self.dir, "DevToolsActivePort")
        for _ in range(300):
            if os.path.exists(marker):
                try:
                    port = int(open(marker).readline().strip())
                except ValueError:
                    port = None
                if port:
                    return port
            if self.proc.poll() is not None:
                # -6 is SIGABRT and almost always means the config was refused;
                # naming that here saves the next person an hour in the patch.
                hint = (
                    "  (-6 is SIGABRT: the core refused the config. Does it have "
                    '"schema_version": 1?)'
                    if self.proc.returncode == -6
                    else ""
                )
                raise RuntimeError(f"the core exited {self.proc.returncode}{hint}")
            time.sleep(0.2)
        raise RuntimeError("the core never wrote DevToolsActivePort")

    def js(self, expression, timeout=40000):
        """Evaluates in the page and returns the value, or a dict with `threw`."""
        r = self.ws.call(
            "Runtime.evaluate",
            {
                "expression": expression,
                "awaitPromise": True,
                "returnByValue": True,
                "timeout": timeout,
            },
            session=self.session,
        )
        if "exceptionDetails" in r:
            return {"threw": str(r["exceptionDetails"].get("text"))}
        # `call` already unwraps the outer "result"; what is left is the
        # RemoteObject, whose value is the answer.
        return r["result"].get("value")

    def close(self):
        try:
            self.ws.close()
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=12)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        if self.owns_dir:
            shutil.rmtree(self.dir, ignore_errors=True)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
        return False


def launch(core, config=None, page=PAGE, extra_args=(), os_crypt_key=None,
           user_data_dir=None):
    """A browser, configured or not. Use as a context manager.

    `extra_args` is for the scripts whose patch only does anything under a
    switch the product does not normally pass — `--enable-automation` being the
    one that matters, since a trace nobody turned on is not a trace.

    `os_crypt_key` is 32 bytes handed over on descriptor 4, the way the agent
    hands a profile its travelling cookie key. `user_data_dir` reuses a profile
    across launches instead of making a throwaway one, which is the only way to
    ask whether what one launch wrote another launch can read.
    """
    return Session(core, config, page, extra_args, os_crypt_key, user_data_dir)


class Claims:
    """What the script asserts, printed one per line."""

    def __init__(self, patch, core):
        self.rows = []
        print(f"verify {patch} — against {core}")

    def check(self, ok, text):
        self.rows.append((bool(ok), text))
        print(f"  {'OK  ' if ok else 'FAIL'} {text}", flush=True)
        return bool(ok)

    def control(self, ok, text):
        """The claim that an unconfigured build answers differently.

        Named apart from `check` so that a script without one is visible at a
        glance. Every check above it proves nothing on its own: a build where
        the patch does nothing, on a machine that happens to agree with the
        persona, passes all of them.
        """
        return self.check(ok, f"CONTROL — {text}")

    def done(self):
        failed = [t for ok, t in self.rows if not ok]
        controls = [t for _, t in self.rows if t.startswith("CONTROL")]
        print()
        if not controls:
            print("FAIL — no control: this script cannot tell a working patch "
                  "from a coincidence")
            return 1
        if failed:
            print(f"FAIL — {len(failed)} of {len(self.rows)} claims are false")
            return 1
        print(f"PASS — {len(self.rows)} claims")
        return 0
