use aegis_core::{ApplicationCore, RuntimeStatus};
use aegis_providers::{
    ChatChunk, ChatCompletionSummary, ChatRequest, OpenAiCompatibleClient, ProviderConfig,
    ProviderError, ProviderModel,
};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

pub struct DesktopState {
    pub core: Arc<ApplicationCore>,
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
}

fn emit_chat(app: &AppHandle, event: ChatEvent) {
    let _ = app.emit("aegis://chat", event);
}

#[tauri::command]
fn runtime_status(state: State<'_, DesktopState>) -> Result<RuntimeStatus, String> {
    state
        .core
        .runtime_status()
        .map_err(|error| error.to_string())
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
    let core = Arc::clone(&state.core);
    let app_for_task = app.clone();

    tauri::async_runtime::spawn(async move {
        emit_chat(
            &app_for_task,
            ChatEvent::Started {
                operation_id: operation_id_text.clone(),
            },
        );
        let operation_for_events = operation_id_text.clone();
        let app_for_chunks = app_for_task.clone();
        let result = client
            .stream_chat(request, cancellation, move |chunk| {
                let event = match chunk {
                    ChatChunk::Content { text } => ChatEvent::Token {
                        operation_id: operation_for_events.clone(),
                        text,
                    },
                    ChatChunk::Reasoning { text } => ChatEvent::Reasoning {
                        operation_id: operation_for_events.clone(),
                        text,
                    },
                };
                emit_chat(&app_for_chunks, event);
            })
            .await;

        match result {
            Ok(summary) => emit_chat(
                &app_for_task,
                finished_event(operation_id_text.clone(), summary),
            ),
            Err(ProviderError::Cancelled) => emit_chat(
                &app_for_task,
                ChatEvent::Cancelled {
                    operation_id: operation_id_text.clone(),
                },
            ),
            Err(error) => emit_chat(
                &app_for_task,
                ChatEvent::Failed {
                    operation_id: operation_id_text.clone(),
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
        .core
        .stop_everything()
        .map_err(|error| error.to_string())
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
        .manage(DesktopState { core })
        .invoke_handler(tauri::generate_handler![
            runtime_status,
            start_chat,
            list_provider_models,
            cancel_operation,
            stop_everything
        ])
        .run(tauri::generate_context!())
    {
        eprintln!("Aegis desktop application stopped with an error: {error}");
    }
}
