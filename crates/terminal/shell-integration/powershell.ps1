# Load after the user's profile. PSReadLine remains the line editor.
if (!(Test-Path Function:PSConsoleHostReadLine)) { return }
if ($global:AlacrtermIntegrated) { return }
$global:AlacrtermIntegrated = $true
$global:AlacrtermOriginalPrompt = (Get-Item Function:prompt).ScriptBlock
function global:prompt {
    $ok = $?
    $signature = "$env:PATH|$((Get-ChildItem Function:).Name -join ',')|$((Get-Alias).Name -join ',')"
    if ($signature -ne $global:AlacrtermInventorySignature -or ([DateTime]::UtcNow - $global:AlacrtermLastRefresh).TotalSeconds -ge 30) {
        alacrterm_refresh_completions
    }
    $alacrtermCwd = $ExecutionContext.SessionState.Path.CurrentLocation.ProviderPath
    if ($alacrtermCwd -match '[\x00-\x1f]') { $alacrtermCwd = '' }
    [Console]::Write("$([char]27)]633;AlacrtermCwd;$alacrtermCwd$([char]7)")
    [Console]::Write("$([char]27)]633;D;$([int](!$ok))$([char]7)$([char]27)]633;A$([char]7)")
    $value = & $global:AlacrtermOriginalPrompt
    return "$value$([char]27)]633;B$([char]7)"
}
if (Test-Path Function:PSConsoleHostReadLine) {
    $global:AlacrtermOriginalReadLine = (Get-Item Function:PSConsoleHostReadLine).ScriptBlock
    function global:PSConsoleHostReadLine {
        $line = & $global:AlacrtermOriginalReadLine
        [Console]::Write("$([char]27)]633;C$([char]7)")
        return $line
    }
}
function global:alacrterm_refresh_completions {
    [Console]::Write("$([char]27)]633;CommandReset$([char]7)")
    Get-Command -CommandType Application,Alias,Function,Cmdlet | ForEach-Object {
        if ($_.Name -cmatch '^[a-zA-Z0-9_.:+-]{1,256}$') {
            [Console]::Write("$([char]27)]633;Command;$($_.Name)$([char]7)")
        }
    }
    $global:AlacrtermInventorySignature = "$env:PATH|$((Get-ChildItem Function:).Name -join ',')|$((Get-Alias).Name -join ',')"
    $global:AlacrtermLastRefresh = [DateTime]::UtcNow
}
alacrterm_refresh_completions
[Console]::Write("$([char]27)]633;AlacrtermShell;powershell$([char]7)")
