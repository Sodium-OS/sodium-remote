# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Builds Sodium Remote Display (the matched display's virtual display driver): SudoMaker's SudoVDA at the pinned commit
# with our patches (patches/*.patch), the Windows Driver Kit from NuGet, then the catalog. Nothing is signed here and no
# secret is used: .github/workflows/sudovda.yml runs this on a windows-2022 runner and uploads -Out as an artifact; sign.sh
# signs it on the Linux machine that keeps the key. It also runs in a Windows build VM with Visual Studio 2022 (Desktop
# development with C++), git and nuget.exe; never on a user's computer.
#
#   .\build.ps1 [-Work C:\sodium-remote-display] [-Out C:\sodium-remote-display\out]
#
# Output in -Out: SodiumRemoteDisplay.inf (stamped), SodiumRemoteDisplay.dll, sodiumremotedisplay.cat (unsigned),
# SHA256SUMS and BUILDINFO (what was built from what). No test signing at any step (the WDK's TestSign is switched off).
param(
    [string]$Work = "$env:SystemDrive\sodium-remote-display",
    [string]$Out = ""
)
$ErrorActionPreference = 'Stop'
# Keep these in step with host-desktop/src/vdd_pin.rs (COMMIT) and packaging/windows/sudovda/UPSTREAM.
$Repo = 'https://github.com/SudoMaker/SudoVDA.git'
$Commit = 'a4b09fa2aa731a964d0cb5d139cb1e6240e4da12'
# The Windows Driver Kit and SDK from NuGet: one version for all three.
$KitVersion = '10.0.28000.2526'
$Name = 'SodiumRemoteDisplay'
$Cat = 'sodiumremotedisplay.cat'
if (-not $Out) { $Out = Join-Path $Work 'out' }
$here = $PSScriptRoot

# 1. The tools.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw 'Visual Studio 2022 is not installed (no vswhere.exe).' }
$msbuild = & $vswhere -latest -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\MSBuild.exe' | Select-Object -First 1
if (-not $msbuild) { throw 'MSBuild not found.' }
foreach ($t in 'git', 'nuget') { if (-not (Get-Command $t -ErrorAction SilentlyContinue)) { throw "$t is not installed." } }
"MSBuild: $msbuild"

# 2. The source at the pinned commit, byte for byte (no line-ending conversion: the patches are made against it), and its
# licence statement (the repository has no LICENSE file; its README states it).
$src = Join-Path $Work 'src'
if (Test-Path $src) { Remove-Item -Recurse -Force $src }
New-Item -ItemType Directory -Force $Work | Out-Null
git -c core.autocrlf=false clone --quiet --no-checkout $Repo $src
if ($LASTEXITCODE) { throw 'git clone failed' }
git -C $src config core.autocrlf false
git -C $src checkout --quiet --detach $Commit
if ($LASTEXITCODE) { throw "git checkout $Commit failed" }
$head = (git -C $src rev-parse HEAD).Trim()
if ($head -ne $Commit) { throw "HEAD is $head, not $Commit" }
$readme = Get-Content (Join-Path $src 'README.md') -Raw
if ($readme -notmatch 'MIT and CC0 or Public Domain') { throw 'The README no longer states "MIT and CC0 or Public Domain": check the licence again (UPSTREAM).' }

# 3. Our patches, in order.
$patches = Get-ChildItem (Join-Path $here 'patches') -Filter '*.patch' | Sort-Object Name
if (-not $patches) { throw 'No patches in patches\.' }
foreach ($p in $patches) {
    git -C $src apply --whitespace=nowarn $p.FullName
    if ($LASTEXITCODE) { throw "$($p.Name) doesn't apply" }
    "applied $($p.Name)"
}

