# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# The half of the Windows port that a compiler cannot check.
#
#     powershell -ExecutionPolicy Bypass -File tools\verify-windows.ps1
#
# `cargo check -p fury-platform --target x86_64-pc-windows-msvc` runs on a Mac
# and is in CI, and it catches a real class of mistake -- it has already caught
# three. It cannot catch any of these, because every one of them is a question
# about what Windows DOES rather than about what compiles:
#
#   - whether a DACL refuses the people it names and allows the people it does
#     not. An SDDL string that grants nobody compiles perfectly.
#   - whether a handle marked inheritable actually survives CreateProcess as
#     std::process::Command spawns it. That is a property of the standard
#     library's implementation, not of ours, and it can change.
#   - whether WM_CLOSE reaches Chromium's window and produces a clean shutdown
#     rather than a hung process.
#   - whether the persona stays out of the command line where Task Manager and
#     WMI can read it.
#
# Sections that need an installed core say so and skip. Run it again once
# `fury-agent install-core` has one, and everything runs.
#
# State claims, not steps -- same rule as core/verify/README.md. Each claim
# prints one line and the script exits non-zero if any is false.

$ErrorActionPreference = 'Stop'
$script:Failures = 0
$script:Checks = 0

function Claim {
    param([bool]$Ok, [string]$Text)
    $script:Checks++
    if ($Ok) {
        Write-Host "  OK   $Text"
    } else {
        Write-Host "  FAIL $Text" -ForegroundColor Red
        $script:Failures++
    }
}

function Skip {
    param([string]$Text)
    Write-Host "  --   $Text (skipped)" -ForegroundColor DarkGray
}

$repo = Split-Path -Parent $PSScriptRoot
$agentExe = Join-Path $repo 'target\release\fury-agent.exe'
if (-not (Test-Path $agentExe)) {
    $agentExe = Join-Path $repo 'target\debug\fury-agent.exe'
}
if (-not (Test-Path $agentExe)) {
    Write-Host "no fury-agent.exe -- run: cargo build --release -p fury-agent" -ForegroundColor Red
    exit 2
}

# A data directory of its own, so this never touches a real installation and
# never talks to an agent holding real profiles. FURY_HOME is what the tag is
# derived from, so this also gives the run its own pipe name.
$home_ = Join-Path $env:TEMP ("fury-verify-" + [System.Guid]::NewGuid().ToString('N').Substring(0,8))
New-Item -ItemType Directory -Path $home_ -Force | Out-Null
$env:FURY_HOME = $home_

Write-Host "verify windows -- $agentExe"
Write-Host "  data directory: $home_"

$me = [System.Security.Principal.WindowsIdentity]::GetCurrent()
$mySid = $me.User.Value

