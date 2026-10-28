//! Immutable date/time values. Local timezone-sensitive APIs are restricted to UTC;
//! the caller supplies a fixed clock for the test environment.
use crate::{
    platform::PlatformError,
    value::{NumKind, Number, Value},
};
use chrono::{DateTime, Datelike, Months, NaiveDate, NaiveDateTime, Timelike, Utc};
use num_traits::ToPrimitive;
type P<T> = Result<T, PlatformError>;
fn error(message: impl Into<String>) -> PlatformError {
    PlatformError::exception("System.TypeException", message)
}
fn integer(value: &Value) -> P<i64> {
    match value {
        Value::Num(n) if n.q.is_integer() => {
            n.q.to_integer()
                .to_i64()
                .ok_or_else(|| error("Integer argument out of range"))
        }
        Value::Null => Err(PlatformError::exception(
            "System.NullPointerException",
            "Date argument cannot be null",
        )),
        _ => Err(PlatformError::unsupported(
            "Date signature requires Integer",
        )),
    }
}
fn text(value: &Value) -> P<&str> {
    match value {
        Value::Str(s) => Ok(s),
        Value::Null => Err(PlatformError::exception(
            "System.NullPointerException",
            "Date argument cannot be null",
        )),
        _ => Err(PlatformError::unsupported("Date signature requires String")),
    }
}
fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
}
fn date(days: i32) -> P<NaiveDate> {
    epoch()
        .checked_add_signed(chrono::Duration::days(days as i64))
        .filter(|d| (1..=9999).contains(&d.year()))
        .ok_or_else(|| error("Date out of range"))
}
fn datetime(ms: i64) -> P<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ms)
        .filter(|d| (1..=9999).contains(&d.year()))
        .ok_or_else(|| error("Datetime out of range"))
}
fn date_value(d: NaiveDate) -> P<Value> {
    if !(1..=9999).contains(&d.year()) {
        return Err(error("Date out of range"));
    }
    Ok(Value::Date((d - epoch()).num_days() as i32))
}
fn dt_value(d: DateTime<Utc>) -> P<Value> {
    if !(1..=9999).contains(&d.year()) {
        return Err(error("Datetime out of range"));
    }
    Ok(Value::Datetime(d.timestamp_millis()))
}
pub fn parse_date(s: &str) -> P<Value> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| error("Invalid date"))?;
    date_value(d)
}
pub fn parse_datetime(s: &str) -> P<Value> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return dt_value(d.with_timezone(&Utc));
    }
    let d = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|_| error("Invalid datetime"))?;
    dt_value(d.and_utc())
}
pub fn date_iso(days: i32) -> P<String> {
    Ok(date(days)?.format("%Y-%m-%d").to_string())
}
pub fn datetime_iso(ms: i64) -> P<String> {
    Ok(datetime(ms)?.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}
pub fn now_iso_utc() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}
pub fn display(value: &Value) -> Option<String> {
    match value {
        Value::Date(d) => Some(date_iso(*d).unwrap_or_else(|_| "<invalid date>".into())),
        Value::Datetime(ms) => Some(
            datetime(*ms)
                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|_| "<invalid datetime>".into()),
        ),
        Value::Time(ms) => Some(format!(
            "{:02}:{:02}:{:02}.{:03}",
            ms / 3_600_000,
            (ms / 60_000) % 60,
            (ms / 1000) % 60,
            ms % 1000
        )),
        _ => None,
    }
}
pub fn invoke(namespace: &str, method: &str, args: &[Value], clock_iso: &str) -> P<Option<Value>> {
    let ns = namespace.to_ascii_lowercase();
    let ns = ns.trim_start_matches("system.");
    let method = method.to_ascii_lowercase();
    if !["date", "datetime", "time"].contains(&ns) {
        return Ok(None);
    }
    let value = match (ns, method.as_str(), args) {
        ("date", "today", []) => {
            let Value::Datetime(ms) = parse_datetime(clock_iso)? else {
                unreachable!()
            };
            date_value(datetime(ms)?.date_naive())?
        }
        ("datetime", "now", []) => parse_datetime(clock_iso)?,
        ("date", "valueof", [v]) => parse_date(text(v)?)?,
        ("datetime", "valueofgmt", [v]) => parse_datetime(text(v)?)?,
        ("date", "newinstance", [y, m, d]) => {
            let y = i32::try_from(integer(y)?).map_err(|_| error("Invalid year"))?;
            let m = u32::try_from(integer(m)?).map_err(|_| error("Invalid month"))?;
            let d = u32::try_from(integer(d)?).map_err(|_| error("Invalid day"))?;
            date_value(NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| error("Invalid date"))?)?
        }
        ("datetime", "newinstancegmt", [y, m, d, h, min, s]) => {
            let y = i32::try_from(integer(y)?).map_err(|_| error("Invalid year"))?;
            let m = u32::try_from(integer(m)?).map_err(|_| error("Invalid month"))?;
            let d = u32::try_from(integer(d)?).map_err(|_| error("Invalid day"))?;
            let h = u32::try_from(integer(h)?).map_err(|_| error("Invalid hour"))?;
            let min = u32::try_from(integer(min)?).map_err(|_| error("Invalid minute"))?;
            let s = u32::try_from(integer(s)?).map_err(|_| error("Invalid second"))?;
            dt_value(
                NaiveDate::from_ymd_opt(y, m, d)
                    .and_then(|d| d.and_hms_opt(h, min, s))
                    .ok_or_else(|| error("Invalid datetime"))?
                    .and_utc(),
            )?
        }
        ("datetime", "newinstance", [millis]) => dt_value(datetime(integer(millis)?)?)?,
        ("datetime", "newinstancegmt", [Value::Date(d), Value::Time(ms)]) => dt_value(
            date(*d)?
                .and_hms_milli_opt(
                    ms / 3_600_000,
                    (ms / 60_000) % 60,
                    (ms / 1000) % 60,
                    ms % 1000,
                )
                .ok_or_else(|| error("Invalid datetime"))?
                .and_utc(),
        )?,
        ("time", "newinstance", [h, m, s, ms]) => {
            let h = integer(h)?;
            let m = integer(m)?;
            let s = integer(s)?;
            let ms = integer(ms)?;
            if !(0..24).contains(&h)
                || !(0..60).contains(&m)
                || !(0..60).contains(&s)
                || !(0..1000).contains(&ms)
            {
                return Err(error("Invalid time"));
            }
            Value::Time((h * 3_600_000 + m * 60_000 + s * 1000 + ms) as u32)
        }
        ("date", "isleapyear", [y]) => {
            let y = i32::try_from(integer(y)?).map_err(|_| error("Invalid year"))?;
            Value::Bool(NaiveDate::from_ymd_opt(y, 2, 29).is_some())
        }
        ("date", "daysinmonth", [y, m]) => {
            let y = i32::try_from(integer(y)?).map_err(|_| error("Invalid year"))?;
            let m = u32::try_from(integer(m)?).map_err(|_| error("Invalid month"))?;
            let first = NaiveDate::from_ymd_opt(y, m, 1).ok_or_else(|| error("Invalid month"))?;
            let next = first
                .checked_add_months(Months::new(1))
                .ok_or_else(|| error("Invalid month"))?;
            Value::Num(Number::integer((next - first).num_days()))
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
pub fn instance(receiver: &Value, method: &str, args: &[Value]) -> P<Option<Value>> {
    let method = method.to_ascii_lowercase();
    let n = |n| Value::Num(Number::integer(n));
    let result = match (receiver, method.as_str(), args) {
        (Value::Date(d), "year", []) => n(date(*d)?.year() as i64),
        (Value::Date(d), "month", []) => n(date(*d)?.month() as i64),
        (Value::Date(d), "day", []) => n(date(*d)?.day() as i64),
        (Value::Date(d), "dayofyear", []) => n(date(*d)?.ordinal() as i64),
        (Value::Date(d), "daysbetween", [Value::Date(other)]) => n(*other as i64 - *d as i64),
        (Value::Date(d), "adddays", [amount]) => {
            let amount = integer(amount)?;
            let next = (*d as i64)
                .checked_add(amount)
                .and_then(|v| i32::try_from(v).ok())
                .ok_or_else(|| error("Date out of range"))?;
            date_value(date(next)?)?
        }
        (Value::Date(d), "addmonths" | "addyears", [amount]) => {
            let months = integer(amount)?
                .checked_mul(if method == "addyears" { 12 } else { 1 })
                .ok_or_else(|| error("Date out of range"))?;
            if months.unsigned_abs() > 120_000 {
                return Err(error("Date out of range"));
            }
            let d = date(*d)?;
            let changed = if months >= 0 {
                d.checked_add_months(Months::new(months as u32))
            } else {
                d.checked_sub_months(Months::new((-months) as u32))
            };
            date_value(changed.ok_or_else(|| error("Date out of range"))?)?
        }
        (Value::Date(d), "tostartofmonth", []) => date_value(date(*d)?.with_day(1).unwrap())?,
        (Value::Datetime(ms), "gettime", []) => {
            let mut num = Number::integer(*ms);
            num.kind = NumKind::Long;
            Value::Num(num)
        }
        (Value::Datetime(ms), "yeargmt", []) => n(datetime(*ms)?.year() as i64),
        (Value::Datetime(ms), "monthgmt", []) => n(datetime(*ms)?.month() as i64),
        (Value::Datetime(ms), "daygmt", []) => n(datetime(*ms)?.day() as i64),
        (Value::Datetime(ms), "hourgmt", []) => n(datetime(*ms)?.hour() as i64),
        (Value::Datetime(ms), "minutegmt", []) => n(datetime(*ms)?.minute() as i64),
        (Value::Datetime(ms), "secondgmt", []) => n(datetime(*ms)?.second() as i64),
        (Value::Datetime(ms), "dategmt", []) => date_value(datetime(*ms)?.date_naive())?,
        (Value::Datetime(ms), "timegmt", []) => {
            let d = datetime(*ms)?;
            Value::Time(d.num_seconds_from_midnight() * 1000 + d.timestamp_subsec_millis())
        }
        (Value::Datetime(ms), "formatgmt", [pattern]) => Value::Str(
            datetime(*ms)?
                .format(&java_format(text(pattern)?)?)
                .to_string(),
        ),
        (
            Value::Datetime(ms),
            "addmilliseconds" | "addseconds" | "addminutes" | "addhours" | "adddays",
            [amount],
        ) => {
            let multiplier = match method.as_str() {
                "addseconds" => 1000,
                "addminutes" => 60_000,
                "addhours" => 3_600_000,
                "adddays" => 86_400_000,
                _ => 1,
            };
            let next = integer(amount)?
                .checked_mul(multiplier)
                .and_then(|d| ms.checked_add(d))
                .ok_or_else(|| error("Datetime out of range"))?;
            dt_value(datetime(next)?)?
        }
        (Value::Time(ms), "hour", []) => n((ms / 3_600_000) as i64),
        (Value::Time(ms), "minute", []) => n(((ms / 60_000) % 60) as i64),
        (Value::Time(ms), "second", []) => n(((ms / 1000) % 60) as i64),
        (Value::Time(ms), "millisecond", []) => n((ms % 1000) as i64),
        (
            Value::Time(ms),
            "addmilliseconds" | "addseconds" | "addminutes" | "addhours",
            [amount],
        ) => {
            let multiplier = match method.as_str() {
                "addseconds" => 1000,
                "addminutes" => 60_000,
                "addhours" => 3_600_000,
                _ => 1,
            };
            let delta = integer(amount)?
                .checked_mul(multiplier)
                .ok_or_else(|| error("Time amount out of range"))?;
            Value::Time((*ms as i64 + delta.rem_euclid(86_400_000)).rem_euclid(86_400_000) as u32)
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}
/// Deliberately rejects unknown formatting tokens instead of silently treating them as text.
pub fn java_format(pattern: &str) -> P<String> {
    if pattern.len() > 4096 {
        return Err(PlatformError::unsupported(
            "Datetime format exceeds host bound",
        ));
    }
    let mut out = String::new();
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut i = 0;
    let mut literal = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            literal = !literal;
            i += 1;
            continue;
        }
        if literal || !c.is_ascii_alphabetic() {
            if c == '%' {
                out.push('%');
            }
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while chars.get(i) == Some(&c) {
            i += 1;
        }
        let count = i - start;
        out.push_str(match (c, count) {
            ('y', 4) | ('u', 4) => "%Y",
            ('y', 2) => "%y",
            ('M', 1) => "%-m",
            ('M', 2) => "%m",
            ('M', 3) => "%b",
            ('M', 4) => "%B",
            ('d', 1) => "%-d",
            ('d', 2) => "%d",
            ('H', 1) => "%-H",
            ('H', 2) => "%H",
            ('h', 1) => "%-I",
            ('h', 2) => "%I",
            ('m', 1) => "%-M",
            ('m', 2) => "%M",
            ('s', 1) => "%-S",
            ('s', 2) => "%S",
            ('S', 3) => "%3f",
            ('a', 1) => "%p",
            ('E', 3) => "%a",
            ('E', 4) => "%A",
            ('z', _) => "UTC",
            ('Z', _) => "+0000",
            _ => {
                return Err(PlatformError::unsupported(format!(
                    "Datetime format token {} is not implemented",
                    chars[start..i].iter().collect::<String>()
                )))
            }
        });
    }
    if literal {
        return Err(error("Unterminated date format quote"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leap_day_and_month_end_are_calendar_aware() {
        let leap = parse_date("2024-02-29").unwrap();
        let next = instance(&leap, "addyears", &[Value::Num(Number::integer(1))])
            .unwrap()
            .unwrap();
        assert_eq!(display(&next).unwrap(), "2025-02-28");
        assert!(parse_date("2023-02-29").is_err());
    }
    #[test]
    fn datetime_format_uses_utc_and_explicit_pattern() {
        let d = parse_datetime("2026-01-15T12:00:00Z").unwrap();
        let out = instance(
            &d,
            "formatgmt",
            &[Value::Str("hh:mm:ss a, MMMM dd, yyyy".into())],
        )
        .unwrap()
        .unwrap();
        assert_eq!(out.display(), "12:00:00 PM, January 15, 2026");
        assert_eq!(
            parse_datetime("2026-01-15T13:00:00+01:00")
                .unwrap()
                .display(),
            d.display()
        );
    }
    #[test]
    fn clock_is_supplied_and_millis_are_exact() {
        let d = invoke("datetime", "now", &[], "2026-01-15T12:00:00.123Z")
            .unwrap()
            .unwrap();
        let Value::Datetime(ms) = d else { panic!() };
        assert_eq!(datetime_iso(ms).unwrap(), "2026-01-15T12:00:00.123Z");
        assert!(datetime(i64::MAX).is_err());
    }
}
