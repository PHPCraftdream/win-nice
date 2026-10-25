//! Job-object limit composition and native struct serialization.
//!
//! `compose` turns a [`Config`] into a [`JobLimits`] value (the single
//! validation choke point), and the `*_info` helpers build the REAL
//! `windows-sys` Win32 structures that `SetInformationJobObject` expects.
//! The structs are never viewed as raw memory (their `#[repr(C)]` padding
//! is not guaranteed initialized): [`extended_bytes`] and
//! [`cpu_rate_bytes`] serialize them field by field into a zeroed buffer.

use crate::win32_err;
use crate::{Config, MemSpec, Sys};

/// Composed job-object limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLimits {
    pub flags: u32,
    pub affinity: usize,
    pub active_process_limit: u32,
    pub priority_class: u32,
    pub job_memory_limit: u64,
    /// Requested CPU cap as a percent 1..=100 (the *100 conversion happens in
    /// the struct builder, `cpu_rate_info`).
    pub cpu_rate: Option<u32>,
}

/// Resolve `MemSpec::Percent` via `GlobalMemoryStatusEx` to an absolute byte
/// count (floor of `total * percent / 100`).
pub fn total_ram_percent_bytes(sys: &dyn Sys, percent: u32) -> Result<u64, String> {
    let total = sys
        .global_memory_status_ex()
        .map_err(|code| win32_err("GlobalMemoryStatusEx", code))?;
    let bytes = (u128::from(total) * u128::from(percent) / 100) as u64;
    if total == 0 || bytes == 0 {
        return Err("wnice: memory limit is too small - rounds to 0 bytes".to_string());
    }
    Ok(bytes)
}

/// Resolve a [`MemSpec`] to absolute bytes.
pub fn resolve_memory(sys: &dyn Sys, mem: MemSpec) -> Result<u64, String> {
    match mem {
        MemSpec::Bytes(b) => Ok(b),
        MemSpec::Percent(p) => total_ram_percent_bytes(sys, p),
    }
}

/// Compose a [`Config`] into [`JobLimits`]. This is the single validation
/// choke point (the CLI parser already validates, but legacy frontends call
/// this too, so keep the checks).
pub fn compose(cfg: &Config, sys: &dyn Sys) -> Result<JobLimits, String> {
    let mut limits = JobLimits {
        flags: crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        affinity: 0,
        active_process_limit: 0,
        priority_class: 0,
        job_memory_limit: 0,
        cpu_rate: None,
    };

    if let Some(n) = cfg.threads {
        let max = sys.logical_processor_count().min(63);
        if n == 0 || n > max {
            return Err(format!(
                "wnice: -t/--threads is out of range (must be 1..={max})"
            ));
        }
        limits.flags |= crate::JOB_OBJECT_LIMIT_AFFINITY;
        limits.affinity = (1usize << n) - 1;
    }

    if let Some(m) = cfg.memory {
        limits.flags |= crate::JOB_OBJECT_LIMIT_JOB_MEMORY;
        limits.job_memory_limit = resolve_memory(sys, m)?;
    }

    if let Some(n) = cfg.procs {
        limits.flags |= crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.active_process_limit = n;
    }

    if let Some(p) = cfg.priority {
        limits.flags |= crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS;
        limits.priority_class = p.to_u32();
    }

    if let Some(p) = cfg.cpu_percent {
        if p == 0 || p > 100 {
            return Err("wnice: -c/--cpu-percent is out of range (must be 1..=100)".to_string());
        }
        limits.cpu_rate = Some(p);
    }

    Ok(limits)
}

fn put(buf: &mut [u8], off: usize, bytes: &[u8]) {
    buf[off..off + bytes.len()].copy_from_slice(bytes);
}

/// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` as the bytes Windows reads:
/// every field written at its `offset_of!`, padding left zero.
pub fn extended_bytes(
    info: &windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
) -> Vec<u8> {
    use std::mem::{offset_of, size_of};
    use windows_sys::Win32::System::JobObjects::{
        JOBOBJECT_BASIC_LIMIT_INFORMATION as B, JOBOBJECT_EXTENDED_LIMIT_INFORMATION as E,
    };
    use windows_sys::Win32::System::Threading::IO_COUNTERS as Io;

    let mut buf = vec![0u8; size_of::<E>()];
    let o = offset_of!(E, BasicLimitInformation);
    let b = &info.BasicLimitInformation;
    put(
        &mut buf,
        o + offset_of!(B, PerProcessUserTimeLimit),
        &b.PerProcessUserTimeLimit.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, PerJobUserTimeLimit),
        &b.PerJobUserTimeLimit.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, LimitFlags),
        &b.LimitFlags.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, MinimumWorkingSetSize),
        &b.MinimumWorkingSetSize.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, MaximumWorkingSetSize),
        &b.MaximumWorkingSetSize.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, ActiveProcessLimit),
        &b.ActiveProcessLimit.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, Affinity),
        &b.Affinity.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, PriorityClass),
        &b.PriorityClass.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(B, SchedulingClass),
        &b.SchedulingClass.to_le_bytes(),
    );
    let o = offset_of!(E, IoInfo);
    let io = &info.IoInfo;
    put(
        &mut buf,
        o + offset_of!(Io, ReadOperationCount),
        &io.ReadOperationCount.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(Io, WriteOperationCount),
        &io.WriteOperationCount.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(Io, OtherOperationCount),
        &io.OtherOperationCount.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(Io, ReadTransferCount),
        &io.ReadTransferCount.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(Io, WriteTransferCount),
        &io.WriteTransferCount.to_le_bytes(),
    );
    put(
        &mut buf,
        o + offset_of!(Io, OtherTransferCount),
        &io.OtherTransferCount.to_le_bytes(),
    );
    put(
        &mut buf,
        offset_of!(E, ProcessMemoryLimit),
        &info.ProcessMemoryLimit.to_le_bytes(),
    );
    put(
        &mut buf,
        offset_of!(E, JobMemoryLimit),
        &info.JobMemoryLimit.to_le_bytes(),
    );
    put(
        &mut buf,
        offset_of!(E, PeakProcessMemoryUsed),
        &info.PeakProcessMemoryUsed.to_le_bytes(),
    );
    put(
        &mut buf,
        offset_of!(E, PeakJobMemoryUsed),
        &info.PeakJobMemoryUsed.to_le_bytes(),
    );
    buf
}

/// `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` as bytes, field by field.
pub fn cpu_rate_bytes(
    info: &windows_sys::Win32::System::JobObjects::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
) -> Vec<u8> {
    use std::mem::{offset_of, size_of};
    use windows_sys::Win32::System::JobObjects::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION as C;

    let mut buf = vec![0u8; size_of::<C>()];
    put(
        &mut buf,
        offset_of!(C, ControlFlags),
        &info.ControlFlags.to_le_bytes(),
    );
    // SAFETY: every union variant is plain integers spanning all 4 bytes, and
    // cpu_rate_info writes CpuRate.
    let rate = unsafe { info.Anonymous.CpuRate };
    put(&mut buf, offset_of!(C, Anonymous), &rate.to_le_bytes());
    buf
}

/// Build `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` (the real windows-sys type,
/// zero-initialized so every byte incl. padding is defined).
/// `kill_on_job_close` controls only the KILL_ON_JOB_CLOSE bit in the flags;
/// all other requested limits are always written (Windows ignores fields
/// whose flag bit is clear).
pub fn extended_limit_info(
    l: &JobLimits,
    kill_on_job_close: bool,
) -> windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;

    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = if kill_on_job_close {
        l.flags | crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    } else {
        l.flags & !crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    };
    info.BasicLimitInformation.ActiveProcessLimit = l.active_process_limit;
    info.BasicLimitInformation.Affinity = l.affinity;
    info.BasicLimitInformation.PriorityClass = l.priority_class;
    info.JobMemoryLimit = l.job_memory_limit as usize;
    info
}

/// Build `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` (real windows-sys type,
/// zero-initialized): `{ ControlFlags: ENABLE | HARD_CAP, CpuRate: percent *
/// 100 }` — CpuRate lives in the `Anonymous` union, written as the probe code
/// in `tests/probe_support.rs` reads it.
pub fn cpu_rate_info(
    percent: u32,
) -> windows_sys::Win32::System::JobObjects::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
    use windows_sys::Win32::System::JobObjects::{
        JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
    };

    let mut info: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { std::mem::zeroed() };
    info.ControlFlags =
        crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | crate::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
    info.Anonymous = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
        CpuRate: percent * 100,
    };
    info
}

