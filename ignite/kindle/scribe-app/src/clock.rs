//! Device-local date and time via libc `localtime_r` (reads `/etc/localtime`).
//! Years before [`MIN_TRUSTED_YEAR`] are treated as an unset clock.

pub const MIN_TRUSTED_YEAR: i32 = 2025;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// 0 = Sunday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
}

const WEEKDAYS: [&str; 7] = [
    "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday",
];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];

/// Current local time, or `None` when the clock looks unset.
pub fn now_local() -> Option<LocalDateTime> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    local_from_unix(i64::try_from(secs).ok()?)
}

/// Convert unix seconds to local time; `None` on failure or an untrusted year.
pub fn local_from_unix(secs: i64) -> Option<LocalDateTime> {
    // `time_t` is i32 on armv7 (Kindle): instants past 2038 are `None` there, never truncated.
    let t = libc::time_t::try_from(secs).ok()?;
    // SAFETY: `tm` is a plain-old-data struct for which all-zero is valid; `t` and `tm`
    // are live locals, and `localtime_r` only writes through the `tm` pointer.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t as *const libc::time_t, &mut tm).is_null() {
            return None;
        }
        tm
    };
    let year = tm.tm_year.checked_add(1900)?;
    if year < MIN_TRUSTED_YEAR {
        return None;
    }
    Some(LocalDateTime {
        year,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        weekday: tm.tm_wday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
    })
}

pub fn ymd(d: &LocalDateTime) -> String {
    format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)
}

pub fn ym(d: &LocalDateTime) -> String {
    format!("{:04}-{:02}", d.year, d.month)
}

fn weekday_name(d: &LocalDateTime) -> &'static str {
    WEEKDAYS[(d.weekday % 7) as usize]
}

fn month_name(d: &LocalDateTime) -> &'static str {
    MONTHS[(d.month.clamp(1, 12) - 1) as usize]
}

pub fn long_date(d: &LocalDateTime) -> String {
    format!("{} {} {} {}", weekday_name(d), d.day, month_name(d), d.year)
}

pub fn stamp(d: &LocalDateTime) -> String {
    format!(
        "{} {} {} {:02}:{:02}",
        &weekday_name(d)[..3],
        d.day,
        &month_name(d)[..3],
        d.hour,
        d.minute
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TZ` and `tzset` are process-global; every test here goes through this once-guard and
    /// relies on UTC. No other test module in this crate may depend on a different `TZ`.
    fn use_utc() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::env::set_var("TZ", "UTC");
            extern "C" {
                // POSIX; not exported by the libc crate on every target.
                fn tzset();
            }
            // SAFETY: tzset has no preconditions; it re-reads TZ.
            unsafe { tzset() };
        });
    }

    #[test]
    fn formats_match_the_spec_for_a_known_instant() {
        use_utc();
        let d = local_from_unix(1_792_950_000).expect("trusted year");
        assert_eq!((d.year, d.month, d.day, d.weekday, d.hour, d.minute), (2026, 10, 25, 0, 17, 40));
        assert_eq!(ymd(&d), "2026-10-25");
        assert_eq!(ym(&d), "2026-10");
        assert_eq!(long_date(&d), "Sunday 25 October 2026");
        assert_eq!(stamp(&d), "Sun 25 Oct 17:40");
    }

    #[test]
    fn out_of_range_instants_are_none() {
        use_utc();
        // Too large for a 32-bit time_t, and past localtime_r's year range on 64-bit.
        assert_eq!(local_from_unix(i64::MAX), None);
    }

    #[test]
    fn years_before_2025_are_untrusted() {
        use_utc();
        assert_eq!(local_from_unix(0), None);
        assert!(local_from_unix(1_735_689_600).is_some());
    }
}
