//! Native management stays separate from OpenAI-compatible inference routes.
use super::*;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedProvider {
    LmStudio,
    Ollama,
}

impl OpenAiCompatibleClient {
    fn management_url(&self, path: &str) -> Result<Url, ProviderError> {
        if !is_loopback_host(&self.base_url) {
            return Err(ProviderError::InvalidConfig(
                "model management requires a local endpoint".into(),
            ));
        }
        let base = self.base_url.as_str().trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        Url::parse(&format!("{base}/{path}"))
            .map_err(|error| ProviderError::InvalidConfig(error.to_string()))
    }

    pub(super) async fn list_managed_models(
        &self,
    ) -> Result<Option<Vec<ProviderModel>>, ProviderError> {
        if !is_loopback_host(&self.base_url) {
            return Ok(None);
        }
        for (kind, path) in [
            (ManagedProvider::LmStudio, "api/v1/models"),
            (ManagedProvider::Ollama, "api/tags"),
        ] {
            let response = self
                .authorized(self.http.get(self.management_url(path)?))
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(|error| ProviderError::Transport(error.to_string()))?;
            if matches!(response.status().as_u16(), 404 | 405) {
                continue;
            }
            let response = checked_response(response).await?;
            let bytes = bounded_response_bytes(
                response,
                &CancellationToken::new(),
                MAX_JSON_RESPONSE_BYTES,
            )
            .await?;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
            // A generic provider can answer unrelated paths with its normal catalog.
            if let Some(models) = parse_managed_catalog(&value, kind) {
                return Ok(Some(models));
            }
        }
        Ok(None)
    }

    pub async fn load_managed_model(&self, kind: ManagedProvider) -> Result<String, ProviderError> {
        let catalog = self.list_managed_models().await?.ok_or_else(|| {
            ProviderError::InvalidResponse("native model management is unavailable".into())
        })?;
        let model = catalog
            .iter()
            .find(|model| model.id == self.config.model && model.load_via == Some(kind))
            .ok_or_else(|| {
                ProviderError::InvalidRequest(
                    "model is no longer in this provider's catalog; refresh it".into(),
                )
            })?;
        let (path, body) = match kind {
            ManagedProvider::LmStudio => ("api/v1/models/load", json!({"model": model.id})),
            ManagedProvider::Ollama => (
                "api/generate",
                json!({"model": model.id, "stream": false, "keep_alive": "5m"}),
            ),
        };
        let response = self
            .authorized(self.http.post(self.management_url(path)?).json(&body))
            .timeout(Duration::from_secs(180))
            .send()
            .await
            .map_err(|error| ProviderError::Transport(error.to_string()))?;
        let response = checked_response(response).await?;
        let bytes =
            bounded_response_bytes(response, &CancellationToken::new(), MAX_JSON_RESPONSE_BYTES)
                .await?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| ProviderError::InvalidResponse(error.to_string()))?;
        match kind {
            ManagedProvider::LmStudio if value["status"] == "loaded" => value["instance_id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 256)
                .map(str::to_owned)
                .ok_or_else(|| {
                    ProviderError::InvalidResponse("load response omitted instance_id".into())
                }),
            ManagedProvider::Ollama if value["done"] == true => Ok(model.id.clone()),
            _ => Err(ProviderError::InvalidResponse(
                "provider did not confirm a completed model load".into(),
            )),
        }
    }
}

fn parse_managed_catalog(value: &Value, kind: ManagedProvider) -> Option<Vec<ProviderModel>> {
    let entries = value.get("models")?.as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|entry| {
                if kind == ManagedProvider::LmStudio && entry["type"] != "llm" {
                    return None;
                }
                let id = entry[match kind {
                    ManagedProvider::LmStudio => "key",
                    ManagedProvider::Ollama => "name",
                }]
                .as_str()?;
                if id.is_empty() || id.len() > 256 {
                    return None;
                }
                Some(ProviderModel {
                    id: id.to_owned(),
                    owned_by: Some(
                        match kind {
                            ManagedProvider::LmStudio => "LM Studio",
                            ManagedProvider::Ollama => "Ollama",
                        }
                        .into(),
                    ),
                    load_via: Some(kind),
                    loaded: if kind == ManagedProvider::LmStudio {
                        Some(
                            entry["loaded_instances"]
                                .as_array()
                                .is_some_and(|items| !items.is_empty()),
                        )
                    } else {
                        None
                    },
                    display_name: entry["display_name"].as_str().map(str::to_owned),
                })
            })
            .take(2048)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lm_catalog_filters_embeddings_and_preserves_load_keys() {
        let payload = json!({"models": [
            {"key":"org/model", "type":"llm", "display_name":"Model", "loaded_instances":[]},
            {"key":"embed", "type":"embedding"}
        ]});
        let models = parse_managed_catalog(&payload, ManagedProvider::LmStudio).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "org/model");
        assert_eq!(models[0].loaded, Some(false));
    }

    #[test]
    fn ollama_catalog_does_not_require_gguf_filename_extensions() {
        let models = parse_managed_catalog(
            &json!({"models":[{"name":"llama3.2:3b"}]}),
            ManagedProvider::Ollama,
        )
        .unwrap();
        assert_eq!(models[0].id, "llama3.2:3b");
        assert_eq!(models[0].load_via, Some(ManagedProvider::Ollama));
    }

    #[test]
    fn management_routes_preserve_proxy_prefix_and_reject_remote_hosts() {
        let client = OpenAiCompatibleClient::new(ProviderConfig {
            base_url: "http://127.0.0.1:1234/proxy/v1".into(),
            model: "m".into(),
            api_key: None,
        })
        .unwrap();
        assert_eq!(
            client
                .management_url("api/v1/models/load")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:1234/proxy/api/v1/models/load"
        );
        let remote = OpenAiCompatibleClient::new(ProviderConfig {
            base_url: "https://example.com/v1".into(),
            model: "m".into(),
            api_key: None,
        })
        .unwrap();
        assert!(remote.management_url("api/v1/models/load").is_err());
    }
}
