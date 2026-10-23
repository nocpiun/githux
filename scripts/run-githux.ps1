# Cargo Windows target runner: make the app-local Windows App SDK runtime ready next to
# the built executable, then start that executable unchanged.
#
# Invoked from scripts/run-githux.cmd, which is the first (and therefore repo-relative)
# element of the runner configured in .cargo/config.toml. See docs/windows-runtime.md.
#
# Scope: only the githux application binary is prepared. Test, bench and example
# executables (under target/<profile>/deps/) are started as-is, so `cargo test` never
# launches the GUI and never touches the network.
$ErrorActionPreference = 'Stop'

# Marker inserted by the .cmd bridge before cargo's arguments. Everything after it is
# [executable, argument...] in the original order.
$marker = '--githux-runner--'

# Serialize one argument using the Windows command-line rules that the C runtime and
# Rust's std::process use, so the child receives byte-identical argv.
function ConvertTo-ArgvToken {
    param([AllowEmptyString()][string] $Value)
    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') { return $Value }
    $builder = [Text.StringBuilder]::new()
    $null = $builder.Append('"')
    $backslashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') { $backslashes++; continue }
        if ($character -eq '"') {
            $null = $builder.Append('\', $backslashes * 2 + 1)
            $null = $builder.Append('"')
        } else {
            if ($backslashes -gt 0) { $null = $builder.Append('\', $backslashes) }
            $null = $builder.Append($character)
        }
        $backslashes = 0
    }
    # Trailing backslashes must be doubled so they do not escape the closing quote.
    if ($backslashes -gt 0) { $null = $builder.Append('\', $backslashes * 2) }
    $null = $builder.Append('"')
    return $builder.ToString()
}

function Join-ArgvString {
    param([string[]] $Arguments)
    $builder = [Text.StringBuilder]::new()
    foreach ($argument in $Arguments) {
        if ($builder.Length -gt 0) { $null = $builder.Append(' ') }
        $null = $builder.Append((ConvertTo-ArgvToken -Value $argument))
    }
    return $builder.ToString()
}

# Start the target executable with inherited stdio and propagate its exit code exactly.
function Invoke-ChildProcess {
    param(
        [Parameter(Mandatory)][string] $Path,
        [string[]] $Arguments = @()
    )
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Path
    # A single raw command line keeps PowerShell from re-quoting the arguments lossily.
    $startInfo.Arguments = Join-ArgvString -Arguments $Arguments
    $startInfo.UseShellExecute = $false
    $child = [Diagnostics.Process]::Start($startInfo)
    if (-not $child) { throw "Cannot start '$Path'." }
    $child.WaitForExit()
    return [int]$child.ExitCode
}

function Get-RepoRoot {
    # Never rely on the caller's working directory: Cargo may be invoked from a subdirectory.
    return (Split-Path -Parent $PSScriptRoot)
}

function Test-ApplicationBinary {
    # The application lives directly in target/<profile>/<name>.exe; everything Cargo runs
    # for tests, benches and examples lives in a subdirectory of the profile directory.
    param([Parameter(Mandatory)][string] $Path)
    $parent = Split-Path -Leaf (Split-Path -Parent $Path)
    if ($parent -in 'deps', 'examples') { return $false }
    return ([IO.Path]::GetFileNameWithoutExtension($Path) -eq 'githux')
}

function Get-TargetCacheRoot {
    # Keep each worktree and target directory independent, matching the built output:
    #   target/<profile>/x.exe        -> target/windows-appsdk
    #   target/<triple>/<profile>/x.exe -> target/<triple>/windows-appsdk
    param([Parameter(Mandatory)][string] $Path)
    $profileDirectory = Split-Path -Parent $Path
    $targetDirectory = Split-Path -Parent $profileDirectory
    if (-not $targetDirectory) { return (Join-Path (Get-RepoRoot) 'target/windows-appsdk') }
    return (Join-Path $targetDirectory 'windows-appsdk')
}

try {
    $commandLine = [Environment]::GetCommandLineArgs()
    $index = [Array]::IndexOf($commandLine, $marker)
    if ($index -lt 0 -or $index + 1 -ge $commandLine.Count) {
        throw 'The Windows runtime runner did not receive a target executable. Check .cargo/config.toml and scripts/run-githux.cmd.'
    }
    # Cargo may pass a path relative to the invocation directory; resolve it now so every
    # later step uses one unambiguous path while the child keeps the caller's directory.
    $executable = $commandLine[$index + 1]
    if (-not [IO.Path]::IsPathRooted($executable)) {
        $executable = [IO.Path]::GetFullPath((Join-Path (Get-Location).ProviderPath $executable))
    }
    $arguments = @()
    if ($index + 2 -le $commandLine.Count - 1) { $arguments = $commandLine[($index + 2)..($commandLine.Count - 1)] }

    if (Test-ApplicationBinary -Path $executable) {
        . (Join-Path $PSScriptRoot 'windows-runtime-lib.ps1')
        $architecture = Get-GithuxPeArchitecture -Path $executable
        if (-not $architecture) {
            throw "Cannot read the target architecture of '$executable'. Build githux with a Windows MSVC target."
        }
        # Optional mirror/offline overrides. Defaults stay the pinned origin and hashes;
        # normal development runs never set these.
        $prepare = @{
            RepoRoot    = (Get-RepoRoot)
            Destination = (Split-Path -Parent $executable)
            Architecture = $architecture
            CacheRoot   = (Get-TargetCacheRoot -Path $executable)
        }
        if ($env:GITHUX_WINDOWS_RUNTIME_SOURCE) { $prepare.SourceBase = $env:GITHUX_WINDOWS_RUNTIME_SOURCE }
        if ($env:GITHUX_WINDOWS_RUNTIME_PACKAGES) { $prepare.PackageManifest = $env:GITHUX_WINDOWS_RUNTIME_PACKAGES }
        $result = Invoke-GithuxWindowsRuntimePrepare @prepare
        if ($result.Downloaded) {
            Write-Host "githux: downloaded the pinned Windows App SDK runtime ($architecture) into $($result.Cache)"
        }
        if ($result.Written -gt 0) {
            Write-Host "githux: Windows App SDK runtime ready in $($result.Destination) ($($result.Written) file(s) written, $($result.Verified) already up to date)"
        }
    }
} catch {
    [Console]::Error.WriteLine("githux: $($_.Exception.Message)")
    [Console]::Error.WriteLine('githux: the app-local Windows App SDK runtime was not prepared, so the application did not start. See docs/windows-runtime.md.')
    exit 1
}

# Outside the try above: a failure to start the already-prepared executable is not a
# runtime-preparation problem and must not be reported as one.
try {
    exit (Invoke-ChildProcess -Path $executable -Arguments $arguments)
} catch {
    [Console]::Error.WriteLine("githux: $($_.Exception.Message)")
    exit 1
}
