---
name: fury
description: Drive Fury, the antidetect browser, on this machine. Open and close browser profiles, warm them up, go to sites, read pages, click and type inside a profile. Use when the person talks about Fury profiles, accounts, warming, or doing something on a site "in profile X".
---

# Fury

Fury keeps browser profiles. Each profile is a separate browser with its own fingerprint, cookies and proxy, usually holding one account on some platform. The person runs Fury on this machine. You drive it through the `fury_*` tools.

## How to work

1. **Find the profile first.** Call `fury_list_profiles`; filter with `tag`, `project` or `query` when the person names a group ("the warm ones", "the DE profiles"). Refer to profiles by the `name` the person sees; pass the `id` or the exact name to other tools.
2. **Open it.** `fury_start_profile` opens a real browser window on the person's screen, through the profile's proxy. Only profiles you opened this way can be driven: one opened from the Fury window has no automation port. If a page tool says so, close it with `fury_stop_profile` and start it again.
3. **Look before you act.** `fury_read_page` returns the text and a numbered list of links, buttons and fields. Use those numbers with `fury_click` and `fury_type`. After a click or a submit the page changes: read it again before using numbers from the old read. Use `fury_screenshot` when the layout matters or the text is not enough.
4. **Close what you opened** with `fury_stop_profile` when the task is done, unless the person wants to keep working in it. The profile keeps its cookies.

For several profiles, work through them one at a time: open, do the task, close, next. Opening many at once makes the machine slow.

## Creating and changing profiles

`fury_create_profiles` makes one profile or a batch ("Shop {n}", up to 50). Each gets its own fingerprint seed and, unless the person names a persona, its own machine picked by how common it is; pass `os` to keep them on the person's OS. `fury_list_personas` shows the machines. Give every account its own proxy: save proxies with `fury_add_proxies` (any common format, one per line) or pass `proxy_line`, then `fury_update_profile` to assign them. Profiles sharing one exit are linked by it, so say so if the person asks for a batch on one proxy.

`fury_update_profile` changes the name, tags, stage, proxy or start pages; it never touches the fingerprint. Changing the proxy of a profile that already holds an account moves that account to another address: ask first.

`fury_move_to_trash` moves a profile to Fury's Trash, from which it can be restored. Ask before using it. Nothing here erases a profile for good.

New profiles are created on this machine. To share one with a team, the person sends it to the server from the Fury window.

## Warming

`fury_warm_up` visits ordinary sites in each profile with human-like pauses so it collects normal cookies before it is used for an account. It runs in the background and closes each profile at the end by default. Check progress with `fury_warm_status`. Use Fury's default site list unless the person gives one.

## Rules

- **The proxy is the identity.** Never suggest opening an account profile without its proxy, and never move a profile to another country's proxy on your own: a sudden change of address is what platforms flag.
- **Act like the person would.** No rapid-fire clicking, no opening dozens of pages a minute. If a site shows a captcha, a login challenge or a ban notice, stop and tell the person; do not try to get around it.
- **Ask before anything irreversible** on a site: sending a message, posting, buying, deleting, changing account settings. Reading and navigating need no confirmation.
- **Logins are the person's.** If a site asks for a password or a code, ask the person to type it in the open window themselves.
- Team profiles (on a Fury team server) are not available here yet; only profiles on this machine are.
- The fingerprint (persona, seed) of an existing profile is not changed from here: a profile whose machine changes overnight is the anomaly platforms look for.
- Errors come back as text: read them. "this profile has no proxy" means the person has to add one in Fury, or tick "Let this profile open without a proxy" on the profile's Proxy tab if it holds no account.
