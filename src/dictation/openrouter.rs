//! OpenRouter as the rewrite backend (fork change): when an API key is set,
//! mode rewrites and meeting notes call OpenRouter's chat completions API
//! directly instead of a running OpenCode service, so OpenCode never has to
//! run just for Hex.
//!
//! The key comes from `OPENROUTER_API_KEY`, else the owner-only file
//! `openrouter-key` in Hex's data folder, which Settings › Modes writes. It
//! is never written to `settings.json`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Result, WrapErr, eyre};
use serde::Deserialize;
use serde_json::{Value, json};

/// The model a mode without its own model choice rewrites with.
pub(crate) const DEFAULT_MODEL: &str = "reflection/beam-1.0-501b-a23b";
const COMPLETIONS_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
/// OpenRouter's `reasoning.effort` values, offered as a model's variants.
const REASONING_EFFORTS: [&str; 5] = ["none", "minimal", "low", "medium", "high"];
const KEY_FILE: &str = "openrouter-key";
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// `OPENROUTER_API_KEY` when set, else the key saved from Settings.
pub(crate) fn api_key() -> Option<String> {
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .and_then(nonempty)
        .or_else(saved_key)
}

pub(crate) fn key_from_environment() -> bool {
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .and_then(nonempty)
        .is_some()
}

/// The key saved from Settings (not the environment's).
pub(crate) fn saved_key() -> Option<String> {
    nonempty(std::fs::read_to_string(key_path().ok()?).ok()?)
}

/// Saves `key` owner-only, or forgets it when empty.
pub(crate) fn save_key(key: &str) -> Result<()> {
    let path = key_path()?;
    let Some(key) = nonempty(key.to_owned()) else {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    };
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        // `mode` applies only to a new file; tighten an existing one too.
        if path.exists() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    use std::io::Write;
    options.open(&path)?.write_all(key.as_bytes())?;
    Ok(())
}

fn key_path() -> Result<std::path::PathBuf> {
    Ok(crate::app_paths::support_dir()?.join(KEY_FILE))
}

fn nonempty(key: String) -> Option<String> {
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_owned())
}

/// One model from OpenRouter's catalog that can answer in text.
pub(crate) struct CatalogModel {
    pub id: String,
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
    name: String,
    #[serde(default)]
    architecture: Option<Architecture>,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

#[derive(Deserialize)]
struct Architecture {
    #[serde(default)]
    output_modalities: Vec<String>,
}

/// OpenRouter's text models. Call off the UI thread.
pub(crate) fn catalog() -> Result<Vec<CatalogModel>> {
    let agent = crate::net::agent(MODELS_URL, |config| {
        config.timeout_global(Some(Duration::from_secs(15)))
    });
    let body = agent
        .get(MODELS_URL)
        .call()
        .map_err(|error| eyre!(crate::net::unreachable(MODELS_URL, &error)))?
        .body_mut()
        .read_to_string()
        .wrap_err("could not read OpenRouter's model list")?;
    parse_catalog(&body)
}

fn parse_catalog(body: &str) -> Result<Vec<CatalogModel>> {
    let models: ModelsResponse =
        serde_json::from_str(body).wrap_err("OpenRouter returned an invalid model list")?;
    Ok(models
        .data
        .into_iter()
        .filter(|model| {
            model.architecture.as_ref().is_none_or(|architecture| {
                architecture.output_modalities.iter().any(|m| m == "text")
            })
        })
        .map(|model| CatalogModel {
            variants: if model.supported_parameters.iter().any(|p| p == "reasoning") {
                REASONING_EFFORTS
                    .iter()
                    .map(|&effort| effort.into())
                    .collect()
            } else {
                Vec::new()
            },
            id: model.id,
            name: model.name,
        })
        .collect())
}

/// One chat completion. `Ok(Err(message))` is OpenRouter's own refusal (bad
/// model, no credits), so the caller may try another model; `Err` is a
/// transport failure, cancellation, or the deadline.
pub(crate) fn complete(
    key: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    deadline: Duration,
    cancelled: &AtomicBool,
) -> Result<std::result::Result<String, String>> {
    let body = request_body(prompt, model, effort);
    let key = key.to_owned();
    let (sender, receiver) = mpsc::sync_channel(1);
    // ureq can't be interrupted mid-request, so the request runs on its own
    // thread (bounded by the same deadline) and a cancel simply abandons it.
    std::thread::spawn(move || {
        let agent = crate::net::agent(COMPLETIONS_URL, |config| {
            config
                .timeout_global(Some(deadline))
                .http_status_as_error(false)
        });
        let result = agent
            .post(COMPLETIONS_URL)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .header("X-Title", "Hex")
            .send(body.as_str())
            .and_then(|mut response| {
                let status = response.status().as_u16();
                response
                    .body_mut()
                    .read_to_string()
                    .map(|text| (status, text))
            });
        let _ = sender.send(result);
    });
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(eyre!("OpenRouter generation cancelled"));
        }
        match receiver.recv_timeout(CANCEL_POLL) {
            Ok(Ok((status, body))) => return Ok(parse_completion(status, &body)),
            Ok(Err(error)) => return Err(eyre!(crate::net::unreachable(COMPLETIONS_URL, &error))),
            Err(mpsc::RecvTimeoutError::Timeout) if started.elapsed() < deadline => {}
            Err(_) => {
                return Err(eyre!(
                    "OpenRouter generation exceeded {} seconds",
                    deadline.as_secs()
                ));
            }
        }
    }
}

