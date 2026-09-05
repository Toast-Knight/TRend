[CmdletBinding()]
param(
    [string]$AnsiDirectory = (Join-Path $PSScriptRoot '..\target\fidelity-ansi'),
    [string]$FontFamily = 'Cascadia Mono',
    [int]$FontSize = 12,
    [switch]$RecordOnly
)

$ErrorActionPreference = 'Stop'
$artifacts = @(
    'area',
    'experimental-expanded',
    'phase6b-reliable',
    'phase6b-unique-expanded',
    'phase6b-pruned',
    'patterns-area',
    'patterns-phase6b-reliable',
    'patterns-phase6b-unique-expanded',
    'patterns-phase6b-pruned'
)

foreach ($artifact in $artifacts) {
    $path = Join-Path $AnsiDirectory "$artifact.ansi"
    if (-not (Test-Path $path -PathType Leaf)) {
        throw "Missing $path. Run: cargo run --release --example fidelity_baseline -- --save-ansi"
    }
}

$size = $Host.UI.RawUI.WindowSize
$record = [ordered]@{
    recorded_at = (Get-Date).ToString('o')
    terminal_program = $env:TERM_PROGRAM
    color_term = $env:COLORTERM
    declared_font_family = $FontFamily
    declared_font_size = $FontSize
    columns = $size.Width
    rows = $size.Height
    required_content_columns = 80
    required_content_rows = 24
    font_verified_by_script = $false
    artifacts = $artifacts
}
$recordPath = Join-Path $AnsiDirectory 'font-validation.json'
$record | ConvertTo-Json -Depth 3 | Set-Content $recordPath -Encoding utf8

Write-Host "Validation record: $recordPath"
Write-Host "Required profile: truecolor, $FontFamily $FontSize, 80x24 content cells, 100% zoom."
Write-Host 'This script records the declared profile but cannot verify terminal font settings.'
if ($size.Width -ne 80 -or $size.Height -lt 24) {
    Write-Warning "Current terminal is $($size.Width)x$($size.Height); resize before visual scoring."
}
if ($RecordOnly) {
    return
}

foreach ($artifact in $artifacts) {
    Clear-Host
    Write-Host "Artifact: $artifact | Declared font: $FontFamily $FontSize"
    [Console]::Write([System.IO.File]::ReadAllText((Join-Path $AnsiDirectory "$artifact.ansi")))
    [Console]::Write("`e[0m")
    Read-Host 'Press Enter for the next artifact'
}

Clear-Host
Write-Host 'Visual review complete. Record observations beside font-validation.json.'