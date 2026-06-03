//! Host timezone → hcsschema `TimeZoneInformation` JSON.
//!
//! The in-guest GCS (`gcs.exe` / inbox `vmcomputeagent.exe`) is reached only
//! via hcsshim's `internal/uvm/start.go::Start`, which ALWAYS attaches a
//! `TimeZoneInformation` block to the cold-start `Create` for a Windows UVM.
//! hcsshim's DEFAULT (no `noInheritHostTimezone`) path queries the host's real
//! timezone via the Win32 `GetDynamicTimeZoneInformation` API and maps it to
//! `hcsschema.TimeZoneInformation` (see `internal/uvm/timezone.go`). The inbox
//! GCS's timezone handling is a critical/fragile path — omitting the field
//! bugchecks the guest (`0xEF CRITICAL_PROCESS_DIED`), and the all-zero
//! transition-date UTC constant is a suspected fragile input. This module
//! reproduces hcsshim's real-host-TZ wire shape so we can send exactly what a
//! production hcsshim host would.
//!
//! The produced JSON matches `hcsschema.TimeZoneInformation` EXACTLY:
//! PascalCase keys, names decoded from the UTF-16 `[u16; 32]` Win32 buffers,
//! and `StandardDate` / `DaylightDate` as nested `SystemTime` objects. Go's
//! `omitempty` semantics are mirrored: zero-valued integer fields and all-zero
//! `SYSTEMTIME` transition dates are omitted (a no-DST timezone serialises with
//! no transition dates; a DST timezone carries non-zero ones).

#![cfg(target_os = "windows")]

use serde_json::{json, Map, Value};
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::System::Time::{
    GetDynamicTimeZoneInformation, GetTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION,
    TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
};

/// Query the host's timezone and build a `serde_json::Value` matching
/// hcsschema's `TimeZoneInformation` shape (the default, real-host-TZ output
/// hcsshim produces). Prefers `GetDynamicTimeZoneInformation`; falls back to
/// `GetTimeZoneInformation`. Returns `None` if both Win32 calls report
/// `TIME_ZONE_ID_INVALID` so the caller can fall back to the UTC constant.
pub fn host_timezone_information() -> Option<Value> {
    // SAFETY: `GetDynamicTimeZoneInformation` writes a fully-initialized
    // `DYNAMIC_TIME_ZONE_INFORMATION` into the out-pointer. We pass a zeroed,
    // properly-sized, stack-allocated struct and only read it back when the
    // call does not report `TIME_ZONE_ID_INVALID`.
    let mut dtzi = DYNAMIC_TIME_ZONE_INFORMATION::default();
    let dyn_ret = unsafe { GetDynamicTimeZoneInformation(&mut dtzi) };
    if dyn_ret != TIME_ZONE_ID_INVALID {
        return Some(tzi_to_hcs_schema(
            dtzi.Bias,
            &dtzi.StandardName,
            &dtzi.StandardDate,
            dtzi.StandardBias,
            &dtzi.DaylightName,
            &dtzi.DaylightDate,
            dtzi.DaylightBias,
        ));
    }

    // SAFETY: identical contract to the dynamic variant above — the call
    // fully initializes the `TIME_ZONE_INFORMATION` out-parameter, and we only
    // read it back on a non-`TIME_ZONE_ID_INVALID` return.
    let mut tzi = TIME_ZONE_INFORMATION::default();
    let ret = unsafe { GetTimeZoneInformation(&mut tzi) };
    if ret == TIME_ZONE_ID_INVALID {
        return None;
    }
    Some(tzi_to_hcs_schema(
        tzi.Bias,
        &tzi.StandardName,
        &tzi.StandardDate,
        tzi.StandardBias,
        &tzi.DaylightName,
        &tzi.DaylightDate,
        tzi.DaylightBias,
    ))
}

/// Map a Win32 timezone-info tuple to hcsschema's `TimeZoneInformation` JSON,
/// applying Go `omitempty` semantics (zero ints and all-zero transition dates
/// are omitted). Mirrors hcsshim's `tziToHCSSchema`-style conversion in
/// `internal/uvm/timezone.go`.
fn tzi_to_hcs_schema(
    bias: i32,
    standard_name: &[u16; 32],
    standard_date: &SYSTEMTIME,
    standard_bias: i32,
    daylight_name: &[u16; 32],
    daylight_date: &SYSTEMTIME,
    daylight_bias: i32,
) -> Value {
    let mut map = Map::new();
    insert_nonzero_i32(&mut map, "Bias", bias);
    insert_nonempty_str(&mut map, "StandardName", decode_utf16_name(standard_name));
    if let Some(date) = systemtime_to_value(standard_date) {
        map.insert("StandardDate".to_string(), date);
    }
    insert_nonzero_i32(&mut map, "StandardBias", standard_bias);
    insert_nonempty_str(&mut map, "DaylightName", decode_utf16_name(daylight_name));
    if let Some(date) = systemtime_to_value(daylight_date) {
        map.insert("DaylightDate".to_string(), date);
    }
    insert_nonzero_i32(&mut map, "DaylightBias", daylight_bias);
    Value::Object(map)
}

