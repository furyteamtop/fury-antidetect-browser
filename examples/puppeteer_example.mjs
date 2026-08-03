// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
// Drive a Fury profile with Puppeteer.
//
//     npm install puppeteer-core        # -core: no bundled Chromium
//     FURY_API_PORT=35000 fury-agent serve
//     node puppeteer_example.mjs <profile-id>
//
// puppeteer-core rather than puppeteer, and the difference matters here: the
// full package downloads its own Chromium on install, which is 150 MB of a
// browser you must not use. The profile's browser is the whole point.

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const PORT = process.env.FURY_API_PORT || 35000;

// The token authorises every logged-in profile on this machine. Read it, never
// write it into a script.
const home =
  process.env.FURY_HOME ||
  (process.platform === "darwin"
    ? join(homedir(), "Library/Application Support/Fury")
    : join(process.env.XDG_DATA_HOME || join(homedir(), ".local/share"), "fury"));

let token;
try {
  token = readFileSync(join(home, "api-token"), "utf8").trim();
} catch {
  console.error(`no ${join(home, "api-token")}`);
  console.error(`The agent writes it the first time it serves the API:`);
  console.error(`  FURY_API_PORT=${PORT} fury-agent serve`);
  process.exit(1);
}

async function api(method, path, body) {
  let res;
  try {
    res = await fetch(`http://127.0.0.1:${PORT}/v1${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(body ? { "Content-Type": "application/json" } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch (e) {
    console.error(`could not reach the agent on port ${PORT}: ${e.message}`);
    console.error(`Start it with: FURY_API_PORT=${PORT} fury-agent serve`);
    process.exit(1);
  }
  const json = await res.json();
  if (!res.ok) {
    // The agent says "this profile has no proxy"; the status line says 400.
    throw new Error(`${path}: ${json.message ?? res.statusText}`);
  }
  return json.data;
}

const profileId = process.argv[2];
if (!profileId) {
  console.error(`usage: node ${process.argv[1]} <profile-id>   (./api.sh lists them)`);
  process.exit(2);
}

const { default: puppeteer } = await import("puppeteer-core").catch(() => {
  console.error("npm install puppeteer-core");
  process.exit(1);
});

const session = await api("POST", "/profiles/start", { id: profileId, cdp: true });

// try/finally around everything after the start. Without it, a script that
// throws leaves the browser open holding the profile's lock — and on a team
// server that lock is what stops a colleague opening the same account
// somewhere else, so a leaked one blocks a person, not a process.
try {
  const ws = session.ws?.puppeteer ?? session.ws_endpoint;
  if (!ws) {
    // Not this script's failure: a role can be forbidden CDP, and then the
    // browser runs with nothing to attach to.
    console.log("the profile started, but CDP was not granted for it");
    process.exit(1);
  }

  const browser = await puppeteer.connect({
    browserWSEndpoint: ws,
    // Puppeteer otherwise resizes the viewport to 800x600 on connect, which
    // contradicts the persona's screen in one line of JS — window.innerWidth
    // against screen.width. null leaves the window as the browser has it.
    defaultViewport: null,
  });

  // The existing page, not a new browser context. A fresh context is incognito:
  // no cookies, no localStorage, none of the logins this profile exists for.
  const pages = await browser.pages();
  const page = pages[0] ?? (await browser.newPage());

  await page.goto("https://example.com", { waitUntil: "domcontentloaded" });
  console.log("title:", await page.title());

  const reported = await page.evaluate(() => ({
    userAgent: navigator.userAgent,
    platform: navigator.platform,
    languages: navigator.languages.join(","),
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    cores: navigator.hardwareConcurrency,
    memory: navigator.deviceMemory,
    screen: `${screen.width}x${screen.height}`,
    webdriver: navigator.webdriver,
  }));
  for (const [k, v] of Object.entries(reported)) {
    console.log(`  ${k.padEnd(12)} ${v}`);
  }

  // disconnect, not close: close() would shut the browser down behind the
  // agent's back. Stopping the profile is the agent's job, below.
  await browser.disconnect();
} finally {
  await api("POST", "/profiles/stop", { id: profileId });
}
