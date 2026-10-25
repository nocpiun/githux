# Offline tests for the Windows development-launch runtime preparation.
#
# Everything runs in a throwaway directory with a local package source: no network, no
# changes to the real worktree target, and no real Windows App SDK download. The pinned
# production origin and checksums are asserted separately so tests cannot relax them.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/tests/windows-runtime.test.ps1
#   powershell ... -SkipCargo      # only the script/library cases, no Cargo fixture
#
# The Cargo cases build a tiny dependency-free fixture crate to prove the runner is
# really invoked as a Windows process chain: argument round-trip, working directory,
# exit code, profiles, explicit --target and custom --target-dir.
[CmdletBinding()]
param([switch] $SkipCargo)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$libPath = Join-Path $repoRoot 'scripts/windows-runtime-lib.ps1'
$prepareScript = Join-Path $repoRoot 'scripts/prepare-windows-runtime.ps1'
$runnerScript = Join-Path $repoRoot 'scripts/run-githux.ps1'
$runnerBridge = Join-Path $repoRoot 'scripts/run-githux.cmd'
$manifestSource = Join-Path $repoRoot 'assets/windows/app.manifest'
foreach ($required in $libPath, $prepareScript, $runnerScript, $runnerBridge, $manifestSource) {
    if (-not (Test-Path -LiteralPath $required)) { throw "Missing required file: $required" }
}

$failures = [Collections.Generic.List[string]]::new()
$cases = 0
function Test-Case {
    param([string] $Name, [scriptblock] $Body)
    $script:cases++
    try {
        & $Body
        Write-Host "  ok   $Name"
    } catch {
        $script:failures.Add("$Name -> $($_.Exception.Message)")
        Write-Host "  FAIL $Name"
        Write-Host "       $($_.Exception.Message)"
    }
}
function Assert-Equal {
    param($Expected, $Actual, [string] $Because)
    if ("$Expected" -ne "$Actual") { throw "$Because (expected '$Expected', got '$Actual')" }
}
function Assert-True {
    param([bool] $Condition, [string] $Because)
    if (-not $Condition) { throw $Because }
}
function Assert-Match {
    param([string] $Text, [string] $Pattern, [string] $Because)
    if ($Text -notmatch $Pattern) { throw "$Because (output did not match /$Pattern/)`n--- output ---`n$Text" }
}
function Get-FileHashOf([string] $Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

$work = Join-Path ([IO.Path]::GetTempPath()) ('githux-runtime-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work -Force | Out-Null

function New-TestPackage {
    # Build a NuGet-shaped archive with a native payload for one architecture. These
    # stand in for the real pinned packages; only hashes and origin change, never the
    # extraction/copy logic under test.
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Marker
    )
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Create)
    $zip = [IO.Compression.ZipArchive]::new($stream, [IO.Compression.ZipArchiveMode]::Create)
    try {
        # Each package ships its own file names, like the real packages do.
        foreach ($architecture in 'x64', 'arm64') {
            $files = [ordered]@{
                "$Marker.Runtime.dll" = "runtime-$Marker-$architecture"
                "$Marker.Resources.pri" = "pri-$Marker-$architecture"
            }
            foreach ($name in $files.Keys) {
                $entry = $zip.CreateEntry("runtimes-framework/win-$architecture/native/$name")
                $writer = [IO.StreamWriter]::new($entry.Open())
                try { $writer.Write($files[$name]) } finally { $writer.Dispose() }
            }
        }
        $license = $zip.CreateEntry('license.txt')
        $writer = [IO.StreamWriter]::new($license.Open())
        try { $writer.Write("license text for $Marker`n") } finally { $writer.Dispose() }
    } finally { $zip.Dispose(); $stream.Dispose() }
}

function New-TestFeed {
    param([Parameter(Mandatory)][string] $Directory)
    New-Item -ItemType Directory -Path $Directory -Force | Out-Null
    $packages = @()
    $index = 0
    foreach ($id in 'microsoft.windowsappsdk.foundation', 'microsoft.windowsappsdk.interactiveexperiences') {
        $index++
        $version = "9.9.$index"
        $name = "$id.$version.nupkg"
        $path = Join-Path $Directory $name
        New-TestPackage -Path $path -Marker "p$index"
        $packages += [pscustomobject]@{ Id = $id; Version = $version; Sha256 = (Get-FileHashOf $path) }
    }
    $manifestPath = Join-Path $Directory 'packages.json'
    [IO.File]::WriteAllText($manifestPath, (ConvertTo-Json -InputObject @($packages) -Depth 4))
    return [pscustomobject]@{ Directory = $Directory; Packages = $packages; Manifest = $manifestPath }
}

