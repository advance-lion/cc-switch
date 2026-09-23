[Console]::InputEncoding = [System.Text.Encoding]::UTF8
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

 = if (.Count -ge 1) { [0] } else { 'hello' }
Write-Host  Message: 

 = Get-Date
 =  | codex exec --json -s danger-full-access -c 'approval_policy=never' --skip-git-repo-check -C . - 2>&1 | Out-String
 = Get-Date
 = ( - ).TotalSeconds
Write-Host Elapsed: 0 s

 =  -split [char]10 | Where-Object { .Trim() -ne '' }
Write-Host Lines: 0 
Write-Host '--- Last 3 ---'
foreach ( in [-3..-1]) {
    Write-Host 
}
