//! `uiup`: raise the desktop shell processes (explorer, dwm, ...) to HIGH
//! priority. Not elevated → re-runs itself elevated (UAC prompt, own window).

use super::native;

const TARGETS: [&str; 8] = [
    "explorer",
    "dwm",
    "sihost",
    "ShellExperienceHost",
    "StartMenuExperienceHost",
    "StartMenu",
    "SearchApp",
    "audiodg",
];
const SELF_ELEVATED: &str = "--self-elevated";

/// PowerShell's ProcessPriorityClass names, as the old uiup printed them.
pub fn class_name(class: u32) -> String {
    match class {
        0x40 => "Idle".to_string(),
        0x4000 => "BelowNormal".to_string(),
        0x20 => "Normal".to_string(),
        0x8000 => "AboveNormal".to_string(),
        0x80 => "High".to_string(),
        0x100 => "RealTime".to_string(),
        other => format!("0x{other:x}"),
    }
}

/// Rows `[Name, Id, Old, New]` for every target, in TARGETS order.
pub fn rows(
    procs: &[(String, u32)],
    raise: &mut dyn FnMut(u32) -> (Result<u32, u32>, Result<(), u32>),
) -> Vec<[String; 4]> {
    let mut out = Vec::new();
    for name in TARGETS {
        let image = format!("{name}.exe");
        let pids: Vec<u32> = procs
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(&image))
            .map(|&(_, pid)| pid)
            .collect();
        if pids.is_empty() {
            out.push([name.into(), "-".into(), "-".into(), "not running".into()]);
            continue;
        }
        for pid in pids {
            let (old, set) = raise(pid);
            let old = old.map(class_name).unwrap_or_else(|_| "-".into());
            let new = match set {
                Ok(()) => "High".to_string(),
                Err(code) => format!("FAILED: {}", native::os_message(code)),
            };
            out.push([name.into(), pid.to_string(), old, new]);
        }
    }
    out
}

/// Left-aligned columns with a dashed header rule (Format-Table style).
pub fn format_table(rows: &[[String; 4]]) -> String {
    let header = ["Name", "Id", "Old", "New"];
    let mut width = header.map(str::len);
    for r in rows {
        for (w, cell) in width.iter_mut().zip(r) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let line = |cells: [&str; 4]| -> String {
        let s: Vec<String> = cells
            .iter()
            .zip(width)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        s.join(" ").trim_end().to_string()
    };
    let mut out = vec![
        line(header),
        line(width.map(|w| "-".repeat(w)).each_ref().map(String::as_str)),
    ];
    for r in rows {
        out.push(line(r.each_ref().map(String::as_str)));
    }
    out.join("\n")
}

pub fn main(args: &[String]) -> i32 {
    let self_elevated = args.iter().any(|a| a == SELF_ELEVATED);
    if !native::is_elevated() {
        println!(
            "Not elevated - requesting admin rights (dwm/sihost run under a different account)..."
        );
        let exe = match std::env::current_exe() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                eprintln!("Elevation was cancelled or failed: {e}");
                return 1;
            }
        };
        return match native::run_elevated(&exe, SELF_ELEVATED) {
            Ok(code) => code as i32,
            Err(code) => {
                eprintln!(
                    "Elevation was cancelled or failed: {}",
                    native::os_message(code)
                );
                1
            }
        };
    }
    println!(
        "{}",
        format_table(&rows(&native::processes(), &mut native::raise_to_high))
    );
    if self_elevated {
        println!();
        println!("Done. Press Enter to close...");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cover_every_target_and_report_each_outcome() {
        let procs = vec![
            ("Explorer.EXE".to_string(), 10),
            ("dwm.exe".to_string(), 20),
            ("other.exe".to_string(), 30),
        ];
        let mut raise = |pid: u32| match pid {
            10 => (Ok(0x20), Ok(())),
            _ => (Ok(0x80), Err(5)),
        };
        let r = rows(&procs, &mut raise);
        assert_eq!(r.len(), TARGETS.len());
        assert_eq!(r[0], ["explorer", "10", "Normal", "High"].map(String::from));
        assert_eq!(r[1][..3], ["dwm", "20", "High"].map(String::from));
        assert!(r[1][3].starts_with("FAILED: "), "{}", r[1][3]);
        assert_eq!(r[2], ["sihost", "-", "-", "not running"].map(String::from));
    }

    #[test]
    fn table_is_aligned() {
        let t = format_table(&[[
            "explorer".into(),
            "10".into(),
            "Normal".into(),
            "High".into(),
        ]]);
        assert_eq!(
            t,
            "Name     Id Old    New\n-------- -- ------ ----\nexplorer 10 Normal High"
        );
    }

    #[test]
    fn class_names_match_powershell() {
        assert_eq!(class_name(0x20), "Normal");
        assert_eq!(class_name(0x8000), "AboveNormal");
        assert_eq!(class_name(0x100), "RealTime");
        assert_eq!(class_name(0x1234), "0x1234");
    }
}
