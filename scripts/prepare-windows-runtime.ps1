# Download the pinned, app-local Windows App SDK runtime. No system installation.
[CmdletBinding()]
param(
    [string] $Destination = 'target/debug',
    [ValidateSet('x64', 'arm64')][string] $Architecture = 'x64'
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repoRoot = Split-Path -Parent $PSScriptRoot
$packages = @(
    @{ Id = 'microsoft.windowsappsdk.foundation'; Version = '1.8.260803002'; Sha256 = 'B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E' },
    @{ Id = 'microsoft.windowsappsdk.interactiveexperiences'; Version = '1.8.260708001'; Sha256 = '496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76' }
)
if (-not [IO.Path]::IsPathRooted($Destination)) { $Destination = Join-Path $repoRoot $Destination }
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
foreach ($package in $packages) {
    $cache = Join-Path $repoRoot "target/windows-appsdk/$($package.Id).$($package.Version)"
    New-Item -ItemType Directory -Path $cache -Force | Out-Null
    $archive = Join-Path $cache 'package.zip'
    if (-not (Test-Path -LiteralPath $archive)) {
        $url = "https://api.nuget.org/v3-flatcontainer/$($package.Id)/$($package.Version)/$($package.Id).$($package.Version).nupkg"
        Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
    }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $package.Sha256) {
        throw "SDK archive checksum mismatch: $archive. Remove the invalid archive and retry."
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $cache -Force
    $native = Join-Path $cache "runtimes-framework/win-$Architecture/native"
    if (-not (Test-Path -LiteralPath $native)) { throw "Missing runtime payload: $native" }
    Get-ChildItem -LiteralPath $native -File | Copy-Item -Destination $Destination -Force
    Copy-Item -LiteralPath (Join-Path $cache 'license.txt') -Destination (Join-Path $Destination "$($package.Id).LICENSE.txt") -Force
}
Copy-Item -LiteralPath (Join-Path $repoRoot 'assets/windows/app.manifest') -Destination (Join-Path $Destination 'githux-wasdk.manifest') -Force
Write-Output "Windows App SDK runtime prepared for $Architecture in $Destination"
