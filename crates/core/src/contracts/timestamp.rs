use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use super::ContractError;

/// A point in time as written on disk and in IPC payloads: RFC 3339, UTC, `Z`, whole seconds
/// (`2026-09-24T18:30:05Z`, CONTRACTS.md §1).
///
/// Every constructor enforces UTC and second precision, so formatting always yields that form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// The current time, truncated to the second.
    pub fn now() -> Self {
        Self::truncate(OffsetDateTime::now_utc())
    }

    /// Converts any `time` value to UTC and truncates it to whole seconds. Fails for years outside
    /// 0000–9999, which RFC 3339 cannot represent.
    pub fn from_offset_date_time(value: OffsetDateTime) -> Result<Self, ContractError> {
        let out_of_range = || ContractError::TimestampOutOfRange(value.to_string());
        let utc = value
            .checked_to_offset(UtcOffset::UTC)
            .ok_or_else(out_of_range)?;
        if !(0..=9999).contains(&utc.year()) {
            return Err(out_of_range());
        }
        Ok(Self::truncate(utc))
    }

    /// Parses exactly `YYYY-MM-DDTHH:MM:SSZ`. Other RFC 3339 spellings (offsets, fractional
    /// seconds, a space or lowercase separators) are rejected.
    pub fn parse(text: &str) -> Result<Self, ContractError> {
        let invalid = || ContractError::InvalidTimestamp(text.to_owned());
        let parsed = OffsetDateTime::parse(text, &Rfc3339).map_err(|_| invalid())?;
        let timestamp = Self(parsed);
        let canonical = parsed.offset() == UtcOffset::UTC && parsed.nanosecond() == 0;
        if !canonical || timestamp.to_string() != text {
            return Err(invalid());
        }
        Ok(timestamp)
    }

    /// The time as a `time` value (always UTC, whole seconds).
    pub fn as_datetime(self) -> OffsetDateTime {
        self.0
    }

    fn truncate(utc: OffsetDateTime) -> Self {
        // replace_nanosecond fails only for values above 999_999_999, so 0 never falls back.
        Self(utc.replace_nanosecond(0).unwrap_or(utc))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Rfc3339 formatting fails only for years outside 0..=9999.
        f.write_str(&self.0.format(&Rfc3339).map_err(|_| fmt::Error)?)
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = self.0.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_whole_seconds_with_z() {
        let ts = Timestamp::parse("2026-09-24T18:30:05Z").unwrap();
        assert_eq!(ts.to_string(), "2026-09-24T18:30:05Z");
        assert_eq!(
            serde_json::to_string(&ts).unwrap(),
            "\"2026-09-24T18:30:05Z\""
        );
        let back: Timestamp = serde_json::from_str("\"2026-09-24T18:30:05Z\"").unwrap();
        assert_eq!(back, ts);
    }

    #[test]
    fn now_has_no_fraction() {
        let now = Timestamp::now();
        assert_eq!(now.as_datetime().nanosecond(), 0);
        assert_eq!(now.as_datetime().offset(), UtcOffset::UTC);
        let text = now.to_string();
        assert_eq!(text.len(), 20, "{text}");
        assert!(text.ends_with('Z'), "{text}");
    }

    #[test]
    fn from_offset_date_time_converts_to_utc_whole_seconds() {
        let base = Timestamp::parse("2026-09-24T18:30:05Z").unwrap();
        let plus_two = UtcOffset::from_hms(2, 0, 0).unwrap();
        let local = (base.as_datetime() + time::Duration::milliseconds(750)).to_offset(plus_two);
        assert_eq!(local.hour(), 20);
        let ts = Timestamp::from_offset_date_time(local).unwrap();
        assert_eq!(ts, base);
        assert_eq!(ts.to_string(), "2026-09-24T18:30:05Z");
    }

    #[test]
    fn from_offset_date_time_accepts_only_rfc3339_years() {
        // 0000-01-01T00:00:00Z and 9999-12-31T23:59:59Z are the bounds.
        for (secs, ok) in [
            (-62_167_219_200, true),
            (-62_167_219_201, false),
            (253_402_300_799, true),
        ] {
            let value = OffsetDateTime::from_unix_timestamp(secs).unwrap();
            let result = Timestamp::from_offset_date_time(value);
            assert_eq!(result.is_ok(), ok, "{secs}: {result:?}");
            if let Ok(ts) = result {
                assert_eq!(Timestamp::parse(&ts.to_string()).unwrap(), ts);
            }
        }
    }

    #[test]
    fn rejects_other_forms() {
        for bad in [
            "2026-09-24T18:30:05.5Z",
            "2026-09-24T18:30:05+00:00",
            "2026-09-24T20:30:05+02:00",
            "2026-09-24 18:30:05Z",
            "2026-09-24t18:30:05z",
            "2026-09-24",
            "",
        ] {
            assert!(Timestamp::parse(bad).is_err(), "{bad}");
            assert!(
                serde_json::from_value::<Timestamp>(serde_json::json!(bad)).is_err(),
                "{bad}"
            );
        }
    }
}