/// Same requested limits as `extended_limit_info(l, true)` but with
/// KILL_ON_JOB_CLOSE cleared — used when releasing the job from a daemon,
/// which keeps its limits (unlike the old per-tool scripts that only kept
/// their own single limit; here all requested limits stay).
pub fn release_extended_info(
    l: &JobLimits,
) -> windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    extended_limit_info(l, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Handle, PriorityClass};

    const GIB: u64 = 1024 * 1024 * 1024;

    struct FakeSys {
        total_ram: u64,
        logical_cpus: u32,
    }
    impl FakeSys {
        fn new() -> Self {
            FakeSys {
                total_ram: 8 * GIB,
                logical_cpus: 8,
            }
        }
    }
    impl Sys for FakeSys {
        fn create_job_object(&self) -> Result<Handle, u32> {
            unimplemented!()
        }
        fn set_information_job_object(&self, _: &Handle, _: u32, _: &[u8]) -> Result<(), u32> {
            unimplemented!()
        }
        fn create_process(&self, _: Option<&str>, _: &str, _: u32) -> Result<crate::Child, u32> {
            unimplemented!()
        }
        fn assign_process_to_job_object(&self, _: &Handle, _: &Handle) -> Result<(), u32> {
            unimplemented!()
        }
        fn resume_thread(&self, _: &Handle) -> Result<(), u32> {
            unimplemented!()
        }
        fn create_waitable_timer(&self) -> Result<Handle, u32> {
            unimplemented!()
        }
        fn set_waitable_timer(&self, _: &Handle, _: i64) -> Result<(), u32> {
            unimplemented!()
        }
        fn wait_for_multiple_objects(&self, _: &[&Handle], _: bool) -> Result<u32, u32> {
            unimplemented!()
        }
        fn get_process_times(&self, _: &Handle) -> Result<i64, u32> {
            unimplemented!()
        }
        fn get_exit_code_process(&self, _: &Handle) -> Result<u32, u32> {
            unimplemented!()
        }
        fn terminate_process(&self, _: &Handle, _: u32) -> Result<(), u32> {
            unimplemented!()
        }
        fn terminate_job_object(&self, _: &Handle, _: u32) -> Result<(), u32> {
            unimplemented!()
        }
        fn global_memory_status_ex(&self) -> Result<u64, u32> {
            Ok(self.total_ram)
        }
        fn logical_processor_count(&self) -> u32 {
            self.logical_cpus
        }
        fn system_directory(&self) -> String {
            String::new()
        }
    }

    #[test]
    fn compose_empty_config_kill_on_close_only() {
        let sys = FakeSys::new();
        let l = compose(&Config::default(), &sys).unwrap();
        assert_eq!(l.flags, crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE);
        assert_eq!(l.affinity, 0);
        assert_eq!(l.active_process_limit, 0);
        assert_eq!(l.priority_class, 0);
        assert_eq!(l.job_memory_limit, 0);
        assert_eq!(l.cpu_rate, None);
    }

    #[test]
    fn compose_threads() {
        let sys = FakeSys::new();
        let l = compose(
            &Config {
                threads: Some(1),
                ..Default::default()
            },
            &sys,
        )
        .unwrap();
        assert_eq!(
            l.flags,
            crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | crate::JOB_OBJECT_LIMIT_AFFINITY
        );
        assert_eq!(l.affinity, 1);
        let l = compose(
            &Config {
                threads: Some(4),
                ..Default::default()
            },
            &sys,
        )
        .unwrap();
        assert_eq!(l.affinity, 0b1111);
    }

    #[test]
    fn compose_memory_bytes() {
        let sys = FakeSys::new();
        let l = compose(
            &Config {
                memory: Some(MemSpec::Bytes(1024)),
                ..Default::default()
            },
            &sys,
        )
        .unwrap();
        assert_eq!(
            l.flags,
            crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | crate::JOB_OBJECT_LIMIT_JOB_MEMORY
        );
        assert_eq!(l.job_memory_limit, 1024);
    }

    #[test]
    fn compose_procs() {
        let sys = FakeSys::new();
        let l = compose(
            &Config {
                procs: Some(3),
                ..Default::default()
            },
            &sys,
        )
        .unwrap();
        assert_eq!(
            l.flags,
            crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        );
        assert_eq!(l.active_process_limit, 3);
    }

    #[test]
    fn compose_priority() {
        let sys = FakeSys::new();
        let l = compose(
            &Config {
                priority: Some(PriorityClass::High),
                ..Default::default()
            },
            &sys,
        )
        .unwrap();
        assert_eq!(
            l.flags,
            crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS
        );
        assert_eq!(l.priority_class, PriorityClass::High.to_u32());
    }

    #[test]
    fn compose_cpu_percent() {
        let sys = FakeSys::new();
        let l = compose(
            &Config {
                cpu_percent: Some(50),
                ..Default::default()
            },
            &sys,
        )
        .unwrap();
        assert_eq!(l.cpu_rate, Some(50));
        // no flag change from cpu_rate
        assert_eq!(l.flags, crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE);
    }

    #[test]
    fn compose_combined() {
        let sys = FakeSys::new();
        let l = compose(
            &Config {
                threads: Some(2),
                memory: Some(MemSpec::Percent(25)),
                procs: Some(5),
                priority: Some(PriorityClass::Idle),
                cpu_percent: Some(75),
                ..Default::default()
            },
            &sys,
        )
        .unwrap();
        assert_eq!(
            l.flags,
            crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | crate::JOB_OBJECT_LIMIT_AFFINITY
                | crate::JOB_OBJECT_LIMIT_JOB_MEMORY
                | crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS
        );
        assert_eq!(l.affinity, 0b11);
        assert_eq!(l.job_memory_limit, 2 * GIB);
        assert_eq!(l.active_process_limit, 5);
        assert_eq!(l.priority_class, PriorityClass::Idle.to_u32());
        assert_eq!(l.cpu_rate, Some(75));
    }

    #[test]
    fn resolve_memory_percent() {
        let sys = FakeSys::new();
        assert_eq!(resolve_memory(&sys, MemSpec::Percent(50)).unwrap(), 4 * GIB);
        assert_eq!(
            resolve_memory(&sys, MemSpec::Percent(100)).unwrap(),
            8 * GIB
        );
        assert_eq!(resolve_memory(&sys, MemSpec::Bytes(12345)).unwrap(), 12345);
    }

    #[test]
    fn total_ram_percent_errors() {
        struct ZeroSys;
        impl Sys for ZeroSys {
            fn create_job_object(&self) -> Result<Handle, u32> {
                unimplemented!()
            }
            fn set_information_job_object(&self, _: &Handle, _: u32, _: &[u8]) -> Result<(), u32> {
                unimplemented!()
            }
            fn create_process(
                &self,
                _: Option<&str>,
                _: &str,
                _: u32,
            ) -> Result<crate::Child, u32> {
                unimplemented!()
            }
            fn assign_process_to_job_object(&self, _: &Handle, _: &Handle) -> Result<(), u32> {
                unimplemented!()
            }
            fn resume_thread(&self, _: &Handle) -> Result<(), u32> {
                unimplemented!()
            }
            fn create_waitable_timer(&self) -> Result<Handle, u32> {
                unimplemented!()
            }
            fn set_waitable_timer(&self, _: &Handle, _: i64) -> Result<(), u32> {
                unimplemented!()
            }
            fn wait_for_multiple_objects(&self, _: &[&Handle], _: bool) -> Result<u32, u32> {
                unimplemented!()
            }
            fn get_process_times(&self, _: &Handle) -> Result<i64, u32> {
                unimplemented!()
            }
            fn get_exit_code_process(&self, _: &Handle) -> Result<u32, u32> {
                unimplemented!()
            }
            fn terminate_process(&self, _: &Handle, _: u32) -> Result<(), u32> {
                unimplemented!()
            }
            fn terminate_job_object(&self, _: &Handle, _: u32) -> Result<(), u32> {
                unimplemented!()
            }
            fn global_memory_status_ex(&self) -> Result<u64, u32> {
                Ok(0)
            }
            fn logical_processor_count(&self) -> u32 {
                8
            }
            fn system_directory(&self) -> String {
                String::new()
            }
        }
        let sys = ZeroSys;
        assert_eq!(
            total_ram_percent_bytes(&sys, 1).unwrap_err(),
            "wnice: memory limit is too small - rounds to 0 bytes"
        );
        struct FailSys;
        impl Sys for FailSys {
            fn create_job_object(&self) -> Result<Handle, u32> {
                unimplemented!()
            }
            fn set_information_job_object(&self, _: &Handle, _: u32, _: &[u8]) -> Result<(), u32> {
                unimplemented!()
            }
            fn create_process(
                &self,
                _: Option<&str>,
                _: &str,
                _: u32,
            ) -> Result<crate::Child, u32> {
                unimplemented!()
            }
            fn assign_process_to_job_object(&self, _: &Handle, _: &Handle) -> Result<(), u32> {
                unimplemented!()
            }
            fn resume_thread(&self, _: &Handle) -> Result<(), u32> {
                unimplemented!()
            }
            fn create_waitable_timer(&self) -> Result<Handle, u32> {
                unimplemented!()
            }
            fn set_waitable_timer(&self, _: &Handle, _: i64) -> Result<(), u32> {
                unimplemented!()
            }
            fn wait_for_multiple_objects(&self, _: &[&Handle], _: bool) -> Result<u32, u32> {
                unimplemented!()
            }
            fn get_process_times(&self, _: &Handle) -> Result<i64, u32> {
                unimplemented!()
            }
            fn get_exit_code_process(&self, _: &Handle) -> Result<u32, u32> {
                unimplemented!()
            }
            fn terminate_process(&self, _: &Handle, _: u32) -> Result<(), u32> {
                unimplemented!()
            }
            fn terminate_job_object(&self, _: &Handle, _: u32) -> Result<(), u32> {
                unimplemented!()
            }
            fn global_memory_status_ex(&self) -> Result<u64, u32> {
                Err(87)
            }
            fn logical_processor_count(&self) -> u32 {
                8
            }
            fn system_directory(&self) -> String {
                String::new()
            }
        }
        let sys = FailSys;
        assert_eq!(
            total_ram_percent_bytes(&sys, 50).unwrap_err(),
            "GlobalMemoryStatusEx failed: 87"
        );
    }

    #[test]
    fn compose_threads_validation() {
        let sys = FakeSys::new();
        assert!(compose(
            &Config {
                threads: Some(0),
                ..Default::default()
            },
            &sys
        )
        .unwrap_err()
        .contains("out of range"));
        assert!(compose(
            &Config {
                threads: Some(64),
                ..Default::default()
            },
            &sys
        )
        .unwrap_err()
        .contains("out of range"));
        let mut many = FakeSys::new();
        many.logical_cpus = 4;
        assert!(compose(
            &Config {
                threads: Some(5),
                ..Default::default()
            },
            &many
        )
        .unwrap_err()
        .contains("out of range"));
        many.logical_cpus = 64;
        assert!(compose(
            &Config {
                threads: Some(63),
                ..Default::default()
            },
            &many
        )
        .is_ok());
    }

    #[test]
    fn compose_cpu_percent_validation() {
        let sys = FakeSys::new();
        assert!(compose(
            &Config {
                cpu_percent: Some(0),
                ..Default::default()
            },
            &sys
        )
        .unwrap_err()
        .contains("out of range"));
        assert!(compose(
            &Config {
                cpu_percent: Some(101),
                ..Default::default()
            },
            &sys
        )
        .unwrap_err()
        .contains("out of range"));
    }

    #[test]
    fn extended_limit_info_layout() {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        };
        use windows_sys::Win32::System::Threading::IO_COUNTERS;

        let l = JobLimits {
            flags: crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | crate::JOB_OBJECT_LIMIT_JOB_MEMORY
                | crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            affinity: 0xF,
            active_process_limit: 7,
            priority_class: 0x80,
            job_memory_limit: 0x1234_5678_9ABC_DEF0,
            cpu_rate: None,
        };
        let info = extended_limit_info(&l, true);
        // Size matches the real windows-sys type and the documented layout
        // (basic part + IO_COUNTERS + 4 usize fields).
        assert_eq!(
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
            std::mem::size_of::<JOBOBJECT_BASIC_LIMIT_INFORMATION>()
                + std::mem::size_of::<IO_COUNTERS>()
                + 4 * std::mem::size_of::<usize>()
        );
        assert_eq!(
            extended_bytes(&info).len(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()
        );
        // Field values read back through the returned struct.
        assert_eq!(info.BasicLimitInformation.LimitFlags, l.flags);
        assert_eq!(info.BasicLimitInformation.ActiveProcessLimit, 7);
        assert_eq!(info.BasicLimitInformation.Affinity, 0xF);
        assert_eq!(info.BasicLimitInformation.PriorityClass, 0x80);
        assert_eq!(info.JobMemoryLimit as u64, l.job_memory_limit);
        // Byte-level checks derive offsets from offset_of! on the real type.
        let bytes = extended_bytes(&info);
        let flags_off =
            std::mem::offset_of!(JOBOBJECT_EXTENDED_LIMIT_INFORMATION, BasicLimitInformation)
                + std::mem::offset_of!(JOBOBJECT_BASIC_LIMIT_INFORMATION, LimitFlags);
        let flags = u32::from_le_bytes(bytes[flags_off..flags_off + 4].try_into().unwrap());
        assert_eq!(flags, l.flags);
        let affinity_off =
            std::mem::offset_of!(JOBOBJECT_EXTENDED_LIMIT_INFORMATION, BasicLimitInformation)
                + std::mem::offset_of!(JOBOBJECT_BASIC_LIMIT_INFORMATION, Affinity);
        let affinity = usize::from_le_bytes(
            bytes[affinity_off..affinity_off + std::mem::size_of::<usize>()]
                .try_into()
                .unwrap(),
        );
        assert_eq!(affinity, 0xF);
        let jobmem_off = std::mem::offset_of!(JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobMemoryLimit);
        let job_mem = usize::from_le_bytes(
            bytes[jobmem_off..jobmem_off + std::mem::size_of::<usize>()]
                .try_into()
                .unwrap(),
        );
        assert_eq!(job_mem as u64, l.job_memory_limit);
    }

    #[test]
    fn job_constants_match_windows_sys() {
        use windows_sys::Win32::System::JobObjects as J;
        assert_eq!(
            crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            J::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        );
        assert_eq!(
            crate::JOB_OBJECT_LIMIT_AFFINITY,
            J::JOB_OBJECT_LIMIT_AFFINITY
        );
        assert_eq!(
            crate::JOB_OBJECT_LIMIT_JOB_MEMORY,
            J::JOB_OBJECT_LIMIT_JOB_MEMORY
        );
        assert_eq!(
            crate::JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            J::JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        );
        assert_eq!(
            crate::JOB_OBJECT_LIMIT_PRIORITY_CLASS,
            J::JOB_OBJECT_LIMIT_PRIORITY_CLASS
        );
        assert_eq!(
            crate::JobObjectExtendedLimitInformation as i32,
            J::JobObjectExtendedLimitInformation
        );
        assert_eq!(
            crate::JobObjectCpuRateControlInformation as i32,
            J::JobObjectCpuRateControlInformation
        );
        assert_eq!(
            crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
            J::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE
        );
        assert_eq!(
            crate::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            J::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP
        );
        assert_eq!(
            crate::CREATE_SUSPENDED,
            windows_sys::Win32::System::Threading::CREATE_SUSPENDED
        );
    }

    #[test]
    fn extended_bytes_writes_fields_and_zero_padding() {
        use std::mem::offset_of;
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_LIMIT_INFORMATION as B, JOBOBJECT_EXTENDED_LIMIT_INFORMATION as E,
        };
        let mut info: E = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = 0xFFFF_FFFF;
        info.BasicLimitInformation.ActiveProcessLimit = 0xFFFF_FFFF;
        info.BasicLimitInformation.MinimumWorkingSetSize = usize::MAX;
        info.BasicLimitInformation.Affinity = usize::MAX;
        let bytes = extended_bytes(&info);
        let o = offset_of!(E, BasicLimitInformation);
        // x64 padding after LimitFlags and after ActiveProcessLimit.
        for (field_end, next) in [
            (
                offset_of!(B, LimitFlags) + 4,
                offset_of!(B, MinimumWorkingSetSize),
            ),
            (
                offset_of!(B, ActiveProcessLimit) + 4,
                offset_of!(B, Affinity),
            ),
        ] {
            assert!(next > field_end, "expected padding on x64");
            assert!(bytes[o + field_end..o + next].iter().all(|&b| b == 0));
        }
        let lf = o + offset_of!(B, LimitFlags);
        assert_eq!(&bytes[lf..lf + 4], &[0xFF; 4]);
        let ap = o + offset_of!(B, ActiveProcessLimit);
        assert_eq!(&bytes[ap..ap + 4], &[0xFF; 4]);
    }

    #[test]
    fn extended_limit_info_kill_bit() {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        };
        let flags_off =
            std::mem::offset_of!(JOBOBJECT_EXTENDED_LIMIT_INFORMATION, BasicLimitInformation)
                + std::mem::offset_of!(JOBOBJECT_BASIC_LIMIT_INFORMATION, LimitFlags);
        let base = JobLimits {
            flags: crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | crate::JOB_OBJECT_LIMIT_JOB_MEMORY,
            affinity: 0,
            active_process_limit: 0,
            priority_class: 0,
            job_memory_limit: 4096,
            cpu_rate: None,
        };
        let on_info = extended_limit_info(&base, true);
        let on_bytes = extended_bytes(&on_info);
        let on = u32::from_le_bytes(on_bytes[flags_off..flags_off + 4].try_into().unwrap());
        assert_eq!(
            on & crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        );
        assert_eq!(
            on & crate::JOB_OBJECT_LIMIT_JOB_MEMORY,
            crate::JOB_OBJECT_LIMIT_JOB_MEMORY
        );
        let off_info = extended_limit_info(&base, false);
        let off_bytes = extended_bytes(&off_info);
        let off = u32::from_le_bytes(off_bytes[flags_off..flags_off + 4].try_into().unwrap());
        assert_eq!(off & crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, 0);
        assert_eq!(
            off & crate::JOB_OBJECT_LIMIT_JOB_MEMORY,
            crate::JOB_OBJECT_LIMIT_JOB_MEMORY
        );
    }

    #[test]
    fn cpu_rate_info_exact() {
        use windows_sys::Win32::System::JobObjects::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION;
        let info = cpu_rate_info(50);
        assert_eq!(
            std::mem::size_of_val(&info),
            std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>()
        );
        assert_eq!(
            std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>(),
            8
        );
        assert_eq!(
            info.ControlFlags,
            crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | crate::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP
        );
        assert_eq!(unsafe { info.Anonymous.CpuRate }, 5000);
        let b = cpu_rate_bytes(&info);
        assert_eq!(b.len(), 8);
        assert_eq!(
            u32::from_le_bytes(b[0..4].try_into().unwrap()),
            crate::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | crate::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP
        );
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 5000);
    }

    #[test]
    fn release_extended_info_clears_kill_bit() {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        };
        let l = JobLimits {
            flags: crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | crate::JOB_OBJECT_LIMIT_JOB_MEMORY,
            affinity: 0,
            active_process_limit: 0,
            priority_class: 0,
            job_memory_limit: 999,
            cpu_rate: None,
        };
        let info = release_extended_info(&l);
        assert_eq!(
            info.BasicLimitInformation.LimitFlags & crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            0
        );
        assert_eq!(
            info.BasicLimitInformation.LimitFlags & crate::JOB_OBJECT_LIMIT_JOB_MEMORY,
            crate::JOB_OBJECT_LIMIT_JOB_MEMORY
        );
        assert_eq!(info.JobMemoryLimit, 999);
        // Byte-level check with offset derived from the real type.
        let flags_off =
            std::mem::offset_of!(JOBOBJECT_EXTENDED_LIMIT_INFORMATION, BasicLimitInformation)
                + std::mem::offset_of!(JOBOBJECT_BASIC_LIMIT_INFORMATION, LimitFlags);
        let bytes = extended_bytes(&info);
        let flags = u32::from_le_bytes(bytes[flags_off..flags_off + 4].try_into().unwrap());
        assert_eq!(flags & crate::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, 0);
    }

    #[test]
    fn extended_limit_info_size_matches_docs() {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        };
        use windows_sys::Win32::System::Threading::IO_COUNTERS;
        // x64 docs: 144 bytes. Derived here from windows-sys types only
        // (basic part + IO_COUNTERS sibling + 4 usize fields).
        let derived = std::mem::size_of::<JOBOBJECT_BASIC_LIMIT_INFORMATION>()
            + std::mem::size_of::<IO_COUNTERS>()
            + 4 * std::mem::size_of::<usize>();
        assert_eq!(
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
            derived
        );
        assert_eq!(
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
            144
        );
    }
}
