//! Strict parsing of complete account/rateLimits/read responses from CLI 0.145.0.
//! Notifications are sparse updates and must trigger another read, not use this parser.
use crate::scheduler::{
    CreditsSnapshot, QuotaBucket, QuotaSnapshot, QuotaWindow, SpendControlWindow,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;

type Result<T> = std::result::Result<T, String>;
const INVALID: &str = "额度返回格式不兼容，已停止自动续跑";
const MAX_TIMESTAMP: i64 = 253_402_300_799; // Last UTC second representable in year 9999.

/// Display-only observations, deliberately not accepted by the scheduler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaTelemetry {
    pub observed_at: i64,
    pub source: &'static str,
    pub buckets: Vec<QuotaBucketTelemetry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucketTelemetry {
    pub id: String,
    pub label: Option<String>,
    pub windows: Vec<QuotaWindowTelemetry>,
    /// True only when a known server field explicitly reports a limit.
    /// False is not proof of availability or permission to execute.
    pub blocked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindowTelemetry {
    pub kind: &'static str,
    pub duration_mins: Option<i64>,
    pub used_percent: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Display known fields even when a future protocol field makes the strict
/// execution parser reject the snapshot. Missing/invalid values stay unknown.
pub fn parse_telemetry(value: &Value, observed_at: i64) -> Option<QuotaTelemetry> {
    if !(0..=MAX_TIMESTAMP).contains(&observed_at) {
        return None;
    }
    let response = value.as_object()?;
    let buckets = match response.get("rateLimitsByLimitId") {
        Some(Value::Object(entries)) => entries
            .iter()
            .filter_map(|(key, value)| display_bucket(value, Some(key)))
            .collect::<Vec<_>>(),
        None | Some(Value::Null) => {
            vec![display_bucket(response.get("rateLimits")?, None)?]
        }
        _ => return None,
    };
    if buckets.is_empty() {
        return None;
    }
    Some(QuotaTelemetry {
        observed_at,
        source: "local",
        buckets,
    })
}

fn display_bucket(value: &Value, map_key: Option<&String>) -> Option<QuotaBucketTelemetry> {
    let value = value.as_object()?;
    let explicit_id = value.get("limitId").filter(|value| !value.is_null());
    let id = match (explicit_id, map_key) {
        (Some(id), key) => {
            let id = identifier(id).ok()?;
            if key.is_some_and(|key| key != &id) {
                return None;
            }
            id
        }
        (None, Some(key)) if !key.is_empty() && key.trim() == key => key.clone(),
        _ => return None,
    };
    let label = value
        .get("limitName")
        .and_then(Value::as_str)
        .filter(|label| !label.trim().is_empty())
        .map(str::to_owned);
    let windows = ["primary", "secondary"]
        .into_iter()
        .filter_map(|kind| {
            let value = value.get(kind)?.as_object()?;
            Some(QuotaWindowTelemetry {
                kind,
                duration_mins: value
                    .get("windowDurationMins")
                    .and_then(Value::as_i64)
                    .filter(|duration| *duration > 0 && *duration <= 9_007_199_254_740_991),
                used_percent: value
                    .get("usedPercent")
                    .and_then(|value| percentage(value).ok()),
                resets_at: value
                    .get("resetsAt")
                    .and_then(|value| timestamp(value).ok()),
            })
        })
        .collect::<Vec<_>>();
    let blocked = windows
        .iter()
        .any(|window| window.used_percent == Some(100))
        || value.get("spendControlReached").and_then(Value::as_bool) == Some(true)
        || value
            .get("individualLimit")
            .and_then(|value| value.get("remainingPercent"))
            .and_then(|value| percentage(value).ok())
            == Some(0)
        || value
            .get("rateLimitReachedType")
            .is_some_and(|value| reached_type(value).is_ok());
    Some(QuotaBucketTelemetry {
        id,
        label,
        windows,
        blocked,
    })
}

/// Preserve every returned bucket. The caller must supply a conservative,
/// explicitly verified bucket policy; this parser never guesses from a model.
pub fn parse_response(value: &Value, observed_at: i64) -> Result<QuotaSnapshot> {
    if !(0..=MAX_TIMESTAMP).contains(&observed_at) {
        return Err(INVALID.into());
    }
    let response = object(
        value,
        &["rateLimits", "rateLimitsByLimitId", "rateLimitResetCredits"],
    )?;
    let compatibility = bucket(&response["rateLimits"])?;
    nullable(&response["rateLimitResetCredits"], validate_reset_credits)?;

    let buckets = match &response["rateLimitsByLimitId"] {
        Value::Null => vec![compatibility],
        Value::Object(entries) if !entries.is_empty() => {
            let mut buckets = Vec::with_capacity(entries.len());
            let mut ids = HashSet::new();
            for (key, value) in entries {
                let parsed = bucket(value)?;
                if key != &parsed.id || !ids.insert(parsed.id.clone()) {
                    return Err("额度标识冲突，已停止自动续跑".into());
                }
                buckets.push(parsed);
            }
            // The compatibility view is a duplicate of one map entry, never an
            // additional source of capacity or permission to ignore other buckets.
            if entries.get(&compatibility.id) != Some(&response["rateLimits"]) {
                return Err("额度兼容视图与完整快照不一致，已停止自动续跑".into());
            }
            buckets
        }
        _ => return Err("未取得完整额度桶，已停止自动续跑".into()),
    };
    Ok(QuotaSnapshot {
        observed_at,
        buckets,
    })
}

fn object<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a Map<String, Value>> {
    let value = value.as_object().ok_or(INVALID)?;
    if value.len() != fields.len() || fields.iter().any(|field| !value.contains_key(*field)) {
        return Err(INVALID.into());
    }
    Ok(value)
}

fn nullable<'a, T>(
    value: &'a Value,
    parse: impl FnOnce(&'a Value) -> Result<T>,
) -> Result<Option<T>> {
    if value.is_null() {
        Ok(None)
    } else {
        parse(value).map(Some)
    }
}

fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| INVALID.into())
}

fn identifier(value: &Value) -> Result<String> {
    let value = text(value)?;
    if value.is_empty() || value.trim() != value {
        return Err("额度标识缺失或无效，已停止自动续跑".into());
    }
    Ok(value.into())
}

fn boolean(value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(|| INVALID.into())
}

fn percentage(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .filter(|number| (0..=100).contains(number))
        .ok_or_else(|| INVALID.into())
}

fn timestamp(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .filter(|number| (0..=MAX_TIMESTAMP).contains(number))
        .ok_or_else(|| INVALID.into())
}

fn enum_value(value: &Value, allowed: &[&str]) -> Result<String> {
    let value = text(value)?;
    if !allowed.contains(&value) {
        return Err(INVALID.into());
    }
    Ok(value.into())
}

fn reached_type(value: &Value) -> Result<String> {
    enum_value(
        value,
        &[
            "rate_limit_reached",
            "workspace_owner_credits_depleted",
            "workspace_member_credits_depleted",
            "workspace_owner_usage_limit_reached",
            "workspace_member_usage_limit_reached",
        ],
    )
}

fn window(value: &Value) -> Result<QuotaWindow> {
    let value = object(value, &["usedPercent", "windowDurationMins", "resetsAt"])?;
    nullable(&value["windowDurationMins"], |value| {
        value
            .as_i64()
            .filter(|value| *value > 0)
            .ok_or(INVALID.into())
    })?;
    Ok(QuotaWindow {
        used_percent: percentage(&value["usedPercent"])?,
        resets_at: nullable(&value["resetsAt"], timestamp)?,
    })
}

fn credits(value: &Value) -> Result<CreditsSnapshot> {
    let value = object(value, &["hasCredits", "unlimited", "balance"])?;
    nullable(&value["balance"], text)?;
    Ok(CreditsSnapshot {
        has_credits: boolean(&value["hasCredits"])?,
        unlimited: boolean(&value["unlimited"])?,
    })
}

fn individual_limit(value: &Value) -> Result<SpendControlWindow> {
    let value = object(value, &["limit", "used", "remainingPercent", "resetsAt"])?;
    text(&value["limit"])?;
    text(&value["used"])?;
    Ok(SpendControlWindow {
        remaining_percent: percentage(&value["remainingPercent"])?,
        // This field is non-nullable in the supported protocol.
        resets_at: Some(timestamp(&value["resetsAt"])?),
    })
}

fn bucket(value: &Value) -> Result<QuotaBucket> {
    let value = object(
        value,
        &[
            "limitId",
            "limitName",
            "primary",
            "secondary",
            "credits",
            "individualLimit",
            "spendControlReached",
            "planType",
            "rateLimitReachedType",
        ],
    )?;
    nullable(&value["limitName"], text)?;
    nullable(&value["planType"], |value| {
        enum_value(
            value,
            &[
                "free",
                "go",
                "plus",
                "pro",
                "prolite",
                "team",
                "self_serve_business_usage_based",
                "business",
                "enterprise_cbp_usage_based",
                "enterprise",
                "edu",
                "unknown",
            ],
        )
    })?;
    Ok(QuotaBucket {
        // Null identifiers are not inferred from the compatibility view or
        // map key: the server can synthesize "codex" for an unidentified bucket.
        id: identifier(&value["limitId"])?,
        primary: nullable(&value["primary"], window)?,
        secondary: nullable(&value["secondary"], window)?,
        credits: nullable(&value["credits"], credits)?,
        individual_limit: nullable(&value["individualLimit"], individual_limit)?,
        spend_control_reached: nullable(&value["spendControlReached"], boolean)?,
        rate_limit_reached_type: nullable(&value["rateLimitReachedType"], reached_type)?,
    })
}

fn validate_reset_credits(value: &Value) -> Result<()> {
    let value = object(value, &["availableCount", "credits"])?;
    value["availableCount"].as_u64().ok_or(INVALID)?;
    if !value["credits"].is_null() {
        let rows = value["credits"].as_array().ok_or(INVALID)?;
        let mut ids = HashSet::new();
        for row in rows {
            let row = object(
                row,
                &[
                    "id",
                    "resetType",
                    "status",
                    "grantedAt",
                    "expiresAt",
                    "title",
                    "description",
                ],
            )?;
            if !ids.insert(identifier(&row["id"])?) {
                return Err(INVALID.into());
            }
            enum_value(&row["resetType"], &["codexRateLimits", "unknown"])?;
            enum_value(
                &row["status"],
                &["available", "redeeming", "redeemed", "unknown"],
            )?;
            timestamp(&row["grantedAt"])?;
            nullable(&row["expiresAt"], timestamp)?;
            nullable(&row["title"], text)?;
            nullable(&row["description"], text)?;
        }
    }
    // Reset credits are only validated, never counted as available quota.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{self, Decision, Ownership, SafetyGate, TaskRecord, TaskState};
    use serde_json::json;

    fn sample_bucket(id: &str, used: i64, reset: Value) -> Value {
        json!({
            "limitId":id, "limitName":null,
            "primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":reset},
            "secondary":null, "credits":null, "individualLimit":null,
            "spendControlReached":false, "planType":"plus", "rateLimitReachedType":null
        })
    }

    fn response(buckets: Vec<Value>) -> Value {
        let compatibility = buckets[0].clone();
        let entries: Map<String, Value> = buckets
            .into_iter()
            .map(|bucket| (bucket["limitId"].as_str().unwrap().to_owned(), bucket))
            .collect();
        json!({
            "rateLimits":compatibility,
            "rateLimitsByLimitId":entries,
            "rateLimitResetCredits":null
        })
    }

    fn decide(quota: &QuotaSnapshot) -> Decision {
        let mut task = TaskRecord::new("task".into(), "fixture".into(), "/fixture".into());
        task.enabled = true;
        task.needs_reconciliation = false;
        task.failed_turn_id = Some("failed-turn".into());
        task.state = TaskState::WaitingQuota;
        // This fixture explicitly selects every returned bucket; the parser
        // itself neither chooses model associations nor grants execution rights.
        task.required_limit_ids = quota
            .buckets
            .iter()
            .map(|bucket| bucket.id.clone())
            .collect();
        scheduler::evaluate(
            &task,
            quota,
            100,
            &SafetyGate {
                ownership: Ownership::ExclusiveStopped,
                execution_configuration_preserved: true,
            },
        )
    }

    #[test]
    fn every_bucket_and_window_contributes_to_latest_reset() {
        let mut second = sample_bucket("separate-limit", 100, json!(300));
        second["secondary"] = json!({"usedPercent":100,"windowDurationMins":10080,"resetsAt":500});
        let value = response(vec![sample_bucket("codex", 100, json!(200)), second]);
        let parsed = parse_response(&value, 100).unwrap();
        assert_eq!(parsed.observed_at, 100);
        assert_eq!(parsed.buckets.len(), 2);
        assert_eq!(decide(&parsed), Decision::WaitUntil(Some(500)));
    }

    #[test]
    fn compatibility_bucket_cannot_hide_an_exhausted_second_bucket() {
        let parsed = parse_response(
            &response(vec![
                sample_bucket("codex", 10, json!(200)),
                sample_bucket("separate-limit", 100, json!(400)),
            ]),
            100,
        )
        .unwrap();
        assert_eq!(decide(&parsed), Decision::WaitUntil(Some(400)));
    }

    #[test]
    fn null_reset_is_preserved_and_never_replaced_with_a_fixed_window() {
        let parsed = parse_response(
            &response(vec![sample_bucket("codex", 100, Value::Null)]),
            100,
        )
        .unwrap();
        assert_eq!(parsed.buckets[0].primary.as_ref().unwrap().resets_at, None);
        assert_eq!(decide(&parsed), Decision::WaitUntil(None));
    }

    #[test]
    fn unknown_spend_state_and_missing_windows_remain_blocking() {
        let mut bucket = sample_bucket("codex", 0, json!(200));
        bucket["spendControlReached"] = Value::Null;
        let parsed = parse_response(&response(vec![bucket.clone()]), 100).unwrap();
        assert_eq!(parsed.buckets[0].spend_control_reached, None);
        assert!(matches!(decide(&parsed), Decision::NeedsAttention(_)));
        bucket["spendControlReached"] = json!(false);
        bucket["primary"] = Value::Null;
        let parsed = parse_response(&response(vec![bucket]), 100).unwrap();
        assert!(matches!(decide(&parsed), Decision::NeedsAttention(_)));
    }

    #[test]
    fn credits_never_override_subscription_or_spend_controls() {
        let mut bucket = sample_bucket("codex", 100, json!(200));
        bucket["credits"] = json!({"hasCredits":true,"unlimited":true,"balance":"9999"});
        let mut value = response(vec![bucket.clone()]);
        value["rateLimitResetCredits"] = json!({"availableCount":99,"credits":null});
        assert_eq!(
            decide(&parse_response(&value, 100).unwrap()),
            Decision::WaitUntil(Some(200))
        );
        bucket["primary"]["usedPercent"] = json!(0);
        bucket["individualLimit"] =
            json!({"limit":"100","used":"100","remainingPercent":0,"resetsAt":450});
        assert_eq!(
            decide(&parse_response(&response(vec![bucket.clone()]), 100).unwrap()),
            Decision::WaitUntil(Some(450))
        );
        bucket["spendControlReached"] = json!(true);
        assert!(matches!(
            decide(&parse_response(&response(vec![bucket]), 100).unwrap()),
            Decision::NeedsAttention(_)
        ));
    }

    #[test]
    fn only_an_explicit_identified_single_bucket_can_replace_a_null_map() {
        let mut value = response(vec![sample_bucket("codex", 25, json!(200))]);
        value["rateLimitsByLimitId"] = Value::Null;
        assert_eq!(parse_response(&value, 100).unwrap().buckets.len(), 1);
        for id in [Value::Null, json!(""), json!(" codex ")] {
            value["rateLimits"]["limitId"] = id;
            assert!(parse_response(&value, 100).is_err());
        }
    }

    #[test]
    fn missing_empty_conflicting_and_mismatched_map_views_are_rejected() {
        let valid = response(vec![sample_bucket("codex", 10, json!(200))]);
        let mut missing = valid.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("rateLimitsByLimitId");
        assert!(parse_response(&missing, 100).is_err());
        for malformed in [json!({}), json!([]), json!(false)] {
            let mut value = valid.clone();
            value["rateLimitsByLimitId"] = malformed;
            assert!(parse_response(&value, 100).is_err());
        }
        let mut different = valid.clone();
        different["rateLimits"]["primary"]["usedPercent"] = json!(90);
        assert!(parse_response(&different, 100).is_err());
        let mut mismatch = valid.clone();
        mismatch["rateLimitsByLimitId"]["other"] = valid["rateLimits"].clone();
        assert!(parse_response(&mismatch, 100).is_err());
        let mut unidentified = valid.clone();
        unidentified["rateLimitsByLimitId"]["codex"]["limitId"] = Value::Null;
        assert!(parse_response(&unidentified, 100).is_err());
        let mut unrelated = valid;
        unrelated["rateLimits"]["limitId"] = json!("unreturned-bucket");
        assert!(parse_response(&unrelated, 100).is_err());
    }

    #[test]
    fn future_fields_and_unknown_reached_reasons_fail_closed() {
        let valid = response(vec![sample_bucket("codex", 0, json!(200))]);
        for path in ["", "/rateLimits", "/rateLimits/primary"] {
            let mut value = valid.clone();
            value.pointer_mut(path).unwrap()["futurePolicy"] = json!(true);
            assert!(parse_response(&value, 100).is_err());
        }
        let mut bucket = sample_bucket("codex", 0, json!(200));
        bucket["rateLimitReachedType"] = json!("new_workspace_cap");
        assert!(parse_response(&response(vec![bucket]), 100).is_err());
    }

    #[test]
    fn malformed_percentages_times_and_nullable_fields_are_not_defaulted() {
        for invalid in [json!(-1), json!(101), json!(0.5), json!("0"), Value::Null] {
            let mut bucket = sample_bucket("codex", 0, json!(200));
            bucket["primary"]["usedPercent"] = invalid;
            assert!(parse_response(&response(vec![bucket]), 100).is_err());
        }
        for invalid in [json!(-1), json!(253402300800_i64), json!(1.5), json!("200")] {
            let mut bucket = sample_bucket("codex", 0, json!(200));
            bucket["primary"]["resetsAt"] = invalid;
            assert!(parse_response(&response(vec![bucket]), 100).is_err());
        }
        for field in ["resetsAt", "windowDurationMins"] {
            let mut bucket = sample_bucket("codex", 0, json!(200));
            bucket["primary"].as_object_mut().unwrap().remove(field);
            assert!(parse_response(&response(vec![bucket]), 100).is_err());
        }
        let mut bucket = sample_bucket("codex", 0, json!(200));
        bucket
            .as_object_mut()
            .unwrap()
            .remove("spendControlReached");
        assert!(parse_response(&response(vec![bucket]), 100).is_err());
        let mut bucket = sample_bucket("codex", 0, json!(200));
        bucket["credits"] = json!({"hasCredits":null,"unlimited":false,"balance":null});
        assert!(parse_response(&response(vec![bucket]), 100).is_err());
        let valid = response(vec![sample_bucket("codex", 0, json!(200))]);
        assert!(parse_response(&valid, -1).is_err());
    }

    #[test]
    fn known_workspace_reasons_parse_but_cannot_allow_execution() {
        for reason in [
            "workspace_owner_credits_depleted",
            "workspace_member_credits_depleted",
            "workspace_owner_usage_limit_reached",
            "workspace_member_usage_limit_reached",
        ] {
            let mut bucket = sample_bucket("codex", 0, json!(200));
            bucket["rateLimitReachedType"] = json!(reason);
            let parsed = parse_response(&response(vec![bucket]), 100).unwrap();
            assert!(matches!(decide(&parsed), Decision::NeedsAttention(_)));
        }
    }

    #[test]
    fn telemetry_keeps_known_fields_when_strict_execution_parsing_refuses() {
        let mut value = response(vec![sample_bucket("codex", 37, json!(250))]);
        value["futurePolicy"] = json!({"notUnderstood":true});
        assert!(parse_response(&value, 100).is_err());
        let display = parse_telemetry(&value, 100).unwrap();
        assert_eq!(display.source, "local");
        assert_eq!(display.observed_at, 100);
        assert_eq!(
            display.buckets[0].windows,
            vec![QuotaWindowTelemetry {
                kind: "primary",
                duration_mins: Some(300),
                used_percent: Some(37),
                resets_at: Some(250),
            }]
        );
        let serialized = serde_json::to_value(display).unwrap();
        assert_eq!(serialized["buckets"][0]["windows"][0]["usedPercent"], 37);
        assert_eq!(serialized["buckets"][0]["windows"][0]["durationMins"], 300);
        assert_eq!(serialized["observedAt"], 100);
    }

    #[test]
    fn telemetry_does_not_turn_invalid_or_missing_measurements_into_zero() {
        for invalid in [Value::Null, json!(-1), json!(101), json!("90"), json!(90.5)] {
            let mut value = response(vec![sample_bucket("codex", 10, json!(200))]);
            value["rateLimitsByLimitId"]["codex"]["primary"]["usedPercent"] = invalid;
            value["rateLimitsByLimitId"]["codex"]["primary"]["resetsAt"] = Value::Null;
            value["rateLimitsByLimitId"]["codex"]["primary"]["windowDurationMins"] = json!(0);
            let display = parse_telemetry(&value, 100).unwrap();
            assert_eq!(display.buckets[0].windows[0].used_percent, None);
            assert_eq!(display.buckets[0].windows[0].resets_at, None);
            assert_eq!(display.buckets[0].windows[0].duration_mins, None);
        }
        assert!(parse_telemetry(&json!({}), 100).is_none());
        assert!(parse_telemetry(&json!({"rateLimitsByLimitId":{}}), 100).is_none());
        let mut value = response(vec![sample_bucket("codex", 10, json!(200))]);
        value["rateLimitsByLimitId"]["codex"]["limitId"] = json!("wrong-id");
        assert!(parse_telemetry(&value, 100).is_none());
    }

    #[test]
    fn telemetry_prefers_the_complete_map_and_marks_explicit_limits() {
        let mut value = response(vec![
            sample_bucket("codex", 20, json!(200)),
            sample_bucket("extra", 100, json!(300)),
        ]);
        value["rateLimits"]["primary"]["usedPercent"] = json!(0);
        assert!(parse_response(&value, 100).is_err());
        let display = parse_telemetry(&value, 100).unwrap();
        assert_eq!(display.buckets.len(), 2);
        let extra = display
            .buckets
            .iter()
            .find(|bucket| bucket.id == "extra")
            .unwrap();
        assert!(extra.blocked);
        let first = display
            .buckets
            .iter()
            .find(|bucket| bucket.id == "codex")
            .unwrap();
        assert!(!first.blocked);
        assert_eq!(first.windows[0].used_percent, Some(20));

        value["rateLimitsByLimitId"]["codex"]["spendControlReached"] = json!(true);
        let display = parse_telemetry(&value, 100).unwrap();
        assert!(display.buckets.iter().all(|bucket| bucket.blocked));
    }
}
