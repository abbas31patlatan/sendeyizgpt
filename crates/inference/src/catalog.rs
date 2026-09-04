//! Safe, metadata-only discovery of local GGUF model files.
//!
//! The scanner deliberately reads only GGUF headers and metadata. It never loads tensor
//! data, follows directory symlinks, executes a model, or guesses an inference result.

use crate::{InferenceError, ModelCapabilities, ModelDescriptor, ModelFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;

const GGUF_MAGIC: &[u8; 4] = b"GGUF";
const GGUF_VERSION_V2: u32 = 2;
const GGUF_VERSION_V3: u32 = 3;
const MAX_METADATA_KEYS: u64 = 4_096;
const MAX_METADATA_STRING_BYTES: u64 = 1_048_576;
const MAX_METADATA_ARRAY_ITEMS: u64 = 16_384;
const MAX_METADATA_DEPTH: usize = 8;
const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
const MAX_METADATA_JSON_BYTES: usize = 4 * 1024 * 1024;

/// Hard upper bound for a single user-triggered directory scan.
pub const MAX_SCAN_FILES: usize = 2_048;
/// Hard upper bound for recursive traversal. Symlinks are never followed.
pub const MAX_SCAN_DEPTH: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScannedModel {
    pub descriptor: ModelDescriptor,
    pub metadata_json: String,
    pub metadata_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelScanIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelScanReport {
    pub root_path: PathBuf,
    pub models: Vec<ScannedModel>,
    pub issues: Vec<ModelScanIssue>,
    pub visited_files: usize,
}

pub fn scan_model_directory(root: impl AsRef<Path>) -> Result<ModelScanReport, InferenceError> {
    let root = root.as_ref();
    let metadata = fs::metadata(root).map_err(|error| scan_error(root, error))?;
    if !metadata.is_dir() {
        return Err(InferenceError::ModelScan(format!(
            "model library is not a directory: {}",
            root.display()
        )));
    }

    let canonical_root = fs::canonicalize(root).map_err(|error| scan_error(root, error))?;
    let mut report = ModelScanReport {
        root_path: canonical_root.clone(),
        models: Vec::new(),
        issues: Vec::new(),
        visited_files: 0,
    };

    scan_directory(&canonical_root, 0, &mut report)
        .map_err(|error| InferenceError::ModelScan(error.to_string()))?;
    report.models.sort_by(|left, right| {
        left.descriptor
            .display_name
            .to_ascii_lowercase()
            .cmp(&right.descriptor.display_name.to_ascii_lowercase())
            .then_with(|| left.descriptor.path.cmp(&right.descriptor.path))
    });
    Ok(report)
}

pub fn inspect_gguf_model(path: impl AsRef<Path>) -> Result<ScannedModel, InferenceError> {
    let path = path.as_ref();
    let metadata = fs::metadata(path).map_err(|error| scan_error(path, error))?;
    if !metadata.is_file() {
        return Err(InferenceError::ModelScan(format!(
            "model path is not a file: {}",
            path.display()
        )));
    }
    if !is_gguf_path(path) {
        return Err(InferenceError::ModelScan(format!(
            "unsupported model extension: {}",
            path.display()
        )));
    }

    let canonical_path = fs::canonicalize(path).map_err(|error| scan_error(path, error))?;
    let file = File::open(&canonical_path).map_err(|error| scan_error(&canonical_path, error))?;
    let mut reader = LimitedReader::new(BufReader::new(file), MAX_METADATA_BYTES);
    let (version, values) = read_gguf_metadata(&mut reader)
        .map_err(|error| InferenceError::ModelScan(error.to_string()))?;
    let metadata_json = serde_json::to_string(&values)
        .map_err(|error| InferenceError::ModelScan(error.to_string()))?;
    if metadata_json.len() > MAX_METADATA_JSON_BYTES {
        return Err(InferenceError::ModelScan(format!(
            "metadata JSON exceeds the {} byte safety limit",
            MAX_METADATA_JSON_BYTES
        )));
    }

    let descriptor = descriptor_from_metadata(
        &canonical_path,
        metadata.len(),
        version,
        &values,
        &metadata_json,
    );
    let metadata_hash = blake3::hash(metadata_json.as_bytes()).to_hex().to_string();

    Ok(ScannedModel {
        descriptor,
        metadata_json,
        metadata_hash,
    })
}

fn scan_directory(
    directory: &Path,
    depth: usize,
    report: &mut ModelScanReport,
) -> Result<(), ScanError> {
    let mut entries = Vec::new();
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            report.issues.push(ModelScanIssue {
                path: directory.to_path_buf(),
                message: format!("directory could not be read: {error}"),
            });
            return Ok(());
        }
    };

    for entry in read_dir {
        match entry {
            Ok(entry) => entries.push(entry),
            Err(error) => report.issues.push(ModelScanIssue {
                path: directory.to_path_buf(),
                message: format!("directory entry could not be read: {error}"),
            }),
        }
    }
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                report.issues.push(ModelScanIssue {
                    path,
                    message: format!("file type could not be read: {error}"),
                });
                continue;
            }
        };

        if file_type.is_symlink() {
            report.issues.push(ModelScanIssue {
                path,
                message: "symlink skipped by the model scanner".to_owned(),
            });
            continue;
        }

        if file_type.is_dir() {
            if depth >= MAX_SCAN_DEPTH {
                report.issues.push(ModelScanIssue {
                    path,
                    message: format!("maximum scan depth ({MAX_SCAN_DEPTH}) reached"),
                });
                continue;
            }
            scan_directory(&path, depth + 1, report)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        if report.visited_files >= MAX_SCAN_FILES {
            report.issues.push(ModelScanIssue {
                path,
                message: format!("maximum scan file limit ({MAX_SCAN_FILES}) reached"),
            });
            return Ok(());
        }
        report.visited_files += 1;

        if !is_gguf_path(&path) {
            continue;
        }

        match inspect_gguf_model(&path) {
            Ok(model) => report.models.push(model),
            Err(error) => report.issues.push(ModelScanIssue {
                path,
                message: error.to_string(),
            }),
        }
    }

    Ok(())
}