/// Insert an `i32` field only when non-zero (Go `int32,omitempty`).
fn insert_nonzero_i32(map: &mut Map<String, Value>, key: &str, value: i32) {
    if value != 0 {
        map.insert(key.to_string(), json!(value));
    }
}

/// Insert a string field only when non-empty (Go `string,omitempty`).
fn insert_nonempty_str(map: &mut Map<String, Value>, key: &str, value: String) {
    if !value.is_empty() {
        map.insert(key.to_string(), Value::String(value));
    }
}

/// Decode a Win32 fixed-size UTF-16 name buffer, trimming at the first NUL.
fn decode_utf16_name(buf: &[u16; 32]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Convert a Win32 `SYSTEMTIME` to hcsschema `SystemTime` JSON, applying
/// `omitempty` to each field. Returns `None` when the whole struct is all-zero
/// (Go serialises a `*SystemTime` pointing at an all-zero value as an omitted
/// field for a no-DST timezone — but hcsshim's real-host UTC constant uses an
/// explicit empty object; here we mirror the real-TZ default where an all-zero
/// transition date is omitted entirely).
fn systemtime_to_value(st: &SYSTEMTIME) -> Option<Value> {
    let mut map = Map::new();
    // hcsschema `SystemTime` fields are `int32,omitempty` and named without the
    // Win32 `w` prefix: Year/Month/DayOfWeek/Day/Hour/Minute/Second/Milliseconds.
    insert_nonzero_i32(&mut map, "Year", i32::from(st.wYear));
    insert_nonzero_i32(&mut map, "Month", i32::from(st.wMonth));
    insert_nonzero_i32(&mut map, "DayOfWeek", i32::from(st.wDayOfWeek));
    insert_nonzero_i32(&mut map, "Day", i32::from(st.wDay));
    insert_nonzero_i32(&mut map, "Hour", i32::from(st.wHour));
    insert_nonzero_i32(&mut map, "Minute", i32::from(st.wMinute));
    insert_nonzero_i32(&mut map, "Second", i32::from(st.wSecond));
    insert_nonzero_i32(&mut map, "Milliseconds", i32::from(st.wMilliseconds));
    if map.is_empty() {
        None
    } else {
        Some(Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn systemtime(values: [u16; 8]) -> SYSTEMTIME {
        SYSTEMTIME {
            wYear: values[0],
            wMonth: values[1],
            wDayOfWeek: values[2],
            wDay: values[3],
            wHour: values[4],
            wMinute: values[5],
            wSecond: values[6],
            wMilliseconds: values[7],
        }
    }

    fn name_buf(s: &str) -> [u16; 32] {
        let mut buf = [0u16; 32];
        for (slot, ch) in buf.iter_mut().zip(s.encode_utf16()) {
            *slot = ch;
        }
        buf
    }

    #[test]
    fn all_zero_systemtime_is_omitted() {
        assert!(systemtime_to_value(&systemtime([0; 8])).is_none());
    }

    #[test]
    fn populated_systemtime_maps_w_prefixed_fields() {
        // A DST transition date (e.g. 2nd Sunday of March, 02:00).
        let st = systemtime([0, 3, 0, 2, 2, 0, 0, 0]);
        let v = systemtime_to_value(&st).expect("non-zero date");
        assert_eq!(
            v,
            json!({ "Month": 3, "Day": 2, "Hour": 2 }),
            "zero-valued fields (Year/DayOfWeek/Minute/Second/Ms) must be omitted"
        );
    }

    #[test]
    fn name_decodes_and_trims_at_nul() {
        assert_eq!(
            decode_utf16_name(&name_buf("Pacific Standard Time")),
            "Pacific Standard Time"
        );
    }

    #[test]
    fn dst_timezone_carries_transition_dates_and_omits_zero_ints() {
        // Mirror a real US Pacific-style TZ: Bias 480, DST -60, non-zero
        // transition dates. Bias != 0 and StandardBias == 0 → StandardBias
        // omitted; transition dates present.
        let v = tzi_to_hcs_schema(
            480,
            &name_buf("Pacific Standard Time"),
            &systemtime([0, 11, 0, 1, 2, 0, 0, 0]),
            0,
            &name_buf("Pacific Daylight Time"),
            &systemtime([0, 3, 0, 2, 2, 0, 0, 0]),
            -60,
        );
        assert_eq!(
            v,
            json!({
                "Bias": 480,
                "StandardName": "Pacific Standard Time",
                "StandardDate": { "Month": 11, "Day": 1, "Hour": 2 },
                "DaylightName": "Pacific Daylight Time",
                "DaylightDate": { "Month": 3, "Day": 2, "Hour": 2 },
                "DaylightBias": -60,
            })
        );
    }

    #[test]
    fn no_dst_timezone_omits_transition_dates() {
        // UTC: Bias 0, no DST. All-zero ints/dates → an empty object, exactly
        // hcsschema's real-host output for a no-DST zone (Go omitempty).
        let v = tzi_to_hcs_schema(
            0,
            &name_buf("UTC"),
            &systemtime([0; 8]),
            0,
            &name_buf("UTC"),
            &systemtime([0; 8]),
            0,
        );
        assert_eq!(v, json!({ "StandardName": "UTC", "DaylightName": "UTC" }));
    }
}
