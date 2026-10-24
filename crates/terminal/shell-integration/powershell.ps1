# Load after the user's profile. PSReadLine remains the line editor.
if (!(Test-Path Function:PSConsoleHostReadLine)) { return }
if ($global:AlacrtermIntegrated) { return }
$global:AlacrtermIntegrated = $true
$global:AlacrtermOriginalPrompt = (Get-Item Function:prompt).ScriptBlock
function global:prompt {
    $ok = $?
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
