# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Prepare a fresh Windows machine to build the core.
#
#     powershell -ExecutionPolicy Bypass -File tools\win-bootstrap.ps1
#
# NEVER RUN. This file was written on a Mac against Chromium's documented
# Windows requirements and this repository's own scripts; no Windows machine has
# executed a line of it. Said here rather than discovered later: the first run is
# the first run, and anything below may be wrong in the way that only contact
# with a real machine reveals.
#
# What this does NOT do: fetch or build. Those are core/build/fetch.sh and
# core/build/build.sh, they are bash, and build.sh already refuses to run
# anywhere but MINGW/MSYS/CYGWIN — that is, Git Bash, which this installs. The
# split is deliberate: machine preparation is a Windows problem, the build is
# not, and duplicating the build into PowerShell would create the second script
# that must stay in step with the first.

#Requires -RunAsAdministrator

$ErrorActionPreference = 'Stop'

function Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Warn($msg) { Write-Host "!!  $msg" -ForegroundColor Yellow }

# Where the tree will live. Short on purpose: Chromium generates paths that are
# already close to Windows' limits, and starting from C:\Users\<name>\... spends
# characters on nothing. See the long-path step below — that raises the ceiling
# but does not make every tool in the build honour it.
$Root = if ($env:FURY_ROOT) { $env:FURY_ROOT } else { 'C:\fury' }

Step "target directory: $Root"

# --- 1. Long paths ------------------------------------------------------------
# Chromium's own paths exceed MAX_PATH (260). Without this the failure arrives
# during checkout as a file that "cannot be found" whose name is plainly there.
Step 'enabling long paths (registry + git)'
Set-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' `
                 -Name 'LongPathsEnabled' -Value 1 -Type DWord
Write-Host '    registry: LongPathsEnabled = 1'

# --- 2. Defender exclusions ---------------------------------------------------
# Done BEFORE anything is downloaded, so the checkout is never scanned even once.
# Real-time scanning of several hundred thousand object files is the difference
# between a build and an evening: reports of 2x are common, and the files are
# ours, produced by our compiler, from source we fetched.
Step 'excluding the build tree from Defender'
foreach ($p in @($Root, "$Root\core\src", "$Root\core\depot_tools")) {
    Add-MpPreference -ExclusionPath $p -ErrorAction SilentlyContinue
    Write-Host "    path: $p"
}
foreach ($proc in @('ninja.exe', 'cl.exe', 'link.exe', 'lld-link.exe', 'python.exe', 'git.exe')) {
    Add-MpPreference -ExclusionProcess $proc -ErrorAction SilentlyContinue
}
Write-Host '    processes: ninja, cl, link, lld-link, python, git'

# --- 3. Power ------------------------------------------------------------------
# A build runs for hours unattended over RDP. A machine that sleeps mid-link
# wastes the whole run, and the disk timeout matters as much as the sleep one.
Step 'preventing sleep during long builds'
powercfg /change standby-timeout-ac 0
powercfg /change hibernate-timeout-ac 0
powercfg /change disk-timeout-ac 0
powercfg /change monitor-timeout-ac 15

# --- 4. Git --------------------------------------------------------------------
# Git Bash is not optional here: it is what runs fetch.sh and build.sh.
Step 'installing Git (brings Git Bash)'
if (Get-Command git -ErrorAction SilentlyContinue) {
    Write-Host "    already present: $(git --version)"
} else {
    winget install --id Git.Git -e --source winget --accept-package-agreements --accept-source-agreements
    $env:PATH = "$env:PATH;C:\Program Files\Git\cmd;C:\Program Files\Git\bin"
}

# core.longpaths is a separate switch from the registry one, and git ignores the
# registry without it.
git config --system core.longpaths true
Write-Host '    git: core.longpaths = true'

# --- 5. Visual Studio Build Tools ---------------------------------------------
# The component list is Chromium's documented set, not a guess. ATL is required;
# it is not part of the C++ workload by default and its absence surfaces as a
# missing header a long way into the build.
Step 'installing Visual Studio 2022 Build Tools'
$vsComponents = @(
    'Microsoft.VisualStudio.Workload.VCTools',
    'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    'Microsoft.VisualStudio.Component.VC.ATL',
    'Microsoft.VisualStudio.Component.VC.ATLMFC',
    'Microsoft.VisualStudio.Component.Windows11SDK.26100'
)
$vsArgs = ($vsComponents | ForEach-Object { "--add $_" }) -join ' '

if (Test-Path 'C:\Program Files (x86)\Microsoft Visual Studio\2022') {
    Write-Host '    a 2022 install already exists — not touching it'
    Warn 'verify it carries the components listed above, ATL included'
} else {
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --source winget `
        --accept-package-agreements --accept-source-agreements `
        --override "--quiet --wait --norestart $vsArgs --includeRecommended"
}

# --- 6. depot_tools' toolchain switch -----------------------------------------
# Without this depot_tools reaches for Google's internal toolchain package,
# which needs credentials nobody outside Google has. The failure text is about
# authentication and reads like a broken checkout.
Step 'DEPOT_TOOLS_WIN_TOOLCHAIN = 0 (machine-wide)'
[Environment]::SetEnvironmentVariable('DEPOT_TOOLS_WIN_TOOLCHAIN', '0', 'Machine')
$env:DEPOT_TOOLS_WIN_TOOLCHAIN = '0'

# --- 7. Disk ------------------------------------------------------------------
# fetch.sh refuses below 150 GB free. That number is its own cautious estimate
# rather than a measurement — docs/03 records ~30 GB of source and ~9 GB per
# build directory actually observed — but the check is what will stop you, so
# check it here where it costs a second.
Step 'checking free space'
# Written the long way on purpose: `??` is PowerShell 7 and `powershell.exe` on
# Windows 11 is 5.1, where it is a parse error — the script would die on load
# with a syntax message pointing at a line that looks fine.
if (-not (Test-Path $Root)) { New-Item -ItemType Directory -Path $Root -Force | Out-Null }
$drive = Get-Item $Root
$free = (Get-PSDrive -Name $drive.PSDrive.Name).Free / 1GB
Write-Host ("    free on {0}: {1:N0} GB" -f $drive.PSDrive.Name, $free)
if ($free -lt 150) { Warn 'fetch.sh refuses below 150 GB free — it will stop you' }

# --- what is left, and it is not scriptable -----------------------------------
Step 'DONE — but read this'

Warn 'Debugging Tools for Windows must be added by hand.'
Write-Host @'
    Chromium requires it and gn fails without it, and it is not a winget
    package: it is an optional feature of the Windows SDK.

      Settings -> Apps -> Installed apps
      -> Windows Software Development Kit  -> Modify -> Change
      -> tick "Debugging Tools for Windows" -> Change

'@

Write-Host @"
Next, from *Git Bash* (not PowerShell — both scripts are bash):

    git clone <your remote> $Root
    cd $Root
    ./core/build/fetch.sh

Then the FIRST build, and use this configuration rather than windows-x64:

    ./core/build/build.sh windows-x64-first

It is is_official_build = false, so no ThinLTO and no PGO. The reason is in the
header of core/args/windows-x64-first.gn: the Windows half of patch 0001 has
never run anywhere, and neither has an official build of this tree, and starting
both at once leaves a failure with two candidate causes.

Once that build exists and tools/verify-windows.ps1 passes, build windows-x64 —
that is the shippable one.
"@
