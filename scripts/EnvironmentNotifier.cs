using System;
using System.Runtime.InteropServices;

public static class EnvironmentNotifier
{
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
    private static extern IntPtr SendMessageTimeout(
        IntPtr hWnd,
        uint msg,
        UIntPtr wParam,
        string lParam,
        uint fuFlags,
        uint uTimeout,
        out UIntPtr result);

    public static void Broadcast()
    {
        UIntPtr result;
        SendMessageTimeout(new IntPtr(0xffff), 0x1A, UIntPtr.Zero, "Environment", 2, 5000, out result);
    }
}
