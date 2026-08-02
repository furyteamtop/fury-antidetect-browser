# Brand assets

Two source files, and everything else is generated from them.

| File | What it is | Used for |
|---|---|---|
| `logo.png` | the flame F beside the wordmark, dark type on transparency | README on a light background, the site |
| `logo-dark.png` | the same with the type inverted, flame untouched | README on a dark background — GitHub has both themes and a black wordmark vanishes in one of them |
| `icon.png` | the flame F alone in a rounded dark square, **1024×1024** | the macOS app icon, the Windows icon, the Tauri shell, the favicon |

`logo-dark.png` is generated from `logo.png`, not drawn separately: only the
near-neutral pixels are inverted, so the flame keeps its colours and the two
files cannot drift apart.

Drop the sources in here and run:

```bash
assets/generate.sh
```

It produces every derived size the build needs: `desktop/src-tauri/icons/*` for
the Tauri shell, and `core/branding/` for patch 0900, which renames the Chromium
bundle. Nothing derived is committed by hand — regenerate instead.

## Why 1024×1024 and why a separate icon

macOS wants a 1024px master to build `app.icns`, and Chromium additionally wants
an `Assets.car` compiled from `AppIcon.icon`. Both come from one square PNG.

The wordmark cannot be the app icon: at 32px the words are unreadable and the
result is a grey smear in the dock. The flame F alone survives the shrink, which
is why there are two files rather than one.

## What must NOT change

The icon and the name are what the USER and the OPERATING SYSTEM see. Nothing a
PAGE can read may move: the User-Agent, `navigator.appName`, `navigator.vendor`
and the Sec-CH-UA brand list belong to patch 0011 and keep saying Chrome. A
branding change that reaches a page is a detection vector rather than branding.
