Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$proc = Get-Process -Id 25880 -ErrorAction SilentlyContinue
if (-not $proc) { Write-Host "Process not found"; exit 1 }
$hwnd = $proc.MainWindowHandle

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI2 {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint cButtons, uint dwExtraInfo);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function Screenshot($name) {
    $rect = New-Object WinAPI2+RECT
    [WinAPI2]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bmp.Size)
    $path = "$env:TEMP\$name.png"
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Host "Saved: $path"
    return $path
}

function ClickAt($screenX, $screenY) {
    Write-Host "  Click at ($screenX, $screenY)"
    [WinAPI2]::SetCursorPos($screenX, $screenY) | Out-Null
    Start-Sleep -Milliseconds 200
    [WinAPI2]::mouse_event(0x0002, 0, 0, 0, 0) | Out-Null
    Start-Sleep -Milliseconds 80
    [WinAPI2]::mouse_event(0x0004, 0, 0, 0, 0) | Out-Null
    Start-Sleep -Milliseconds 400
}

[WinAPI2]::ShowWindowAsync($hwnd, 9) | Out-Null
Start-Sleep -Milliseconds 500
[WinAPI2]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 800

$rect = New-Object WinAPI2+RECT
[WinAPI2]::GetWindowRect($hwnd, [ref]$rect) | Out-Null
$wx = $rect.Left
$wy = $rect.Top
$ww = $rect.Right - $rect.Left
$wh = $rect.Bottom - $rect.Top
Write-Host "Window: ${ww}x${wh} at ($wx, $wy)"

Screenshot "t2-step0-initial"

# Try multiple positions for the "Codex 助手" button
$targets = @(
    @{ name = "topbar-850"; x = $wx + 850; y = $wy + 20 },
    @{ name = "topbar-800"; x = $wx + 800; y = $wy + 20 },
    @{ name = "topbar-750"; x = $wx + 750; y = $wy + 20 },
    @{ name = "floating-700-350"; x = $wx + 700; y = $wy + 350 }
)

foreach ($t in $targets) {
    Write-Host "Trying $($t.name)..."
    ClickAt $t.x $t.y
    Start-Sleep -Milliseconds 800
    Screenshot "t2-try-$($t.name)"
}

Write-Host "Done trying positions"
