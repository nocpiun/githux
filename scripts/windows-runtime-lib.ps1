# Shared logic for the app-local Windows App SDK runtime used by development launches.
#
# Dot-source this file; it never exits the host. The pinned packages, their SHA256
# checksums and the download origin are production constants. `-PackageManifest` and
# `-SourceBase` exist for offline tests and air-gapped mirrors and never relax the
# default origin or hashes.
#
# Facts this file relies on (verified by scripts/tests/windows-runtime.test.ps1):
# - The runtime is the plain DLL/PRI set from `runtimes-framework/win-<arch>/native`,
#   beside the executable, plus `githux-wasdk.manifest` and the SDK licenses.
# - Nothing here installs anything system-wide, needs administrator rights, changes
#   ExecutionPolicy, or starts a background service.

$script:GithuxWindowsRuntimePackages = @(
    [pscustomobject]@{ Id = 'microsoft.windowsappsdk.foundation'; Version = '1.8.260803002'; Sha256 = 'B9232041AFD605B606C6F78F442D92EAD0076453F1F2A3260D2B7F8089BCAB0E' },
    [pscustomobject]@{ Id = 'microsoft.windowsappsdk.interactiveexperiences'; Version = '1.8.260708001'; Sha256 = '496EEA92D353B5D3601B67353F06DCADD6D2D9B635575ACEBE6E42587DBFAD76' }
)

$script:GithuxWindowsRuntimeSource = 'https://api.nuget.org/v3-flatcontainer'

function Get-GithuxSha256 {
    param([Parameter(Mandatory)][string] $Path)
    $sha = [Security.Cryptography.SHA256]::Create()
    $stream = [IO.File]::OpenRead($Path)
    try { return (($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) -join '') }
    finally { $stream.Dispose(); $sha.Dispose() }
}

function Get-GithuxSha256OfStream {
    param([Parameter(Mandatory)][IO.Stream] $Stream)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return (($sha.ComputeHash($Stream) | ForEach-Object { $_.ToString('x2') }) -join '') }
    finally { $sha.Dispose() }
}

function Resolve-GithuxRepoPath {
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $RepoRoot
    )
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $RepoRoot $Path))
}

# Cargo names the runtime payload directory after the target architecture, while the
# executable carries the authoritative answer in its PE header. Reading it keeps
# `--target`, `--target-dir` and cross-target builds consistent with the built binary.
function Get-GithuxPeArchitecture {
    param([Parameter(Mandatory)][string] $Path)
    $stream = [IO.File]::OpenRead($Path)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        if ($stream.Length -lt 0x40 -or $reader.ReadUInt16() -ne 0x5A4D) { return $null }
        $stream.Position = 0x3C
        $offset = $reader.ReadInt32()
        if ($offset -lt 0 -or ($offset + 6) -gt $stream.Length) { return $null }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x00004550) { return $null }
        switch ($reader.ReadUInt16()) {
            0x8664 { return 'x64' }
            0xAA64 { return 'arm64' }
            0x014C { return 'x86' }
            default { return $null }
        }
    } finally { $stream.Dispose() }
}

function Get-GithuxRuntimePackageList {
    param([string] $PackageManifest, [string] $RepoRoot)
    if (-not $PackageManifest) { return $script:GithuxWindowsRuntimePackages }
    $path = Resolve-GithuxRepoPath -Path $PackageManifest -RepoRoot $RepoRoot
    if (-not (Test-Path -LiteralPath $path)) { throw "Package manifest not found: $path" }
    # ConvertFrom-Json emits a JSON array as one object in Windows PowerShell 5.1, so it
    # cannot be wrapped with @() alone; iterate the parsed value to flatten it.
    $parsed = ConvertFrom-Json -InputObject (Get-Content -LiteralPath $path -Raw)
    $list = @()
    foreach ($package in $parsed) { $list += $package }
    foreach ($package in $list) {
        foreach ($field in 'Id', 'Version', 'Sha256') {
            if (-not $package.PSObject.Properties[$field]) { throw "Package manifest entry is missing '$field': $path" }
        }
    }
    if ($list.Count -eq 0) { throw "Package manifest has no entries: $path" }
    return $list
}

