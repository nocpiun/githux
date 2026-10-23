# Regenerate the app-local activation manifest from the pinned SDK packages.
[CmdletBinding()]
param([switch] $Check)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$packages = @('microsoft.windowsappsdk.foundation.1.8.260803002', 'microsoft.windowsappsdk.interactiveexperiences.1.8.260708001')
$manifest = [Text.StringBuilder]::new()
[void]$manifest.AppendLine('<?xml version="1.0" encoding="utf-8"?>')
[void]$manifest.AppendLine('<!-- Generated from the pinned Windows App SDK package.appxfragment files. -->')
[void]$manifest.AppendLine('<assembly manifestVersion="1.0" xmlns="urn:schemas-microsoft-com:asm.v1" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3" xmlns:winrtv1="urn:schemas-microsoft-com:winrt.v1">')
foreach ($package in $packages) {
    $fragment = Join-Path $repoRoot "target/windows-appsdk/$package/runtimes-framework/package.appxfragment"
    if (-not (Test-Path -LiteralPath $fragment)) { throw 'Run prepare-windows-runtime.ps1 first.' }
    [xml]$xml = Get-Content -LiteralPath $fragment -Raw
    foreach ($server in $xml.SelectNodes('//*[local-name()="InProcessServer"]')) {
        $dll = $server.SelectSingleNode('*[local-name()="Path"]').InnerText
        [void]$manifest.AppendLine("  <asmv3:file name=`"$dll`">")
        foreach ($class in $server.SelectNodes('*[local-name()="ActivatableClass"]')) {
            [void]$manifest.AppendLine("    <winrtv1:activatableClass name=`"$($class.ActivatableClassId)`" threadingModel=`"$($class.ThreadingModel)`"/>")
        }
        [void]$manifest.AppendLine('  </asmv3:file>')
    }
}
[void]$manifest.AppendLine('</assembly>')
$path = Join-Path $repoRoot 'assets/windows/app.manifest'
if ($Check) {
    if ((Get-Content -LiteralPath $path -Raw) -cne $manifest.ToString()) { throw 'Generated Windows manifest differs.' }
    Write-Output 'Windows activation manifest matches pinned metadata.'
} else {
    [IO.File]::WriteAllText($path, $manifest.ToString())
}