# 4. The kit from NuGet, imported into every project of the tree by Directory.Build.props/.targets.
@"
<?xml version="1.0" encoding="utf-8"?>
<packages>
  <package id="Microsoft.Windows.SDK.CPP" version="$KitVersion" targetFramework="native" />
  <package id="Microsoft.Windows.SDK.CPP.x64" version="$KitVersion" targetFramework="native" />
  <package id="Microsoft.Windows.WDK.x64" version="$KitVersion" targetFramework="native" />
</packages>
"@ | Set-Content -Encoding utf8 (Join-Path $src 'packages.config')
nuget restore (Join-Path $src 'packages.config') -PackagesDirectory (Join-Path $src 'packages') -NonInteractive
if ($LASTEXITCODE) { throw 'nuget restore failed' }
foreach ($kind in 'props', 'targets') {
    "<Project><Import Project=`"`$(MSBuildThisFileDirectory)packages\*\build\native\*.$kind`" /></Project>" |
        Set-Content -Encoding utf8 (Join-Path $src "Directory.Build.$kind")
}
$inf2cat = Get-ChildItem -Recurse (Join-Path $src 'packages') -Filter 'Inf2Cat.exe' | Where-Object FullName -match '\\x86\\' | Select-Object -First 1
if (-not $inf2cat) { throw 'Inf2Cat.exe is not in the kit packages.' }
"Kit: $KitVersion; Inf2Cat: $($inf2cat.FullName)"

# 5. Build: Release x64, the WDK's test signing off.
$sln = Join-Path $src 'Virtual Display Driver (HDR)\SudoVDA.sln'
& $msbuild $sln /m /nologo /v:minimal /p:Configuration=Release /p:Platform=x64 /p:SignMode=Off
if ($LASTEXITCODE) { throw 'MSBuild failed' }
$built = Join-Path $src 'Virtual Display Driver (HDR)'
$dll = Get-ChildItem -Recurse $built -Filter "$Name.dll" | Where-Object FullName -match '\\x64\\Release\\' | Select-Object -First 1
$inf = Get-ChildItem -Recurse $built -Filter "$Name.inf" | Where-Object FullName -match '\\x64\\Release\\' | Select-Object -First 1
if (-not $dll -or -not $inf) { throw "The build made no x64 Release $Name.dll and stamped $Name.inf." }
if ((Get-Content $inf.FullName -Raw) -match '\$ARCH\$|\$UMDFVERSION\$|DriverVer\s*=\s*;') { throw "$($inf.FullName) wasn't stamped (stampinf)." }

# 6. The package: the stamped INF, the DLL and the catalog of the two (unsigned; sign.sh signs the DLL and the catalog).
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null
Copy-Item $inf.FullName, $dll.FullName $Out
& $inf2cat.FullName "/driver:$Out" '/os:10_X64' /uselocaltime
if ($LASTEXITCODE) { throw 'Inf2Cat failed' }
if (-not (Test-Path (Join-Path $Out $Cat))) { throw "Inf2Cat made no $Cat" }

# 7. What was made, from what.
$self = if ($env:GITHUB_SHA) { $env:GITHUB_SHA } else { (git -C $here rev-parse HEAD).Trim() }
$info = @("upstream $Repo $Commit", "sodium-remote $self", "kit $KitVersion")
if ($env:GITHUB_RUN_ID) { $info += "run $env:GITHUB_SERVER_URL/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID" }
foreach ($p in $patches) { $info += 'patch {0} {1}' -f (Get-FileHash -Algorithm SHA256 $p.FullName).Hash.ToLower(), $p.Name }
$info | Set-Content -Encoding ascii (Join-Path $Out 'BUILDINFO')
Get-ChildItem $Out -File | Where-Object Name -notin 'SHA256SUMS', 'BUILDINFO' | ForEach-Object {
    '{0}  {1}' -f (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLower(), $_.Name
} | Set-Content -Encoding ascii (Join-Path $Out 'SHA256SUMS')
Get-Content (Join-Path $Out 'BUILDINFO'), (Join-Path $Out 'SHA256SUMS')
"Built into $Out (unsigned). Sign it with sign.sh on the machine that keeps the key."
