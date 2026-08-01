# Verifying a patch on a browser that is running

`git apply --check` says a patch applies. A compiler says it builds. Neither
says it works, and three times now the difference has been the whole story: the
`--lang` conclusion was right about the source and wrong about the browser,
patch 0110 typechecked in every language it was written in and did not compile,
and patch 0082 applied, linked, and answered every `getCurrentPosition` with
"Timeout expired" because macOS asks the operating system for permission before
it asks any provider for a position.

So: scripts here drive the built core over CDP and ask it questions.

    core/verify/verify-0082.py core/src/out/macos-arm64-lowmem/Chromium.app/Contents/MacOS/Chromium

Each one launches the core itself with a hand-written config on fd 3 — no agent,
no relay, no proxy — so a failure is the patch and not the stack around it. They
print one line per claim and exit non-zero if any claim is false.

`cdp.py` is a CDP client in about a hundred lines. It exists because neither
node nor python-websockets is on this machine, and a verification that depends
on installing something is a verification that gets skipped.

## Writing one

State claims, not steps. "position is the configured one" is a claim; "call
getCurrentPosition" is a step, and a step cannot fail informatively.

Run headed. `--headless=new` denies notification permission by policy and
reports `navigator.webdriver`, and a browser configured in a way the product
never uses answers questions nobody asked. Park the window off-screen
(`--window-position=-4000,-4000`) instead.

Watch what the harness itself changes. `Browser.grantPermissions` *replaces* an
origin's permissions — everything not in the list becomes denied — which cost a
rebuild spent blaming patch 0090 for a result the test had caused.
