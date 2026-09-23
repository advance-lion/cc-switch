Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$proc = Get-Process -Id 25880 -ErrorAction SilentlyContinue
if (-not $proc) { Write-Host "Process not found"; exit 1 }
$hwnd = $proc.MainWindowHandle

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint cButtons, uint dwExtraInfo);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function TakeScreenshot($name) {
    $rect = New-Object WinAPI+RECT
    [WinAPI]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bmp.Size)
    $path = "$env:TEMP\$name.png"
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose()
    $bmp.Dispose()
    Write-Host "Screenshot: $path ($w x $h at $($rect.Left),$($rect.Top))"
    return $path
}

function ClickAt($x, $y) {
    [WinAPI]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 200
    # Left button down
    [WinAPI]::mouse_event(0x0002, 0, 0, 0, 0) | Out-Null
    Start-Sleep -Milliseconds 100
    # Left button up
    [WinAPI]::mouse_event(0x0004, 0, 0, 0, 0) | Out-Null
    Start-Sleep -Milliseconds 300
}

# Bring window to foreground
[WinAPI]::ShowWindowAsync($hwnd, 9) | Out-Null
Start-Sleep -Milliseconds 500
[WinAPI]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 800

# Get window position
$rect = New-Object WinAPI+RECT
[WinAPI]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$left = $rect.Left
$top = $rect.Top
Write-Host "Window at ($left, $top), size $($rect.Right - $left)x$($rect.Bottom - $top)"

# Take before screenshot
TakeScreenshot "tauri-step0-before"

# The left sidebar with buttons. With 915px width and Overlay title bar,
# the sidebar is about 200px wide. The "Codex 助手" button should be
# at approximately X=50, Y=430 from window top-left.
# Click on the Codex Assistant button
$btnX = $left + 50
$btnY = $top + 430
Write-Host "Clicking at ($btnX, $btnY) for Codex Assistant button"
ClickAt $btnX $btnY
Start-Sleep -Milliseconds 1000
TakeScreenshot "tauri-step1-after-click"

# If a "打开 Codex 助手" (Open Codex Assistant) button appears, click it
# It should be at approximately X=450, Y=300
$openX = $left + 450
$openY = $top + 300
Write-Host "Clicking at ($openX, $openY) for Open button"
ClickAt $openX $openY
Start-Sleep -Milliseconds 1500
TakeScreenshot "tauri-step2-assistant-open"

# Now type in the input field
# The input field should be at the bottom of the assistant panel
# Click on it first
$inputX = $left + 500
$inputY = $top + 550
Write-Host "Clicking at ($inputX, $inputY) for input field"
ClickAt $inputX $inputY
Start-Sleep -Milliseconds 500

# Type "你好" using SendKeys
Write-Host "Typing message..."
[System.Windows.Forms.SendKeys]::SendWait("{ENTER}")
Start-Sleep -Milliseconds 200

# Use clipboard to paste Chinese text
Set-Clipboard "你好"
Start-Sleep -Milliseconds 200
[System.Windows.Forms.SendKeys]::SendWait("^v")
Start-Sleep -Milliseconds 500
TakeScreenshot "tauri-step3-typed"

# Record start time and press Enter to send
$startTime = Get-Date
Write-Host "Send time: $($startTime.ToString('HH:mm:ss.fff'))"
[System.Windows.Forms.SendKeys]::SendWait("{ENTER}")

# Wait and take screenshots at intervals
Start-Sleep -Milliseconds 3000
TakeScreenshot "tauri-step4-3s"

Start-Sleep -Milliseconds 3000
TakeScreenshot "tauri-step5-6s"

Start-Sleep -Milliseconds 3000
TakeScreenshot "tauri-step6-9s"

Start-Sleep -Milliseconds 3000
TakeScreenshot "tauri-step7-12s"

$endTime = Get-Date
$elapsed = ($endTime - $startTime).TotalSeconds
Write-Host "Total elapsed: $([math]::Round($elapsed, 2))s"
Write-Host "End time: $($endTime.ToString('HH:mm:ss.fff'))"