# Download (or reuse) one verified package archive. The temporary file is only renamed
# into place after the pinned checksum matches, so an interrupted download can never be
# mistaken for a valid cache entry.
function Get-GithuxPackageArchive {
    param(
        [Parameter(Mandatory)][string] $Cache,
        [Parameter(Mandatory)] $Package,
        [string] $SourceBase = $script:GithuxWindowsRuntimeSource
    )
    $archive = Join-Path $Cache 'package.zip'
    if (Test-Path -LiteralPath $archive) {
        if ((Get-GithuxSha256 -Path $archive) -eq $Package.Sha256) { return $archive }
        Remove-Item -LiteralPath $archive -Force
    }
    $name = "$($Package.Id).$($Package.Version).nupkg"
    $url = "$($SourceBase.TrimEnd('/'))/$($Package.Id)/$($Package.Version)/$name"
    $temp = Join-Path $Cache ('package.zip.' + [guid]::NewGuid().ToString('N') + '.tmp')
    $downloaded = $false
    try {
        try {
            if ($SourceBase -match '^https?://') {
                Invoke-WebRequest -Uri $url -OutFile $temp -UseBasicParsing -ErrorAction Stop
            } else {
                Copy-Item -LiteralPath (Join-Path $SourceBase $name) -Destination $temp -ErrorAction Stop
            }
            $downloaded = $true
        } catch {
            throw "Could not download the Windows App SDK package $name from $url. $($_.Exception.Message)"
        }
        $actual = Get-GithuxSha256 -Path $temp
        if ($actual -ne $Package.Sha256) {
            throw "The downloaded Windows App SDK package $name failed verification (expected $($Package.Sha256), got $actual). The transfer was incomplete or tampered with; rerun to download it again."
        }
        Move-Item -LiteralPath $temp -Destination $archive -Force
        return $archive
    } finally {
        if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue }
        $script:GithuxWindowsRuntimeDownloaded = $script:GithuxWindowsRuntimeDownloaded -or $downloaded
    }
}

# Enumerate the runtime payload of one architecture together with the expected content
# hash of every file. Deriving the expectation from the verified archive keeps the
# completeness check honest: no marker file on disk is trusted.
function Get-GithuxRuntimePayload {
    param(
        [Parameter(Mandatory)][string] $Archive,
        [Parameter(Mandatory)][ValidateSet('x64', 'arm64', 'x86')][string] $Architecture
    )
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $prefix = "runtimes-framework/win-$Architecture/native/"
        $entries = @($zip.Entries | Where-Object { $_.FullName.StartsWith($prefix, [StringComparison]::Ordinal) -and $_.Name })
        if ($entries.Count -eq 0) { throw "The pinned Windows App SDK package has no win-$Architecture runtime payload." }
        $files = @()
        foreach ($entry in $entries) {
            $stream = $entry.Open()
            try { $hash = Get-GithuxSha256OfStream -Stream $stream }
            finally { $stream.Dispose() }
            $files += [pscustomobject]@{ Name = $entry.Name; Length = $entry.Length; Sha256 = $hash }
        }
        $license = @($zip.Entries | Where-Object { $_.FullName -eq 'license.txt' })
        if ($license.Count -ne 1) { throw 'The pinned Windows App SDK package is missing license.txt.' }
        $stream = $license[0].Open()
        try { $licenseHash = Get-GithuxSha256OfStream -Stream $stream }
        finally { $stream.Dispose() }
        return [pscustomobject]@{
            Files   = $files
            License = [pscustomobject]@{ Name = 'license.txt'; Length = $license[0].Length; Sha256 = $licenseHash }
        }
    } finally { $zip.Dispose() }
}

# Copy one archive entry into place through a temporary file next to the destination.
# Unchanged files are left untouched (no rewrite, no new timestamp). A file that cannot
# be replaced because a running process holds it produces an actionable error instead of
# being overwritten in place and breaking that process.
function Install-GithuxRuntimeEntry {
    param(
        [Parameter(Mandatory)][string] $Archive,
        [Parameter(Mandatory)][string] $EntryName,
        [Parameter(Mandatory)][string] $Destination,
        [Parameter(Mandatory)][string] $Sha256,
        [long] $Length = -1
    )
    if (Test-Path -LiteralPath $Destination) {
        $existing = Get-Item -LiteralPath $Destination
        if ($Length -lt 0 -or $existing.Length -eq $Length) {
            if ((Get-GithuxSha256 -Path $Destination) -eq $Sha256) { return $false }
        }
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $temp = Join-Path (Split-Path -Parent $Destination) ('.githux-tmp-' + [guid]::NewGuid().ToString('N'))
    try {
        $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
        try {
            $entry = @($zip.Entries | Where-Object { $_.FullName -eq $EntryName })
            if ($entry.Count -ne 1) { throw "The pinned Windows App SDK package is missing '$EntryName'." }
            $input = $entry[0].Open()
            $output = [IO.File]::Create($temp)
            try { $input.CopyTo($output) }
            finally { $output.Dispose(); $input.Dispose() }
        } finally { $zip.Dispose() }
        if ((Get-GithuxSha256 -Path $temp) -ne $Sha256) {
            throw "The extracted Windows App SDK file $EntryName does not match its pinned checksum. The cached package is damaged; delete the cache and retry."
        }
        if (Test-Path -LiteralPath $Destination) {
            try { [IO.File]::Replace($temp, $Destination, [NullString]::Value) }
            catch [IO.IOException] {
                throw "Cannot update '$Destination' because another process is using it. Close the running Githux window (or any app using that folder) and retry."
            }
        } else {
            [IO.File]::Move($temp, $Destination)
        }
        return $true
    } finally {
        if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue }
    }
}

