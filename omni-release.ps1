<#
.SYNOPSIS
Builds the Windows Gyroflow (Omni) plugins and uploads them to a draft GitHub release.

.DESCRIPTION
Omni has no CI, so releases are built locally on each OS:
  1. Windows: .\omni-release.ps1            builds, then creates the draft release v<version> and uploads the Windows files
  2. macOS:   ./omni-release.sh             builds and uploads the macOS files to the same draft
  3. Either:  .\omni-release.ps1 -Publish   checks every file is there, then publishes the release

The app only sees published releases, so users aren't offered a release until it's published.
The version comes from openfx/Cargo.toml (adobe and frei0r must match). Bump it with `just publish <version>` or by hand first.

.PARAMETER NotesFile
Markdown file with the release notes, which the app shows in its update popup. Without it, GitHub generates notes from the commits.

.PARAMETER Publish
Publish the existing draft instead of building. Fails if any expected file is missing.

.PARAMETER SkipBuild
Upload the files already in target\ without rebuilding.

.PARAMETER DryRun
Build and check the files, but don't change anything on GitHub.
#>
param(
    [string]$NotesFile,
    [switch]$Publish,
    [switch]$SkipBuild,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'yshui1/gyroflow-plugins-omni'
$Root = $PSScriptRoot
$Target = Join-Path $Root 'target'
# Files the app's plugin installer downloads; keep in sync with gyroflow-omni src/nle_plugins.rs
$WindowsAssets = @('Gyroflow-OpenFX-windows.zip', 'Gyroflow-Adobe-windows.aex')
$MacAssets     = @('Gyroflow-OpenFX-macos.zip', 'Gyroflow-Adobe-macos.zip')

function Get-CrateVersion([string]$Path) {
    $m = Select-String -LiteralPath $Path -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $m) { throw "No version in $Path" }
    $m.Matches[0].Groups[1].Value
}
function Invoke-Checked([string]$What, [scriptblock]$Block) {
    & $Block
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}
function Get-Release([string]$Tag) {
    # Windows PowerShell turns redirected native stderr into errors, which 'Stop' would throw on.
    $ErrorActionPreference = 'Continue'
    $json = gh release view $Tag -R $Repo --json isDraft,assets 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    $json | ConvertFrom-Json
}

foreach ($tool in 'gh', 'git') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool is not installed." }
}

$Version = Get-CrateVersion (Join-Path $Root 'openfx/Cargo.toml')
foreach ($crate in 'adobe', 'frei0r') {
    $v = Get-CrateVersion (Join-Path $Root "$crate/Cargo.toml")
    if ($v -ne $Version) { throw "$crate/Cargo.toml is $v but openfx/Cargo.toml is $Version. Set them to the same version." }
}
$Tag = "v$Version"
Write-Host "Gyroflow (Omni) plugins $Tag" -ForegroundColor Cyan

if ($Publish) {
    $release = Get-Release $Tag
    if (-not $release) { throw "No release $Tag on $Repo. Build and upload first." }
    if (-not $release.isDraft) { Write-Host "$Tag is already published."; exit 0 }
    $have = @($release.assets | ForEach-Object { $_.name })
    $missing = @($WindowsAssets + $MacAssets | Where-Object { $have -notcontains $_ })
    if ($missing) { throw "Not publishing, the draft is missing: $($missing -join ', ')" }
    $answer = Read-Host "Publish $Tag to everyone (the app will offer it as an update)? [y/N]"
    if ($answer -notmatch '^(y|yes)$') { Write-Host 'Cancelled.'; exit 1 }
    Invoke-Checked 'gh release edit' { gh release edit $Tag -R $Repo --draft=false --latest }
    Write-Host "Published https://github.com/$Repo/releases/tag/$Tag" -ForegroundColor Green
    exit 0
}

# The release tag is created at this commit, so it has to be on GitHub.
Push-Location $Root
try {
    $sha = (git rev-parse HEAD).Trim()
    if (git status --porcelain --untracked-files=no) { throw 'There are uncommitted changes. Commit them first so the release matches a commit.' }
    git fetch --quiet origin
    if (-not (git branch -r --contains $sha)) { throw "Commit $($sha.Substring(0, 8)) isn't pushed to origin. Push it first." }
} finally { Pop-Location }

if (-not $SkipBuild) {
    if (-not (Get-Command just -ErrorAction SilentlyContinue)) { throw 'just is not installed (winget install Casey.Just).' }
    if (-not $env:LIBCLANG_PATH -and (Test-Path 'C:\Program Files\LLVM\bin\libclang.dll')) { $env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin' }
    if (-not (Get-Command 7z -ErrorAction SilentlyContinue)) {
        if (Test-Path 'C:\Program Files\7-Zip\7z.exe') { $env:PATH = "C:\Program Files\7-Zip;$env:PATH" } else { throw '7-Zip is not installed (winget install 7zip.7zip).' }
    }
    $env:OMNI_RELEASE = '1'   # clean version numbers (1.0.0.0 instead of 1.0.0-dev)
    Push-Location $Root
    try {
        Invoke-Checked 'common tests' { Push-Location common; try { cargo test --locked } finally { Pop-Location } }
        Invoke-Checked 'OpenFX build' { just ofx deploy }
        Invoke-Checked 'Adobe build'  { just adobe deploy }
    } finally { Pop-Location; Remove-Item Env:OMNI_RELEASE }
}

$files = foreach ($name in $WindowsAssets) {
    $path = Join-Path $Target $name
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing $path" }
    $path
}
$ofx = Join-Path $Target 'gyroflow-ofx-windows/GyroflowOmni.ofx.bundle/Contents/Win64/GyroflowOmni.ofx'
$ofxVersion = (Get-Item -LiteralPath $ofx).VersionInfo.ProductVersion
if ($ofxVersion -ne "$Version.0") { throw "GyroflowOmni.ofx reports version $ofxVersion, expected $Version.0. Rebuild without -SkipBuild." }
foreach ($f in $files) {
    $hash = (Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash
    Write-Host ("  {0}  {1:N1} MB  SHA256 {2}" -f (Split-Path $f -Leaf), ((Get-Item $f).Length / 1MB), $hash)
}

if ($DryRun) { Write-Host 'Dry run: nothing uploaded.' -ForegroundColor Yellow; exit 0 }

$release = Get-Release $Tag
if ($release -and -not $release.isDraft) { throw "$Tag is already published. Bump the version for a new release." }
if (-not $release) {
    $notes = if ($NotesFile) { @('--notes-file', (Resolve-Path $NotesFile).Path) } else { @('--generate-notes') }
    Invoke-Checked 'gh release create' { gh release create $Tag -R $Repo --draft --target $sha --title "Gyroflow (Omni) plugins $Tag" @notes }
} elseif ($NotesFile) {
    Invoke-Checked 'gh release edit' { gh release edit $Tag -R $Repo --notes-file (Resolve-Path $NotesFile).Path }
}
Invoke-Checked 'gh release upload' { gh release upload $Tag -R $Repo --clobber @files }

Write-Host "Uploaded to draft $Tag. Next: run ./omni-release.sh on the Mac, then .\omni-release.ps1 -Publish" -ForegroundColor Green