fn is_gguf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
}

fn descriptor_from_metadata(
    path: &Path,
    file_size_bytes: u64,
    version: u32,
    values: &BTreeMap<String, Value>,
    metadata_json: &str,
) -> ModelDescriptor {
    let architecture = metadata_string(values, "general.architecture");
    let architecture_key = architecture.as_deref().unwrap_or("llama");
    let display_name = metadata_string(values, "general.name")
        .or_else(|| metadata_string(values, "general.basename"))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("GGUF model")
                .to_owned()
        });
    let family = metadata_string(values, "general.family").or_else(|| architecture.clone());

    let quantization = metadata_u32(values, "general.file_type")
        .map(quantization_label)
        .or_else(|| metadata_string(values, "general.quantization"));
    let context_capacity = first_u32(
        values,
        &[
            format!("{architecture_key}.context_length"),
            "general.context_length".to_owned(),
        ],
    );
    let layer_count = first_u32(
        values,
        &[
            format!("{architecture_key}.block_count"),
            "general.block_count".to_owned(),
        ],
    );
    let attention_head_count = first_u32(
        values,
        &[
            format!("{architecture_key}.attention.head_count"),
            "general.attention.head_count".to_owned(),
        ],
    );
    let key_value_head_count = first_u32(
        values,
        &[
            format!("{architecture_key}.attention.head_count_kv"),
            "general.attention.head_count_kv".to_owned(),
        ],
    );
    let embedding_length = first_u32(
        values,
        &[
            format!("{architecture_key}.embedding_length"),
            "general.embedding_length".to_owned(),
        ],
    );
    let parameter_count = first_u64(
        values,
        &[
            "general.parameter_count".to_owned(),
            "general.parameters".to_owned(),
        ],
    );

    let metadata_text = metadata_json.to_ascii_lowercase();
    let model_text = format!(
        "{} {} {} {}",
        display_name,
        family.as_deref().unwrap_or_default(),
        architecture.as_deref().unwrap_or_default(),
        metadata_text
    )
    .to_ascii_lowercase();
    let capabilities = ModelCapabilities {
        vision: model_text.contains("vision")
            || model_text.contains("clip")
            || model_text.contains("image"),
        tool_calling: model_text.contains("tool")
            || model_text.contains("function_call")
            || model_text.contains("function-call"),
        reasoning: model_text.contains("reason")
            || model_text.contains("deepseek-r1")
            || model_text.contains("qwq"),
        embeddings: model_text.contains("embedding")
            || model_text.contains("nomic-embed")
            || model_text.contains("sentence-transformer"),
        audio_input: model_text.contains("audio")
            || model_text.contains("whisper")
            || model_text.contains("speech"),
    };

    let model_id = {
        let digest = blake3::hash(path.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        format!("gguf-{}", &digest[..24])
    };

    ModelDescriptor {
        id: model_id,
        display_name,
        path: path.to_path_buf(),
        format: ModelFormat::Gguf,
        family,
        parameter_count,
        architecture,
        quantization: quantization.clone(),
        gguf_version: Some(version.to_string()),
        file_size_bytes,
        context_capacity,
        layer_count,
        attention_head_count,
        key_value_head_count,
        embedding_length,
        bits_per_weight: quantization.as_deref().and_then(bits_per_weight),
        capabilities,
    }
}

