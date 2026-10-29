//! Decision provider configuration and protocol conversion, independent of task policy.
use anyhow::{Result, ensure};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Protocol {
    Typesafe,
    OpenaiDecisions,
}
impl Protocol {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Typesafe => "typesafe",
            Self::OpenaiDecisions => "openai_decisions",
        }
    }
}
pub(crate) fn valid_provider(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
}
pub(crate) fn enabled(value: Option<String>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
}
pub(crate) fn nonblank(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}
pub(crate) struct DecisionConfig {
    pub provider: String,
    pub protocol: Protocol,
    pub url: reqwest::Url,
    pub authorization: reqwest::header::HeaderValue,
    pub model: String,
    pub threshold: f64,
}
impl DecisionConfig {
    pub(crate) fn read_memory(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let provider =
            nonblank(get("TASKIX_DECISION_PROVIDER")).unwrap_or_else(|| "typesafe".into());
        if !valid_provider(&provider) {
            return None;
        }
        let protocol =
            nonblank(get("TASKIX_DECISION_PROTOCOL")).or_else(|| match provider.as_str() {
                "typesafe" => Some("typesafe".into()),
                "openai" => Some("openai_decisions".into()),
                _ => None,
            })?;
        let protocol = match protocol.as_str() {
            "typesafe" => Protocol::Typesafe,
            "openai_decisions" => Protocol::OpenaiDecisions,
            _ => return None,
        };
        let legacy = provider == "typesafe" && protocol == Protocol::Typesafe;
        let openai = provider == "openai" && protocol == Protocol::OpenaiDecisions;
        if !enabled(
            get("TASKIX_DECISION_ENABLED")
                .or_else(|| legacy.then(|| get("TASKIX_JEV_ENABLED")).flatten()),
        ) {
            return None;
        }
        let value = |suffix| {
            nonblank(get(&format!("TASKIX_DECISION_{suffix}"))).or_else(|| {
                if legacy {
                    nonblank(get(&format!("TASKIX_JEV_{suffix}")))
                } else {
                    None
                }
            })
        };
        let url = value("URL")
            .or_else(|| openai.then(|| "https://api.openai.com/v1/decisions".into()))?;
        let url = reqwest::Url::parse(&url).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let key = value("API_KEY").or_else(|| {
            if openai {
                nonblank(get("OPENAI_API_KEY"))
            } else {
                None
            }
        })?;
        if !key.bytes().all(|c| (0x20..=0x7e).contains(&c)) {
            return None;
        }
        let mut authorization =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).ok()?;
        authorization.set_sensitive(true);
        let model = value("MODEL").or_else(|| {
            if legacy {
                Some("jev-latest".into())
            } else if openai {
                Some("gpt-6-luna".into())
            } else {
                None
            }
        })?;
        let threshold = nonblank(get("TASKIX_MEMORY_DECISION_MIN_CONFIDENCE"))
            .or_else(|| {
                if legacy {
                    nonblank(get("TASKIX_MEMORY_JEV_MIN_CONFIDENCE"))
                } else {
                    None
                }
            })
            .map_or(Some(0.75), |v| v.parse::<f64>().ok())?;
        if !threshold.is_finite() || !(0.5..=1.0).contains(&threshold) {
            return None;
        }
        Some(Self {
            provider,
            protocol,
            url,
            authorization,
            model,
            threshold,
        })
    }
    pub(crate) fn encode(&self, request: &Value) -> Result<String> {
        if self.protocol == Protocol::Typesafe {
            return Ok(request.to_string());
        }
        let questions = request["questions"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Invalid decision questions"))?;
        ensure!(
            !questions.is_empty() && questions.len() <= 400,
            "Invalid decision question count"
        );
        let questions: Result<Vec<_>> = questions.iter().map(|(name, q)| {
            let criteria = q["criteria"].as_object().ok_or_else(|| anyhow::anyhow!("Invalid decision choices"))?;
            ensure!(q["type"] == "choice" && (2..=255).contains(&criteria.len()), "Invalid decision choices");
            let choices: Vec<_> = criteria.iter().map(|(value, description)| json!({"value":value,"description":description.as_str().map_or_else(|| description.to_string(), str::to_owned)})).collect();
            Ok(json!({"name":name,"type":"choice","instructions":q["instructions"],"choices":choices}))
        }).collect();
        Ok(json!({"model":self.model,"input":[{"role":"user","content":[{"type":"input_json","data":request["state"]}]}],"questions":questions?}).to_string())
    }
    pub(crate) fn decode(&self, reply: Value) -> Result<Value> {
        if self.protocol == Protocol::Typesafe {
            return Ok(reply);
        }
        let entries = reply["answers"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid decision answers"))?;
        let mut answers = Map::new();
        for answer in entries {
            let name = answer["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid decision answer name"))?;
            ensure!(
                !answers.contains_key(name),
                "Duplicate decision answer name"
            );
            let mut normalized = answer.clone();
            if answer["type"] == "choice" {
                let items = answer["probabilities"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("Invalid decision distribution"))?;
                let mut probabilities = Map::new();
                for item in items {
                    let key = item["value"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid decision probability key"))?;
                    ensure!(
                        !probabilities.contains_key(key),
                        "Duplicate decision probability key"
                    );
                    probabilities.insert(key.into(), item["probability"].clone());
                }
                normalized["probabilities"] = Value::Object(probabilities);
            }
            answers.insert(name.into(), normalized);
        }
        Ok(json!({"answers":answers}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_configuration_contract_matches_javascript() {
        let cases: Value = serde_json::from_str(include_str!(
            "../../../plugins/taskix-manager/tests/fixtures/decision-config.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let config =
                DecisionConfig::read_memory(|key| case["env"][key].as_str().map(str::to_owned));
            let expected = &case["expected"];
            if expected.is_null() {
                assert!(config.is_none(), "{}", case["name"]);
            } else {
                let config = config.unwrap_or_else(|| panic!("{}", case["name"]));
                assert_eq!(
                    json!({"provider":config.provider,"protocol":config.protocol.as_str(),"url":config.url.as_str(),"model":config.model,"key":config.authorization.to_str().unwrap().strip_prefix("Bearer ").unwrap()}),
                    *expected,
                    "{}",
                    case["name"]
                );
                assert!((config.threshold - 0.75).abs() < f64::EPSILON);
            }
        }
    }
    #[test]
    fn memory_threshold_is_independent_and_neutral_value_overrides_legacy() {
        let env = std::collections::BTreeMap::from([
            ("TASKIX_JEV_ENABLED", "true"),
            ("TASKIX_JEV_URL", "https://typesafe.test/evaluate"),
            ("TASKIX_JEV_API_KEY", "key"),
            ("TASKIX_MEMORY_JEV_MIN_CONFIDENCE", ".9"),
            ("TASKIX_MEMORY_DECISION_MIN_CONFIDENCE", ".8"),
            ("TASKIX_DECISION_MIN_CONFIDENCE", "invalid"),
        ]);
        let config =
            DecisionConfig::read_memory(|key| env.get(key).map(ToString::to_string)).unwrap();
        assert!((config.threshold - 0.8).abs() < f64::EPSILON);
    }
}
