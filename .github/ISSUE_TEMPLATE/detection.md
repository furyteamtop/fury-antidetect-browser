---
name: A site caught a profile
about: The most valuable report this project can get
labels: detection
---

<!--
Please do not paste account credentials, and please do not attach a RAW
capture — it contains the public IP your proxy exits from. Redact it first:

    cargo run -p fury-detect -- redact capture.json capture-redacted.json
-->

**What caught it**

The site, or — if you would rather not name it — what it checks.

**What happened**

A block, a captcha, a silent shadow-ban, an account asked to re-verify?

**Captures**

- [ ] From the Fury profile (redacted)
- [ ] From ordinary Chrome on the same machine (redacted), if you can

`fury-detect diff` between the two is usually the whole diagnosis.

**The profile**

- Persona: <!-- the id shown in the Machine tab -->
- Proxy type: <!-- socks5 / http, residential / datacentre / mobile -->
- Anything unusual: extensions, a timezone or language set by hand, automation
  driving it over CDP

**Gate**

```
cargo run -p fury-detect -- gate <your-capture>.json
```

Paste the output. If it already fails a check, that is very likely the answer.