function Invoke-Prepare {
    param(
        [Parameter(Mandatory)][string] $Destination,
        [Parameter(Mandatory)] $Feed,
        [string] $CacheRoot,
        [string] $PackageManifest,
        [string] $SourceBase
    )
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $prepareScript,
        '-Destination', $Destination,
        '-Architecture', 'x64',
        '-CacheRoot', $(if ($CacheRoot) { $CacheRoot } else { Join-Path $Destination '.cache' }),
        '-SourceBase', $(if ($SourceBase) { $SourceBase } else { $Feed.Directory }),
        '-PackageManifest', $(if ($PackageManifest) { $PackageManifest } else { $Feed.Manifest }))
    return Invoke-NativeProcess -FilePath 'powershell' -Arguments $arguments
}

# Run a native process and capture stdout/stderr plus the real exit code.
# Reading the streams through .NET keeps Windows PowerShell from turning a child's
# stderr into a terminating error, which would hide the exit code and message under test.
function Invoke-NativeProcess {
    param(
        [Parameter(Mandatory)][string] $FilePath,
        [AllowEmptyCollection()][AllowEmptyString()][string[]] $Arguments = @(),
        [string] $WorkingDirectory,
        [hashtable] $Environment = @{}
    )
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.Arguments = Join-NativeArgumentList -Arguments $Arguments
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    if ($WorkingDirectory) { $startInfo.WorkingDirectory = $WorkingDirectory }
    foreach ($key in $Environment.Keys) {
        $startInfo.EnvironmentVariables[$key] = $Environment[$key]
    }
    $process = [Diagnostics.Process]::Start($startInfo)
    if (-not $process) { throw "Cannot start '$FilePath'." }
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $text = $stdout.Result + $stderr.Result
    return [pscustomobject]@{ ExitCode = [int]$process.ExitCode; Output = $text }
}

# Quote one argument for a native command line, using the same rules the C runtime and
# Rust's std::process re-parse, so test arguments reach the child unchanged.
function Join-NativeArgumentList {
    param([AllowEmptyCollection()][AllowEmptyString()][string[]] $Arguments)
    $builder = [Text.StringBuilder]::new()
    $quote = [char]34
    $slash = [char]92
    foreach ($argument in $Arguments) {
        if ($builder.Length -gt 0) { $null = $builder.Append([char]32) }
        $value = if ($null -eq $argument) { '' } else { $argument }
        if ($value.Length -gt 0 -and $value -notmatch '[\s"]') { $null = $builder.Append($value); continue }
        $null = $builder.Append($quote)
        $backslashes = 0
        foreach ($character in $value.ToCharArray()) {
            if ($character -eq $slash) { $backslashes++; continue }
            if ($character -eq $quote) {
                # Backslashes before a quote are doubled, then the quote is escaped.
                if ($backslashes -gt 0) { $null = $builder.Append($slash, $backslashes * 2) }
                $null = $builder.Append($slash)
                $null = $builder.Append($quote)
            } else {
                if ($backslashes -gt 0) { $null = $builder.Append($slash, $backslashes) }
                $null = $builder.Append($character)
            }
            $backslashes = 0
        }
        # Trailing backslashes are doubled so they cannot escape the closing quote.
        if ($backslashes -gt 0) { $null = $builder.Append($slash, $backslashes * 2) }
        $null = $builder.Append($quote)
    }
    return $builder.ToString()
}

