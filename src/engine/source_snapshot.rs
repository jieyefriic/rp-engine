//! Inspection-only YAML snapshots. Never mutate the executable workflow.

use crate::error::EngineError;
use serde_yaml::Value;
use std::collections::HashSet;

const REDACTED: &str = "[REDACTED]";

pub(super) fn redacted_yaml(yaml: &str) -> Result<String, EngineError> {
    let mut value: Value = serde_yaml::from_str(yaml)
        .map_err(|_| EngineError::InvalidWorkflow("Cannot parse source snapshot".into()))?;
    let mut secrets = HashSet::new();
    redact(&mut value, false, &mut secrets);
    redact_aliases(&mut value, &secrets);
    // Re-serialization removes comments. The second pass also removes exact
    // copies of known secrets (including YAML aliases in other fields).
    serde_yaml::to_string(&value)
        .map_err(|_| EngineError::StateError("Cannot serialize source snapshot".into()))
}

fn normalized(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn sensitive(key: &str) -> bool {
    key.contains("password")
        || key.contains("secret")
        || key.ends_with("token")
        || key.ends_with("apikey")
        || key.ends_with("privatekey")
        || matches!(
            key,
            "authorization"
                | "proxyauthorization"
                | "cookie"
                | "setcookie"
                | "accesskey"
                | "accesskeyid"
                | "credentials"
        )
}

fn env_reference(value: &str) -> bool {
    value
        .strip_prefix("${")
        .and_then(|s| s.strip_suffix('}'))
        .map(|name| {
            !name.is_empty()
                && name.bytes().enumerate().all(|(i, c)| {
                    c.is_ascii_alphabetic() || c == b'_' || (i > 0 && c.is_ascii_digit())
                })
        })
        .unwrap_or(false)
}

fn hide(value: &mut Value, secrets: &mut HashSet<String>) {
    match value {
        Value::String(text) if env_reference(text) || text.is_empty() => {}
        Value::Null => {}
        _ => {
            collect_secrets(value, secrets);
            *value = Value::String(REDACTED.into());
        }
    }
}

fn redact(value: &mut Value, headers: bool, secrets: &mut HashSet<String>) {
    match value {
        Value::Mapping(map) => {
            for (key, value) in map {
                let key = normalized(key.as_str().unwrap_or(""));
                if headers
                    || sensitive(&key)
                    || (key == "auth" && !matches!(value, Value::Mapping(_)))
                {
                    hide(value, secrets);
                } else if matches!(
                    key.as_str(),
                    "url" | "baseurl" | "endpoint" | "uri" | "dsn" | "connectionstring"
                ) {
                    if let Value::String(text) = value {
                        if !env_reference(text) {
                            // Conservative for DSNs and signed URLs: preserve only
                            // parseable endpoints with no credentials/query/fragment.
                            let safe = reqwest::Url::parse(text)
                                .map(|url| {
                                    url.username().is_empty()
                                        && url.password().is_none()
                                        && url.query().is_none()
                                        && url.fragment().is_none()
                                })
                                .unwrap_or(false);
                            if !safe {
                                hide(value, secrets);
                            }
                        }
                    } else {
                        redact(value, false, secrets);
                    }
                } else {
                    redact(value, key == "headers", secrets);
                }
            }
        }
        Value::Sequence(items) => {
            for item in items {
                redact(item, headers, secrets);
            }
        }
        Value::Tagged(tagged) => redact(&mut tagged.value, headers, secrets),
        _ => {}
    }
}

fn collect_secrets(value: &Value, secrets: &mut HashSet<String>) {
    match value {
        Value::String(text) if !text.is_empty() && !env_reference(text) => {
            secrets.insert(text.clone());
        }
        Value::Mapping(map) => {
            for value in map.values() {
                collect_secrets(value, secrets);
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_secrets(item, secrets);
            }
        }
        Value::Tagged(tagged) => collect_secrets(&tagged.value, secrets),
        _ => {}
    }
}

fn redact_aliases(value: &mut Value, secrets: &HashSet<String>) {
    match value {
        Value::String(text) if secrets.contains(text) => *text = REDACTED.into(),
        Value::Mapping(map) => {
            for value in map.values_mut() {
                redact_aliases(value, secrets);
            }
        }
        Value::Sequence(items) => {
            for item in items {
                redact_aliases(item, secrets);
            }
        }
        Value::Tagged(tagged) => redact_aliases(&mut tagged.value, secrets),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_headers_urls_and_comments_are_removed() {
        let yaml = r#"
# comment-only-secret
shared: &credential alias-secret
providers:
  openai: {api_key: inline-provider-secret, base_url: "https://example.invalid/v1"}
data_sources:
  db:
    config:
      password: db-secret
      url: "postgresql://user:dsn-secret@example.invalid/db"
      access_key_id: access-id
      secret_access_key: s3-secret
      headers: {X-Custom-Auth: header-secret, Accept: application/json}
mcp_servers:
  server:
    url: "https://example.invalid/mcp?key=query-secret"
    auth: {type: bearer, token: mcp-secret}
extra: {api_key: *credential}
subgraphs:
  child:
    providers:
      custom: {apiKey: nested-secret}
nodes: [{id: start, type: start}, {id: response, type: response}]
"#;
        let snapshot = redacted_yaml(yaml).unwrap();
        for secret in [
            "comment-only-secret",
            "inline-provider-secret",
            "db-secret",
            "dsn-secret",
            "access-id",
            "s3-secret",
            "header-secret",
            "query-secret",
            "mcp-secret",
            "nested-secret",
            "alias-secret",
        ] {
            assert!(!snapshot.contains(secret), "snapshot leaked {secret}");
        }
        assert!(snapshot.contains("https://example.invalid/v1"));
        assert!(snapshot.contains("id: response"));
    }

    #[test]
    fn environment_references_and_regular_parameters_are_preserved() {
        let snapshot = redacted_yaml(
            "api_key: '${MODEL_KEY}'\npassword: '${DB_PASSWORD}'\nurl: '${DB_URL}'\nmax_tokens: 2048\ntemperature: 0.7\n"
        ).unwrap();
        let value: Value = serde_yaml::from_str(&snapshot).unwrap();
        assert_eq!(value["api_key"].as_str(), Some("${MODEL_KEY}"));
        assert_eq!(value["password"].as_str(), Some("${DB_PASSWORD}"));
        assert_eq!(value["url"].as_str(), Some("${DB_URL}"));
        assert_eq!(value["max_tokens"].as_u64(), Some(2048));
        assert!(!env_reference("${KEY}-literal"));
    }
}
