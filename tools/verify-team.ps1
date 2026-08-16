# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
#
# Does a stranger see my profile, my proxy and my credentials?
#
#     powershell -ExecutionPolicy Bypass -File tools\verify-team.ps1
#
# The question an operator asks before trusting a shared server, answered
# against a running fury-server and a real PostgreSQL rather than by reading
# handlers. Written 16.08.2026, the first time this project's database paths had
# ever been executed: until that day there was no psql and no docker on any
# machine it was developed on, so every SQL statement here was compiled and
# never run.
#
# TWO THINGS THIS DOES NOT COVER, said here because a test that is trusted for
# more than it checks is worse than no test:
#
#   1. Only strangers. Both users are OWNERS OF DIFFERENT ORGANISATIONS. The
#      sharper question -- a colleague invited INTO your organisation with a
#      Launcher role, who should see the profile but never the proxy password --
#      needs the invitation flow and is not here yet.
#
#   2. This proves the HANDLERS refuse. It does not prove the database would.
#      Row-level security is declared in the schema and is not in force: the
#      application connects as the owner of the tables, which is exempt from its
#      own policies until they are FORCE'd, and bind_rls_user is dead code. So
#      what passes below is one layer, not two. See task 5.4 in docs/16.
#
# The crypto fields are base64 and hex filler on purpose. The server decodes them
# and stores them without parsing -- it never holds a key it could use -- so this
# is a test about ACCESS, and inventing real keys would only add a way for it to
# fail for reasons that are not the question.
#
# EVERY REFUSAL IS PAIRED WITH THE SAME CALL SUCCEEDING FOR THE OWNER. That is
# the part worth keeping. The first version of this file "passed" while three of
# its claims were meeting 405 Method Not Allowed and 422 Unprocessable Entity --
# the requests were malformed, never reached an access check, and were counted as
# proof that access was denied. The owner control is what caught it: A got 422
# too.
$ErrorActionPreference = 'Continue'
$base = 'http://127.0.0.1:8080'
$b64  = [Convert]::ToBase64String([byte[]](1..32))

$pass = 0; $fail = 0
function Claim($ok, $what) {
    if ($ok) { $script:pass++; Write-Host "  OK   $what" }
    else     { $script:fail++; Write-Host "  FAIL $what" -ForegroundColor Red }
}
function Api($method, $path, $token, $body) {
    $h = @{}
    if ($token) { $h['Authorization'] = "Bearer $token" }
    $args = @{ Uri = "$base$path"; Method = $method; Headers = $h; TimeoutSec = 10; UseBasicParsing = $true }
    if ($body) { $args['ContentType'] = 'application/json'; $args['Body'] = ($body | ConvertTo-Json -Depth 8) }
    try { return @{ ok = $true; code = 200; data = (Invoke-RestMethod @args) } }
    catch {
        $code = try { $_.Exception.Response.StatusCode.value__ } catch { 0 }
        # ErrorDetails.Message is where PowerShell puts the response body for a
        # non-2xx; reading the stream afterwards gets nothing, it is consumed.
        $msg = if ($_.ErrorDetails -and $_.ErrorDetails.Message) { $_.ErrorDetails.Message } else { $_.Exception.Message }
        return @{ ok = $false; code = $code; err = $msg }
    }
}
function Signup($email, $org) {
    Api POST '/v1/auth/signup' $null @{
        email = $email; password = 'correct-horse-battery-staple'; org_name = $org
        public_key = $b64; wrapped_private_key = $b64; kdf_salt = $b64; wrapped_ork = $b64
    }
}

Write-Host "== two users, two organisations"
$stamp = [guid]::NewGuid().ToString('N').Substring(0,6)
$a = Signup "anna-$stamp@example.com" "Anna's agency"
$b = Signup "boris-$stamp@example.com" "Boris's agency"
Claim ($a.ok) "user A signed up$(if(-not $a.ok){' -> '+$a.code+' '+$a.err})"
Claim ($b.ok) "user B signed up$(if(-not $b.ok){' -> '+$b.code+' '+$b.err})"
if (-not ($a.ok -and $b.ok)) { Write-Host "`nCANNOT CONTINUE"; exit 1 }
$ta = $a.data.token; $tb = $b.data.token