/// OpenRouter's ID for a saved `provider/id` key. A key written as
/// `openrouter/<vendor>/<model>` names the vendor's model, while OpenRouter's
/// own models (`openrouter/auto`) keep the prefix.
pub(crate) fn model_id(provider: &str, id: &str) -> String {
    if provider == "openrouter" && id.contains('/') {
        id.to_owned()
    } else {
        format!("{provider}/{id}")
    }
}

fn request_body(prompt: &str, model: Option<&str>, effort: Option<&str>) -> String {
    let mut body = json!({
        "messages": [{ "role": "user", "content": prompt }],
    });
    // Without a model, OpenRouter answers with the account's default model.
    if let Some(model) = model {
        body["model"] = json!(model);
    }
    if let Some(effort) = effort {
        body["reasoning"] = json!({ "effort": effort });
    }
    body.to_string()
}

fn parse_completion(status: u16, body: &str) -> std::result::Result<String, String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Err(format!("OpenRouter answered HTTP {status} without JSON"));
    };
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        return Err(message.to_owned());
    }
    if !(200..300).contains(&status) {
        return Err(format!("OpenRouter answered HTTP {status}"));
    }
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "OpenRouter returned no text".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_the_model_and_effort_only_when_given() {
        let body: Value =
            serde_json::from_str(&request_body("fix this", Some(DEFAULT_MODEL), Some("low")))
                .unwrap();
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["messages"][0]["content"], "fix this");

        let body: Value = serde_json::from_str(&request_body("fix this", None, None)).unwrap();
        assert!(body.get("model").is_none());
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn an_openrouter_prefix_is_dropped_only_before_a_vendor() {
        assert_eq!(
            model_id("openrouter", "reflection/beam-1.0-501b-a23b"),
            DEFAULT_MODEL
        );
        assert_eq!(model_id("openrouter", "auto"), "openrouter/auto");
        assert_eq!(model_id("reflection", "beam-1.0-501b-a23b"), DEFAULT_MODEL);
    }

    #[test]
    fn a_completion_yields_its_text_and_a_refusal_its_message() {
        let ok = r#"{"choices":[{"message":{"role":"assistant","content":"Hello."}}]}"#;
        assert_eq!(parse_completion(200, ok), Ok("Hello.".into()));

        let refused = r#"{"error":{"message":"Insufficient credits","code":402}}"#;
        assert_eq!(
            parse_completion(402, refused),
            Err("Insufficient credits".into())
        );
        assert!(parse_completion(502, "<html>").is_err());
        assert!(parse_completion(200, r#"{"choices":[]}"#).is_err());
    }

    #[test]
    fn the_catalog_keeps_text_models_and_offers_efforts_for_reasoning_ones() {
        let body = r#"{"data":[
            {"id":"reflection/beam-1.0-501b-a23b","name":"Beam","architecture":{"output_modalities":["text"]},"supported_parameters":["reasoning"]},
            {"id":"acme/plain","name":"Plain","architecture":{"output_modalities":["text"]},"supported_parameters":[]},
            {"id":"acme/painter","name":"Painter","architecture":{"output_modalities":["image"]}}
        ]}"#;
        let models = parse_catalog(body).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, DEFAULT_MODEL);
        assert_eq!(models[0].variants.len(), REASONING_EFFORTS.len());
        assert!(models[1].variants.is_empty());
    }
}
