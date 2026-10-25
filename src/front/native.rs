//! Native calls used only by the `admin`/`uiup` frontends: token elevation,
//! ShellExecuteEx "runas" (the UAC prompt) and the process list.

use std::mem::size_of;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetPriorityClass, OpenProcess, OpenProcessToken,
    SetPriorityClass, WaitForSingleObject, HIGH_PRIORITY_CLASS, INFINITE,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};

const SW_SHOWNORMAL: i32 = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Windows' message for a Win32 error code.
pub fn os_message(code: u32) -> String {
    std::io::Error::from_raw_os_error(code as i32).to_string()
}

/// True when this process runs with an elevated (administrator) token.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Start `file` elevated via ShellExecuteEx "runas" (UAC consent prompt),
/// wait for it and return its exit code. `Err` is a Win32 error code
/// (1223 = the user cancelled the prompt).
pub fn run_elevated(file: &str, params: &str) -> Result<u32, u32> {
    let verb = wide("runas");
    let file_w = wide(file);
    let params_w = wide(params);
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file_w.as_ptr();
        info.lpParameters = if params.is_empty() {
            std::ptr::null()
        } else {
            params_w.as_ptr()
        };
        info.nShow = SW_SHOWNORMAL;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(GetLastError());
        }
        if info.hProcess.is_null() {
            return Ok(0);
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let ok = GetExitCodeProcess(info.hProcess, &mut code);
        let err = GetLastError();
        CloseHandle(info.hProcess);
        if ok == 0 {
            Err(err)
        } else {
            Ok(code)
        }
    }
}

/// `(image name, pid)` of every running process.
pub fn processes() -> Vec<(String, u32)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            out.push((
                String::from_utf16_lossy(&entry.szExeFile[..len]),
                entry.th32ProcessID,
            ));
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    out
}

/// Raise `pid` to HIGH priority. Returns `(old class, result)`.
pub fn raise_to_high(pid: u32) -> (Result<u32, u32>, Result<(), u32>) {
    unsafe {
        let h = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION,
            0,
            pid,
        );
        if h.is_null() {
            let e = GetLastError();
            return (Err(e), Err(e));
        }
        let old = GetPriorityClass(h);
        let old = if old == 0 {
            Err(GetLastError())
        } else {
            Ok(old)
        };
        let set = if SetPriorityClass(h, HIGH_PRIORITY_CLASS) == 0 {
            Err(GetLastError())
        } else {
            Ok(())
        };
        CloseHandle(h);
        (old, set)
    }
}
