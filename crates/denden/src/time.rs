// Timestamps: local "YYYY-MM-DD HH:MM:SS", ISO 8601 UTC for the daemon log, and "5 min ago" for
// what the user reads. OS calls only, no date library.

pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub milli: u32,
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn utc(ms: u64) -> Civil {
    let secs = (ms / 1000) as i64;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400) as u32;
    Civil { year, month, day, hour: rem / 3600, minute: rem / 60 % 60, second: rem % 60, milli: (ms % 1000) as u32 }
}

pub fn iso(ms: u64) -> String {
    let c = utc(ms);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", c.year, c.month, c.day, c.hour, c.minute, c.second, c.milli)
}

#[cfg(windows)]
pub fn local(ms: u64) -> Civil {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = ms * 10_000 + 116_444_736_000_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut lt: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        if FileTimeToSystemTime(&ft, &mut st) == 0 || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &st, &mut lt) == 0 {
            return utc(ms);
        }
    }
    Civil {
        year: lt.wYear as i64,
        month: lt.wMonth as u32,
        day: lt.wDay as u32,
        hour: lt.wHour as u32,
        minute: lt.wMinute as u32,
        second: lt.wSecond as u32,
        milli: lt.wMilliseconds as u32,
    }
}

#[cfg(unix)]
#[allow(deprecated)] // libc::time_t on musl: already 64-bit since musl 1.2, which is what we pass
pub fn local(ms: u64) -> Civil {
    let t = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return utc(ms);
    }
    Civil {
        year: tm.tm_year as i64 + 1900,
        month: tm.tm_mon as u32 + 1,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
        milli: (ms % 1000) as u32,
    }
}

/// new Date(ts).toLocaleString('sv-SE').slice(0, 19)
pub fn fmt_time(ms: u64) -> String {
    let c = local(ms);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", c.year, c.month, c.day, c.hour, c.minute, c.second)
}

/// "HH:MM", local time.
pub fn fmt_hm(ms: u64) -> String {
    fmt_time(ms)[11..16].to_string()
}

/// How long ago `then` was, for a person: "just now", "5 min ago", "2 h ago", "3 days ago".
pub fn ago(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then) / 1000;
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => {
            let (h, m) = (secs / 3600, secs % 3600 / 60);
            if h < 10 && m >= 5 { format!("{h} h {m:02} min ago") } else { format!("{h} h ago") }
        }
        _ => {
            let d = secs / 86400;
            if d == 1 { "1 day ago".into() } else { format!("{d} days ago") }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_reads_well() {
        let m = 60_000;
        assert_eq!(ago(10 * m, 10 * m), "just now");
        assert_eq!(ago(10 * m, 5 * m), "5 min ago");
        assert_eq!(ago(200 * m, 50 * m), "2 h 30 min ago");
        assert_eq!(ago(200 * m, 79 * m), "2 h ago", "minutes under 5 are dropped");
        assert_eq!(ago(3 * 1440 * m, 0), "3 days ago");
        assert_eq!(ago(0, 5 * m), "just now", "a clock going back");
    }
}
