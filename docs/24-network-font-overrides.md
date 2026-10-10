# Network information and hidden fonts

Profiles derive these NetworkInformation defaults:

```json
{
  "netinfo": {
    "effectiveType": "4g",
    "downlink": 5.5,
    "rtt": 150,
    "saveData": false
  }
}
```

Advanced settings allow each field to be overridden. These are profile defaults,
not a measurement of a particular proxy. Use the exit's measured values when
available. Clearing an override restores the profile default. The shared profile
API stores them in `machine_overrides.netinfo`, with the same camelCase keys.
Partial overrides merge into the defaults. Existing profiles need no migration.

`effectiveType` accepts `slow-2g`, `2g`, `3g`, or `4g`. Downlink is in Mbps,
0–10 in 0.05 steps; RTT is in milliseconds, 0–3000 in 50 ms steps. Zero and
`saveData: false` are valid values. A core launched directly without a config,
or with a field missing, retains Chromium's estimate for that field.

Additional hidden font families can be entered one per line in Advanced settings
or supplied as `machine_overrides.fonts_hidden`. They are appended to the
persona's derived `fontsHidden` blocklist, case-insensitively deduplicated.
This lets a profile explicitly hide host-specific design or CAD fonts that the
persona capture did not probe. Unknown families are not automatically hidden:
the persona's `fonts` list is a capture's candidate intersection, not a complete
inventory of the OS. Generic and last-resort fonts remain available for rendering.

Patch 0051 moves the existing resolution filter ahead of the platform font cache,
covering lookups and availability checks that bypass `GetFontData`, including
`local()` requests whose names match the hidden list. Downloaded web fonts do
not use this installed-font lookup. Full/PostScript face aliases with different
names need their own blocklist entries.

## Browser verification

After rebuilding the core with patches 0051 and 0122, run:

```bash
python3 core/verify/verify-0122.py <core-binary>
python3 core/verify/verify-0050.py <core-binary> Georgia Arial
```

Choose two families actually installed on the verification host. The font script
tests `offsetWidth` against monospace, sans-serif and serif, canvas widths,
case-insensitive blocking, blocklist precedence and generic rendering. The
network script checks an unconfigured control, configured values in a frame and
worker, native getters, and changes to the host's network estimates. These
scripts use the existing POSIX CDP harness. A source-application check and Rust
tests do not replace running them against the rebuilt browser.
