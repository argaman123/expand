# Test harness for the pointer hand-over (Windows side). Moves the real pointer with injected
# relative movements, the same path a physical mouse takes, and reports where it ends up.
#   pointer-probe.ps1 pos                  -> "x,y" in physical pixels
#   pointer-probe.ps1 at X Y               -> place the pointer (no movement events)
#   pointer-probe.ps1 move DX DY [STEPS]   -> move by DX,DY in STEPS relative steps, then "x,y"
#   pointer-probe.ps1 down | up            -> press or release the left button
#   pointer-probe.ps1 keys 1E 30 2E       -> press and release keys by hardware scancode (hex)
#   pointer-probe.ps1 click                -> left click where the pointer is
#   pointer-probe.ps1 screens              -> monitor rectangles and the viewer window rectangle
#   Arguments can also come in $env:PROBE_ARGS ("move -300 0 10"), which avoids PowerShell
#   reading a negative number as a parameter name.
param([string]$cmd = "pos", [int]$a = 0, [int]$b = 0, [int]$steps = 20)
if ($env:PROBE_ARGS) {
    $w = $env:PROBE_ARGS -split ' '
    $cmd = $w[0]
    if ($cmd -notin @('keys', 'chord', 'hold')) {
        if ($w.Count -gt 1) { $a = [int]$w[1] }
        if ($w.Count -gt 2) { $b = [int]$w[2] }
        if ($w.Count -gt 3) { $steps = [int]$w[3] }
    }
}

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Threading;
public static class Probe {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint data, flags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MOUSEINPUT mi; }
    [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, INPUT[] i, int size);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    public static void Aware() { SetProcessDpiAwarenessContext(new IntPtr(-4)); }
    public static string Pos() { POINT p; GetCursorPos(out p); return p.X + "," + p.Y; }
    public static void Button(uint flag) {
        var inp = new INPUT[1];
        inp[0].type = 0; inp[0].mi.flags = flag;
        SendInput(1, inp, Marshal.SizeOf(typeof(INPUT)));
    }
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort vk, scan; public uint flags, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct KINPUT { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KEYBDINPUT ki; [FieldOffset(8)] public MOUSEINPUT pad; }
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, KINPUT[] i, int size);
    /// Presses and releases each hardware scancode in turn (0x1E = A, 0x30 = B, 0x2E = C...).
    public static void Keys(int[] scancodes) {
        foreach (int sc in scancodes) {
            foreach (uint f in new uint[] { 0x0008, 0x0008 | 0x0002 }) {
                var inp = new KINPUT[1];
                inp[0].type = 1; inp[0].ki.scan = (ushort)sc; inp[0].ki.flags = f;
                SendInput(1, inp, Marshal.SizeOf(typeof(KINPUT)));
                Thread.Sleep(30);
            }
        }
    }
    /// Holds all the keys down together, then lets go (E05B = left Windows key).
    public static void Chord(int[] codes) {
        foreach (bool up in new[] { false, true }) {
            var order = up ? System.Linq.Enumerable.Reverse(codes) : codes;
            foreach (int code in order) {
                var inp = new KINPUT[1];
                inp[0].type = 1; inp[0].ki.scan = (ushort)(code & 0xFF);
                inp[0].ki.flags = 0x0008u | ((code & 0xE000) == 0xE000 ? 0x0001u : 0u) | (up ? 0x0002u : 0u);
                SendInput(1, inp, Marshal.SizeOf(typeof(KINPUT)));
                Thread.Sleep(40);
            }
        }
    }
    /// Holds one key (hardware scancode) down for a while, then lets go.
    public static void Hold(int code, int ms) {
        foreach (bool up in new[] { false, true }) {
            var inp = new KINPUT[1];
            inp[0].type = 1; inp[0].ki.scan = (ushort)code; inp[0].ki.flags = 0x0008u | (up ? 0x0002u : 0u);
            SendInput(1, inp, Marshal.SizeOf(typeof(KINPUT)));
            if (!up) Thread.Sleep(ms);
        }
    }
    public static void Click() {
        foreach (uint f in new uint[] { 0x0002, 0x0004 }) {
            var inp = new INPUT[1];
            inp[0].type = 0; inp[0].mi.flags = f;
            SendInput(1, inp, Marshal.SizeOf(typeof(INPUT)));
            Thread.Sleep(40);
        }
    }
    public static void Move(int dx, int dy, int steps) {
        for (int i = 0; i < steps; i++) {
            int sx = dx * (i + 1) / steps - dx * i / steps;
            int sy = dy * (i + 1) / steps - dy * i / steps;
            var inp = new INPUT[1];
            inp[0].type = 0; inp[0].mi.dx = sx; inp[0].mi.dy = sy; inp[0].mi.flags = 0x0001;
            if (SendInput(1, inp, Marshal.SizeOf(typeof(INPUT))) != 1) throw new Exception("SendInput failed: " + Marshal.GetLastWin32Error());
            Thread.Sleep(12);
        }
    }
}
'@
[Probe]::Aware()
switch ($cmd) {
    "pos" { [Probe]::Pos() }
    "at" { [Probe]::SetCursorPos($a, $b) | Out-Null; [Probe]::Pos() }
    "down" { [Probe]::Button(0x0002); Start-Sleep -Milliseconds 150; [Probe]::Pos() }
    "up" { [Probe]::Button(0x0004); Start-Sleep -Milliseconds 150; [Probe]::Pos() }
    "keys" { [Probe]::Keys(@($env:PROBE_ARGS.Split(' ')[1..99] | Where-Object { $_ } | ForEach-Object { [Convert]::ToInt32($_, 16) })); [Probe]::Pos() }
    "hold" { $w = $env:PROBE_ARGS.Split(' '); [Probe]::Hold([Convert]::ToInt32($w[1], 16), [int]$w[2]); [Probe]::Pos() }
    "chord" { [Probe]::Chord(@($env:PROBE_ARGS.Split(' ')[1..99] | Where-Object { $_ } | ForEach-Object { [Convert]::ToInt32($_, 16) })); [Probe]::Pos() }
    "click" { [Probe]::Click(); Start-Sleep -Milliseconds 150; [Probe]::Pos() }
    "move" { [Probe]::Move($a, $b, $steps); Start-Sleep -Milliseconds 150; [Probe]::Pos() }
    "screens" {
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { "{0} {1}" -f $_.DeviceName, $_.Bounds }
        $p = Get-Process Moonlight -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($p) { $r = New-Object Probe+RECT; [Probe]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null; "viewer $($r.L),$($r.T) - $($r.R),$($r.B)" }
    }
}
