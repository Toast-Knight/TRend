param(
    [Parameter(Mandatory = $true)]
    [string]$Media,

    [string]$Bridge,

    [ValidateRange(1, 65535)]
    [int]$Width = 15,

    [ValidateRange(1, 65535)]
    [int]$Height = 15,

    [int]$RunMilliseconds = 0
)

$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Bridge) {
    $packagedBridge = Join-Path $repository 'bin\trend-python-tui-bridge.exe'
    $Bridge = if (Test-Path -LiteralPath $packagedBridge) {
        $packagedBridge
    } else {
        Join-Path $repository 'target\release\trend-python-tui-bridge.exe'
    }
}

function Read-BridgeLine([System.IO.Stream]$Stream) {
    $bytes = New-Object System.Collections.Generic.List[byte]
    while ($true) {
        $value = $Stream.ReadByte()
        if ($value -lt 0) {
            throw 'TRend bridge closed unexpectedly.'
        }
        if ($value -eq 10) {
            return [Text.Encoding]::UTF8.GetString($bytes.ToArray()).TrimEnd("`r")
        }
        $bytes.Add([byte]$value)
    }
}

function Read-BridgeBytes([System.IO.Stream]$Stream, [int]$Length) {
    [byte[]]$bytes = New-Object byte[] $Length
    $offset = 0
    while ($offset -lt $Length) {
        $count = $Stream.Read($bytes, $offset, $Length - $offset)
        if ($count -eq 0) {
            throw 'TRend bridge closed during an ANSI frame.'
        }
        $offset += $count
    }
    return $bytes
}

function Write-BridgeLine([System.IO.Stream]$Stream, [string]$Value) {
    $bytes = [Text.Encoding]::ASCII.GetBytes($Value + "`n")
    $Stream.Write($bytes, 0, $bytes.Length)
    $Stream.Flush()
}

if (-not (Test-Path -LiteralPath $Media -PathType Leaf)) {
    throw "Media file not found: $Media"
}
if (-not (Test-Path -LiteralPath $Bridge -PathType Leaf)) {
    throw "TRend bridge not found: $Bridge. Run cargo build -p trend-python-tui-bridge --release."
}

$startInfo = New-Object System.Diagnostics.ProcessStartInfo
$startInfo.FileName = (Resolve-Path -LiteralPath $Bridge).Path
$startInfo.Arguments = '"' + (Resolve-Path -LiteralPath $Media).Path + '"'
$startInfo.UseShellExecute = $false
$startInfo.RedirectStandardInput = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$startInfo.CreateNoWindow = $true

$originalInputEncoding = [Console]::InputEncoding
[Console]::InputEncoding = [Text.Encoding]::ASCII
$process = New-Object System.Diagnostics.Process
$process.StartInfo = $startInfo
$null = $process.Start()
$output = [Console]::OpenStandardOutput()
$escape = [char]27

try {
    Write-BridgeLine $process.StandardInput.BaseStream 'META'
    $metadata = Read-BridgeLine $process.StandardOutput.BaseStream
    $fields = $metadata -split ' '
    if ($fields.Count -ne 9 -or $fields[0] -ne 'META_OK') {
        throw "Unexpected metadata response: $metadata"
    }

    $durationNanoseconds = [int64]$fields[4]
    $clipMilliseconds = if ($durationNanoseconds -gt 0) {
        [Math]::Max(1, [Math]::Ceiling($durationNanoseconds / 1000000.0))
    } else {
        1
    }

    $hideCursor = [Text.Encoding]::ASCII.GetBytes("$escape[?25l")
    $output.Write($hideCursor, 0, $hideCursor.Length)
    $clock = [Diagnostics.Stopwatch]::StartNew()

    while ($RunMilliseconds -le 0 -or $clock.ElapsedMilliseconds -lt $RunMilliseconds) {
        try { $terminalWidth = [Console]::WindowWidth } catch { $terminalWidth = 80 }
        $column = [Math]::Max(1, $terminalWidth - $Width + 1)
        $command = "FRAME $Width $Height 1 $column quality truecolor unicode $($clock.ElapsedMilliseconds) clip 0 $clipMilliseconds loop"
        Write-BridgeLine $process.StandardInput.BaseStream $command
        $header = Read-BridgeLine $process.StandardOutput.BaseStream

        if ($header -eq 'WAIT') {
            Start-Sleep -Milliseconds 16
            continue
        }
        if ($header.StartsWith('ERR ')) {
            throw $header.Substring(4)
        }

        $response = $header -split ' '
        if ($response.Count -ne 9 -or $response[0] -ne 'OK') {
            throw "Unexpected frame response: $header"
        }

        $payloadLength = [int]$response[1]
        if ($payloadLength -gt 0) {
            [byte[]]$payload = Read-BridgeBytes $process.StandardOutput.BaseStream $payloadLength
            $output.Write($payload, 0, $payload.Length)
            $output.Flush()
        }
        Start-Sleep -Milliseconds 33
    }
} finally {
    if (-not $process.HasExited) {
        try { Write-BridgeLine $process.StandardInput.BaseStream 'QUIT' } catch {}
        if (-not $process.WaitForExit(1000)) {
            $process.Kill()
        }
    }
    $restore = [Text.Encoding]::ASCII.GetBytes("$escape[0m$escape[?25h")
    $output.Write($restore, 0, $restore.Length)
    $output.Flush()
    $process.Dispose()
    [Console]::InputEncoding = $originalInputEncoding
}