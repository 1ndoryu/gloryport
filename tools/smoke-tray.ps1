# Smoke funcional de bandeja (Windows): lanza GLORYPORT, hace clic fisico sobre el
# icono con SendInput, verifica el popup, captura una imagen y cierra con 2. clic.
# Uso: powershell -NoProfile -ExecutionPolicy Bypass -File tools\smoke-tray.ps1

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$releaseDir = if ($env:CARGO_TARGET_DIR) {
    Join-Path $env:CARGO_TARGET_DIR 'release'
} else {
    Join-Path $repo 'target\release'
}
$exe = Join-Path $releaseDir 'gloryport.exe'
if (-not (Test-Path $exe)) { throw "No existe el binario release: $exe" }

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Win32Smoke {
    [DllImport("user32.dll", SetLastError=true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public MOUSEINPUT mi; }
    public static void Click(int x, int y, int sw, int sh) {
        int nx = (int)(x * 65535L / (sw - 1));
        int ny = (int)(y * 65535L / (sh - 1));
        SendInput(1, new INPUT[] { new INPUT { type = 0, mi = new MOUSEINPUT { dx = nx, dy = ny, dwFlags = 0x8001 } } }, Marshal.SizeOf(typeof(INPUT)));
        SendInput(1, new INPUT[] { new INPUT { type = 0, mi = new MOUSEINPUT { dx = nx, dy = ny, dwFlags = 0x8002 } } }, Marshal.SizeOf(typeof(INPUT)));
        SendInput(1, new INPUT[] { new INPUT { type = 0, mi = new MOUSEINPUT { dx = nx, dy = ny, dwFlags = 0x8004 } } }, Marshal.SizeOf(typeof(INPUT)));
    }
}
public static class Win32PopupProbe {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    public static int[] Probe() {
        IntPtr h = FindWindowW("GloryPortPopupWnd", null);
        if (h == IntPtr.Zero) return null;
        RECT r;
        GetWindowRect(h, out r);
        return new int[] { h.ToInt32(), r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top };
    }
}
"@

function Find-Popup {
    $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ClassNameProperty, 'GloryPortPopupWnd')
    $found = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if (-not $found) { return $null }
    $rc = $found.Current.BoundingRectangle
    return @($found, [int]$rc.X, [int]$rc.Y, [int]$rc.Width, [int]$rc.Height)
}

function Get-PopupProbe {
    return [Win32PopupProbe]::Probe()
}

Get-Process gloryport -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
$proc = Start-Process -FilePath $exe -ArgumentList 'tray' -WindowStyle Hidden -PassThru

try {
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $name = 'GLORYPORT ' + [char]0x2014 + ' puertos TCP en escucha'
    $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $name)
    $icon = $null
    for ($i = 0; $i -lt 40 -and -not $icon; $i++) {
        $icon = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
        if (-not $icon) { Start-Sleep -Milliseconds 250 }
    }
    if (-not $icon) { throw 'Icono GLORYPORT no encontrado en la bandeja' }

    $r = $icon.Current.BoundingRectangle
    $cx = [int]($r.X + $r.Width / 2); $cy = [int]($r.Y + $r.Height / 2)
    $sw = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width
    $sh = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height
    Write-Host "Icono en $cx,$cy (rect $($r.X),$($r.Y),$($r.Width)x$($r.Height))"

    $t0 = [DateTime]::UtcNow
    [Win32Smoke]::Click($cx, $cy, $sw, $sh)
    $popup = $null
    for ($i = 0; $i -lt 20 -and -not $popup; $i++) { Start-Sleep -Milliseconds 50; $popup = Find-Popup }
    $ms = ([DateTime]::UtcNow - $t0).TotalMilliseconds
    if (-not $popup) { throw 'Popup no visible tras el clic' }
    Write-Host "Popup visible en $([math]::Round($ms)) ms - $($popup[3])x$($popup[4]) px"

    Start-Sleep -Milliseconds 250
    $bmp = New-Object System.Drawing.Bitmap($popup[3], $popup[4])
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($popup[1], $popup[2], 0, 0, $bmp.Size)
    $out = Join-Path $repo 'assets\popup.png'
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Write-Host "Captura guardada: $out"

    Start-Sleep -Milliseconds 200
    $before = Get-PopupProbe
    $overlap = if ($before -and $cx -ge $before[1] -and $cx -lt $before[1] + $before[3] -and $cy -ge $before[2] -and $cy -lt $before[2] + $before[4]) { 'SI' } else { 'NO' }
    if ($before) {
        Write-Host ("Popup antes del 2do clic: hwnd=0x{0:X} rect={1},{2},{3}x{4} icono_en_popup=$overlap" -f [int64]$before[0], $before[1], $before[2], $before[3], $before[4])
    }
    [Win32Smoke]::Click($cx, $cy, $sw, $sh)
    Start-Sleep -Milliseconds 250
    $after = Find-Popup
    if ($after) {
        $probe = Get-PopupProbe
        if ($probe) {
            Write-Host ("Diagnostico 250ms: popup hwnd=0x{0:X} rect={1},{2},{3}x{4} (antes 0x{5:X})" -f [int64]$probe[0], $probe[1], $probe[2], $probe[3], $probe[4], [int64]$before[0])
        } else {
            Write-Host 'Diagnostico 250ms: popup UIA encontrado pero FindWindowW no lo ve'
        }
        Start-Sleep -Milliseconds 600
        $after = Find-Popup
        $probe2 = Get-PopupProbe
        if ($probe2) {
            Write-Host ("Diagnostico 850ms: popup hwnd=0x{0:X} rect={1},{2},{3}x{4}" -f [int64]$probe2[0], $probe2[1], $probe2[2], $probe2[3], $probe2[4])
        } else {
            Write-Host 'Diagnostico 850ms: popup UIA encontrado pero FindWindowW no lo ve'
        }
    }
    if ($after) { throw 'Popup sigue abierto tras el segundo clic' }
    Write-Host 'OK: segundo clic cerro el popup sin reabrirlo'
}
finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
