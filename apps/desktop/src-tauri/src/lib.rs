use aegis_core::{ApplicationCore, RuntimeStatus};
use aegis_database::{
    AutomationRecord, ConversationRecord, Database, ModelLibraryRecord, ModelProfileRecord,
    ModelRecord, WorkspaceRecord,
};
use aegis_inference::{
    LlamaServerRuntime, LoadPreset, LoadProfile, MemoryEstimate, ModelFormat, NativeRuntimePhase,
    NativeRuntimeStatus, ScannedModel, inspect_gguf_model, scan_model_directory,
};
use aegis_providers::{
    AssistantToolCall, ChatChunk, ChatCompletionSummary, ChatMessage, ChatRequest, ChatRole,
    OpenAiCompatibleClient, ProviderConfig, ProviderError, ProviderModel, ToolCallDelta,
    ToolCallFunction, ToolExecution, builtin_tool_definitions, execute_builtin_tool,
    tool_system_instructions,
};
use aegis_providers::{ManagedProvider, tools::validate_tool_request};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
mod model_watch;

pub struct DesktopState {
    pub core: Arc<ApplicationCore>,
    pub database: Arc<Database>,
    pub native_runtime: Arc<LlamaServerRuntime>,
    pub model_watcher: std::sync::Mutex<model_watch::ModelWatcher>,
}