# ---------------------------------------------------------------- pinned constants
Write-Host 'Production pinning'
Test-Case 'default package set keeps the pinned origin, versions and checksums' {
    . $libPath
    $list = @(Get-GithuxRuntimePackageList -RepoRoot $repoRoot)
    Assert-Equal 2 $list.Count 'two pinned packages must stay pinned'
    Assert-Equal 'microsoft.windowsappsdk.foundation' $list[0].Id 'foundation package id'
    Assert-Equal '1.8.260803002' $list[0].Version 'foundation package version'
    Assert-Equal 'b9232041afd605b606c6f78f442d92ead0076453f1f2a3260d2b7f8089bcab0e' $list[0].Sha256.ToLowerInvariant() 'foundation checksum'
    Assert-Equal 'microsoft.windowsappsdk.interactiveexperiences' $list[1].Id 'interactive experiences package id'
    Assert-Equal '1.8.260708001' $list[1].Version 'interactive experiences package version'
    Assert-Equal '496eea92d353b5d3601b67353f06dcadd6d2d9b635575acebe6e42587dbfad76' $list[1].Sha256.ToLowerInvariant() 'interactive experiences checksum'
    Assert-Equal 'https://api.nuget.org/v3-flatcontainer' $script:GithuxWindowsRuntimeSource 'default download origin'
    # The shipped activation manifest is real repository content and must keep listing
    # the title-bar types the Tall caption path activates.
    $manifest = [xml](Get-Content -LiteralPath $manifestSource -Raw)
    $referenced = @($manifest.GetElementsByTagName('asmv3:file') | ForEach-Object { $_.name })
    Assert-True ($referenced.Count -gt 0) 'the activation manifest lists runtime files'
    Assert-True ($referenced -contains 'Microsoft.UI.Windowing.Core.dll') 'the manifest still activates the title-bar types'
    Assert-True ($referenced -contains 'Microsoft.WindowsAppRuntime.dll') 'the manifest still activates the runtime types'
}
Test-Case 'PE architecture parsing matches the built executable' {
    . $libPath
    $fixture = Join-Path $work 'pe-probe.exe'
    # 0x8664 (amd64) at PE+4, reached through the e_lfanew pointer at 0x3C.
    $bytes = [byte[]]::new(0x100)
    $bytes[0] = 0x4D; $bytes[1] = 0x5A
    $bytes[0x3C] = 0x40
    $bytes[0x40] = 0x50; $bytes[0x41] = 0x45; $bytes[0x42] = 0; $bytes[0x43] = 0
    $bytes[0x44] = 0x64; $bytes[0x45] = 0x86
    [IO.File]::WriteAllBytes($fixture, $bytes)
    Assert-Equal 'x64' (Get-GithuxPeArchitecture -Path $fixture) 'amd64 header maps to the x64 payload'
    $bytes[0x44] = 0x64; $bytes[0x45] = 0xAA
    [IO.File]::WriteAllBytes($fixture, $bytes)
    Assert-Equal 'arm64' (Get-GithuxPeArchitecture -Path $fixture) 'arm64 header maps to the arm64 payload'
    $bytes[0x44] = 0x4C; $bytes[0x45] = 0x01
    [IO.File]::WriteAllBytes($fixture, $bytes)
    Assert-Equal 'x86' (Get-GithuxPeArchitecture -Path $fixture) 'x86 header maps to the x86 payload'
    [IO.File]::WriteAllBytes($fixture, [byte[]](1, 2, 3, 4))
    Assert-Equal '' (Get-GithuxPeArchitecture -Path $fixture) 'a non-PE file has no architecture'
}

# ------------------------------------------------------------------- prepare logic
Write-Host 'Runtime preparation (offline fixture feed)'
$feed = New-TestFeed -Directory (Join-Path $work 'feed')
$destination = Join-Path $work 'out dir ünïcode/debug'
$cache = Join-Path $work 'cache dir ünïcode/windows-appsdk'

Test-Case 'fresh prepare writes the payload, licenses and activation manifest' {
    $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache
    Assert-Equal 0 $result.ExitCode "prepare must succeed`n$($result.Output)"
    Assert-Match $result.Output 'runtime ready' 'prepare reports success'
    foreach ($name in 'p1.Runtime.dll', 'p1.Resources.pri', 'p2.Runtime.dll', 'p2.Resources.pri') {
        Assert-True (Test-Path -LiteralPath (Join-Path $destination $name)) "payload file $name must exist"
    }
    foreach ($package in $feed.Packages) {
        Assert-True (Test-Path -LiteralPath (Join-Path $destination "$($package.Id).LICENSE.txt")) "license for $($package.Id) must exist"
    }
    Assert-True (Test-Path -LiteralPath (Join-Path $destination 'githux-wasdk.manifest')) 'activation manifest must exist beside the executable'
    # The cache lives where it was asked to live, separate from the destination folder.
    Assert-True (Test-Path -LiteralPath (Join-Path $cache "$($feed.Packages[0].Id).$($feed.Packages[0].Version)/package.zip")) 'verified archive is cached under CacheRoot'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $destination 'package.zip'))) 'packages are not copied into the runtime destination'
    Assert-Equal 0 @(Get-ChildItem -LiteralPath $destination -Filter '.githux-tmp-*' -File).Count 'no temporary files are left behind'
}

