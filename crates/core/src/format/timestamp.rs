//! Common Log Format timestamps: `10/Oct/2026:13:55:36 +0000`.

/// Parses a CLF timestamp (without the surrounding brackets) into Unix
/// seconds in UTC, applying the offset written in the log line.
pub fn parse_clf(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 26
        || b[2] != b'/'
        || b[6] != b'/'
        || b[11] != b':'
        || b[14] != b':'
        || b[17] != b':'
        || b[20] != b' '
    {
        return None;
    }

    let day = num(&b[0..2])?;
    let month = month(&b[3..6])?;
    let year = num(&b[7..11])?;
    let hour = num(&b[12..14])?;
    let minute = num(&b[15..17])?;
    // 60 allows for a leap second.
    let second = num(&b[18..20])?;
    let sign = match b[21] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let off_hour = num(&b[22..24])?;
    let off_minute = num(&b[24..26])?;

    if day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
        || off_hour > 23
        || off_minute > 59
    {
        return None;
    }

    let local = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    Some(local - sign * (off_hour * 3_600 + off_minute * 60))
}

fn num(digits: &[u8]) -> Option<i64> {
    let mut n = 0i64;
    for &d in digits {
        if !d.is_ascii_digit() {
            return None;
        }
        n = n * 10 + i64::from(d - b'0');
    }
    Some(n)
}

fn month(name: &[u8]) -> Option<i64> {
    Some(match name {
        b"Jan" => 1,
        b"Feb" => 2,
        b"Mar" => 3,
        b"Apr" => 4,
        b"May" => 5,
        b"Jun" => 6,
        b"Jul" => 7,
        b"Aug" => 8,
        b"Sep" => 9,
        b"Oct" => 10,
        b"Nov" => 11,
        b"Dec" => 12,
        _ => return None,
    })
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch() {
        assert_eq!(parse_clf("01/Jan/1970:00:00:00 +0000"), Some(0));
    }

    #[test]
    fn negative_offset_moves_forward_to_utc() {
        // The example from the Apache docs: 2000-10-10T20:55:36Z.
        assert_eq!(parse_clf("10/Oct/2000:13:55:36 -0700"), Some(971_211_336));
    }

    #[test]
    fn positive_offset_can_cross_midnight_backwards() {
        // 00:30 at +0100 is 23:30 the previous day in UTC.
        let utc = parse_clf("31/Dec/2025:23:30:00 +0000").unwrap();
        assert_eq!(parse_clf("01/Jan/2026:00:30:00 +0100"), Some(utc));
    }

    #[test]
    fn half_hour_offsets() {
        let utc = parse_clf("15/Nov/2026:10:00:00 +0000").unwrap();
        assert_eq!(parse_clf("15/Nov/2026:15:30:00 +0530"), Some(utc));
        assert_eq!(parse_clf("15/Nov/2026:06:15:00 -0345"), Some(utc));
    }

    #[test]
    fn leap_day() {
        assert!(parse_clf("29/Feb/2024:12:00:00 +0000").is_some());
        assert!(parse_clf("29/Feb/2026:12:00:00 +0000").is_none());
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "10/Oct/2026:13:55:36",
            "10/Oct/2026:13:55:36 0000",
            "10/Okt/2026:13:55:36 +0000",
            "32/Oct/2026:13:55:36 +0000",
            "10/Oct/2026:24:00:00 +0000",
            "10-Oct-2026:13:55:36 +0000",
            "1o/Oct/2026:13:55:36 +0000",
        ] {
            assert_eq!(parse_clf(bad), None, "{bad:?}");
        }
    }
}
