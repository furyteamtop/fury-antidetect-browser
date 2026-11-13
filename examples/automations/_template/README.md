# What it does

One paragraph: what the automation is for and what a run looks like from the
outside (which profiles it opens, which sites it visits, how long it takes).

## Needs

- Fury 0.5.0 or later, with the Local API on: `FURY_API_PORT=35000`
- Python 3.9+ (or Node, or nothing beyond curl)
- Anything else: libraries, an MCP client

## Run

```bash
python3 main.py <profile-id>
```

## What it changes

What it leaves behind in a profile (cookies, history, extension state), and
what it never touches.

## Tested on

Fury release and OS it was run on, e.g. "0.5.3, macOS 26 and Windows 11".