fn first_u32(values: &BTreeMap<String, Value>, keys: &[String]) -> Option<u32> {
    keys.iter().find_map(|key| metadata_u32(values, key))
}

fn first_u64(values: &BTreeMap<String, Value>, keys: &[String]) -> Option<u64> {
    keys.iter().find_map(|key| metadata_u64(values, key))
}

fn metadata_string(values: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    values.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn metadata_u64(values: &BTreeMap<String, Value>, key: &str) -> Option<u64> {
    values.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().filter(|value| *value >= 0).map(|value| value as u64))
    })
}

fn metadata_u32(values: &BTreeMap<String, Value>, key: &str) -> Option<u32> {
    metadata_u64(values, key).and_then(|value| u32::try_from(value).ok())
}

fn quantization_label(file_type: u32) -> String {
    match file_type {
        0 => "F32".to_owned(),
        1 => "F16".to_owned(),
        2 => "Q4_0".to_owned(),
        3 => "Q4_1".to_owned(),
        6 => "Q5_0".to_owned(),
        7 => "Q5_1".to_owned(),
        8 => "Q8_0".to_owned(),
        9 => "Q8_1".to_owned(),
        10 => "Q2_K".to_owned(),
        11 => "Q2_K_S".to_owned(),
        12 => "Q3_K_S".to_owned(),
        13 => "Q3_K_M".to_owned(),
        14 => "Q3_K_L".to_owned(),
        15 => "Q4_K_S".to_owned(),
        16 => "Q4_K_M".to_owned(),
        17 => "Q5_K_S".to_owned(),
        18 => "Q5_K_M".to_owned(),
        19 => "Q6_K".to_owned(),
        20 => "TQ1_0".to_owned(),
        21 => "TQ2_0".to_owned(),
        _ => format!("file_type_{file_type}"),
    }
}

fn bits_per_weight(quantization: &str) -> Option<f32> {
    let upper = quantization.to_ascii_uppercase();
    if upper.contains("F32") {
        return Some(32.0);
    }
    if upper.contains("F16") || upper.contains("BF16") {
        return Some(16.0);
    }

    let start = upper
        .char_indices()
        .find(|(_, character)| *character == 'Q' || *character == 'I')
        .map(|(index, _)| index + 1)?;
    let digits = upper[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty())
        .then(|| digits.parse::<f32>().ok())
        .flatten()
}

struct LimitedReader<R> {
    inner: R,
    consumed: u64,
    limit: u64,
}