function Install-GithuxRuntimeCopy {
    param(
        [Parameter(Mandatory)][string] $Source,
        [Parameter(Mandatory)][string] $Destination
    )
    $expected = Get-GithuxSha256 -Path $Source
    if (Test-Path -LiteralPath $Destination) {
        $existing = Get-Item -LiteralPath $Destination
        if ($existing.Length -eq (Get-Item -LiteralPath $Source).Length -and (Get-GithuxSha256 -Path $Destination) -eq $expected) {
            return $false
        }
    }
    $temp = Join-Path (Split-Path -Parent $Destination) ('.githux-tmp-' + [guid]::NewGuid().ToString('N'))
    try {
        Copy-Item -LiteralPath $Source -Destination $temp
        if ((Get-GithuxSha256 -Path $temp) -ne $expected) { throw "Copying '$Source' produced unexpected content." }
        if (Test-Path -LiteralPath $Destination) {
            try { [IO.File]::Replace($temp, $Destination, [NullString]::Value) }
            catch [IO.IOException] {
                throw "Cannot update '$Destination' because another process is using it. Close the running Githux window (or any app using that folder) and retry."
            }
        } else {
            [IO.File]::Move($temp, $Destination)
        }
        return $true
    } finally {
        if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue }
    }
}

<#
.SYNOPSIS
Ensure the app-local Windows App SDK runtime beside a development executable.

.DESCRIPTION
Verifies the pinned packages, then makes `Destination` contain exactly the runtime
DLL/PRI set, the activation manifest and the SDK licenses. Files that already match are
not touched, so repeat runs stay fast and offline. Throws on any unrecoverable problem.
#>
function Invoke-GithuxWindowsRuntimePrepare {
    param(
        [Parameter(Mandatory)][string] $RepoRoot,
        [Parameter(Mandatory)][string] $Destination,
        [ValidateSet('x64', 'arm64', 'x86')][string] $Architecture = 'x64',
        [string] $CacheRoot,
        [string] $SourceBase = $script:GithuxWindowsRuntimeSource,
        [string] $PackageManifest
    )
    $script:GithuxWindowsRuntimeDownloaded = $false
    $destinationPath = Resolve-GithuxRepoPath -Path $Destination -RepoRoot $RepoRoot
    if (-not $CacheRoot) { $CacheRoot = Join-Path $RepoRoot 'target/windows-appsdk' }
    $cachePath = Resolve-GithuxRepoPath -Path $CacheRoot -RepoRoot $RepoRoot
    $packages = @(Get-GithuxRuntimePackageList -PackageManifest $PackageManifest -RepoRoot $RepoRoot)

    New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null
    # Leftovers from an interrupted copy are never valid runtime content.
    Get-ChildItem -LiteralPath $destinationPath -Filter '.githux-tmp-*' -File -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue

    $written = 0
    $verified = 0
    $repaired = 0
    foreach ($package in $packages) {
        $cache = Join-Path $cachePath "$($package.Id).$($package.Version)"
        New-Item -ItemType Directory -Path $cache -Force | Out-Null
        $archive = Get-GithuxPackageArchive -Cache $cache -Package $package -SourceBase $SourceBase
        $payload = Get-GithuxRuntimePayload -Archive $archive -Architecture $Architecture
        foreach ($file in $payload.Files) {
            $target = Join-Path $destinationPath $file.Name
            $existed = Test-Path -LiteralPath $target
            $changed = Install-GithuxRuntimeEntry -Archive $archive `
                -EntryName "runtimes-framework/win-$Architecture/native/$($file.Name)" `
                -Destination $target -Sha256 $file.Sha256 -Length $file.Length
            if ($changed) { $written++; if ($existed) { $repaired++ } } else { $verified++ }
        }
        $licenseTarget = Join-Path $destinationPath "$($package.Id).LICENSE.txt"
        $licenseExisted = Test-Path -LiteralPath $licenseTarget
        $licenseChanged = Install-GithuxRuntimeEntry -Archive $archive -EntryName 'license.txt' `
            -Destination $licenseTarget -Sha256 $payload.License.Sha256 -Length $payload.License.Length
        if ($licenseChanged) { $written++; if ($licenseExisted) { $repaired++ } } else { $verified++ }
    }

    $manifestSource = Join-Path $RepoRoot 'assets/windows/app.manifest'
    if (-not (Test-Path -LiteralPath $manifestSource)) { throw "Missing activation manifest: $manifestSource" }
    $manifestTarget = Join-Path $destinationPath 'githux-wasdk.manifest'
    $manifestExisted = Test-Path -LiteralPath $manifestTarget
    if (Install-GithuxRuntimeCopy -Source $manifestSource -Destination $manifestTarget) {
        $written++
        if ($manifestExisted) { $repaired++ }
    } else { $verified++ }

    return [pscustomobject]@{
        Destination = $destinationPath
        Architecture = $Architecture
        Cache       = $cachePath
        Written     = $written
        Verified    = $verified
        Repaired    = $repaired
        Downloaded  = [bool]$script:GithuxWindowsRuntimeDownloaded
    }
}
