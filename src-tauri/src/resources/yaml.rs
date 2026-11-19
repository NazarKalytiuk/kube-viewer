//! Object → YAML for the detail view.

use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};

const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// Converts an object into display JSON: drops `managedFields` (noise) and,
/// for Secrets, replaces every value with a size placeholder. The
/// last-applied annotation is masked too because `kubectl apply` stores the
/// full Secret manifest there.
pub fn display_value<K: Serialize>(obj: &K, is_secret: bool) -> AppResult<Value> {
    let mut value = serde_json::to_value(obj).map_err(|e| AppError::Internal(e.to_string()))?;
    if let Some(meta) = value.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
        if is_secret
            && let Some(ann) = meta.get_mut("annotations").and_then(Value::as_object_mut)
            && let Some(v) = ann.get_mut(LAST_APPLIED)
        {
            *v = Value::String("<hidden>".into());
        }
    }
    if is_secret {
        for field in ["data", "stringData"] {
            if let Some(map) = value.get_mut(field).and_then(Value::as_object_mut) {
                for v in map.values_mut() {
                    let len = v
                        .as_str()
                        .map(|s| decoded_len(s, field == "data"))
                        .unwrap_or(0);
                    *v = Value::String(format!("<hidden: {len} bytes>"));
                }
            }
        }
    }
    Ok(value)
}

/// Byte length of a Secret value: `data` is base64, `stringData` is plain.
fn decoded_len(s: &str, base64: bool) -> usize {
    if !base64 {
        return s.len();
    }
    let trimmed = s.trim_end_matches('=');
    trimmed.len() * 3 / 4
}

pub fn to_yaml(value: &Value) -> AppResult<String> {
    serde_saphyr::to_string(value).map_err(|e| AppError::Internal(format!("yaml: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{ConfigMap, Secret};
    use serde_json::json;

    #[test]
    fn strips_managed_fields_and_keeps_order_of_content() {
        let cm: ConfigMap = serde_json::from_value(json!({
            "metadata": { "name": "cfg", "managedFields": [{ "manager": "kubectl" }] },
            "data": { "enabled": "true", "port": "8080" }
        }))
        .unwrap();
        let value = display_value(&cm, false).unwrap();
        assert!(value["metadata"].get("managedFields").is_none());
        let yaml = to_yaml(&value).unwrap();
        assert!(yaml.contains("name: cfg"), "{yaml}");
        // Strings that look like scalars must stay strings.
        assert!(
            yaml.contains("enabled: \"true\"") || yaml.contains("enabled: 'true'"),
            "{yaml}"
        );
    }

    #[test]
    fn masks_secret_values_and_last_applied() {
        let secret: Secret = serde_json::from_value(json!({
            "metadata": { "name": "db", "annotations": { LAST_APPLIED: "{\"data\":{\"password\":\"c2VjcmV0\"}}" } },
            "data": { "password": "c2VjcmV0" },
            "type": "Opaque"
        }))
        .unwrap();
        let yaml = to_yaml(&display_value(&secret, true).unwrap()).unwrap();
        assert!(!yaml.contains("c2VjcmV0"), "secret leaked: {yaml}");
        assert!(yaml.contains("<hidden: 6 bytes>"), "{yaml}");
    }
}
