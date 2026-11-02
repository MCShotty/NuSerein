# Compare the icon Windows extracts from a packaged executable with the shipped artwork.
param([Parameter(Mandatory = $true, Position = 0)][string]$Executable)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$path = (Resolve-Path -LiteralPath $Executable).Path
$actualIcon = [System.Drawing.Icon]::ExtractAssociatedIcon($path)
if ($null -eq $actualIcon) { throw "No application icon in $path" }
$expectedIcon = $null
$actual = $null
$expected = $null
try {
    $source = Join-Path $PSScriptRoot 'Serein.ico'
    $expectedIcon = [System.Drawing.Icon]::new($source, $actualIcon.Width, $actualIcon.Height)
    $actual = $actualIcon.ToBitmap()
    $expected = $expectedIcon.ToBitmap()
    if ($actual.Size -ne $expected.Size) { throw "Wrong icon size in $path" }
    for ($y = 0; $y -lt $actual.Height; $y++) {
        for ($x = 0; $x -lt $actual.Width; $x++) {
            if ($actual.GetPixel($x, $y).ToArgb() -ne $expected.GetPixel($x, $y).ToArgb()) {
                throw "Packaged icon differs from NuSerein artwork in $path at $x,$y"
            }
        }
    }
    Write-Host "Verified NuSerein icon: $path ($($actual.Width)x$($actual.Height))"
} finally {
    if ($null -ne $actual) { $actual.Dispose() }
    if ($null -ne $expected) { $expected.Dispose() }
    if ($null -ne $expectedIcon) { $expectedIcon.Dispose() }
    $actualIcon.Dispose()
}
