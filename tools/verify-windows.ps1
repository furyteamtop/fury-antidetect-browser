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
#
#     powershell -ExecutionPolicy Bypass -File tools\verify-windows.ps1 `
#         -CoreArchive dist\fury-core-windows.tar.xz
#
# -CoreArchive is how the core section actually runs. Without it that section
# always skips, and it always skipped: this script gives the run a private
# FURY_HOME so it cannot disturb a real installation, and a private FURY_HOME by
# definition has no core in it. So the check with no macOS counterpart -- the one
# that proves the config crosses CreateProcess as an inherited HANDLE and that
# the persona is not in anybody's command line -- had never once executed, while
# the script reported success. Pass the archive and it is installed into that
# private home first.

param(
    [string]$CoreArchive = ''
)

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

if ($CoreArchive) {
    if (-not (Test-Path $CoreArchive)) {
        Write-Host "no such archive: $CoreArchive" -ForegroundColor Red
        exit 2
    }
    Write-Host "  installing the core into it from $CoreArchive"
    # ErrorActionPreference is relaxed for this one call, for the reason spelled
    # out further down beside `doctor`: with it at Stop, PowerShell turns ANY
    # stderr output from a native program into a terminating NativeCommandError.
    # install-core writes its progress there, so a successful install --
    # "installed 150.0.7871.187" -- killed the run that was verifying it.
    $said = & {
        $ErrorActionPreference = 'Continue'
        & $agentExe install-core $CoreArchive 2>&1 | Out-String
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host $said -ForegroundColor Red
        Write-Host "install-core failed" -ForegroundColor Red
        exit 2
    }
}

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
    # Get-Acl cannot read this, and the way it fails is worth writing down so
    # nobody rediscovers it: it goes through the FileSystem provider, which does
    # not open \\.\pipe\..., and reports
    #
    #     Cannot find path '...\\.\pipe\fury-xxxxxxxx' because it does not exist
    #
    # about a pipe that plainly does exist and that this script listed four lines
    # earlier. Measured 16.08.2026 on the first Windows run of this file; the
    # check had never executed, so the DACL it was written to prove had never
    # been looked at.
    #
    # PipeStream.GetAccessControl() is the API that works. It needs a connected
    # client, and that connection is not a detour: if this user could not open
    # the pipe there would be no descriptor to read, so the claim below about
    # our own access is partly made by getting this far. Measured too: reading
    # it does not exhaust the instance, and the client at the bottom of this
    # script still connects afterwards.
    $aclClient = New-Object System.IO.Pipes.NamedPipeClientStream(
        '.', $pipe.Name, [System.IO.Pipes.PipeDirection]::In)
    $aclClient.Connect(5000)
    $pipeSec = $aclClient.GetAccessControl()
    $sddl = $pipeSec.GetSecurityDescriptorSddlForm('Owner,Group,Access')
    Write-Host "  sddl: $sddl"

    # Asked for as SecurityIdentifier, so there is no Translate() to fail on an
    # account that has no name -- a deleted user, or a SID from another machine.
    $granted = $pipeSec.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]) |
               Where-Object { $_.AccessControlType -eq 'Allow' } |
               ForEach-Object { $_.IdentityReference.Value }
    $aclClient.Dispose()

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
    # $ErrorActionPreference is Stop for this whole file, and that turns a native
    # command's stderr into a thrown NativeCommandError the moment it is merged
    # with 2>&1. `fury-agent doctor` prints its banner ("fury-agent 0.0.1") on
    # stderr, so this line aborted the script on output that was not an error at
    # all -- measured 16.08.2026, and it stopped the run before the section that
    # matters most, the one with no macOS equivalent.
    #
    # The preference is restored immediately rather than relaxed for the rest of
    # the file: everything after this still deserves to stop on a real failure.
    $core = & {
        $ErrorActionPreference = 'Continue'
        & $agentExe doctor 2>&1 | Select-String -Pattern 'core' | Out-String
    }
    $haveCore = ($core -notmatch 'no core|not installed' -and $core.Trim().Length -gt 0)
    if (-not $haveCore) {
        Skip "no core installed -- run 'fury-agent install-core <path>' and run this again"
        Skip "  (this is the section that checks handle inheritance, which is the"
        Skip "   one thing in the port with no macOS equivalent to fall back on)"
    } else {
        # The claim: the config travels as an inherited HANDLE and only the slot
        # number is in argv. Task Manager, WMI and any process of this user can
        # read a command line, so a persona there is a persona anyone can read.
        #
        # Written 16.08.2026, the day a Windows core first existed to write it
        # against. Everything above this point has a macOS counterpart that has
        # been passing for weeks; this section does not, because a file
        # descriptor and an inherited HANDLE are different mechanisms and only
        # one of them had ever been executed.
        #
        # `fury-agent launch` rather than a stored profile: the bare CLI takes a
        # persona file and a proxy and needs no database, no vault and no shell.
        # Fewer moving parts between the claim and the thing it is about.
        $persona = Join-Path $repo 'shared\personas\windows-11-rtx4060-1920x1080.json'
        Claim (Test-Path $persona) "the Windows persona file is where the test expects it"

        # Port 9 is discard: nothing listens, so the exit lookup fails at once
        # instead of spending fifteen seconds timing out. A launch does not need
        # the proxy to work -- the relay is local and the browser is pointed at
        # it -- and this test asserts nothing about traffic.
        $dbgPort = 9333
        $launchLog = Join-Path $home_ 'launch.log'
        $launcher = Start-Process -FilePath $agentExe `
            -ArgumentList @('launch', $persona, '--proxy', 'socks5://127.0.0.1:9',
                            '--debug-port', "$dbgPort", '--seed', '7') `
            -PassThru -WindowStyle Hidden `
            -RedirectStandardOutput (Join-Path $home_ 'launch.out') `
            -RedirectStandardError  $launchLog

        # Wait for the core, not for the launcher: the launcher returns quickly
        # and then blocks on the child, so its own liveness proves nothing.
        # The browser process and its children carry DIFFERENT switches, and
        # conflating them is the mistake this test made on its own first run.
        # carrier.rs:273 warns about exactly this collision:
        #
        #   --fury-fp-file-handle   browser, Windows   an inherited HANDLE
        #   --fury-fp-handle        children, both     a shared memory region
        #
        # So the browser is the one WITHOUT --type=, and asserting the child
        # switch on it fails while everything is working correctly.
        # Scoped to THIS launch by its data directory, not to "any chrome.exe on
        # the machine". That distinction is not pedantry: an earlier run of this
        # section left processes behind, the next run adopted them, and the
        # result was three claims failing while three others passed about the
        # same browser -- a combination that cannot happen and therefore says the
        # test is measuring the wrong process. A build machine has other
        # Chromiums on it; a browser this test did not start is not evidence
        # about the core this test was given.
        $mineOnly = { $_.CommandLine -like "*$($home_.Replace('\','\\'))*" -or $_.CommandLine -like "*$home_*" }
        $coreProc = $null
        for ($i = 0; $i -lt 150; $i++) {
            Start-Sleep -Milliseconds 200
            $coreProc = Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" -ErrorAction SilentlyContinue |
                        Where-Object { $_.CommandLine -notmatch '--type=' } |
                        Where-Object $mineOnly |
                        Select-Object -First 1
            if ($coreProc) { break }
        }

        if (-not $coreProc) {
            Claim $false "the core started -- see $launchLog"
            Get-Content $launchLog -ErrorAction SilentlyContinue | Select-Object -Last 8 |
                ForEach-Object { Write-Host "       $_" }
        } else {
            # Give the children time to exist; a renderer that has not spawned
            # yet cannot be checked, and an empty set would pass every claim.
            Start-Sleep -Seconds 3
            $all = @(Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" -ErrorAction SilentlyContinue)
            $kids = @($all | Where-Object { $_.CommandLine -match '--type=' -and $_.CommandLine -notmatch 'crashpad' })

            $cmdline = $coreProc.CommandLine
            Claim ($cmdline -match '--fury-fp-file-handle=\d+') `
                "the browser gets --fury-fp-file-handle=<n>, an inherited HANDLE and nothing else"
            Claim ($kids.Count -gt 0) "the browser spawned children to check ($($kids.Count))"
            Claim (@($kids | Where-Object { $_.CommandLine -notmatch '--fury-fp-handle=\d+' }).Count -eq 0) `
                "every child gets --fury-fp-handle=<n>, a shared memory region and nothing else"

            # The whole point of the mechanism, and it has to hold for the
            # CHILDREN too: Chromium copies --user-data-dir into every one of
            # them, so a persona in that path is a persona in eight command
            # lines. That is how the directory naming below was found.
            foreach ($secret in @('RTX 4060', 'NVIDIA', 'i5-13400F', 'Win64; x64', 'rtx4060', '1920')) {
                $leaky = @($all | Where-Object { $_.CommandLine -like "*$secret*" })
                Claim ($leaky.Count -eq 0) `
                    "no process has '$secret' in argv$(if ($leaky.Count) { " (leaked by $($leaky.Count) of $($all.Count))" })"
            }

            # And the other half: absent from argv is worthless if it never
            # arrived. /json/version is CDP's HTTP side and reports the browser's
            # own User-Agent, which is what patch 0011 overrides -- no WebSocket
            # needed, and PowerShell 5.1 has no comfortable one.
            $ver = $null
            for ($i = 0; $i -lt 50; $i++) {
                Start-Sleep -Milliseconds 200
                try { $ver = Invoke-RestMethod "http://127.0.0.1:$dbgPort/json/version" -TimeoutSec 2; break }
                catch { }
            }
            Claim ($null -ne $ver) "CDP answers on 127.0.0.1:$dbgPort"
            if ($ver) {
                $ua = $ver.'User-Agent'
                Write-Host "  UA: $ua"
                Claim ($ua -like '*Windows NT 10.0; Win64; x64*') `
                    "the browser reports the persona's platform, not the host's"
                Claim ($ua -notlike '*Headless*') "and does not announce itself as headless"
            }

            # Close it the way a person would, so this also exercises the
            # shutdown path the port had to write from scratch.
            Stop-Process -Id $coreProc.ProcessId -Force -ErrorAction SilentlyContinue
        }
        if ($launcher -and -not $launcher.HasExited) {
            Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue
        }
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
