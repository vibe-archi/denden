// Long waits (watch, wait, the CLI Stop hook in --wait mode) end once whoever started them is gone.
//
// Claude Code's Monitor runs `bash -c "<im> watch ..."`, the `im` launcher being a sh script, and on
// expiry it kills the outer bash only: sh stays alive, waiting for us, so watching our parent alone
// would see nothing. We watch our first three ancestors instead (the launcher, the host's shell,
// the host), as they were when we started, and exit as soon as one of them has exited.

use std::sync::Once;
use std::time::Duration;

const DEPTH: usize = 3;
const EVERY: Duration = Duration::from_secs(2);

/// Starts (once) a thread that exits the process, with status 0, when an ancestor has exited.
pub fn exit_when_orphaned() {
    static START: Once = Once::new();
    START.call_once(|| {
        let watched = imp::ancestors(DEPTH);
        if watched.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(EVERY);
                if watched.iter().any(imp::Ancestor::gone) {
                    std::process::exit(0);
                }
            }
        });
    });
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    /// An open handle to the ancestor: while we hold it, its PID cannot be given to another process.
    pub struct Ancestor(usize);

    impl Ancestor {
        pub fn gone(&self) -> bool {
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(self.0 as HANDLE, &mut code) != 0 && code != STILL_ACTIVE as u32 }
        }
    }

    /// pid -> parent pid, for every process.
    fn parents() -> HashMap<u32, u32> {
        let mut map = HashMap::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE || snap.is_null() {
                return map;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e);
            while ok != 0 {
                map.insert(e.th32ProcessID, e.th32ParentProcessID);
                ok = Process32NextW(snap, &mut e);
            }
            CloseHandle(snap);
        }
        map
    }

    fn created(h: HANDLE) -> Option<u64> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut x, mut k, mut u) = (zero, zero, zero, zero);
        let ok = unsafe { GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u) };
        (ok != 0).then(|| ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }

    pub fn ancestors(depth: usize) -> Vec<Ancestor> {
        let map = parents();
        let mut out = Vec::new();
        let mut pid = std::process::id();
        let mut child_created = created(unsafe { GetCurrentProcess() });
        for _ in 0..depth {
            let Some(&ppid) = map.get(&pid) else { break };
            if ppid == 0 || ppid == 4 {
                break; // the idle and System processes
            }
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, ppid) };
            if h.is_null() {
                break; // gone already, or not ours to open
            }
            let c = created(h);
            let a = Ancestor(h as usize);
            // A "parent" created after its child is another process that got the PID; one that has
            // already exited cannot be watched either. Stop there.
            if c.zip(child_created).is_none_or(|(c, k)| c > k) || a.gone() {
                unsafe { CloseHandle(h) };
                break;
            }
            out.push(a);
            child_created = c;
            pid = ppid;
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod imp {
    /// (state, parent pid, start time) from /proc/<pid>/stat.
    fn stat(pid: u32) -> Option<(char, u32, u64)> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name, in parentheses, may contain anything: fields resume after the last ')'.
        let f: Vec<&str> = s.get(s.rfind(')')? + 1..)?.split_whitespace().collect();
        Some((f.first()?.chars().next()?, f.get(1)?.parse().ok()?, f.get(19)?.parse().ok()?))
    }

    pub struct Ancestor {
        pid: u32,
        start: u64,
    }

    impl Ancestor {
        /// Exited (a zombie not yet reaped counts), or its PID now names another process.
        pub fn gone(&self) -> bool {
            stat(self.pid).is_none_or(|(state, _, start)| state == 'Z' || state == 'X' || start != self.start)
        }
    }

    pub fn ancestors(depth: usize) -> Vec<Ancestor> {
        let mut out = Vec::new();
        let mut pid = std::process::id();
        for _ in 0..depth {
            let Some((_, ppid, _)) = stat(pid) else { break };
            if ppid <= 1 {
                break; // init: we are not anyone's any more, or never were
            }
            let Some((state, _, start)) = stat(ppid) else { break };
            if state == 'Z' || state == 'X' {
                break;
            }
            out.push(Ancestor { pid: ppid, start });
            pid = ppid;
        }
        out
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod imp {
    /// Without /proc: our parent only, gone once we are re-parented.
    pub struct Ancestor(u32);

    impl Ancestor {
        pub fn gone(&self) -> bool {
            (unsafe { libc::getppid() }) as u32 != self.0
        }
    }

    pub fn ancestors(_depth: usize) -> Vec<Ancestor> {
        let p = unsafe { libc::getppid() } as u32;
        if p <= 1 { Vec::new() } else { vec![Ancestor(p)] }
    }
}
