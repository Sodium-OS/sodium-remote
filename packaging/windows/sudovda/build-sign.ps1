# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Builds SudoVDA (the matched display's virtual display driver) from SudoMaker's source at the pinned commit, makes its
# catalog and signs the DLL and the catalog with Sodium Remote's own certificate. Runs in a Windows build VM with Visual
# Studio 2022 (Desktop development with C++), the matching Windows Driver Kit and git; never on a user's computer.
#
#   $env:SODIUM_REMOTE_SIGNING_PASS = '...'     # the PFX's password (never on the command line)
#   .\build-sign.ps1 -Pfx C:\sign\signing.pfx [-Work C:\sudovda-build] [-Out C:\sudovda-build\out] [-NoTimestamp]
#
# The PFX comes from make-cert.sh on the Linux machine that keeps the key; copy it in for this run only and delete it from
# the VM afterwards. Output in -Out: SudoVDA.inf, SudoVDA.dll, sudovda.cat (signed) and SHA256SUMS. Copy that folder back
# to remote/build/sudovda/ on Linux, then run pin.sh there (it checks the signer and prints the pins for vdd_pin.rs).
# No test signing is used at any step (the WDK's own TestSign is switched off).
param(
    [Parameter(Mandatory = $true)][string]$Pfx,
    [string]$Work = "$env:SystemDrive\sudovda-build",
    [string]$Out = "",
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [switch]$NoTimestamp
)
$ErrorActionPreference = 'Stop'
# Keep these two in step with host-desktop/src/vdd_pin.rs (COMMIT) and packaging/windows/sudovda/UPSTREAM.
$Repo = 'https://github.com/SudoMaker/SudoVDA.git'
$Commit = 'a4b09fa2aa731a964d0cb5d139cb1e6240e4da12'
if (-not $Out) { $Out = Join-Path $Work 'out' }
$pass = $env:SODIUM_REMOTE_SIGNING_PASS
if (-not $pass) { throw 'Set $env:SODIUM_REMOTE_SIGNING_PASS to the PFX password first.' }
if (-not (Test-Path $Pfx)) { throw "No PFX at $Pfx" }

# 1. The tools: MSBuild from Visual Studio, Inf2Cat and signtool from the newest kit.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw 'Visual Studio 2022 is not installed (no vswhere.exe).' }
$msbuild = & $vswhere -latest -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1
if (-not $msbuild) { throw 'MSBuild not found.' }
$kits = Get-ItemPropertyValue 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots' 'KitsRoot10'
$kitVer = (Get-ChildItem "$kits\bin" -Directory | Where-Object Name -match '^10\.' | Sort-Object { [version]$_.Name } | Select-Object -Last 1).Name
$inf2cat = "$kits\bin\$kitVer\x86\Inf2Cat.exe"
$signtool = "$kits\bin\$kitVer\x64\signtool.exe"
foreach ($t in $inf2cat, $signtool) { if (-not (Test-Path $t)) { throw "Not found: $t (install the WDK)" } }
if (-not (Get-Command git -ErrorAction SilentlyContinue)) { throw 'git is not installed.' }
"MSBuild: $msbuild"; "Kit: $kitVer"

# 2. The source at the pinned commit, and its licence statement (the repository has no LICENSE file; its README states it).
$src = Join-Path $Work 'src'
if (Test-Path $src) { Remove-Item -Recurse -Force $src }
New-Item -ItemType Directory -Force $Work | Out-Null
git clone --quiet --no-checkout $Repo $src
if ($LASTEXITCODE) { throw 'git clone failed' }
git -C $src checkout --quiet --detach $Commit
if ($LASTEXITCODE) { throw "git checkout $Commit failed" }
$head = (git -C $src rev-parse HEAD).Trim()
if ($head -ne $Commit) { throw "HEAD is $head, not $Commit" }
$readme = Get-Content (Join-Path $src 'README.md') -Raw
if ($readme -notmatch 'MIT and CC0 or Public Domain') { throw 'The README no longer states "MIT and CC0 or Public Domain": check the licence again (UPSTREAM).' }

# 3. Build: Release x64, the WDK's test signing off.
$sln = Join-Path $src 'Virtual Display Driver (HDR)\SudoVDA.sln'
& $msbuild $sln /m /nologo /v:minimal /p:Configuration=Release /p:Platform=x64 /p:SignMode=Off
if ($LASTEXITCODE) { throw 'MSBuild failed' }
$built = Join-Path $src 'Virtual Display Driver (HDR)'
$dll = Get-ChildItem -Recurse $built -Filter 'SudoVDA.dll' | Where-Object FullName -match '\\x64\\Release\\' | Select-Object -First 1
$inf = Get-ChildItem -Recurse $built -Filter 'SudoVDA.inf' | Where-Object FullName -match '\\x64\\Release\\' | Select-Object -First 1
if (-not $dll -or -not $inf) { throw 'The build made no x64 Release SudoVDA.dll and stamped SudoVDA.inf.' }
if ((Get-Content $inf.FullName -Raw) -match '\$ARCH\$|\$UMDFVERSION\$|DriverVer\s*=\s*;') { throw "$($inf.FullName) wasn't stamped (stampinf)." }

# 4. The package: the stamped INF and the DLL; the DLL signed, then the catalog made and signed.
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null
Copy-Item $inf.FullName, $dll.FullName $Out
$ts = @()
if (-not $NoTimestamp) { $ts = @('/tr', $TimestampUrl, '/td', 'sha256') }
& $signtool sign /q /fd sha256 /f $Pfx /p $pass @ts (Join-Path $Out 'SudoVDA.dll')
if ($LASTEXITCODE) { throw 'signing SudoVDA.dll failed' }
& $inf2cat "/driver:$Out" '/os:10_X64' /uselocaltime
if ($LASTEXITCODE) { throw 'Inf2Cat failed' }
$cat = Join-Path $Out 'sudovda.cat'
if (-not (Test-Path $cat)) { throw 'Inf2Cat made no sudovda.cat' }
& $signtool sign /q /fd sha256 /f $Pfx /p $pass @ts $cat
if ($LASTEXITCODE) { throw 'signing sudovda.cat failed' }

# 5. What was made. The signatures aren't trusted on this VM (our root isn't imported here): the signer is what to check.
foreach ($f in 'SudoVDA.dll', 'sudovda.cat') {
    $s = Get-AuthenticodeSignature (Join-Path $Out $f)
    "{0}: signer {1} ({2})" -f $f, $s.SignerCertificate.Subject, $s.SignerCertificate.Thumbprint
}
Get-ChildItem $Out -File | Where-Object Name -ne 'SHA256SUMS' | ForEach-Object {
    '{0}  {1}' -f (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLower(), $_.Name
} | Set-Content -Encoding ascii (Join-Path $Out 'SHA256SUMS')
Get-Content (Join-Path $Out 'SHA256SUMS')
"Built from $Repo at $Commit into $Out. Now delete $Pfx from this VM."