impl<R: Read> LimitedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            consumed: 0,
            limit,
        }
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), ScanError> {
        let length = buffer.len() as u64;
        if self.consumed.saturating_add(length) > self.limit {
            return Err(ScanError::Limit(format!(
                "metadata exceeds the {} byte safety limit",
                self.limit
            )));
        }
        self.inner.read_exact(buffer)?;
        self.consumed += length;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, ScanError> {
        let mut buffer = [0_u8; 1];
        self.read_exact(&mut buffer)?;
        Ok(buffer[0])
    }

    fn u16(&mut self) -> Result<u16, ScanError> {
        let mut buffer = [0_u8; 2];
        self.read_exact(&mut buffer)?;
        Ok(u16::from_le_bytes(buffer))
    }

    fn u32(&mut self) -> Result<u32, ScanError> {
        let mut buffer = [0_u8; 4];
        self.read_exact(&mut buffer)?;
        Ok(u32::from_le_bytes(buffer))
    }

    fn u64(&mut self) -> Result<u64, ScanError> {
        let mut buffer = [0_u8; 8];
        self.read_exact(&mut buffer)?;
        Ok(u64::from_le_bytes(buffer))
    }

    fn i8(&mut self) -> Result<i8, ScanError> {
        Ok(self.u8()? as i8)
    }

    fn i16(&mut self) -> Result<i16, ScanError> {
        let mut buffer = [0_u8; 2];
        self.read_exact(&mut buffer)?;
        Ok(i16::from_le_bytes(buffer))
    }

    fn i32(&mut self) -> Result<i32, ScanError> {
        let mut buffer = [0_u8; 4];
        self.read_exact(&mut buffer)?;
        Ok(i32::from_le_bytes(buffer))
    }

    fn i64(&mut self) -> Result<i64, ScanError> {
        let mut buffer = [0_u8; 8];
        self.read_exact(&mut buffer)?;
        Ok(i64::from_le_bytes(buffer))
    }

    fn f32(&mut self) -> Result<f32, ScanError> {
        let mut buffer = [0_u8; 4];
        self.read_exact(&mut buffer)?;
        Ok(f32::from_le_bytes(buffer))
    }

    fn f64(&mut self) -> Result<f64, ScanError> {
        let mut buffer = [0_u8; 8];
        self.read_exact(&mut buffer)?;
        Ok(f64::from_le_bytes(buffer))
    }

    fn string(&mut self) -> Result<String, ScanError> {
        let length = self.u64()?;
        if length > MAX_METADATA_STRING_BYTES {
            return Err(ScanError::Limit(format!(
                "metadata string exceeds the {} byte safety limit",
                MAX_METADATA_STRING_BYTES
            )));
        }
        let mut buffer = vec![0_u8; usize::try_from(length).map_err(|_| {
            ScanError::Limit("metadata string length does not fit in memory".to_owned())
        })?];
        self.read_exact(&mut buffer)?;
        String::from_utf8(buffer)
            .map_err(|_| ScanError::Invalid("metadata string is not valid UTF-8".to_owned()))
    }
}

fn read_gguf_metadata(
    reader: &mut LimitedReader<BufReader<File>>,
) -> Result<(u32, BTreeMap<String, Value>), ScanError> {
    let mut magic = [0_u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != GGUF_MAGIC {
        return Err(ScanError::Invalid("GGUF magic is missing".to_owned()));
    }

    let version = reader.u32()?;
    if !matches!(version, GGUF_VERSION_V2 | GGUF_VERSION_V3) {
        return Err(ScanError::Invalid(format!(
            "unsupported GGUF version {version}; supported versions are 2 and 3"
        )));
    }

    let _tensor_count = reader.u64()?;
    let metadata_count = reader.u64()?;
    if metadata_count > MAX_METADATA_KEYS {
        return Err(ScanError::Limit(format!(
            "metadata key count exceeds the {MAX_METADATA_KEYS} key safety limit"
        )));
    }

    let mut values = BTreeMap::new();
    for _ in 0..metadata_count {
        let key = reader.string()?;
        let value_type = reader.u32()?;
        let value = read_value(reader, value_type, 0)?;
        values.insert(key, value);
    }
    Ok((version, values))
}

fn read_value(
    reader: &mut LimitedReader<BufReader<File>>,
    value_type: u32,
    depth: usize,
) -> Result<Value, ScanError> {
    match value_type {
        0 => Ok(Value::from(reader.u8()?)),
        1 => Ok(Value::from(reader.i8()?)),
        2 => Ok(Value::from(reader.u16()?)),
        3 => Ok(Value::from(reader.i16()?)),
        4 => Ok(Value::from(reader.u32()?)),
        5 => Ok(Value::from(reader.i32()?)),
        6 => number_value(f64::from(reader.f32()?)),
        7 => match reader.u8()? {
            0 => Ok(Value::Bool(false)),
            1 => Ok(Value::Bool(true)),
            value => Err(ScanError::Invalid(format!(
                "invalid GGUF boolean value {value}"
            ))),
        },
        8 => Ok(Value::String(reader.string()?)),
        9 => {
            if depth >= MAX_METADATA_DEPTH {
                return Err(ScanError::Limit(format!(
                    "metadata array nesting exceeds the {MAX_METADATA_DEPTH} level safety limit"
                )));
            }
            let element_type = reader.u32()?;
            let length = reader.u64()?;
            if length > MAX_METADATA_ARRAY_ITEMS {
                return Err(ScanError::Limit(format!(
                    "metadata array exceeds the {MAX_METADATA_ARRAY_ITEMS} item safety limit"
                )));
            }
            let mut values = Vec::with_capacity(usize::try_from(length).map_err(|_| {
                ScanError::Limit("metadata array length does not fit in memory".to_owned())
            })?);
            for _ in 0..length {
                values.push(read_value(reader, element_type, depth + 1)?);
            }
            Ok(Value::Array(values))
        }
        10 => Ok(Value::from(reader.u64()?)),
        11 => Ok(Value::from(reader.i64()?)),
        12 => number_value(reader.f64()?),
        unsupported => Err(ScanError::Invalid(format!(
            "unsupported GGUF metadata value type {unsupported}"
        ))),
    }
}

fn number_value(value: f64) -> Result<Value, ScanError> {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| ScanError::Invalid("metadata contains a non-finite number".to_owned()))
}

