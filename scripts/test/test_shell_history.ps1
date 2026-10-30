param([Parameter(Mandatory = $true)][string]$HistoryScript,
      [Parameter(Mandatory = $true)][string]$WorkDirectory)
$ErrorActionPreference = 'Stop'
. $HistoryScript
$dir = $WorkDirectory
[void][IO.Directory]::CreateDirectory($dir)
$path = "$dir/custom-history.txt"
[IO.File]::WriteAllText($path, "echo old-one`necho multi```necho fragment`necho 中文`n")
$state = @{}
alacrterm_history_worker $state $path @() 'test-session' 1
if ($state.Entries.Count -ne 2 -or $state.Entries[1] -cne 'echo 中文') { throw "initial failed: $($state | ConvertTo-Json -Depth 5)" }
[IO.File]::AppendAllText($path, "echo new-file`n")
alacrterm_history_worker $state $path @('echo memory-only') 'test-session' 2
if ($state.Entries[-1] -cne 'echo memory-only' -or !$state.Entries.Contains('echo new-file')) { throw 'append failed' }
[IO.File]::WriteAllText($path, '')
alacrterm_history_worker $state $path @('echo memory-only') 'test-session' 3
if ($state.Entries.Count -ne 0) { throw "truncate failed $($state.Entries)" }
[IO.File]::AppendAllText($path, 'echo partial')
alacrterm_history_worker $state $path @('echo memory-only') 'test-session' 4
if ($state.Entries.Count -ne 0) { throw 'partial line imported' }
[IO.File]::AppendAllText($path, "`n")
alacrterm_history_worker $state $path @('echo memory-only') 'test-session' 5
if ($state.Entries[0] -cne 'echo partial') { throw 'partial completion failed' }
# Real asynchronous runspace transport must write the first batch without
# waiting for another prompt or a call to EndInvoke.
$asyncState = @{}
$worker = [PowerShell]::Create()
try {
    [void]$worker.AddScript(${function:alacrterm_history_worker}.ToString()).AddArgument($asyncState).AddArgument($path).AddArgument([string[]]@('echo async')).AddArgument('async-session').AddArgument(1)
    $async = $worker.BeginInvoke()
    if (!$async.AsyncWaitHandle.WaitOne(10000)) { throw 'worker blocked' }
    [void]$worker.EndInvoke($async)
    if (!$asyncState.Entries.Contains('echo async')) { throw 'async state not shared' }
} finally { $worker.Dispose() }

# Bound the cache and ensure whole multiline records are discarded.
$large = [Text.StringBuilder]::new()
for ($i = 0; $i -lt 1100; $i++) { [void]$large.Append("echo item-$i`n") }
[IO.File]::WriteAllText($path, $large.ToString())
$bounded = @{}
alacrterm_history_worker $bounded $path @() 'bounded' 1
if ($bounded.Entries.Count -ne 1000 -or $bounded.Entries[0] -ne 'echo item-100') { throw 'history limit failed' }
Write-Output 'HISTORY_WORKER_OK' 