Test-Case 'repeat prepare is offline and rewrites nothing' {
    $before = @{}
    Get-ChildItem -LiteralPath $destination -File | ForEach-Object { $before[$_.Name] = "$($_.LastWriteTimeUtc.Ticks)/$($_.Length)" }
    # An unusable source proves the second run uses the verified cache only.
    $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache -SourceBase (Join-Path $work 'missing-source')
    Assert-Equal 0 $result.ExitCode "repeat prepare must not need the network`n$($result.Output)"
    Assert-Match $result.Output '0 written' 'repeat prepare writes no files'
    foreach ($entry in $before.GetEnumerator()) {
        $path = Join-Path $destination $entry.Key
        Assert-Equal $entry.Value "$((Get-Item -LiteralPath $path).LastWriteTimeUtc.Ticks)/$((Get-Item -LiteralPath $path).Length)" "unchanged file must not be rewritten: $($entry.Key)"
    }
}

Test-Case 'deleted and corrupted payload files are repaired' {
    $payload = Join-Path $destination 'p1.Runtime.dll'
    $expected = Get-FileHashOf $payload
    Remove-Item -LiteralPath $payload -Force
    $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache
    Assert-Equal 0 $result.ExitCode 'prepare repairs a missing file'
    Assert-Match $result.Output '1 written' 'only the missing file is written'
    Assert-Equal $expected (Get-FileHashOf $payload) 'repaired file matches the package content'
    [IO.File]::WriteAllText($payload, 'truncated')
    $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache
    Assert-Equal 0 $result.ExitCode 'prepare repairs a damaged file'
    Assert-Equal $expected (Get-FileHashOf $payload) 'damaged file is restored from the verified package'
}

Test-Case 'a damaged cached archive is discarded and fetched again' {
    $archive = Join-Path $cache "$($feed.Packages[0].Id).$($feed.Packages[0].Version)/package.zip"
    [IO.File]::WriteAllText($archive, 'not a package')
    $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache
    Assert-Equal 0 $result.ExitCode "prepare recovers from a damaged cache`n$($result.Output)"
    Assert-Equal $feed.Packages[0].Sha256 (Get-FileHashOf $archive) 'cache holds the verified archive again'
}

Test-Case 'a checksum mismatch fails loudly and installs nothing' {
    $badFeedDirectory = Join-Path $work 'bad-feed'
    New-Item -ItemType Directory -Path $badFeedDirectory -Force | Out-Null
    $packages = @()
    foreach ($package in $feed.Packages) {
        $name = "$($package.Id).$($package.Version).nupkg"
        Copy-Item -LiteralPath (Join-Path $feed.Directory $name) -Destination (Join-Path $badFeedDirectory $name)
        $packages += [pscustomobject]@{ Id = $package.Id; Version = $package.Version; Sha256 = ('0' * 64) }
    }
    $badManifest = Join-Path $badFeedDirectory 'packages.json'
    [IO.File]::WriteAllText($badManifest, (ConvertTo-Json -InputObject @($packages) -Depth 4))
    $badDestination = Join-Path $work 'bad-destination'
    $badCache = Join-Path $work 'bad-cache'
    $result = Invoke-Prepare -Destination $badDestination -Feed $feed -CacheRoot $badCache -PackageManifest $badManifest
    Assert-True ($result.ExitCode -ne 0) 'a bad checksum must produce a non-zero exit code'
    Assert-Match $result.Output 'failed verification' 'the error explains the checksum failure'
    Assert-Equal 0 @(Get-ChildItem -LiteralPath $badDestination -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike '.githux-tmp-*' }).Count 'no payload is left behind'
    Assert-Equal 0 @(Get-ChildItem -LiteralPath $badCache -Recurse -File -Filter 'package.zip' -ErrorAction SilentlyContinue).Count 'the half package is not kept as a valid cache entry'
}

