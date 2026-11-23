use std::time::Duration;

use llm::{ChoiceAnswer, ChoiceQuestion, DecisionModel, LlmError, Usage};
use serde_json::{json, Map, Value};
use storage::{DiscoveredModel, Model, Provider};

/// Returns true if `base_url` targets Cloudflare Workers AI's `/ai/run` REST
/// endpoint (e.g. `https://api.cloudflare.com/client/v4/accounts/<id>/ai/run`).
pub fn is_cloudflare_ai_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = url.path().trim_end_matches('/').to_ascii_lowercase();
    host == "api.cloudflare.com"
        && path.starts_with("/client/v4/accounts/")
        && path.ends_with("/ai/run")
}

/// Built-in Clef decision models hosted on Cloudflare Workers AI.
pub fn cloudflare_decision_models() -> Vec<DiscoveredModel> {
    vec![
        DiscoveredModel {
            id: "@cf/cloudflare/clef-flash".into(),
            context_window: Some(24_576),
        },
        DiscoveredModel {
            id: "@cf/cloudflare/clef".into(),
            context_window: Some(65_536),
        },
    ]
}

/// Generic HTTP client for System One / Jev-compatible decision models
/// (Cloudflare Workers AI Clef / Clef-flash, Typesafe Jev, and self-hosted
/// Clef / Kev / Laya endpoints).
#[derive(Clone)]
pub struct DecisionClient {
    http: reqwest::Client,
    endpoint: String,
    model_field: String,
    api_key: String,
}

impl DecisionClient {
    pub fn new(provider: &Provider, api_key: &str, model: &Model) -> Self {
        let (endpoint, model_field) = resolve_decision_endpoint(&provider.base_url, &model.id);
        let http = reqwest::Client::builder()
            // Keep well under the 5s Discord classification budget so a stalled
            // decision endpoint still leaves time for Utility LLM fallback.
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            endpoint,
            model_field,
            api_key: api_key.to_owned(),
        }
    }
}

fn resolve_decision_endpoint(base_url: &str, model_id: &str) -> (String, String) {
    let trimmed = base_url.trim();
    if is_cloudflare_ai_url(trimmed) {
        if let Ok(mut url) = reqwest::Url::parse(trimmed) {
            let model_path = model_id.trim_start_matches('/');
            let base_path = url.path().trim_end_matches('/').to_owned();
            url.set_path(&format!("{base_path}/{model_path}"));
            let short = model_path.rsplit('/').next().unwrap_or(model_path);
            return (url.into(), short.to_owned());
        }
    }
    (trimmed.to_owned(), model_id.to_owned())
}

#[async_trait::async_trait]
impl DecisionModel for DecisionClient {
    async fn choose(
        &self,
        state: &str,
        question: ChoiceQuestion<'_>,
    ) -> Result<ChoiceAnswer, LlmError> {
        let mut criteria = Map::new();
        for (key, description) in question.criteria {
            criteria.insert((*key).to_owned(), Value::String((*description).to_owned()));
        }
        let body = json!({
            "model": self.model_field,
            "state": state,
            "questions": {
                "decision": {
                    "type": "choice",
                    "instructions": question.instructions,
                    "criteria": Value::Object(criteria),
                }
            }
        });
        let mut req = self.http.post(&self.endpoint).json(&body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .map_err(|error| LlmError::Request(format!("decision request failed: {error}")))?;
        let status = resp.status();
        if !status.is_success() {
            let code = status.as_u16();
            let reason = status.canonical_reason().unwrap_or("request failed");
            return Err(match code {
                402 => LlmError::QuotaExhausted(format!("decision provider returned HTTP {code}")),
                429 => LlmError::RateLimited {
                    message: format!("decision provider returned HTTP {code}"),
                    retry_after: None,
                },
                _ => LlmError::Request(format!("decision provider returned HTTP {code} ({reason})")),
            });
        }
        let raw: Value = resp
            .json()
            .await
            .map_err(|error| LlmError::Request(format!("invalid decision JSON: {error}")))?;
        if raw.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(LlmError::Request("decision provider reported failure".into()));
        }
        parse_choice_response(&raw, question.criteria)
    }
}

