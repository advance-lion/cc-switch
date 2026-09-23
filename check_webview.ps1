$pids = @()
Get-CimInstance Win32_Process | ForEach-Object {
    if ($_.CommandLine -match 'msedgewebview|cc-switch|codex') {
        $cmd = $_.CommandLine
        if ($cmd.Length -gt 200) { $cmd = $cmd.Substring(0, 200) }
        Write-Output "PID: $($_.ProcessId) | Name: $($_.Name) | Cmd: $cmd"
    }
}
