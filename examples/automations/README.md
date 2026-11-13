# Automation catalogue

Ready-made automations for Fury: scripts and MCP prompts that drive profiles
through the [Local API](../README.md), the built-in MCP server
(`fury-agent mcp`), Playwright or Puppeteer. Proposed in issue 30.

Official entries are written and kept working by the project. Community entries
come in by pull request and are reviewed before they are listed; listing one is
not a promise that it is maintained, and each says who wrote it.

## The catalogue

| Automation | What it does | Drives through | Needs | By |
|---|---|---|---|---|
| [api.sh](../api.sh) | Lists profiles, starts one, opens a page, stops it. The whole API in curl | Local API | curl, `FURY_API_PORT` | official |
| [playwright_example.py](../playwright_example.py) | Starts a profile and takes it over with `connect_over_cdp` | Playwright | Python 3.9+, `playwright` | official |
| [puppeteer_example.mjs](../puppeteer_example.mjs) | The same from Node with `puppeteer.connect` | Puppeteer | Node 18+, `puppeteer-core` | official |
| [rotate.py](../rotate.py) | Runs one job across every profile of a project, one at a time, and never leaves a browser open on a failure | Local API + Playwright | Python 3.9+, `playwright` | official |

New entries go in this table, one row each, in the folder layout below.

## Adding one

1. Copy [`_template/`](_template/) to a folder named after what it does, in
   lower case with hyphens: `examples/automations/warm-up-news-sites/`.
2. Fill in `automation.json` and `README.md`, and put the code beside them.
3. Add a row to the table above and open a pull request.

`automation.json`:

```json
{
  "name": "Warm up on news sites",
  "summary": "Opens a profile, reads three news sites for a few minutes like a person, closes it.",
  "author": "your GitHub login",
  "drives": ["local-api", "playwright"],
  "requires": ["python>=3.9", "playwright"],
  "entry": "warm_up.py",
  "licence": "AGPL-3.0-or-later",
  "fury": ">=0.5.0"
}
```

`drives` is any of `local-api`, `mcp`, `playwright`, `puppeteer`, `selenium`.
`fury` is the oldest release it was run against.

## What review checks

- **The token is read from the file**, never pasted into the script or taken
  as an argument (see [Do not put the token in your script](../README.md#do-not-put-the-token-in-your-script)).
  [`fury.py`](../fury.py) does it in twenty lines; copy it.
- **Profiles are made in the application**, not by the script. The API has no
  create-profile endpoint on purpose, and an automation that works around it
  by writing the database will not be listed.
- **Nothing leaves a browser open on failure.** Use `with running(...)` from
  `fury.py`, as the template does: an open browser holds the profile's lock,
  and on a team server that blocks a colleague.
- **No secrets, accounts or personal data** in the folder, screenshots and
  logs included.
- **It does something a person could do.** Within
  [ACCEPTABLE_USE.md](../../ACCEPTABLE_USE.md): no mass registration, no
  credential stuffing, no fake reviews, nothing that defeats a site's
  verification. Those are closed without review.
- **It ran.** The pull request says on which Fury release and which OS.

If the catalogue grows, it gets a searchable page on the site built from the
`automation.json` files; until then this table is the index.