fn parse_choice_response(
    raw: &Value,
    criteria: &[(&str, &str)],
) -> Result<ChoiceAnswer, LlmError> {
    let payload = raw.get("result").unwrap_or(raw);
    let decision = payload
        .get("answers")
        .and_then(|a| a.get("decision"))
        .ok_or_else(|| LlmError::Request("decision response missing answers.decision".into()))?;
    let raw_choice = decision
        .get("choice")
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError::Request("decision response missing choice".into()))?;
    let canonical = criteria
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(raw_choice))
        .map(|(key, _)| *key)
        .ok_or_else(|| {
            LlmError::Request(format!(
                "decision response returned unexpected choice: {raw_choice}"
            ))
        })?;
    let probs = decision
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| LlmError::Request("decision response missing probabilities".into()))?;
    let probability = probs
        .get(raw_choice)
        .or_else(|| probs.get(canonical))
        .or_else(|| {
            probs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(canonical))
                .map(|(_, v)| v)
        })
        .and_then(Value::as_f64)
        .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
        .ok_or_else(|| {
            LlmError::Request("decision response missing valid probability in [0, 1]".into())
        })?;
    let usage = payload.get("usage").map(|u| {
        let input = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let output = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            total: input + output,
        }
    });
    Ok(ChoiceAnswer {
        choice: canonical.to_owned(),
        probability,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_url_detection_and_response_parsing() {
        assert!(is_cloudflare_ai_url(
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/run"
        ));
        assert!(is_cloudflare_ai_url(
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/run/"
        ));
        assert!(!is_cloudflare_ai_url("https://api.typesafe.ai/v1/decide"));
        assert!(!is_cloudflare_ai_url("http://127.0.0.1:8320/v1"));

        // Preserves trailing slash on generic URLs and preserves query strings on CF URLs.
        let (ep, mf) = resolve_decision_endpoint("https://api.example.com/decide/", "jev-1");
        assert_eq!(ep, "https://api.example.com/decide/");
        assert_eq!(mf, "jev-1");
        let (cf_ep, cf_mf) = resolve_decision_endpoint(
            "https://api.cloudflare.com/client/v4/accounts/abc/ai/run?foo=1",
            "@cf/cloudflare/clef-flash",
        );
        assert_eq!(
            cf_ep,
            "https://api.cloudflare.com/client/v4/accounts/abc/ai/run/@cf/cloudflare/clef-flash?foo=1"
        );
        assert_eq!(cf_mf, "clef-flash");

        let criteria = &[("RESPOND", "yes"), ("IGNORE", "no")];
        let cf_body = json!({
            "success": true,
            "result": {
                "model": "clef-flash",
                "answers": {
                    "decision": {
                        "type": "choice",
                        "choice": "RESPOND",
                        "probabilities": { "RESPOND": 0.939, "IGNORE": 0.061 }
                    }
                },
                "usage": { "input_tokens": 312, "output_tokens": 0 }
            }
        });
        let ans = parse_choice_response(&cf_body, criteria).unwrap();
        assert_eq!(ans.choice, "RESPOND");
        assert!((ans.probability - 0.939).abs() < 1e-6);
        assert_eq!(ans.usage.unwrap().input, 312);
        assert_eq!(ans.usage.unwrap().output, 0);

        // Rejects unknown choice, missing probability, or out-of-range probability.
        assert!(parse_choice_response(
            &json!({"answers": {"decision": {"choice": "MAYBE", "probabilities": {"MAYBE": 0.9}}}}),
            criteria
        )
        .is_err());
        assert!(parse_choice_response(
            &json!({"answers": {"decision": {"choice": "RESPOND"}}}),
            criteria
        )
        .is_err());
        assert!(parse_choice_response(
            &json!({"answers": {"decision": {"choice": "RESPOND", "probabilities": {"RESPOND": 1.5}}}}),
            criteria
        )
        .is_err());

        // Direct Jev-style response (no `result` wrapper).
        let direct_body = json!({
            "answers": {
                "decision": {
                    "choice": "INLINE",
                    "probabilities": { "INLINE": 0.97, "THREAD": 0.03 }
                }
            }
        });
        let ans =
            parse_choice_response(&direct_body, &[("INLINE", "short"), ("THREAD", "long")]).unwrap();
        assert_eq!(ans.choice, "INLINE");
        assert!((ans.probability - 0.97).abs() < 1e-6);
    }
}