fn scan_error(path: &Path, error: io::Error) -> InferenceError {
    InferenceError::ModelScan(format!("{}: {error}", path.display()))
}

#[derive(Debug, Error)]
enum ScanError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid GGUF metadata: {0}")]
    Invalid(String),
    #[error("GGUF scan limit exceeded: {0}")]
    Limit(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use uuid::Uuid;

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_string(bytes: &mut Vec<u8>, value: &str) {
        push_u64(bytes, value.len() as u64);
        bytes.extend_from_slice(value.as_bytes());
    }

    fn push_string_value(bytes: &mut Vec<u8>, key: &str, value: &str) {
        push_string(bytes, key);
        push_u32(bytes, 8);
        push_string(bytes, value);
    }

    fn push_u32_value(bytes: &mut Vec<u8>, key: &str, value: u32) {
        push_string(bytes, key);
        push_u32(bytes, 4);
        push_u32(bytes, value);
    }

    fn minimal_gguf() -> Vec<u8> {
        let entries = [
            ("general.architecture", "llama"),
            ("general.name", "Aegis Test Model"),
        ];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(GGUF_MAGIC);
        push_u32(&mut bytes, GGUF_VERSION_V3);
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, 7);
        for (key, value) in entries {
            push_string_value(&mut bytes, key, value);
        }
        push_u32_value(&mut bytes, "llama.context_length", 8192);
        push_u32_value(&mut bytes, "llama.block_count", 32);
        push_u32_value(&mut bytes, "llama.attention.head_count", 32);
        push_u32_value(&mut bytes, "llama.attention.head_count_kv", 8);
        push_u32_value(&mut bytes, "llama.embedding_length", 4096);
        push_u32_value(&mut bytes, "general.file_type", 16);
        bytes
    }

    #[test]
    fn parses_gguf_metadata_without_loading_tensors() {
        let file_name = format!(
            "aegis-gguf-test-{}-{}.gguf",
            std::process::id(),
            Uuid::new_v4()
        );
        let path = std::env::temp_dir().join(file_name);
        fs::write(&path, minimal_gguf()).expect("test GGUF writes");

        let scanned = inspect_gguf_model(&path).expect("GGUF parses");
        assert_eq!(scanned.descriptor.display_name, "Aegis Test Model");
        assert_eq!(scanned.descriptor.context_capacity, Some(8192));
        assert_eq!(scanned.descriptor.layer_count, Some(32));
        assert_eq!(scanned.descriptor.key_value_head_count, Some(8));
        assert_eq!(scanned.descriptor.quantization.as_deref(), Some("Q4_K_M"));
        assert!(!scanned.metadata_hash.is_empty());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn directory_scan_reports_corrupt_gguf_and_keeps_valid_models() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("aegis-scan-{suffix}"));
        fs::create_dir_all(&directory).expect("directory creates");
        let valid = directory.join("valid.gguf");
        let invalid = directory.join("invalid.gguf");
        fs::write(&valid, minimal_gguf()).expect("valid GGUF writes");
        fs::write(&invalid, b"not a GGUF").expect("invalid GGUF writes");

        let report = scan_model_directory(&directory).expect("directory scans");
        assert_eq!(report.models.len(), 1);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.models[0].descriptor.display_name, "Aegis Test Model");

        let _ = fs::remove_file(valid);
        let _ = fs::remove_file(invalid);
        let _ = fs::remove_dir(directory);
    }
}