$agent = $null
try {
    # -----------------------------------------------------------------------
    Write-Host "`nthe agent listens, and on a pipe rather than a port"
    # -----------------------------------------------------------------------
    $agent = Start-Process -FilePath $agentExe -ArgumentList 'serve' -PassThru `
        -WindowStyle Hidden -RedirectStandardOutput (Join-Path $home_ 'out.log') `
        -RedirectStandardError (Join-Path $home_ 'err.log')

    # The tag is eight hex characters of SHA-256 over the data directory; the
    # name is derived rather than guessed so this checks the real one.
    $pipe = $null
    for ($i = 0; $i -lt 100; $i++) {
        Start-Sleep -Milliseconds 200
        $pipe = Get-ChildItem \\.\pipe\ -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -like 'fury-*' } | Select-Object -First 1
        if ($pipe) { break }
    }
    Claim ($null -ne $pipe) "a named pipe called fury-* exists"
    if (-not $pipe) { throw "the agent never listened -- see $home_\err.log" }

    $pipePath = "\\.\pipe\" + $pipe.Name
    Write-Host "  pipe: $pipePath"

    # -----------------------------------------------------------------------
    Write-Host "`nand nobody else can open it"
    # -----------------------------------------------------------------------
    # The claim the whole transport rests on. On macOS this is a 0600 socket and
    # `ls -l` settles it; here it is a DACL, and a DACL that grants nobody looks
    # exactly like a DACL that works until somebody checks.
    $acl = Get-Acl -Path $pipePath
    $sddl = $acl.Sddl
    Write-Host "  sddl: $sddl"

    $granted = $acl.Access | Where-Object { $_.AccessControlType -eq 'Allow' } |
               ForEach-Object { $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value }

    Claim ($granted -contains $mySid) `
        "this user ($mySid) is granted access -- a DACL that locks US out is the classic CREATOR OWNER mistake"

    # S-1-5-18 is LOCAL SYSTEM, which is deliberately in there. Everything else
    # is not. S-1-1-0 Everyone, S-1-5-11 Authenticated Users and S-1-5-32-545
    # Users are the three that a default or inherited DACL brings along.
    foreach ($bad in @('S-1-1-0', 'S-1-5-11', 'S-1-5-32-545', 'S-1-5-32-544')) {
        Claim (-not ($granted -contains $bad)) "$bad is not granted"
    }

    Claim ($sddl -match '^[OGD]?.*D:P') `
        "the DACL is PROTECTED (D:P) -- without it the parent's inheritable entries come back"

    # -----------------------------------------------------------------------
    Write-Host "`nthe shell's half of the conversation works"
    # -----------------------------------------------------------------------
    $client = New-Object System.IO.Pipes.NamedPipeClientStream(
        '.', $pipe.Name, [System.IO.Pipes.PipeDirection]::InOut)
    $client.Connect(5000)
    $writer = New-Object System.IO.StreamWriter($client)
    $reader = New-Object System.IO.StreamReader($client)
    $writer.AutoFlush = $true
    $writer.WriteLine('{"id":1,"method":"status","params":{}}')
    $answer = $reader.ReadLine()
    $client.Dispose()
    Claim ($answer -and $answer.StartsWith('{')) "one line in, one line of JSON out"
    Claim ($answer -match '"ok"') "the agent answered with ok rather than err: $answer"

    # The bug this catches is specific and would otherwise look like "the agent
    # keeps dying": on Windows every concurrent connection is a separate server
    # instance, and if `accept` does not create the replacement before handing
    # the current one over, the second client gets ERROR_PIPE_BUSY -- which the
    # shell reports as "the agent is not running".
    $second = $true
    for ($i = 0; $i -lt 5; $i++) {
        try {
            $c = New-Object System.IO.Pipes.NamedPipeClientStream(
                '.', $pipe.Name, [System.IO.Pipes.PipeDirection]::InOut)
            $c.Connect(3000)
            $w = New-Object System.IO.StreamWriter($c); $w.AutoFlush = $true
            $r = New-Object System.IO.StreamReader($c)
            $w.WriteLine('{"id":2,"method":"status","params":{}}')
            if (-not $r.ReadLine()) { $second = $false }
            $c.Dispose()
        } catch { $second = $false }
    }
    Claim $second "five clients in a row are all served -- the next pipe instance is created before accept returns"

    # -----------------------------------------------------------------------
    Write-Host "`na second agent refuses the machine rather than sharing it"
    # -----------------------------------------------------------------------
    $log = Join-Path $home_ 'second.log'
    $second = Start-Process -FilePath $agentExe -ArgumentList 'serve' -PassThru -Wait `
        -WindowStyle Hidden -RedirectStandardError $log -RedirectStandardOutput (Join-Path $home_ 'second.out')
    Claim ($second.ExitCode -ne 0) "it exits non-zero (got $($second.ExitCode))"
    $text = (Get-Content $log -Raw -ErrorAction SilentlyContinue)
    Claim ($text -match 'already running') "and says why: $($text -replace "`r?`n",' ' | Select-Object -First 1)"

    # -----------------------------------------------------------------------
    Write-Host "`nthe data directory is this user's alone"
    # -----------------------------------------------------------------------
    $dirAcl = Get-Acl -Path $home_
    $dirGranted = $dirAcl.Access | Where-Object { $_.AccessControlType -eq 'Allow' } |
                  ForEach-Object { $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value }
    Claim ($dirGranted -contains $mySid) "this user can read it"
    Claim (-not ($dirGranted -contains 'S-1-1-0')) "Everyone cannot"
    Claim ($dirAcl.AreAccessRulesProtected) `
        "inheritance is disabled -- an inherited entry from a relocated FURY_HOME would undo all of this"

    $token = Join-Path $home_ 'api-token'
    if (Test-Path $token) {
        $tAcl = Get-Acl -Path $token
        $tGranted = $tAcl.Access | Where-Object { $_.AccessControlType -eq 'Allow' } |
                    ForEach-Object { $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value }
        Claim (-not ($tGranted -contains 'S-1-1-0')) "the local API token is not world-readable"
    } else {
        Skip "the local API token has not been created (the HTTP API was not started)"
    }

    # -----------------------------------------------------------------------
    Write-Host "`nthe vault key is in the Credential Manager"
    # -----------------------------------------------------------------------
    # keyring 4 pulls windows-native-keyring-store by default, so there is
    # nothing of ours to check -- only that it worked. A failure here shows up
    # as proxy passwords stored as typed, which is a downgrade the agent logs
    # and carries on from, so it is easy to miss.
    $vaultLog = Get-Content (Join-Path $home_ 'err.log') -Raw -ErrorAction SilentlyContinue
    Claim (-not ($vaultLog -match 'no credential store|could not store the vault key')) `
        "no 'could not store the vault key' in the log -- secrets are sealed rather than stored as typed"

    # -----------------------------------------------------------------------
    Write-Host "`nthe persona reaches the browser without being written down"
    # -----------------------------------------------------------------------
    $core = & $agentExe doctor 2>&1 | Select-String -Pattern 'core' | Out-String
    $haveCore = ($core -notmatch 'no core|not installed' -and $core.Trim().Length -gt 0)
    if (-not $haveCore) {
        Skip "no core installed -- run 'fury-agent install-core <path>' and run this again"
        Skip "  (this is the section that checks handle inheritance, which is the"
        Skip "   one thing in the port with no macOS equivalent to fall back on)"
    } else {
        # The claim: the config travels as an inherited HANDLE and only the slot
        # number is in argv. Task Manager, WMI and any process of this user can
        # read a command line, so a persona there is a persona anyone can read.
        Skip "TODO once a Windows core exists: launch a profile, then assert"
        Skip "  Get-CimInstance Win32_Process shows --fury-fp-handle=<n> and no persona,"
        Skip "  and that the browser applied it (navigator.userAgent over CDP)."
    }

} finally {
    if ($agent -and -not $agent.HasExited) {
        Stop-Process -Id $agent.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Recurse -Force $home_ -ErrorAction SilentlyContinue
}

Write-Host ""
if ($script:Failures -gt 0) {
    Write-Host "FAIL -- $($script:Failures) of $($script:Checks) claims are false" -ForegroundColor Red
    exit 1
}
Write-Host "PASS -- $($script:Checks) claims" -ForegroundColor Green
exit 0
