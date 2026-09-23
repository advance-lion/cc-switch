$msg = if ($args.Count -ge 1) { $args[0] } else { "hello" }
Write-Host "Testing message: $msg"
$start = Get-Date
$output = $msg | codex exec --json -s danger-full-access -c 'approval_policy="never"' --skip-git-repo-check -C . - 2>&1 | Out-String
$end = Get-Date
$elapsed = ($end - $start).TotalSeconds
Write-Host "Elapsed: $([math]::Round($elapsed, 2))s"
$lines = $output -split "`n" | Where-Object { $_.Trim() -ne "" }
Write-Host "Output lines: $($lines.Count)"
Write-Host "--- Last 5 lines ---"
foreach ($line in $lines[-5..-1]) {
    Write-Host $line
}