Test-Case 'a truncated download is not treated as a cache hit' {
    $partialFeed = Join-Path $work 'partial-feed'
    New-Item -ItemType Directory -Path $partialFeed -Force | Out-Null
    foreach ($package in $feed.Packages) {
        $name = "$($package.Id).$($package.Version).nupkg"
        $bytes = [IO.File]::ReadAllBytes((Join-Path $feed.Directory $name))
        [IO.File]::WriteAllBytes((Join-Path $partialFeed $name), $bytes[0..([Math]::Max(0, $bytes.Length - 40))])
    }
    $partialDestination = Join-Path $work 'partial-destination'
    $result = Invoke-Prepare -Destination $partialDestination -Feed $feed -CacheRoot (Join-Path $work 'partial-cache') -SourceBase $partialFeed
    Assert-True ($result.ExitCode -ne 0) 'a truncated package must fail'
    Assert-Equal 0 @(Get-ChildItem -LiteralPath $partialDestination -File -ErrorAction SilentlyContinue).Count 'nothing is installed from a truncated package'
    Assert-Equal 0 @(Get-ChildItem -LiteralPath (Join-Path $work 'partial-cache') -Recurse -File -Filter '*.tmp' -ErrorAction SilentlyContinue).Count 'the truncated temporary download is removed'
}

Test-Case 'an unreachable source fails with an actionable error' {
    $result = Invoke-Prepare -Destination (Join-Path $work 'offline-destination') -Feed $feed -CacheRoot (Join-Path $work 'offline-cache') -SourceBase (Join-Path $work 'does-not-exist')
    Assert-True ($result.ExitCode -ne 0) 'a missing source must fail'
    Assert-Match $result.Output 'Could not download' 'the error says the download failed'
    Assert-Match $result.Output 'microsoft.windowsappsdk' 'the error names the package that could not be fetched'
}

Test-Case 'a payload file held by another process is not overwritten blindly' {
    $locked = Join-Path $destination 'p1.Resources.pri'
    $original = Get-Content -LiteralPath $locked -Raw
    # Replace its content first, then hold it open the way a loaded DLL or a running app
    # does: sharing reads only, so the file cannot be replaced or rewritten in place.
    [IO.File]::WriteAllText($locked, 'stale content that must be repaired')
    $handle = [IO.File]::Open($locked, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache
        Assert-True ($result.ExitCode -ne 0) 'a file in use must fail instead of being replaced in place'
        Assert-Match $result.Output 'another process is using it' 'the error names the in-use file and the remedy'
        Assert-Equal 'stale content that must be repaired' (Get-Content -LiteralPath $locked -Raw) 'the in-use file keeps its content'
    } finally { $handle.Dispose() }
    $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache
    Assert-Equal 0 $result.ExitCode 'preparation succeeds once the file is released'
    Assert-Equal $original (Get-Content -LiteralPath $locked -Raw) 'the stale file is then restored from the verified package'
}

Test-Case 'stale temporary files from an interrupted run are cleaned up' {
    $stale = Join-Path $destination '.githux-tmp-deadbeef'
    [IO.File]::WriteAllText($stale, 'partial')
    $result = Invoke-Prepare -Destination $destination -Feed $feed -CacheRoot $cache
    Assert-Equal 0 $result.ExitCode 'prepare succeeds with a leftover temporary file'
    Assert-True (-not (Test-Path -LiteralPath $stale)) 'the leftover temporary file is removed'
}

