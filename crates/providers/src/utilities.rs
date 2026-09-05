use super::{
    ToolDefinition,
    tools::{BuiltinToolError, ToolExecution},
};
use serde_json::{Value, json};

pub(super) fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::function(
            "text_search",
            "Find literal text in supplied content and return matching lines. No files are accessed.",
            json!({"type":"object","additionalProperties":false,"required":["text","query"],"properties":{"text":{"type":"string","maxLength":48000},"query":{"type":"string","minLength":1,"maxLength":256},"case_sensitive":{"type":"boolean"}}}),
        ),
        ToolDefinition::function(
            "json_query",
            "Extract a value from supplied JSON using an RFC 6901 JSON Pointer, such as /items/0/name. No code is evaluated.",
            json!({"type":"object","additionalProperties":false,"required":["json","pointer"],"properties":{"json":{"type":"string","maxLength":48000},"pointer":{"type":"string","maxLength":512}}}),
        ),
        ToolDefinition::function(
            "convert_units",
            "Convert length, mass, time or temperature units. Supported: mm, cm, m, km, in, ft, mi; g, kg, lb; s, min, h; C, F, K. Never converts between different dimensions.",
            json!({"type":"object","additionalProperties":false,"required":["value","from","to"],"properties":{"value":{"type":"number"},"from":{"type":"string","maxLength":8},"to":{"type":"string","maxLength":8}}}),
        ),
    ]
}

fn string<'a>(args: &'a Value, name: &str) -> Result<&'a str, BuiltinToolError> {
    args[name]
        .as_str()
        .filter(|text| text.len() <= 48_000)
        .ok_or_else(|| {
            BuiltinToolError::InvalidArguments(format!("{name} must be a bounded string"))
        })
}

pub(super) fn execute(name: &str, args: &Value) -> Result<ToolExecution, BuiltinToolError> {
    let output = match name {
        "text_search" => {
            let text = string(args, "text")?;
            let query = string(args, "query")?;
            if query.is_empty() {
                return Err(BuiltinToolError::InvalidArguments("query is empty".into()));
            }
            let sensitive = args["case_sensitive"].as_bool().unwrap_or(false);
            let needle = if sensitive {
                query.to_owned()
            } else {
                query.to_lowercase()
            };
            let matches: Vec<Value> = text.lines().enumerate().filter(|(_,line)| {
                if sensitive {line.contains(&needle)} else {line.to_lowercase().contains(&needle)}
            }).take(51).map(|(index,line)| json!({"line":index+1,"text":line.chars().take(512).collect::<String>()})).collect();
            json!({"matches":matches.iter().take(50).collect::<Vec<_>>(),"truncated":matches.len()>50})
        }
        "json_query" => {
            let value: Value = serde_json::from_str(string(args, "json")?).map_err(|error| {
                BuiltinToolError::InvalidArguments(format!("invalid JSON: {error}"))
            })?;
            let pointer = string(args, "pointer")?;
            let result = value.pointer(pointer).ok_or_else(|| {
                BuiltinToolError::InvalidArguments("JSON pointer did not match a value".into())
            })?;
            json!({"pointer":pointer,"value":result})
        }
        "convert_units" => {
            let value = args["value"].as_f64().ok_or_else(|| {
                BuiltinToolError::InvalidArguments("value must be a finite number".into())
            })?;
            let from = unit(string(args, "from")?)?;
            let to = unit(string(args, "to")?)?;
            if from.0 != to.0 {
                return Err(BuiltinToolError::InvalidArguments(
                    "cannot convert between different dimensions".into(),
                ));
            }
            let base = value * from.1 + from.2;
            if from.0 == "temperature" && base < -273.15 {
                return Err(BuiltinToolError::InvalidArguments(
                    "temperature is below absolute zero".into(),
                ));
            }
            let result = (base - to.2) / to.1;
            if !result.is_finite() {
                return Err(BuiltinToolError::InvalidArguments(
                    "result is not finite".into(),
                ));
            }
            json!({"value":value,"from":args["from"],"to":args["to"],"result":result})
        }
        _ => {
            return Err(BuiltinToolError::InvalidArguments(format!(
                "unknown tool: {name}"
            )));
        }
    };
    Ok(ToolExecution {
        tool_id: name.to_owned(),
        output: output.to_string(),
        source_urls: vec![],
    })
}

fn unit(name: &str) -> Result<(&'static str, f64, f64), BuiltinToolError> {
    Ok(match name {
        "mm" => ("length", 0.001, 0.0),
        "cm" => ("length", 0.01, 0.0),
        "m" => ("length", 1.0, 0.0),
        "km" => ("length", 1000.0, 0.0),
        "in" => ("length", 0.0254, 0.0),
        "ft" => ("length", 0.3048, 0.0),
        "mi" => ("length", 1609.344, 0.0),
        "g" => ("mass", 0.001, 0.0),
        "kg" => ("mass", 1.0, 0.0),
        "lb" => ("mass", 0.45359237, 0.0),
        "s" => ("time", 1.0, 0.0),
        "min" => ("time", 60.0, 0.0),
        "h" => ("time", 3600.0, 0.0),
        "C" => ("temperature", 1.0, 0.0),
        "F" => ("temperature", 5.0 / 9.0, -160.0 / 9.0),
        "K" => ("temperature", 1.0, -273.15),
        _ => {
            return Err(BuiltinToolError::InvalidArguments(format!(
                "unsupported unit: {name}"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversions_handle_offsets_and_reject_dimension_mismatches() {
        let result = execute("convert_units", &json!({"value":32,"from":"F","to":"C"})).unwrap();
        let value: Value = serde_json::from_str(&result.output).unwrap();
        assert!(value["result"].as_f64().unwrap().abs() < 1e-10);
        assert!(execute("convert_units", &json!({"value":1,"from":"m","to":"kg"})).is_err());
        assert!(execute("convert_units", &json!({"value":-1,"from":"K","to":"C"})).is_err());
    }
    #[test]
    fn json_pointer_supports_escaped_keys_and_array_indices() {
        let result = execute(
            "json_query",
            &json!({"json":"{\"a/b\":[42]}","pointer":"/a~1b/0"}),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result.output).unwrap()["value"],
            42
        );
    }
    #[test]
    fn text_search_treats_regular_expression_syntax_as_literal() {
        let result = execute("text_search", &json!({"text":"abc\na.*b","query":"a.*b"})).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result.output).unwrap()["matches"][0]["line"],
            2
        );
    }
}