#[derive(Debug, Serialize)]
struct OperationStarted {
    operation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
enum ChatEvent {
    Started {
        operation_id: String,
    },
    Token {
        operation_id: String,
        text: String,
    },
    Reasoning {
        operation_id: String,
        text: String,
    },
    Finished {
        operation_id: String,
        generated_tokens: u64,
        prompt_tokens: Option<u64>,
        time_to_first_token_ms: Option<f64>,
        generation_duration_ms: f64,
        finish_reason: Option<String>,
    },
    Failed {
        operation_id: String,
        code: String,
        message: String,
        retryable: bool,
    },
    Cancelled {
        operation_id: String,
    },
    Tool {
        operation_id: String,
        tool_id: String,
        phase: String,
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDiagnostics {
    status: String,
    endpoint: String,
    local: bool,
    latency_ms: f64,
    model_count: usize,
    models: Vec<ProviderModel>,
    error: Option<String>,
    retryable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspacePathDiagnostics {
    exists: bool,
    is_directory: bool,
    canonical_path: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelScanIssueView {
    path: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelScanSummary {
    library: ModelLibraryRecord,
    models: Vec<ModelRecord>,
    scanned_count: usize,
    visited_files: usize,
    issues: Vec<ModelScanIssueView>,
    duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelLoadEstimate {
    model: ModelRecord,
    profile: LoadProfile,
    estimate: MemoryEstimate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelDiscoverySummary {
    libraries: Vec<ModelLibraryRecord>,
    models: Vec<ModelRecord>,
    scanned_roots: usize,
    visited_files: usize,
    issues: Vec<ModelScanIssueView>,
    duration_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeModelStartRequest {
    model_id: String,
    preset: LoadPreset,
    runtime_path: Option<String>,
}

fn emit_chat(app: &AppHandle, event: ChatEvent) {
    let _ = app.emit("aegis://chat", event);
}

#[tauri::command]
fn runtime_status(state: State<'_, DesktopState>) -> Result<RuntimeStatus, String> {
    let native = state
        .native_runtime
        .status()
        .map_err(|error| error.to_string())?;
    let mut status = state
        .core
        .runtime_status()
        .map_err(|error| error.to_string())?;

    match native.phase {
        NativeRuntimePhase::Starting | NativeRuntimePhase::Loading | NativeRuntimePhase::Ready => {
            status.model_name = native.model_name.clone();
            status.backend_name = Some("llama.cpp native server".to_owned());
            status.context_length = native.context_length;
            status.last_error = None;
        }
        NativeRuntimePhase::Error => {
            status.last_error = native.message.clone();
        }
        NativeRuntimePhase::Stopped | NativeRuntimePhase::Stopping => {}
    }

    Ok(status)
}

#[tauri::command]
fn load_conversations(state: State<'_, DesktopState>) -> Result<Vec<ConversationRecord>, String> {
    state
        .database
        .list_conversations()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_conversation(
    state: State<'_, DesktopState>,
    conversation: ConversationRecord,
) -> Result<(), String> {
    state
        .database
        .save_conversation(&conversation)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_conversation(
    state: State<'_, DesktopState>,
    conversation_id: String,
) -> Result<bool, String> {
    state
        .database
        .delete_conversation(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_workspaces(state: State<'_, DesktopState>) -> Result<Vec<WorkspaceRecord>, String> {
    state
        .database
        .list_workspaces()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_workspace(
    state: State<'_, DesktopState>,
    workspace: WorkspaceRecord,
) -> Result<(), String> {
    state
        .database
        .save_workspace(&workspace)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_workspace(state: State<'_, DesktopState>, workspace_id: String) -> Result<bool, String> {
    state
        .database
        .delete_workspace(&workspace_id)
        .map_err(|error| error.to_string())
}

fn validate_directory_path(path: &str) -> WorkspacePathDiagnostics {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return WorkspacePathDiagnostics {
            exists: false,
            is_directory: false,
            canonical_path: None,
            error: Some("workspace path cannot be empty".to_owned()),
        };
    }

    let path_ref = Path::new(trimmed);
    match std::fs::metadata(path_ref) {
        Ok(metadata) => {
            let is_directory = metadata.is_dir();
            WorkspacePathDiagnostics {
                exists: true,
                is_directory,
                canonical_path: std::fs::canonicalize(path_ref)
                    .ok()
                    .map(|value| value.to_string_lossy().into_owned()),
                error: (!is_directory).then(|| "path is not a directory".to_owned()),
            }
        }
        Err(error) => WorkspacePathDiagnostics {
            exists: false,
            is_directory: false,
            canonical_path: None,
            error: Some(error.to_string()),
        },
    }
}

#[tauri::command]
fn validate_workspace_path(path: String) -> WorkspacePathDiagnostics {
    validate_directory_path(&path)
}

#[tauri::command]
fn load_automations(state: State<'_, DesktopState>) -> Result<Vec<AutomationRecord>, String> {
    state
        .database
        .list_automations()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_automation(
    state: State<'_, DesktopState>,
    automation: AutomationRecord,
) -> Result<(), String> {
    state
        .database
        .save_automation(&automation)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_automation(
    state: State<'_, DesktopState>,
    automation_id: String,
) -> Result<bool, String> {
    state
        .database
        .delete_automation(&automation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_model_libraries(state: State<'_, DesktopState>) -> Result<Vec<ModelLibraryRecord>, String> {
    state
        .database
        .list_model_libraries()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_model_library(
    state: State<'_, DesktopState>,
    mut library: ModelLibraryRecord,
) -> Result<(), String> {
    let diagnostics = validate_directory_path(&library.root_path);
    if !diagnostics.exists || !diagnostics.is_directory {
        return Err(diagnostics
            .error
            .unwrap_or_else(|| "model library path is not a directory".to_owned()));
    }
    library.root_path = diagnostics
        .canonical_path
        .unwrap_or_else(|| library.root_path.trim().to_owned());
    state
        .database
        .save_model_library(&library)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_model_library(
    state: State<'_, DesktopState>,
    library_id: String,
) -> Result<bool, String> {
    state
        .database
        .delete_model_library(&library_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_local_models(state: State<'_, DesktopState>) -> Result<Vec<ModelRecord>, String> {
    state
        .database
        .list_models(None)
        .map_err(|error| error.to_string())
}

fn default_model_roots() -> Vec<(String, PathBuf)> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.extend([
            ("LM Studio".to_owned(), home.join(".lmstudio/models")),
            (
                "LM Studio · cache".to_owned(),
                home.join(".cache/lm-studio/models"),
            ),
            (
                "Hugging Face · cache".to_owned(),
                home.join(".cache/huggingface/hub"),
            ),
            ("Ollama · models".to_owned(), home.join(".ollama/models")),
            ("Modeller".to_owned(), home.join("Models")),
            ("İndirilenler".to_owned(), home.join("Downloads")),
            (
                "Belgeler · modeller".to_owned(),
                home.join("Documents/Models"),
            ),
        ]);
    }
    if let Some(local_app_data) = local_app_data {
        candidates.extend([
            (
                "LM Studio · local".to_owned(),
                local_app_data.join("LM Studio/models"),
            ),
            (
                "LM Studio · app".to_owned(),
                local_app_data.join("lm-studio/models"),
            ),
        ]);
    }

    let mut unique = std::collections::BTreeSet::new();
    candidates
        .into_iter()
        .filter(|(_, path)| unique.insert(path.to_string_lossy().to_ascii_lowercase()))
        .collect()
}

fn automatic_library_id(path: &Path) -> String {
    let digest = blake3::hash(path.to_string_lossy().as_bytes())
        .to_hex()
        .to_string();
    format!("model-library-auto-{}", &digest[..24])
}

#[tauri::command]
async fn discover_local_models(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<ModelDiscoverySummary, String> {
    static SCAN_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _scan = SCAN_GATE
        .try_lock()
        .map_err(|_| "model discovery is already running".to_owned())?;
    let database = Arc::clone(&state.database);
    let started = Instant::now();
    let mut scanned_roots = 0;
    let mut visited_files = 0;
    let mut issues = Vec::new();

    let existing = database
        .list_model_libraries()
        .map_err(|error| error.to_string())?;
    let mut roots = default_model_roots();
    roots.extend(
        existing
            .iter()
            .map(|library| (library.name.clone(), PathBuf::from(&library.root_path))),
    );
    let mut seen = std::collections::BTreeSet::new();
    let mut watched_roots = std::collections::BTreeSet::new();
    for (name, candidate) in roots.into_iter().take(64) {
        if !candidate.is_dir() {
            continue;
        }
        let canonical_path = match std::fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) => {
                issues.push(ModelScanIssueView {
                    path: candidate.to_string_lossy().into_owned(),
                    message: format!("directory could not be resolved: {error}"),
                });
                continue;
            }
        };
        if !seen.insert(canonical_path.clone()) {
            continue;
        }
        let registered = existing
            .iter()
            .find(|library| Path::new(&library.root_path) == canonical_path);
        if registered.is_some_and(|library| !library.enabled) {
            continue;
        }
        watched_roots.insert(canonical_path.clone());
        let id = registered.map_or_else(
            || automatic_library_id(&canonical_path),
            |library| library.id.clone(),
        );
        let now = unix_now_millis();
        let library = ModelLibraryRecord {
            id,
            name: registered.map_or(name, |library| library.name.clone()),
            root_path: canonical_path.to_string_lossy().into_owned(),
            enabled: true,
            last_scan_at: None,
            created_at: registered.map_or(now, |library| library.created_at),
            updated_at: now,
        };
        database
            .save_model_library(&library)
            .map_err(|error| error.to_string())?;
        let root_for_scan = canonical_path.clone();
        let report =
            tauri::async_runtime::spawn_blocking(move || scan_model_directory(root_for_scan))
                .await
                .map_err(|error| format!("model discovery worker failed: {error}"))?;
        let report = match report {
            Ok(report) => report,
            Err(error) => {
                issues.push(ModelScanIssueView {
                    path: canonical_path.to_string_lossy().into_owned(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        scanned_roots += 1;
        visited_files += report.visited_files;
        issues.extend(report.issues.into_iter().map(|issue| ModelScanIssueView {
            path: issue.path.to_string_lossy().into_owned(),
            message: issue.message,
        }));
        let scan_time = unix_now_millis();
        let records = report
            .models
            .iter()
            .map(|model| model_record_from_scan(&library.id, model, scan_time))
            .collect::<Vec<_>>();
        database
            .replace_model_library_models(&library.id, &records, scan_time)
            .map_err(|error| error.to_string())?;
    }

    let watch_result = state
        .model_watcher
        .lock()
        .map_err(|_| "model watcher lock failed".to_owned())
        .and_then(|mut watcher| watcher.sync(&app, watched_roots));
    if let Err(error) = watch_result {
        issues.push(ModelScanIssueView {
            path: String::new(),
            message: format!(
                "live watching unavailable; periodic discovery remains active: {error}"
            ),
        });
    }
    Ok(ModelDiscoverySummary {
        libraries: database
            .list_model_libraries()
            .map_err(|error| error.to_string())?,
        models: database
            .list_models(None)
            .map_err(|error| error.to_string())?,
        scanned_roots,
        visited_files,
        issues,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command]
async fn scan_model_library(
    state: State<'_, DesktopState>,
    library_id: String,
) -> Result<ModelScanSummary, String> {
    let database = Arc::clone(&state.database);
    let library = database
        .get_model_library(&library_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "model library does not exist".to_owned())?;
    if !library.enabled {
        return Err("model library is disabled".to_owned());
    }

    let diagnostics = validate_directory_path(&library.root_path);
    let canonical_path = diagnostics
        .canonical_path
        .filter(|_| diagnostics.exists && diagnostics.is_directory)
        .ok_or_else(|| {
            diagnostics
                .error
                .unwrap_or_else(|| "model library path is unavailable".to_owned())
        })?;
    let started = Instant::now();
    let root = PathBuf::from(canonical_path);
    let report = tauri::async_runtime::spawn_blocking(move || scan_model_directory(root))
        .await
        .map_err(|error| format!("model scan worker failed: {error}"))?
        .map_err(|error| error.to_string())?;
    let scan_timestamp = unix_now_millis();
    let scanned_count = report.models.len();
    let records = report
        .models
        .iter()
        .map(|model| model_record_from_scan(&library.id, model, scan_timestamp))
        .collect::<Vec<_>>();
    database
        .replace_model_library_models(&library.id, &records, scan_timestamp)
        .map_err(|error| error.to_string())?;

    let library = database
        .get_model_library(&library.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "model library disappeared during scan".to_owned())?;
    let models = database
        .list_models(Some(&library.id))
        .map_err(|error| error.to_string())?;
    let issues = report
        .issues
        .into_iter()
        .map(|issue| ModelScanIssueView {
            path: issue.path.to_string_lossy().into_owned(),
            message: issue.message,
        })
        .collect();

    Ok(ModelScanSummary {
        library,
        models,
        scanned_count,
        visited_files: report.visited_files,
        issues,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command]
fn load_model_profiles(state: State<'_, DesktopState>) -> Result<Vec<ModelProfileRecord>, String> {
    state
        .database
        .list_model_profiles()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn save_model_profile(
    state: State<'_, DesktopState>,
    profile: ModelProfileRecord,
) -> Result<(), String> {
    state
        .database
        .save_model_profile(&profile)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_model_profile(
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<bool, String> {
    state
        .database
        .delete_model_profile(&profile_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn estimate_model_load(
    state: State<'_, DesktopState>,
    model_id: String,
    preset: LoadPreset,
) -> Result<ModelLoadEstimate, String> {
    let stored_model = state
        .database
        .get_model(&model_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "model does not exist; scan the library again".to_owned())?;
    let scanned = inspect_gguf_model(&stored_model.file_path).map_err(|error| error.to_string())?;
    if scanned.descriptor.file_size_bytes != stored_model.file_size_bytes
        || stored_model
            .metadata_hash
            .as_deref()
            .is_some_and(|hash| hash != scanned.metadata_hash)
    {
        return Err("model file changed since the last scan; scan the library again".to_owned());
    }

    let profile = LoadProfile::for_preset(preset);
    let estimate = MemoryEstimate::for_model(&scanned.descriptor, &profile)
        .map_err(|error| error.to_string())?;
    Ok(ModelLoadEstimate {
        model: stored_model,
        profile,
        estimate,
    })
}

fn default_llama_server_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

fn is_bare_executable(value: &str) -> bool {
    let path = Path::new(value);
    path.components().count() == 1 && !value.contains('/') && !value.contains('\\')
}

fn resolve_llama_server_path(app: &AppHandle, requested: Option<&str>) -> Result<PathBuf, String> {
    let executable_name = default_llama_server_name();
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        if is_bare_executable(requested) {
            return Ok(PathBuf::from(requested));
        }
        let path = Path::new(requested);
        if !path.is_file() {
            return Err(format!(
                "llama.cpp executable was not found at {}",
                path.display()
            ));
        }
        return std::fs::canonicalize(path)
            .map_err(|error| format!("llama.cpp executable could not be resolved: {error}"));
    }

    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("runtime").join(executable_name));
        candidates.push(resource_dir.join(executable_name));
    }
    for candidate in candidates {
        if candidate.is_file() {
            return std::fs::canonicalize(&candidate).map_err(|error| {
                format!("bundled llama.cpp executable could not be resolved: {error}")
            });
        }
    }

    Ok(PathBuf::from(executable_name))
}

#[tauri::command]
async fn native_runtime_status(
    state: State<'_, DesktopState>,
) -> Result<NativeRuntimeStatus, String> {
    state
        .native_runtime
        .status_with_metrics()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn start_native_model(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: NativeModelStartRequest,
) -> Result<NativeRuntimeStatus, String> {
    let model_id = request.model_id.trim().to_owned();
    if model_id.is_empty() {
        return Err("a local GGUF model must be selected first".to_owned());
    }

    let stored_model = state
        .database
        .get_model(&model_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "model does not exist; scan the library again".to_owned())?;
    let scanned = inspect_gguf_model(&stored_model.file_path).map_err(|error| error.to_string())?;
    if scanned.descriptor.id != stored_model.id
        || scanned.descriptor.file_size_bytes != stored_model.file_size_bytes
        || stored_model
            .metadata_hash
            .as_deref()
            .is_some_and(|hash| hash != scanned.metadata_hash)
    {
        return Err(
            "model file or metadata changed since the last scan; scan the library again".to_owned(),
        );
    }

    let profile = LoadProfile::for_preset(request.preset);
    MemoryEstimate::for_model(&scanned.descriptor, &profile).map_err(|error| error.to_string())?;
    let executable = resolve_llama_server_path(&app, request.runtime_path.as_deref())?;
    let runtime = Arc::clone(&state.native_runtime);
    runtime
        .start(executable, scanned.descriptor, profile)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn stop_native_model(state: State<'_, DesktopState>) -> Result<NativeRuntimeStatus, String> {
    state
        .native_runtime
        .stop()
        .map_err(|error| error.to_string())
}

fn model_record_from_scan(
    library_id: &str,
    scanned: &ScannedModel,
    last_seen_at: i64,
) -> ModelRecord {
    let descriptor = &scanned.descriptor;
    ModelRecord {
        id: descriptor.id.clone(),
        library_id: library_id.to_owned(),
        display_name: descriptor.display_name.clone(),
        file_path: descriptor.path.to_string_lossy().into_owned(),
        format: model_format_name(descriptor.format).to_owned(),
        family: descriptor.family.clone(),
        parameter_count: descriptor.parameter_count,
        architecture: descriptor.architecture.clone(),
        quantization: descriptor.quantization.clone(),
        gguf_version: descriptor.gguf_version.clone(),
        file_size_bytes: descriptor.file_size_bytes,
        context_capacity: descriptor.context_capacity,
        vision: descriptor.capabilities.vision,
        tool_calling: descriptor.capabilities.tool_calling,
        reasoning: descriptor.capabilities.reasoning,
        embeddings: descriptor.capabilities.embeddings,
        metadata_hash: Some(scanned.metadata_hash.clone()),
        last_seen_at,
        metadata_json: scanned.metadata_json.clone(),
    }
}

fn model_format_name(format: ModelFormat) -> &'static str {
    match format {
        ModelFormat::Gguf => "gguf",
        ModelFormat::Safetensors => "safetensors",
        ModelFormat::Unknown => "unknown",
    }
}

fn unix_now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
struct PendingAgentToolCall {
    id: String,
    name: String,
    arguments: String,
}

fn prepare_agent_request(mut request: ChatRequest) -> ChatRequest {
    let definitions = builtin_tool_definitions(request.web_tools, request.worker.is_some());
    if definitions.is_empty() {
        return request;
    }
    request.tools = definitions.clone();
    let instructions = tool_system_instructions(&definitions);
    if let Some(system_message) = request
        .messages
        .iter_mut()
        .find(|message| message.role == ChatRole::System)
    {
        system_message.content.push_str("\n\n");
        system_message.content.push_str(&instructions);
    } else {
        request.messages.insert(
            0,
            ChatMessage {
                role: ChatRole::System,
                content: instructions,
                name: None,
                tool_call_id: None,
                tool_calls: None,
            },
        );
    }
    request
}

fn collect_tool_call(
    calls: &mut Vec<PendingAgentToolCall>,
    delta: ToolCallDelta,
) -> Result<(), String> {
    if delta.index >= 8 {
        return Err("a model may request at most eight tools per round".into());
    }
    if calls.len() <= delta.index {
        calls.resize_with(delta.index + 1, PendingAgentToolCall::default);
    }
    let call = &mut calls[delta.index];
    if delta.id.as_ref().is_some_and(|id| id.len() > 256)
        || call.name.len() + delta.name.as_ref().map_or(0, String::len) > 128
        || call.arguments.len() + delta.arguments.as_ref().map_or(0, String::len) > 64 * 1024
    {
        return Err("streamed tool call exceeded its size limit".into());
    }
    if let Some(id) = delta.id.filter(|value| !value.trim().is_empty()) {
        call.id = id;
    }
    if let Some(name) = delta.name.filter(|value| !value.trim().is_empty()) {
        call.name.push_str(&name);
    }
    if let Some(arguments) = delta.arguments {
        call.arguments.push_str(&arguments);
    }
    Ok(())
}

fn assistant_tool_call(call: &PendingAgentToolCall, index: usize) -> AssistantToolCall {
    AssistantToolCall {
        id: if call.id.trim().is_empty() {
            format!("aegis-tool-call-{index}")
        } else {
            call.id.clone()
        },
        kind: "function".to_owned(),
        function: ToolCallFunction {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        },
    }
}

async fn run_delegate_tool(
    master: OpenAiCompatibleClient,
    worker: Option<OpenAiCompatibleClient>,
    arguments: &Value,
    max_tokens: u32,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<ToolExecution, String> {
    let task = arguments
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "delegate_task requires a non-empty task".to_owned())?;
    if task.len() > 12_000 {
        return Err("delegate task exceeds the 12,000 byte limit".to_owned());
    }
    let context = arguments
        .get("context")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(24_000)
        .collect::<String>();
    let worker_client = worker.ok_or_else(|| "no worker model is configured".to_owned())?;
    let worker_config = worker_client.config_clone();
    let worker_prompt = if context.is_empty() {
        task.to_owned()
    } else {
        format!("Task:\n{task}\n\nContext:\n{context}")
    };
    let worker_request = ChatRequest {
        provider: worker_config,
        messages: vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are Aegis Worker. Complete only the delegated bounded task. Return concise, factual output for the master model. Do not call tools or claim actions you did not perform.".to_owned(),
                name: None,
                tool_call_id: None,
                tool_calls: None,
            },
            ChatMessage {
                role: ChatRole::User,
                content: worker_prompt.clone(),
                name: None,
                tool_call_id: None,
                tool_calls: None,
            },
        ],
        max_tokens: max_tokens.clamp(64, 4_096),
        temperature: 0.2,
        tools: Vec::new(),
        worker: None,
        web_tools: false,
    };
    let worker_result = match tokio::time::timeout(
        Duration::from_secs(45),
        worker_client.complete_chat(worker_request, cancellation.clone()),
    )
    .await
    {
        Ok(Ok(completion)) if !completion.message.content.trim().is_empty() => Ok(completion),
        Ok(Ok(_)) => Err("worker returned an empty result".to_owned()),
        Ok(Err(ProviderError::Cancelled)) => return Err("tool execution was cancelled".to_owned()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("worker timed out after 45 seconds".to_owned()),
    };

    match worker_result {
        Ok(completion) => Ok(ToolExecution {
            tool_id: "delegate_task".to_owned(),
            output: json!({
                "source": "worker",
                "model": worker_client.model_name(),
                "content": completion.message.content
            })
            .to_string(),
            source_urls: Vec::new(),
        }),
        Err(worker_error) => {
            // Graceful degradation: the master takes over the same bounded
            // task when the worker endpoint is unavailable or times out.
            let fallback_request = ChatRequest {
                provider: master.config_clone(),
                messages: vec![
                    ChatMessage {
                        role: ChatRole::System,
                        content: "You are the Aegis master model. The worker was unavailable. Solve the delegated task directly and state that you used the master fallback.".to_owned(),
                        name: None,
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    ChatMessage {
                        role: ChatRole::User,
                        content: worker_prompt,
                        name: None,
                        tool_call_id: None,
                        tool_calls: None,
                    },
                ],
                max_tokens: max_tokens.clamp(64, 4_096),
                temperature: 0.2,
                tools: Vec::new(),
                worker: None,
                web_tools: false,
            };
            let fallback = master
                .complete_chat(fallback_request, cancellation)
                .await
                .map_err(|fallback_error| {
                    format!(
                        "worker failed ({worker_error}); master fallback failed ({fallback_error})"
                    )
                })?;
            Ok(ToolExecution {
                tool_id: "delegate_task".to_owned(),
                output: json!({
                    "source": "master_fallback",
                    "content": fallback.message.content,
                    "worker_error": worker_error
                })
                .to_string(),
                source_urls: Vec::new(),
            })
        }
    }
}

async fn run_agent_tool(
    master: OpenAiCompatibleClient,
    worker: Option<OpenAiCompatibleClient>,
    call: PendingAgentToolCall,
    max_tokens: u32,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<ToolExecution, String> {
    let arguments: Value = serde_json::from_str(&call.arguments)
        .map_err(|error| format!("invalid tool JSON arguments: {error}"))?;
    if call.name == "delegate_task" {
        return run_delegate_tool(master, worker, &arguments, max_tokens, cancellation).await;
    }
    execute_builtin_tool(&call.name, &arguments, cancellation)
        .await
        .map_err(|error| error.to_string())
}

fn is_tool_schema_unsupported(error: &ProviderError) -> bool {
    match error {
        ProviderError::HttpStatus {
            status: 400 | 422,
            body,
        } => {
            let message = body.to_ascii_lowercase();
            (message.contains("tool") || message.contains("function"))
                && (message.contains("unsupported")
                    || message.contains("not support")
                    || message.contains("not allowed"))
        }
        _ => false,
    }
}

async fn stream_chat_with_tools(
    client: OpenAiCompatibleClient,
    request: ChatRequest,
    app: AppHandle,
    operation_id: String,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<ChatCompletionSummary, ProviderError> {
    let plain_request = request.clone();
    let prepared = prepare_agent_request(request);
    let worker_client = prepared
        .worker
        .clone()
        .map(OpenAiCompatibleClient::new)
        .transpose()?;
    let definitions = builtin_tool_definitions(prepared.web_tools, prepared.worker.is_some());
    if definitions.is_empty() {
        let operation_for_events = operation_id.clone();
        let app_for_chunks = app.clone();
        return client
            .stream_chat(prepared, cancellation, move |chunk| match chunk {
                ChatChunk::Content { text } => emit_chat(
                    &app_for_chunks,
                    ChatEvent::Token {
                        operation_id: operation_for_events.clone(),
                        text,
                    },
                ),
                ChatChunk::Reasoning { text } => emit_chat(
                    &app_for_chunks,
                    ChatEvent::Reasoning {
                        operation_id: operation_for_events.clone(),
                        text,
                    },
                ),
                ChatChunk::ToolCallDelta(_) => {}
            })
            .await;
    }

    let plain_fallback = {
        let mut fallback = plain_request;
        fallback.tools.clear();
        fallback.web_tools = false;
        fallback.worker = None;
        fallback
    };
    let mut working = prepared;
    let mut tool_error_counts = std::collections::BTreeMap::<String, u8>::new();

    for round in 0..8 {
        if cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let mut pending = Vec::new();
        let mut pending_error = None;
        let mut round_content = String::new();
        let round_cancellation = cancellation.child_token();
        let operation_for_events = operation_id.clone();
        let app_for_chunks = app.clone();
        let result = client
            .stream_chat(
                working.clone(),
                round_cancellation.clone(),
                |chunk| match chunk {
                    ChatChunk::Content { text } => {
                        if round_content.len() + text.len() > 256 * 1024 {
                            pending_error =
                                Some("assistant tool context exceeded its size limit".into());
                            round_cancellation.cancel();
                            return;
                        }
                        round_content.push_str(&text);
                        emit_chat(
                            &app_for_chunks,
                            ChatEvent::Token {
                                operation_id: operation_for_events.clone(),
                                text,
                            },
                        )
                    }
                    ChatChunk::Reasoning { text } => emit_chat(
                        &app_for_chunks,
                        ChatEvent::Reasoning {
                            operation_id: operation_for_events.clone(),
                            text,
                        },
                    ),
                    ChatChunk::ToolCallDelta(delta) => {
                        if let Err(error) = collect_tool_call(&mut pending, delta) {
                            pending_error = Some(error);
                            round_cancellation.cancel();
                        }
                    }
                },
            )
            .await;
        if let Some(error) = pending_error {
            return Err(ProviderError::InvalidResponse(error));
        }
        let summary = match result {
            Ok(summary) => summary,
            Err(error) if round == 0 && is_tool_schema_unsupported(&error) => {
                emit_chat(
                    &app,
                    ChatEvent::Tool {
                        operation_id: operation_id.clone(),
                        tool_id: "tools".into(),
                        phase: "error".into(),
                        detail: "unsupported".into(),
                    },
                );
                let operation_for_events = operation_id.clone();
                let app_for_chunks = app.clone();
                return client
                    .stream_chat(plain_fallback, cancellation, move |chunk| match chunk {
                        ChatChunk::Content { text } => emit_chat(
                            &app_for_chunks,
                            ChatEvent::Token {
                                operation_id: operation_for_events.clone(),
                                text,
                            },
                        ),
                        ChatChunk::Reasoning { text } => emit_chat(
                            &app_for_chunks,
                            ChatEvent::Reasoning {
                                operation_id: operation_for_events.clone(),
                                text,
                            },
                        ),
                        ChatChunk::ToolCallDelta(_) => {}
                    })
                    .await;
            }
            Err(error) => return Err(error),
        };
        pending.retain(|call| !call.name.trim().is_empty());
        if pending.is_empty() {
            return Ok(summary);
        }

        for (index, call) in pending.iter_mut().enumerate() {
            if call.id.trim().is_empty() {
                call.id = format!("aegis-tool-call-{round}-{index}");
            }
        }

        let assistant_calls = pending
            .iter()
            .enumerate()
            .map(|(index, call)| assistant_tool_call(call, index))
            .collect::<Vec<_>>();
        working.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: round_content,
            name: None,
            tool_call_id: None,
            tool_calls: Some(assistant_calls),
        });

        for call in &pending {
            emit_chat(
                &app,
                ChatEvent::Tool {
                    operation_id: operation_id.clone(),
                    tool_id: call.name.clone(),
                    phase: "running".to_owned(),
                    detail: "running".to_owned(),
                },
            );
        }

        let max_tokens = working.max_tokens;
        let worker_config = worker_client.clone();
        let futures = pending.iter().cloned().map(|call| {
            let master = client.clone();
            let worker = worker_config.clone();
            let cancellation = cancellation.clone();
            let definitions = &definitions;
            async move {
                let validated = serde_json::from_str::<Value>(&call.arguments)
                    .map_err(|error| format!("invalid tool JSON: {error}"))
                    .and_then(|arguments| {
                        validate_tool_request(definitions, &call.name, &arguments)
                            .map_err(|error| error.to_string())
                    });
                let result = match validated {
                    Ok(()) => {
                        run_agent_tool(master, worker, call.clone(), max_tokens, cancellation)
                            .await
                            .and_then(|execution| {
                                if execution.output.len() <= 64 * 1024 {
                                    Ok(execution)
                                } else {
                                    Err("tool output exceeded 64 KiB; use a smaller input".into())
                                }
                            })
                    }
                    Err(error) => Err(error),
                };
                (call, result)
            }
        });
        let outcomes = futures_util::future::join_all(futures).await;
        for (call, result) in outcomes {
            if cancellation.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let call_id = call.id.clone();
            let (output, phase, detail) = match result {
                Ok(execution) => {
                    tool_error_counts.remove(&call.name);
                    let source_detail = if execution.source_urls.is_empty() {
                        "completed".to_owned()
                    } else {
                        format!("completed_sources:{}", execution.source_urls.len())
                    };
                    (execution.output, "completed".to_owned(), source_detail)
                }
                Err(error) => {
                    let count = tool_error_counts.entry(call.name.clone()).or_insert(0);
                    *count = count.saturating_add(1);
                    if *count >= 3 {
                        return Err(ProviderError::InvalidRequest(format!(
                            "tool {} failed three times: {error}",
                            call.name
                        )));
                    }
                    (
                        json!({
                            "error": error,
                            "retryable": true,
                            "attempt": *count,
                            "instruction": "Correct the JSON arguments and call the same tool again."
                        })
                        .to_string(),
                        "error".to_owned(),
                        format!("retry:{}", *count),
                    )
                }
            };
            emit_chat(
                &app,
                ChatEvent::Tool {
                    operation_id: operation_id.clone(),
                    tool_id: call.name.clone(),
                    phase,
                    detail,
                },
            );
            working.messages.push(ChatMessage {
                role: ChatRole::Tool,
                content: output,
                name: Some(call.name),
                tool_call_id: Some(call_id),
                tool_calls: None,
            });
        }
    }
    Err(ProviderError::InvalidRequest(
        "tool loop exceeded the maximum of 8 rounds".to_owned(),
    ))
}

#[tauri::command]
async fn start_chat(
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: ChatRequest,
) -> Result<OperationStarted, String> {
    request.validate().map_err(|error| error.to_string())?;
    let client =
        OpenAiCompatibleClient::new(request.provider.clone()).map_err(|error| error.to_string())?;
    let (operation_id, cancellation) = state
        .core
        .start_operation()
        .map_err(|error| error.to_string())?;
    let operation_id_text = operation_id.to_string();
    let operation_id_for_task = operation_id_text.clone();
    let core = Arc::clone(&state.core);
    let app_for_task = app.clone();

    tauri::async_runtime::spawn(async move {
        emit_chat(
            &app_for_task,
            ChatEvent::Started {
                operation_id: operation_id_for_task.clone(),
            },
        );
        let result = stream_chat_with_tools(
            client,
            request,
            app_for_task.clone(),
            operation_id_for_task.clone(),
            cancellation,
        )
        .await;

        match result {
            Ok(summary) => emit_chat(
                &app_for_task,
                finished_event(operation_id_for_task.clone(), summary),
            ),
            Err(ProviderError::Cancelled) => emit_chat(
                &app_for_task,
                ChatEvent::Cancelled {
                    operation_id: operation_id_for_task.clone(),
                },
            ),
            Err(error) => emit_chat(
                &app_for_task,
                ChatEvent::Failed {
                    operation_id: operation_id_for_task.clone(),
                    code: error_code(&error).to_owned(),
                    message: error.to_string(),
                    retryable: error.is_retryable(),
                },
            ),
        }
        let _ = core.finish_operation(operation_id);
    });

    Ok(OperationStarted {
        operation_id: operation_id_text,
    })
}

fn finished_event(operation_id: String, summary: ChatCompletionSummary) -> ChatEvent {
    ChatEvent::Finished {
        operation_id,
        generated_tokens: summary.generated_tokens,
        prompt_tokens: summary.prompt_tokens,
        time_to_first_token_ms: summary.time_to_first_token_ms,
        generation_duration_ms: summary.generation_duration_ms,
        finish_reason: summary.finish_reason,
    }
}

fn error_code(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::InvalidConfig(_) => "invalid_config",
        ProviderError::InvalidRequest(_) => "invalid_request",
        ProviderError::Transport(_) => "transport",
        ProviderError::Client(_) => "client",
        ProviderError::HttpStatus { .. } => "http_status",
        ProviderError::InvalidResponse(_) => "invalid_response",
        ProviderError::Cancelled => "cancelled",
    }
}

#[tauri::command]
async fn list_provider_models(config: ProviderConfig) -> Result<Vec<ProviderModel>, String> {
    let client = OpenAiCompatibleClient::new(config).map_err(|error| error.to_string())?;
    client
        .list_models()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn load_provider_model(
    config: ProviderConfig,
    kind: ManagedProvider,
) -> Result<String, String> {
    OpenAiCompatibleClient::new(config)
        .map_err(|error| error.to_string())?
        .load_managed_model(kind)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn inspect_provider(config: ProviderConfig) -> Result<ProviderDiagnostics, String> {
    let endpoint_url = config.validate().map_err(|error| error.to_string())?;
    let endpoint = endpoint_url.as_str().trim_end_matches('/').to_owned();
    let local = endpoint_url.scheme() == "http";
    let client = OpenAiCompatibleClient::new(config).map_err(|error| error.to_string())?;
    let started = Instant::now();

    match client.list_models().await {
        Ok(models) => Ok(ProviderDiagnostics {
            status: "connected".to_owned(),
            endpoint,
            local,
            latency_ms: started.elapsed().as_secs_f64() * 1000.0,
            model_count: models.len(),
            models,
            error: None,
            retryable: false,
        }),
        Err(error) => Ok(ProviderDiagnostics {
            status: "error".to_owned(),
            endpoint,
            local,
            latency_ms: started.elapsed().as_secs_f64() * 1000.0,
            model_count: 0,
            models: Vec::new(),
            error: Some(error.to_string()),
            retryable: error.is_retryable(),
        }),
    }
}

#[tauri::command]
fn cancel_operation(state: State<'_, DesktopState>, operation_id: String) -> Result<bool, String> {
    let operation_id =
        Uuid::parse_str(&operation_id).map_err(|error| format!("invalid operation id: {error}"))?;
    state
        .core
        .cancel_operation(operation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn stop_everything(state: State<'_, DesktopState>) -> Result<usize, String> {
    state
        .native_runtime
        .stop()
        .map_err(|error| error.to_string())?;
    state
        .core
        .stop_everything()
        .map_err(|error| error.to_string())
}

fn initialize_database(app: &AppHandle) -> Result<Database, std::io::Error> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    std::fs::create_dir_all(&data_dir)?;
    Database::open(data_dir.join("aegis.sqlite3"))
        .map_err(|error| std::io::Error::other(error.to_string()))
}

pub fn run() {
    let core = match ApplicationCore::new() {
        Ok(core) => Arc::new(core),
        Err(error) => {
            eprintln!("Aegis core failed to initialize: {error}");
            return;
        }
    };

    if let Err(error) = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| -> Result<(), Box<dyn std::error::Error>> {
            let database = initialize_database(app.handle())?;
            let native_runtime = Arc::new(
                LlamaServerRuntime::new()
                    .map_err(|error| std::io::Error::other(error.to_string()))?,
            );
            app.manage(DesktopState {
                core: Arc::clone(&core),
                database: Arc::new(database),
                native_runtime,
                model_watcher: std::sync::Mutex::new(model_watch::ModelWatcher::default()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            runtime_status,
            load_conversations,
            save_conversation,
            delete_conversation,
            load_workspaces,
            save_workspace,
            delete_workspace,
            validate_workspace_path,
            load_automations,
            save_automation,
            delete_automation,
            load_model_libraries,
            save_model_library,
            delete_model_library,
            load_local_models,
            discover_local_models,
            scan_model_library,
            load_model_profiles,
            save_model_profile,
            delete_model_profile,
            estimate_model_load,
            native_runtime_status,
            start_native_model,
            stop_native_model,
            start_chat,
            list_provider_models,
            load_provider_model,
            inspect_provider,
            cancel_operation,
            stop_everything
        ])
        .run(tauri::generate_context!())
    {
        eprintln!("Aegis desktop application stopped with an error: {error}");
    }
}