if (-not $SkipCargo) {
    Write-Host 'Cargo runner (real Windows process chain)'
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo is required for the runner cases (use -SkipCargo to skip)' }

    $fixture = Join-Path $work 'fixture repo ünï'
    $fixtureScripts = Join-Path $fixture 'scripts'
    $fixtureCargo = Join-Path $fixture '.cargo'
    New-Item -ItemType Directory -Path $fixtureScripts, $fixtureCargo, (Join-Path $fixture 'src'), (Join-Path $fixture 'assets/windows'), (Join-Path $fixture 'sub dir') -Force | Out-Null
    Copy-Item -LiteralPath $runnerScript, $runnerBridge, $libPath -Destination $fixtureScripts
    Copy-Item -LiteralPath $manifestSource -Destination (Join-Path $fixture 'assets/windows/app.manifest')
    [IO.File]::WriteAllText((Join-Path $fixture 'Cargo.toml'), @'
[package]
name = "githux"
version = "0.0.1"
edition = "2021"
default-run = "githux"

[[bin]]
name = "githux"
path = "src/main.rs"

[[bin]]
name = "other-tool"
path = "src/other.rs"
'@)
    # Observe argv, working directory and exit code exactly; length prefixes keep
    # trailing spaces and empty arguments unambiguous.
    [IO.File]::WriteAllText((Join-Path $fixture 'src/main.rs'), @'
fn main() {
    for argument in std::env::args().skip(1) {
        println!("ARG|{}|{}", argument.chars().count(), argument);
    }
    println!("CWD|{}", std::env::current_dir().unwrap().display());
    if let Ok(code) = std::env::var("GITHUX_TEST_EXIT") {
        std::process::exit(code.parse().unwrap());
    }
}

#[test]
fn harness_runs() {
    assert_eq!(std::env::var("GITHUX_TEST_CANARY").as_deref(), Ok("yes"));
}
'@)
    [IO.File]::WriteAllText((Join-Path $fixture 'src/other.rs'), @'
fn main() {
    println!("OTHER_RAN");
}
'@)
    [IO.File]::WriteAllText((Join-Path $fixture '.cargo/config.toml'), @'
[target.'cfg(windows)']
runner = ["./scripts/run-githux.cmd"]
'@)

    $fixtureFeed = New-TestFeed -Directory (Join-Path $work 'fixture-feed')
    $env:GITHUX_WINDOWS_RUNTIME_SOURCE = $fixtureFeed.Directory
    $env:GITHUX_WINDOWS_RUNTIME_PACKAGES = $fixtureFeed.Manifest

    function Invoke-Cargo {
        param([AllowEmptyCollection()][AllowEmptyString()][Parameter(Mandatory)][string[]] $Arguments, [string] $WorkingDirectory = $fixture, [hashtable] $Environment = @{})
        return Invoke-NativeProcess -FilePath 'cargo' -Arguments $Arguments -WorkingDirectory $WorkingDirectory -Environment $Environment
    }
    function Get-ReportedArguments {
        param([string] $Output)
        $reported = @()
        foreach ($line in $Output -split "`n") {
            $match = [regex]::Match($line, '^ARG\|(\d+)\|(.*)$')
            if ($match.Success) {
                $text = $match.Groups[2].Value
                Assert-Equal $match.Groups[1].Value $text.Length 'reported argument length prefix matches the value'
                $reported += $text
            }
        }
        return , $reported
    }
    function Get-ReportedWorkingDirectory {
        param([string] $Output)
        $match = [regex]::Match($Output, '(?m)^CWD\|(.*)$')
        if (-not $match.Success) { throw "the fixture did not report its working directory`n$Output" }
        return $match.Groups[1].Value
    }

    Test-Case 'cargo run prepares the runtime and passes hostile arguments through' {
        $debugTarget = Join-Path $fixture 'target'
        $hostile = @('plain', 'with space', 'he said "hi"', 'and&or', '%PATH%', '!delayed!', '^caret',
            'semi;colon', 'pipe|', 'redirect>', 'paren(s)', 'equation=', 'back\slash\', 'trailing\\',
            'quote"inside', 'ünïcode 路径', '')
        $result = Invoke-Cargo -Arguments (@('run', '--quiet', '--') + $hostile)
        Assert-Equal 0 $result.ExitCode "cargo run must succeed`n$($result.Output)"
        $reported = Get-ReportedArguments -Output $result.Output
        Assert-Equal $hostile.Count $reported.Count 'every argument reaches the application'
        for ($i = 0; $i -lt $hostile.Count; $i++) {
            Assert-Equal $hostile[$i] $reported[$i] "argument $i must survive unchanged"
        }
        Assert-Equal ([IO.Path]::GetFullPath($fixture)) (Get-ReportedWorkingDirectory -Output $result.Output) 'the application keeps the caller working directory'
        foreach ($name in 'p1.Runtime.dll', 'p1.Resources.pri', 'p2.Runtime.dll', 'p2.Resources.pri', 'githux-wasdk.manifest') {
            Assert-True (Test-Path -LiteralPath (Join-Path $debugTarget "debug/$name")) "cargo run prepares $name beside the executable"
        }
        Assert-True (Test-Path -LiteralPath (Join-Path $debugTarget 'windows-appsdk')) 'the package cache follows the target directory'
        Assert-Match $result.Output 'Windows App SDK runtime ready' 'the first run reports the preparation'
    }

    Test-Case 'cargo run from a subdirectory keeps cwd and still finds the repository runner' {
        # Remove one payload file first: repairing it proves the repository runner really
        # ran, because a runner path resolved against the current directory would not exist.
        $repaired = Join-Path $fixture 'target/debug/p2.Resources.pri'
        $expectedContent = Get-Content -LiteralPath $repaired -Raw
        Remove-Item -LiteralPath $repaired -Force
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--', 'from sub', 'a & b') -WorkingDirectory (Join-Path $fixture 'sub dir')
        Assert-Equal 0 $result.ExitCode "cargo run from a subdirectory must succeed`n$($result.Output)"
        $reported = Get-ReportedArguments -Output $result.Output
        Assert-Equal 'from sub|a & b' ($reported -join '|') 'arguments from a subdirectory invocation survive'
        Assert-Equal ([IO.Path]::GetFullPath((Join-Path $fixture 'sub dir'))) (Get-ReportedWorkingDirectory -Output $result.Output) 'the working directory is the subdirectory'
        Assert-Match $result.Output 'runtime ready' 'the runner prepares the runtime from a subdirectory invocation'
        Assert-True (Test-Path -LiteralPath $repaired) 'the runtime file is restored beside the executable, not relative to the cwd'
        Assert-Equal $expectedContent (Get-Content -LiteralPath $repaired -Raw) 'the restored file has the package content'
    }

    Test-Case 'cargo run --release prepares the release output directory' {
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--release', '--', 'release arg')
        Assert-Equal 0 $result.ExitCode "cargo run --release must succeed`n$($result.Output)"
        Assert-Equal 'release arg' ((Get-ReportedArguments -Output $result.Output) -join '|') 'release argument survives'
        $releaseTarget = Join-Path $fixture 'target/release'
        foreach ($name in 'p1.Runtime.dll', 'p2.Runtime.dll', 'githux-wasdk.manifest') {
            Assert-True (Test-Path -LiteralPath (Join-Path $releaseTarget $name)) "release run prepares $name"
        }
    }

    Test-Case 'cargo run --target places the runtime under the triple directory' {
        $triple = 'x86_64-pc-windows-msvc'
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--target', $triple, '--', 'triple arg')
        Assert-Equal 0 $result.ExitCode "cargo run --target must succeed`n$($result.Output)"
        Assert-Equal 'triple arg' ((Get-ReportedArguments -Output $result.Output) -join '|') 'explicit-target argument survives'
        $tripleTarget = Join-Path $fixture "target/$triple"
        Assert-True (Test-Path -LiteralPath (Join-Path $tripleTarget 'debug/p1.Runtime.dll')) 'runtime lands next to the triple-specific executable'
        Assert-True (Test-Path -LiteralPath (Join-Path $tripleTarget 'windows-appsdk')) 'the cache follows the triple-specific target directory'
        Assert-True (Test-Path -LiteralPath (Join-Path $tripleTarget 'debug/githux-wasdk.manifest')) 'the activation manifest follows the triple-specific output'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $fixture 'target/debug/x86_64-pc-windows-msvc'))) 'the triple build never writes into the plain target directory'
    }

    Test-Case 'a custom --target-dir with spaces and Unicode works and stays isolated' {
        $customTarget = Join-Path $work 'custom target ünï'
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--target-dir', $customTarget, '--', 'custom dir arg', 'a&b')
        Assert-Equal 0 $result.ExitCode "cargo run --target-dir must succeed`n$($result.Output)"
        Assert-Equal 'custom dir arg|a&b' ((Get-ReportedArguments -Output $result.Output) -join '|') 'custom target-dir arguments survive'
        foreach ($name in 'debug/p1.Runtime.dll', 'debug/githux-wasdk.manifest', 'windows-appsdk') {
            Assert-True (Test-Path -LiteralPath (Join-Path $customTarget $name)) "the custom target directory receives $name"
        }
        $fixtureCache = Join-Path $fixture 'target/windows-appsdk'
        $customCache = Join-Path $customTarget 'windows-appsdk'
        Assert-True (-not [string]::Equals($fixtureCache, $customCache, [StringComparison]::OrdinalIgnoreCase)) 'caches are not shared between target directories'
    }

    Test-Case 'the application exit code reaches the caller unchanged' {
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--', 'exit probe') -Environment @{ GITHUX_TEST_EXIT = '7' }
        Assert-Equal 7 $result.ExitCode 'a non-zero application exit code is propagated'
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--', 'exit probe') -Environment @{ GITHUX_TEST_EXIT = '0' }
        Assert-Equal 0 $result.ExitCode 'a zero exit code is propagated'
    }

    Test-Case 'cargo test runs the harness without preparing the GUI runtime' {
        $testTarget = Join-Path $work 'test target'
        $result = Invoke-Cargo -Arguments @('test', '--quiet', '--target-dir', $testTarget) -Environment @{ GITHUX_TEST_CANARY = 'yes' }
        Assert-Equal 0 $result.ExitCode "cargo test must pass through the runner`n$($result.Output)"
        Assert-Match $result.Output '1 passed' 'the test harness ran and reported its result'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $testTarget 'debug/p1.Runtime.dll'))) 'no runtime is prepared for the test harness'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $testTarget 'debug/githux-wasdk.manifest'))) 'no activation manifest is prepared for the test harness'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $testTarget 'windows-appsdk'))) 'a test run does not download or cache SDK packages'
    }

    Test-Case 'other binaries in the same crate are run without runtime preparation' {
        $otherTarget = Join-Path $work 'other target'
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--bin', 'other-tool', '--target-dir', $otherTarget)
        Assert-Equal 0 $result.ExitCode "cargo run --bin other-tool must succeed`n$($result.Output)"
        Assert-Match $result.Output 'OTHER_RAN' 'the other binary actually ran'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $otherTarget 'debug/p1.Runtime.dll'))) 'unrelated binaries do not receive the SDK runtime'
    }

    Test-Case 'a preparation failure stops the launch with a non-zero exit code' {
        $badFeedDirectory = Join-Path $work 'runner-bad-feed'
        New-Item -ItemType Directory -Path $badFeedDirectory -Force | Out-Null
        $packages = @()
        foreach ($package in $fixtureFeed.Packages) {
            $name = "$($package.Id).$($package.Version).nupkg"
            Copy-Item -LiteralPath (Join-Path $fixtureFeed.Directory $name) -Destination (Join-Path $badFeedDirectory $name)
            $packages += [pscustomobject]@{ Id = $package.Id; Version = $package.Version; Sha256 = ('1' * 64) }
        }
        $badManifest = Join-Path $badFeedDirectory 'packages.json'
        [IO.File]::WriteAllText($badManifest, (ConvertTo-Json -InputObject @($packages) -Depth 4))
        $failureTarget = Join-Path $work 'failure target'
        $result = Invoke-Cargo -Arguments @('run', '--quiet', '--target-dir', $failureTarget, '--', 'never') -Environment @{ GITHUX_WINDOWS_RUNTIME_PACKAGES = $badManifest }
        Assert-True ($result.ExitCode -ne 0) 'the launch fails instead of falling back to a window without the runtime'
        Assert-Match $result.Output 'Windows App SDK runtime' 'the error is actionable and names the runtime'
        Assert-Match $result.Output 'docs/windows-runtime.md' 'the error points at the documentation'
        Assert-True (-not ($result.Output -match 'ARG\|')) 'the application is not started after a preparation failure'
    }

    Remove-Item Env:GITHUX_WINDOWS_RUNTIME_SOURCE -ErrorAction SilentlyContinue
    Remove-Item Env:GITHUX_WINDOWS_RUNTIME_PACKAGES -ErrorAction SilentlyContinue
}

try {
    if ($failures.Count -gt 0) {
        Write-Host ''
        Write-Host "FAILED: $($failures.Count) of $cases case(s)"
        $failures | ForEach-Object { Write-Host "  - $_" }
        exit 1
    }
    Write-Host ''
    Write-Host "PASSED: $cases case(s)"
    exit 0
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
