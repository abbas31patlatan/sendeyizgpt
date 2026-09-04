//! Runtime-independent inference contracts and transparent resource estimates.
//!
//! Native inference is supervised out-of-process: the llama.cpp server owns GGUF tensor
//! loading while this crate owns validation, lifecycle, health checks and cancellation boundaries.

pub mod catalog;
pub mod llama_server;

pub use catalog::{
    ModelScanIssue, ModelScanReport, ScannedModel, inspect_gguf_model, scan_model_directory,
};
pub use llama_server::{LlamaServerRuntime, NativeRuntimePhase, NativeRuntimeStatus};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    Gguf,
    Safetensors,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceleratorKind {
    Cpu,
    Vulkan,
    Cuda,
    Hip,
    Rocm,
    Remote,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub vision: bool,
    pub tool_calling: bool,
    pub reasoning: bool,
    pub embeddings: bool,
    pub audio_input: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub path: PathBuf,
    pub format: ModelFormat,
    pub family: Option<String>,
    pub parameter_count: Option<u64>,
    pub architecture: Option<String>,
    pub quantization: Option<String>,
    pub gguf_version: Option<String>,
    pub file_size_bytes: u64,
    pub context_capacity: Option<u32>,
    pub layer_count: Option<u32>,
    pub attention_head_count: Option<u32>,
    pub key_value_head_count: Option<u32>,
    pub embedding_length: Option<u32>,
    pub bits_per_weight: Option<f32>,
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDescriptor {
    pub id: String,
    pub name: String,
    pub version: String,
    pub supported_formats: Vec<ModelFormat>,
    pub supported_accelerators: Vec<AcceleratorKind>,
    pub supported_architectures: Vec<String>,
    pub isolated_process: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDescriptor {
    pub id: String,
    pub name: String,
    pub version: String,
    pub accelerator: AcceleratorKind,
    pub supports_streaming: bool,
    pub supports_cancellation: bool,
    pub supports_cpu_gpu_split: bool,
    pub supports_metrics: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadPreset {
    Eco,
    Balanced,
    Performance,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheQuantization {
    F32,
    F16,
    Q8,
    Q6,
    Q4,
}

impl CacheQuantization {
    fn bytes_per_element(self) -> f64 {
        match self {
            Self::F32 => 4.0,
            Self::F16 => 2.0,
            Self::Q8 => 1.0,
            Self::Q6 => 0.75,
            Self::Q4 => 0.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadProfile {
    pub preset: LoadPreset,
    pub context_length: u32,
    pub gpu_offload_percent: u8,
    pub cpu_threads: u16,
    pub batch_size: u32,
    pub physical_batch_size: u32,
    pub flash_attention: bool,
    pub kv_cache_offload: bool,
    pub k_cache_quantization: CacheQuantization,
    pub v_cache_quantization: CacheQuantization,
    pub mmap: bool,
    pub mlock: bool,
    pub parallel_requests: u8,
    pub reasoning_enabled: bool,
    pub reasoning_budget_tokens: Option<u32>,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub min_p: f32,
    pub repeat_penalty: f32,
    pub seed: Option<u64>,
}

impl LoadProfile {
    pub fn eco() -> Self {
        Self {
            preset: LoadPreset::Eco,
            context_length: 4096,
            gpu_offload_percent: 75,
            cpu_threads: 4,
            batch_size: 128,
            physical_batch_size: 64,
            flash_attention: false,
            kv_cache_offload: false,
            k_cache_quantization: CacheQuantization::Q8,
            v_cache_quantization: CacheQuantization::Q8,
            mmap: true,
            mlock: false,
            parallel_requests: 1,
            reasoning_enabled: false,
            reasoning_budget_tokens: None,
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            min_p: 0.05,
            repeat_penalty: 1.1,
            seed: None,
        }
    }

    pub fn balanced() -> Self {
        Self {
            preset: LoadPreset::Balanced,
            context_length: 8192,
            gpu_offload_percent: 100,
            cpu_threads: 8,
            batch_size: 256,
            physical_batch_size: 128,
            flash_attention: true,
            kv_cache_offload: true,
            k_cache_quantization: CacheQuantization::Q8,
            v_cache_quantization: CacheQuantization::Q8,
            mmap: true,
            mlock: false,
            parallel_requests: 1,
            reasoning_enabled: false,
            reasoning_budget_tokens: None,
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            min_p: 0.05,
            repeat_penalty: 1.1,
            seed: None,
        }
    }

    pub fn performance() -> Self {
        Self {
            preset: LoadPreset::Performance,
            context_length: 16384,
            gpu_offload_percent: 100,
            cpu_threads: 12,
            batch_size: 512,
            physical_batch_size: 512,
            flash_attention: true,
            kv_cache_offload: true,
            k_cache_quantization: CacheQuantization::F16,
            v_cache_quantization: CacheQuantization::F16,
            mmap: true,
            mlock: false,
            parallel_requests: 2,
            reasoning_enabled: false,
            reasoning_budget_tokens: None,
            temperature: 0.7,
            top_p: 0.95,
            top_k: 40,
            min_p: 0.03,
            repeat_penalty: 1.05,
            seed: None,
        }
    }

    pub fn for_preset(preset: LoadPreset) -> Self {
        match preset {
            LoadPreset::Eco => Self::eco(),
            LoadPreset::Balanced => Self::balanced(),
            LoadPreset::Performance => Self::performance(),
            LoadPreset::Custom => Self::balanced_with_custom_tag(),
        }
    }

    pub fn validate(&self) -> Result<(), InferenceError> {
        if self.context_length == 0
            || self.context_length > 1_048_576
            || self.cpu_threads == 0
            || self.cpu_threads > 256
            || self.batch_size == 0
            || self.batch_size > 65_536
            || self.physical_batch_size == 0
            || self.physical_batch_size > self.batch_size
            || self.parallel_requests == 0
            || self.parallel_requests > 16
        {
            return Err(InferenceError::InvalidProfile(
                "context, threads, batches and parallel requests are outside the supported limits"
                    .to_owned(),
            ));
        }
        if self.gpu_offload_percent > 100 {
            return Err(InferenceError::InvalidProfile(
                "GPU offload must be between 0 and 100 percent".to_owned(),
            ));
        }
        if !(0.0..=2.0).contains(&self.temperature)
            || !(0.0..=1.0).contains(&self.top_p)
            || !(0.0..=1.0).contains(&self.min_p)
            || self.repeat_penalty <= 0.0
        {
            return Err(InferenceError::InvalidProfile(
                "sampling values are outside their supported ranges".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn validate_for_model(&self, model: &ModelDescriptor) -> Result<(), InferenceError> {
        self.validate()?;
        if let Some(capacity) = model.context_capacity {
            if self.context_length > capacity {
                return Err(InferenceError::InvalidProfile(format!(
                    "context length {} exceeds model capacity {}",
                    self.context_length, capacity
                )));
            }
        }
        Ok(())
    }

    fn balanced_with_custom_tag() -> Self {
        let mut profile = Self::balanced();
        profile.preset = LoadPreset::Custom;
        profile
    }
}

impl Default for LoadProfile {
    fn default() -> Self {
        Self::balanced()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEstimate {
    pub weights_bytes: u64,
    pub kv_cache_bytes: u64,
    pub scratch_bytes: u64,
    pub estimated_vram_bytes: u64,
    pub estimated_ram_bytes: u64,
    pub confidence: EstimateConfidence,
    pub assumptions: Vec<String>,
}

impl MemoryEstimate {
    pub fn for_model(
        model: &ModelDescriptor,
        profile: &LoadProfile,
    ) -> Result<Self, InferenceError> {
        profile.validate_for_model(model)?;
        let file_bytes = model.file_size_bytes as f64;
        let weights_bytes = (file_bytes * 1.05).ceil() as u64;
        let mut assumptions = vec![
            "Weights use GGUF file size plus a 5% runtime overhead estimate".to_owned(),
            "Actual allocator, driver and backend usage may differ".to_owned(),
        ];

        let kv_cache_bytes = match (
            model.layer_count,
            model.key_value_head_count,
            model.embedding_length,
            model.attention_head_count,
        ) {
            (Some(layers), Some(kv_heads), Some(embedding), Some(attention_heads))
                if attention_heads > 0 =>
            {
                let head_dim = embedding as f64 / attention_heads as f64;
                let tokens = profile.context_length as f64 * profile.parallel_requests as f64;
                let k = profile.k_cache_quantization.bytes_per_element();
                let v = profile.v_cache_quantization.bytes_per_element();
                (tokens * layers as f64 * kv_heads as f64 * head_dim * (k + v)).ceil() as u64
            }
            _ => {
                assumptions
                    .push("KV cache metadata is incomplete; KV estimate is omitted".to_owned());
                0
            }
        };
        let scratch_bytes =
            (256_u64 * 1024 * 1024).saturating_add((profile.batch_size as u64) * 1024 * 1024 / 2);
        let gpu_weight_bytes =
            weights_bytes.saturating_mul(profile.gpu_offload_percent as u64) / 100;
        let cpu_weight_bytes = weights_bytes.saturating_sub(gpu_weight_bytes);
        let gpu_kv_bytes = if profile.kv_cache_offload {
            kv_cache_bytes
        } else {
            0
        };
        let cpu_kv_bytes = kv_cache_bytes.saturating_sub(gpu_kv_bytes);

        let confidence = if model.layer_count.is_some()
            && model.key_value_head_count.is_some()
            && model.embedding_length.is_some()
            && model.attention_head_count.is_some()
        {
            EstimateConfidence::High
        } else if model.file_size_bytes > 0 {
            EstimateConfidence::Medium
        } else {
            EstimateConfidence::Low
        };

        Ok(Self {
            weights_bytes,
            kv_cache_bytes,
            scratch_bytes,
            estimated_vram_bytes: gpu_weight_bytes
                .saturating_add(gpu_kv_bytes)
                .saturating_add(scratch_bytes),
            estimated_ram_bytes: cpu_weight_bytes
                .saturating_add(cpu_kv_bytes)
                .saturating_add(scratch_bytes),
            confidence,
            assumptions,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerateRequest {
    pub request_id: Uuid,
    pub model: Uuid,
    pub prompt: String,
    pub max_new_tokens: u32,
    pub profile: LoadProfile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum GenerationEvent {
    Started { request_id: Uuid },
    Token { text: String, token_count: u32 },
    Reasoning { text: String },
    Metrics { metrics: RuntimeMetrics },
    Finished { summary: GenerationSummary },
    Failed { code: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationSummary {
    pub generated_tokens: u64,
    pub prompt_tokens: u64,
    pub time_to_first_token_ms: Option<f64>,
    pub generation_duration_ms: f64,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeMetrics {
    pub tokens_per_second: Option<f64>,
    pub prompt_tokens_per_second: Option<f64>,
    pub time_to_first_token_ms: Option<f64>,
    pub vram_bytes: Option<u64>,
    pub ram_bytes: Option<u64>,
    pub cpu_load_percent: Option<f32>,
    pub gpu_load_percent: Option<f32>,
    pub gpu_temperature_celsius: Option<f32>,
    pub power_watts: Option<f32>,
    pub joules_per_generated_token: Option<f64>,
    pub sampled_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedModel {
    pub handle: Uuid,
    pub model_id: String,
    pub backend_id: String,
}

#[async_trait]
pub trait InferenceBackend: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    async fn load_model(
        &self,
        model: ModelDescriptor,
        profile: LoadProfile,
        cancellation: CancellationToken,
    ) -> Result<LoadedModel, InferenceError>;

    async fn unload_model(&self, model: LoadedModel) -> Result<(), InferenceError>;

    async fn stream(
        &self,
        request: GenerateRequest,
        events: mpsc::Sender<GenerationEvent>,
        cancellation: CancellationToken,
    ) -> Result<GenerationSummary, InferenceError>;

    async fn tokenize(&self, model: &LoadedModel, text: &str) -> Result<Vec<u32>, InferenceError>;

    async fn detokenize(
        &self,
        model: &LoadedModel,
        tokens: &[u32],
    ) -> Result<String, InferenceError>;

    async fn model_info(&self, model: &LoadedModel) -> Result<ModelDescriptor, InferenceError>;

    async fn memory_estimate(
        &self,
        model: &ModelDescriptor,
        profile: &LoadProfile,
    ) -> Result<MemoryEstimate, InferenceError>;

    async fn cancel_generation(&self, request_id: Uuid) -> Result<(), InferenceError>;

    async fn runtime_metrics(&self) -> Result<RuntimeMetrics, InferenceError>;
}

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("invalid load profile: {0}")]
    InvalidProfile(String),
    #[error("model is incompatible with this backend: {0}")]
    IncompatibleModel(String),
    #[error("inference worker is unavailable: {0}")]
    WorkerUnavailable(String),
    #[error("local model scan failed: {0}")]
    ModelScan(String),
    #[error("inference operation was cancelled")]
    Cancelled,
    #[error("inference backend failed: {0}")]
    Backend(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelDescriptor {
        ModelDescriptor {
            id: "qwen-test".to_owned(),
            display_name: "Qwen test".to_owned(),
            path: PathBuf::from("C:\\Models\\qwen.gguf"),
            format: ModelFormat::Gguf,
            family: Some("qwen".to_owned()),
            parameter_count: Some(7_000_000_000),
            architecture: Some("qwen2".to_owned()),
            quantization: Some("Q4_K_M".to_owned()),
            gguf_version: Some("3".to_owned()),
            file_size_bytes: 4_500_000_000,
            context_capacity: Some(32_768),
            layer_count: Some(32),
            attention_head_count: Some(32),
            key_value_head_count: Some(8),
            embedding_length: Some(4096),
            bits_per_weight: Some(4.5),
            capabilities: ModelCapabilities::default(),
        }
    }

    #[test]
    fn profiles_validate_and_estimates_are_labeled() {
        let estimate =
            MemoryEstimate::for_model(&model(), &LoadProfile::balanced()).expect("estimate");
        assert!(estimate.estimated_vram_bytes > 0);
        assert_eq!(estimate.confidence, EstimateConfidence::High);
        assert!(!estimate.assumptions.is_empty());
    }

    #[test]
    fn physical_batch_cannot_exceed_logical_batch() {
        let mut profile = LoadProfile::balanced();
        profile.physical_batch_size = profile.batch_size + 1;
        assert!(profile.validate().is_err());
    }
}
