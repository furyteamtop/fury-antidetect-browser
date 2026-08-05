# Security

## Reporting a vulnerability

Use [GitHub's private advisory form](../../security/advisories/new). It opens a
report only maintainers can see, and it is the right channel for anything below.

If that is not available to you, write to the Telegram contact in the README and
say only that you have a security report — do not put the details in the first
message.

Please do not open a public issue for a vulnerability. Everything else belongs
in a public issue, including bugs that merely look alarming; if you are unsure
which one you have, use the private form and we will move it out if it is fine
in the open.

There is no bounty. There is no company behind this and no budget to run one,
and saying so plainly is better than a page that implies otherwise.

## What counts as a vulnerability here

This is an anti-detect browser, so "security" means two different things and
both matter.

**The usual kind.** Anything that lets code that should not have access get it:
a page reaching the agent's IPC channel, the local HTTP API answering without
its token, a profile bundle decryptable without the key, a server endpoint
returning another organisation's rows, a proxy password recoverable from disk,
an escape from Chromium's sandbox that our patches introduced.

**The kind specific to this project: a fingerprint that gives the profile away.**
If you can distinguish a Fury profile from the browser it claims to be, that is
a real defect and we want it. Concretely:

- a value the persona sets that some context reports differently — a Worker, a
  ServiceWorker, an iframe, an AudioWorklet. Cross-context disagreement is the
  single most valuable class of report, because it is what real detectors look
  for and it is invisible from the main frame.
- a property that reveals automation, a patched engine, or Fury itself.
- a real value leaking through a spoofed one — a timezone, a locale, a font
  list, a device, an address.

A working page that demonstrates it is worth more than a description, and
`tools/detect-suite/` is the harness we use ourselves.

**Not a vulnerability:** that a site detects a *proxy*, a datacentre IP, or an
account's behaviour. Fury spoofs a browser, and never claimed to spoof a network
or a person. The README's "Not done yet" table lists what is known to be
missing, and something already on it is not a new finding — though a measurement
that makes it worse than the table says certainly is.

## What we will do

You will get a reply. If the report is a fingerprint difference, the reply will
usually include whether we could reproduce it, because a claim about a browser
that nobody has run is not worth much in either direction.

Fixes land as a patch in `core/patches/` with a verify script beside it in
`core/verify/`, so the fix and the proof that it works arrive together. That is
the house rule for everything here and it applies to security fixes in
particular: a patch that applies and compiles has been wrong about the browser
before.

Credit in the commit and in the release notes, under whatever name you want,
including none.

## Supported versions

None yet — there has been no release. Until there is, the supported version is
`main`, and a report against anything else cannot be acted on.

## What this project cannot protect you from

Worth stating, because an anti-detect browser attracts the assumption that it
does more than it does.

It hides how your browser looks. It does not hide who you are: an account you
log into identifies you, a payment identifies you, and a pattern of behaviour
identifies you across every profile that shares it. It is not Tor and is not a
substitute for it. It does not make anything legal that was not.

`ACCEPTABLE_USE.md` says what we ask of you. It is a request, not a technical
control, and it is deliberately not enforced in code — a browser that decides
what you may visit is a browser with an opinion, and one that reports the
decision is worse than the problem.
