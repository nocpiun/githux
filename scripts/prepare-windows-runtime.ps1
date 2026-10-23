# Prepare the pinned, app-local Windows App SDK runtime next to a Windows executable.
#
# Normal development does not need this script: `cargo run` invokes the same logic
# through the Cargo runner in .cargo/config.toml. Use it directly to stage a packaged
# build (CI does this before uploading the artifact) or to repair a folder by hand.
#
# This is a development/packaging step, not a system installation: files are written
# next to the executable, nothing is registered with the OS, no administrator rights are
# required, ExecutionPolicy is unchanged and no background service is created.
[CmdletBinding()]
param(
    # Destination directory for the runtime payload. Defaults to this worktree's
    # target/debug, which is where `cargo build`/`cargo run` place the debug executable.
    [string] $Destination = 'target/debug',
    # Architecture of the executable in that destination. x64 is the default build.
    [ValidateSet('x64', 'arm64', 'x86')][string] $Architecture = 'x64',
    # Verified package cache. Defaults to <repo>/target/windows-appsdk.
    [string] $CacheRoot,
    # Offline tests/mirrors only; the defaults are the pinned official origin and hashes.
    [string] $SourceBase,
    [string] $PackageManifest
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repoRoot = Split-Path -Parent $PSScriptRoot

. (Join-Path $PSScriptRoot 'windows-runtime-lib.ps1')

$prepare = @{ RepoRoot = $repoRoot; Destination = $Destination; Architecture = $Architecture }
if ($CacheRoot) { $prepare.CacheRoot = $CacheRoot }
if ($SourceBase) { $prepare.SourceBase = $SourceBase }
if ($PackageManifest) { $prepare.PackageManifest = $PackageManifest }

try {
    $result = Invoke-GithuxWindowsRuntimePrepare @prepare
} catch {
    [Console]::Error.WriteLine("Windows App SDK runtime was not prepared: $($_.Exception.Message)")
    exit 1
}
Write-Output "Windows App SDK runtime ready for $Architecture in $($result.Destination) ($($result.Verified) unchanged, $($result.Written) written)"
