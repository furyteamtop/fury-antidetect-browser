// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
// Build the agent and place it where the bundler expects a sidecar.
//
// This exists because `tauri build` rewrites the app from scratch: a binary
// copied in by hand after a build survives exactly until the next one, which is
// how the shipped app twice lost the agent it depends on and then reported "the
// local agent is not running" on a machine where it had worked minutes earlier.
// The bundler has to own the copy.
//
// WHY THIS IS JAVASCRIPT AND NOT THE SHELL SCRIPT IT USED TO BE.
//
// It was sidecar.sh, and tauri.conf.json ran it as `./scripts/sidecar.sh`.
// Measured 16.08.2026, first attempt to build the shell on Windows:
//
//     Running beforeBuildCommand `./scripts/sidecar.sh && npm run build`
//     '.' is not recognized as an internal or external command
//     beforeBuildCommand failed with exit code 1
//
// Tauri runs beforeBuildCommand through the platform shell, which on Windows is
// cmd.exe, and cmd cannot execute a POSIX path. The old script's own header said
// "run this from Git Bash on Windows" -- true, and irrelevant, because the
// bundler is what runs it and the bundler does not use Git Bash. So the Windows
// installer could not be produced at all, by a build that had never been tried.
//
// The obvious repair was `bash scripts/sidecar.sh`, and it was rejected: Git for
// Windows puts bash.exe in a directory it does not add to PATH by default, so
// that fix works on the machine you tested it on. The other obvious repair was a
// PowerShell twin, which the old file already argued against in its own comment
// -- two scripts that must stay in step is the arrangement that put the agent
// address in two files and let them drift.
//
// Node is the one interpreter guaranteed to be present, because it is the thing
// running `tauri build` in the first place. One script, three platforms.

import { execFileSync } from 'node:child_process'
import { mkdirSync, copyFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..')
const run = (cmd, args, opts = {}) =>
  execFileSync(cmd, args, { cwd: repo, encoding: 'utf8', stdio: 'pipe', ...opts })

// The host triple as rustc reports it. Asking rustc rather than mapping
// process.platform ourselves: the answer has to be the one cargo will use, and
// a hand-written map is a second source of truth for the same fact.
const triple = run('rustc', ['-vV'])
  .split('\n')
  .find((l) => l.startsWith('host:'))
  ?.slice('host:'.length)
  .trim()

if (!triple) {
  console.error('!! could not read the host triple from `rustc -vV`')
  process.exit(1)
}

// Tauri looks for `fury-agent-<triple><exe suffix>` and the suffix is not
// optional on Windows: a sidecar named without `.exe` is a sidecar the bundler
// reports as missing, in a message that names the triple and not the extension.
const exe = triple.includes('windows') ? '.exe' : ''

console.log('==> cargo build --release -p fury-agent')
run('cargo', ['build', '--release', '-p', 'fury-agent'], { stdio: 'inherit' })

const binaries = join(repo, 'desktop', 'src-tauri', 'binaries')
mkdirSync(binaries, { recursive: true })
copyFileSync(
  join(repo, 'target', 'release', `fury-agent${exe}`),
  join(binaries, `fury-agent-${triple}${exe}`),
)
console.log(`==> sidecar: fury-agent-${triple}${exe}`)

// Sign the sidecar HERE, before `tauri build` runs, and not afterwards.
//
// Code signatures nest inside-out: the outer bundle's signature seals its
// contents, so signing a binary that is already inside a signed .app invalidates
// the .app. Signing the source file instead means the signature travels with the
// copy the bundler makes, and the bundler then seals an already-signed sidecar.
//
// This matters because the bundler signs the app, and Apple requires EVERY
// executable in the bundle to be signed -- including nested ones. An unsigned
// sidecar is the second most common notarisation rejection after a stray
// get-task-allow, and it fails at upload, several minutes in, with a message
// that names a path and not a cause.
//
// Both flags are mandatory for notarisation, not preferences: --options runtime
// enables the hardened runtime, --timestamp attaches the secure timestamp that
// lets the signature keep validating after the certificate expires.
//
// APPLE_SIGNING_IDENTITY is the same variable the Tauri bundler reads, and that
// is deliberate: the sidecar and the app it lives in must carry the SAME Team
// ID, or library validation refuses the bundle. One variable cannot drift from
// itself; two would.
if (triple.includes('darwin')) {
  const sidecar = join(binaries, `fury-agent-${triple}${exe}`)
  const identity = process.env.APPLE_SIGNING_IDENTITY
  if (identity) {
    console.log(`==> signing sidecar with ${identity}`)
    run('codesign', ['--force', '--sign', identity, '--options', 'runtime', '--timestamp', sidecar], {
      stdio: 'inherit',
    })
    run('codesign', ['--verify', '--strict', '--verbose=2', sidecar], { stdio: 'inherit' })
  } else {
    // Not an error: a developer building locally has no certificate and must
    // still get a working app. Said out loud because a release built without it
    // fails notarisation LATER, and a silent skip here is what makes that
    // failure look like it came from somewhere else.
    console.log('==> sidecar NOT signed: APPLE_SIGNING_IDENTITY is unset.')
    console.log('    Fine for local builds. A release build needs it -- see docs/17.')
  }
}
