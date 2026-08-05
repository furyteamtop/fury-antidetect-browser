<!--
Delete whatever does not apply. A short PR that answers the two questions below
is worth more than a long one that answers neither.
-->

## What this changes

<!-- One or two sentences. What is different afterwards. -->

## How you know it works

<!--
The one question this project actually cares about, because the answer has been
"it doesn't" often enough to be worth asking every time.

  git apply --check   says a patch applies.
  the compiler        says it builds.

Neither says it works, and three times here the difference was the whole story:
a --lang conclusion that was right about the source and wrong about the browser;
a patch that typechecked in every language it was written in and did not
compile; and a geolocation patch that applied, linked, and answered every
request with "Timeout expired" because macOS asks the OS for permission before
it asks any provider for a position.

So: what did you run, and what did it print?
-->

## For a fingerprint patch

- [ ] There is a `core/verify/verify-NNNN.py` beside it, and it passes.
- [ ] It has a **control**: an unconfigured build reports something *different*.
      A check that also passes when nothing is wired up is not a check.
- [ ] It reads the value from more than the main frame — a Worker and an iframe
      at least. A config that reaches the top document and not the one below it
      is exactly the cross-context disagreement a detector looks for.
- [ ] The entry in `core/patches/series` says *why*, not just *what*. The series
      is the design record; a patch whose reasoning is only in the diff is a
      patch nobody can rebase.

## Checklist

- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --all-targets` is clean for anything you touched.
- [ ] If you touched anything platform-specific:
      `cargo check -p fury-platform --target x86_64-pc-windows-msvc`.
- [ ] No measurement was quoted that you did not take. If a number is an
      estimate, the text says so — that rule is why the README says "measured
      rather than estimated" in some places and not in others.
