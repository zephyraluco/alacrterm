# History workers use a separate runspace. They never evaluate command text.
function global:alacrterm_initialize_history {
    $global:AlacrtermHistorySession = [Guid]::NewGuid().ToString('N')
    $global:AlacrtermHistoryBatch = 0
    $global:AlacrtermHistorySignature = $null
    $global:AlacrtermHistoryWorker = $null
    $global:AlacrtermHistoryState = @{}
    [Console]::Write("$([char]27)]633;History;session;$global:AlacrtermHistorySession$([char]7)")
}
function global:alacrterm_history_worker {
    param($state, [string]$path, [string[]]$memory, [string]$session, [long]$batch)
    $ErrorActionPreference = 'Stop'
    try {
        $reset = !$state.ContainsKey('Entries')
        if ($reset) { $state.Entries = [Collections.Generic.List[string]]::new() }
        $changes = [Collections.Generic.List[string]]::new()
        $valid = { param([string]$command)
            ![string]::IsNullOrWhiteSpace($command) -and
                [Text.Encoding]::UTF8.GetByteCount($command) -le 4096 -and
                ![regex]::IsMatch($command, '\p{Cc}')
        }
        if ($path -and [IO.File]::Exists($path)) {
            $file = [IO.File]::Open($path, 'Open', 'Read', [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
            try {
                $length = $file.Length
                $stamp = [IO.File]::GetLastWriteTimeUtc($path).Ticks
                $sameFile = $state.Path -eq $path -and $state.ContainsKey('Offset')
                $fileReset = !$sameFile -or $length -lt $state.Offset -or
                    ($length -eq $state.Offset -and $stamp -ne $state.Stamp)
                if (!$fileReset -and $state.Anchor) {
                    [void]$file.Seek([Math]::Max(0, $state.Offset - 64), 'Begin')
                    $check = [byte[]]::new([Math]::Min(64, $state.Offset))
                    $read = $file.Read($check, 0, $check.Length)
                    $fileReset = $read -ne $check.Length -or [Convert]::ToBase64String($check) -ne $state.Anchor
                }
                if ($fileReset) {
                    $reset = $true
                    $state.Entries.Clear()
                    $state.SkipMultiline = $false
                }
                $start = if ($fileReset) { [Math]::Max(0, $length - 8MB) } else { [long]$state.Offset }
                # Bound work even after a large external history append.
                if ($length - $start -gt 8MB) { $start = $length - 8MB; $fileReset = $true; $reset = $true; $state.Entries.Clear() }
                [void]$file.Seek($start, 'Begin')
                $buffer = [byte[]]::new($length - $start)
                $read = 0
                while ($read -lt $buffer.Length) {
                    $n = $file.Read($buffer, $read, $buffer.Length - $read)
                    if (!$n) { break }
                    $read += $n
                }
                # Leave an incomplete final line for the next append.
                $lastNewline = $read - 1
                while ($lastNewline -ge 0 -and $buffer[$lastNewline] -ne 10) { $lastNewline-- }
                if ($lastNewline -ge 0) {
                    $text = [Text.Encoding]::UTF8.GetString($buffer, 0, $lastNewline + 1)
                    $lines = $text.Split([char]10)
                    $skipFragment = $fileReset -and $start -gt 0
                    $skipRecord = $skipFragment
                    for ($i = 0; $i -lt $lines.Length - 1; $i++) {
                        $line = $lines[$i].TrimEnd([char]13)
                        if ($skipFragment) { $skipFragment = $false; continue }
                        if ($skipRecord) {
                            if (!$line.EndsWith('`')) { $skipRecord = $false }
                            continue
                        }
                        # PSReadLine escapes embedded newlines as backtick + LF.
                        # Skip the complete multiline record, never its fragments.
                        if ($line.EndsWith('`')) { $state.SkipMultiline = $true; continue }
                        if ($state.SkipMultiline) { $state.SkipMultiline = $false; continue }
                        if (& $valid $line) { $changes.Add($line) }
                    }
                    $state.Offset = $start + $lastNewline + 1
                } else { $state.Offset = $start }
                $state.Path = $path
                $state.Stamp = $stamp
                [void]$file.Seek([Math]::Max(0, $state.Offset - 64), 'Begin')
                $anchor = [byte[]]::new([Math]::Min(64, $state.Offset))
                [void]$file.Read($anchor, 0, $anchor.Length)
                $state.Anchor = [Convert]::ToBase64String($anchor)
            } finally { $file.Dispose() }
        } elseif ($state.Path) {
            $reset = $true
            $state.Entries.Clear()
            $state.Remove('Path')
            $state.Remove('Offset')
        }
        # Use PSReadLine's accepted memory history, not Get-History: the latter
        # can include commands rejected by PSReadLine's AddToHistoryHandler.
        $previousMemory = @($state.Memory)
        $memoryStart = 0
        if ($previousMemory.Count -and $memory.Count) {
            $last = $previousMemory[-1]
            for ($i = $memory.Count - 1; $i -ge 0; $i--) {
                if ($memory[$i] -ceq $last) { $memoryStart = $i + 1; break }
            }
        } elseif ($state.ContainsKey('Memory') -and $previousMemory.Count -and !$memory.Count) {
            $reset = $true
            $state.Entries.Clear()
            $changes.Clear()
        }
        for ($i = $memoryStart; $i -lt $memory.Count; $i++) {
            if (& $valid $memory[$i]) { $changes.Add($memory[$i]) }
        }
        $state.Memory = $memory
        foreach ($command in $changes) {
            [void]$state.Entries.Remove($command)
            $state.Entries.Add($command)
            if ($state.Entries.Count -gt 1000) { $state.Entries.RemoveAt(0) }
        }
        if (!$reset -and !$changes.Count) { return }
        $send = if ($reset) { @($state.Entries.ToArray()) } else { @($changes.ToArray() | Select-Object -Last 1000) }
        $mode = if ($reset) { 'snapshot' } else { 'append' }
        $output = [Text.StringBuilder]::new()
        [void]$output.Append("$([char]27)]633;History;begin;$session;$batch;$mode$([char]7)")
        $count = 0
        foreach ($command in $send) {
            $hex = [BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes($command)).Replace('-', '')
            [void]$output.Append("$([char]27)]633;History;entry;$session;$batch;$count;$hex$([char]7)")
            $count++
        }
        [void]$output.Append("$([char]27)]633;History;end;$session;$batch;$count$([char]7)")
        [Console]::Write($output.ToString())
    } catch {
        # No partial OSC batch is emitted. Retry on a later prompt.
        $state.Clear()
    }
}
function global:alacrterm_sync_history {
    if ($env:ALACRTERM_HISTORY_ENABLED -eq '0') { return }
    if ($global:AlacrtermHistoryWorker) {
        if (!$global:AlacrtermHistoryWorker.Async.IsCompleted) { return }
        try { [void]$global:AlacrtermHistoryWorker.PowerShell.EndInvoke($global:AlacrtermHistoryWorker.Async) } catch {}
        $global:AlacrtermHistoryWorker.PowerShell.Dispose()
        $global:AlacrtermHistoryWorker = $null
    }
    try {
        $path = (Get-PSReadLineOption).HistorySavePath
        $file = Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
        $memory = @([Microsoft.PowerShell.PSConsoleReadLine]::GetHistoryItems() | Select-Object -Last 1000 | ForEach-Object { $_.CommandLine })
        $hash = [Security.Cryptography.SHA256]::Create()
        try { $digest = [Convert]::ToBase64String($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes(($memory -join "`0")))) } finally { $hash.Dispose() }
        $signature = "$path|$($file.Length)|$($file.LastWriteTimeUtc.Ticks)|$digest"
        if ($signature -eq $global:AlacrtermHistorySignature -and $global:AlacrtermHistoryState.ContainsKey('Entries')) { return }
        $global:AlacrtermHistorySignature = $signature
        $global:AlacrtermHistoryBatch++
        $worker = [PowerShell]::Create()
        [void]$worker.AddScript(${function:alacrterm_history_worker}.ToString()).AddArgument($global:AlacrtermHistoryState).AddArgument($path).AddArgument([string[]]$memory).AddArgument($global:AlacrtermHistorySession).AddArgument($global:AlacrtermHistoryBatch)
        $global:AlacrtermHistoryWorker = @{ PowerShell = $worker; Async = $worker.BeginInvoke() }
    } catch {
        # PSReadLine/history can be unavailable; keep the native prompt working.
    }
}