Write-Host "`n== A creates a project, a proxy and a profile"
$proj = Api POST '/v1/projects' $ta @{ name = 'Anna client work' }
Claim ($proj.ok) "project created$(if(-not $proj.ok){' -> '+$proj.code+' '+$proj.err})"
$projId = $proj.data.id

$hex = ($b64.ToCharArray() | ForEach-Object { [int]$_ } | ForEach-Object { '{0:x2}' -f ($_ -band 0xff) }) -join ''
$px = Api POST '/v1/proxies' $ta @{
    name = 'anna-residential'; kind = 'socks5'; host = 'secret-exit.example.net'
    port = 1080; credentials_enc = $hex; wrapped_dek = $hex
}
Claim ($px.ok) "proxy created$(if(-not $px.ok){' -> '+$px.code+' '+$px.err})"

$prof = Api POST "/v1/projects/$projId/profiles" $ta @{
    name = 'Annas facebook'; persona_id = 'win11-rtx4060-1920x1080'
    fp_seed = '0123456789abcdef'; timezone = 'Europe/Berlin'
    languages = @('de-DE','de'); proxy_id = $px.data.id
}
Claim ($prof.ok) "profile created$(if(-not $prof.ok){' -> '+$prof.code+' '+$prof.err})"
$profId = $prof.data.id

Write-Host "`n== A sees their own"
$mine = Api GET "/v1/projects/$projId/profiles" $ta $null
Claim ($mine.ok -and @($mine.data).Count -ge 1) "A lists their profile"

Write-Host "`n== B, a stranger in another organisation"
$steal = Api GET "/v1/projects/$projId/profiles" $tb $null
$stolen = if ($steal.ok) { @($steal.data).Count } else { 0 }
Claim ((-not $steal.ok) -or $stolen -eq 0) "B cannot list profiles in A's project (code $($steal.code), rows $stolen)"

$edit = Api PATCH "/v1/profiles/$profId" $tb @{ name = 'stolen by boris'; timezone = 'Europe/Berlin'; languages = @('de-DE','de'); proxy_id = $px.data.id }
Claim (-not $edit.ok) "B cannot rename A's profile (got $($edit.code))"

$del = Api DELETE "/v1/profiles/$profId" $tb $null
Claim (-not $del.ok) "B cannot delete A's profile (got $($del.code))"

$creds = Api GET "/v1/profiles/$profId/credentials" $tb $null
Claim (-not $creds.ok) "B cannot read A's stored credentials (got $($creds.code))"

$pxlist = Api GET '/v1/proxies' $tb $null
$leaked = @($pxlist.data | Where-Object { $_.name -eq 'anna-residential' -or "$($_.display)" -like '*secret-exit*' })
Claim ($leaked.Count -eq 0) "B's proxy list does not contain A's proxy"

$bundle = Api GET "/v1/profiles/$profId/bundle" $tb $null
Claim (-not $bundle.ok) "B cannot download A's profile bundle (got $($bundle.code))"

$lock = Api POST "/v1/profiles/$profId/lock" $tb @{ machine_id = 'boris-machine-01'; machine_name = 'boris-pc' }
Claim (-not $lock.ok) "B cannot take the lock on A's profile (got $($lock.code))"

$projs = Api GET '/v1/projects' $tb $null
$seen = @($projs.data | Where-Object { $_.name -eq 'Anna client work' })
Claim ($seen.Count -eq 0) "B's project list does not contain A's project"

Write-Host "`n== and the same calls succeed for A, so the refusals above are about access"
$aEdit = Api PATCH "/v1/profiles/$profId" $ta @{ name = 'Annas facebook renamed'; timezone = 'Europe/Berlin'; languages = @('de-DE','de'); proxy_id = $px.data.id }
Claim ($aEdit.ok) "A can rename their own profile (got $($aEdit.code))"
$aLock = Api POST "/v1/profiles/$profId/lock" $ta @{ machine_id = 'anna-machine-01'; machine_name = 'anna-mac' }
Claim ($aLock.ok) "A can take the lock on their own profile (got $($aLock.code)$(if(-not $aLock.ok){' '+$aLock.err}))"
$aCred = Api GET "/v1/profiles/$profId/credentials" $ta $null
Claim ($aCred.ok) "A can list their own credentials (got $($aCred.code))"

Write-Host "`n$(if($fail -eq 0){'PASS'}else{'FAIL'}) -- $pass ok, $fail false"
