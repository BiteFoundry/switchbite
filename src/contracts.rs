//! The embedded schema is the single source of truth for docking validation.
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value, json};
use std::{fmt, sync::LazyLock};

pub const MODEL: &str = "jev-1.14.0";
pub const MAX_JSON_BYTES: usize = 65_536;
pub const MAX_JUDGE_BYTES: usize = 131_072;
pub const SCHEMA_JSON: &str = include_str!("../schemas/docking-v1.json");
pub static SCHEMA: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(SCHEMA_JSON).expect("embedded schema"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContractError {
    InvalidRequest,
    InvalidJudgment,
    JudgeFailed,
    UnknownOperation,
}
impl ContractError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidJudgment => "invalid_judgment",
            Self::JudgeFailed => "judge_failed",
            Self::UnknownOperation => "unknown_operation",
        }
    }
}
impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "Invalid docking v1 request.",
            Self::InvalidJudgment => "Invalid typed judgment.",
            Self::JudgeFailed => "Judgment provider failed; no proposal was produced.",
            Self::UnknownOperation => "Unsupported plugin operation.",
        })
    }
}
impl std::error::Error for ContractError {}

// Value's usual deserializer silently overwrites duplicate object members.
// Reject these at byte boundaries before the domain or SDK sees the object.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("unique-member JSON")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueValue(v)) = seq.next_element()? {
                    values.push(v);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate member"));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        d.deserialize_any(JsonVisitor)
    }
}

pub fn parse_json(raw: &[u8], limit: usize) -> Result<Value, ContractError> {
    if raw.len() > limit {
        return Err(ContractError::InvalidRequest);
    }
    // serde_json can coerce integer tokens larger than u64 into f64. Preserve
    // docking's signed-64-bit integer rule before that information is lost.
    let mut index = 0;
    let mut quoted = false;
    let mut escaped = false;
    while index < raw.len() {
        let byte = raw[index];
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'-' || byte.is_ascii_digit() {
            let start = index;
            while index < raw.len()
                && matches!(raw[index], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
            {
                index += 1;
            }
            let token = &raw[start..index];
            if !token.iter().any(|b| matches!(b, b'.' | b'e' | b'E'))
                && std::str::from_utf8(token)
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .is_none()
            {
                return Err(ContractError::InvalidRequest);
            }
            continue;
        }
        index += 1;
    }
    serde_json::from_slice::<UniqueValue>(raw)
        .map(|v| v.0)
        .map_err(|_| ContractError::InvalidRequest)
}
pub fn parse_request(raw: &[u8]) -> Result<Value, ContractError> {
    let value = parse_json(raw, MAX_JSON_BYTES)?;
    bounded(&value, MAX_JSON_BYTES)?;
    Ok(value)
}

pub fn bounded(value: &Value, max_bytes: usize) -> Result<(), ContractError> {
    let mut stack = vec![(value, 0)];
    let mut count = 0;
    while let Some((v, depth)) = stack.pop() {
        count += 1;
        if depth > 12 || count > 8192 {
            return Err(ContractError::InvalidRequest);
        }
        match v {
            Value::Object(map) => {
                if map.len() > 128 {
                    return Err(ContractError::InvalidRequest);
                }
                for (key, child) in map {
                    count += 1;
                    if key.is_empty() || key.chars().count() > 256 || depth + 1 > 12 {
                        return Err(ContractError::InvalidRequest);
                    }
                    stack.push((child, depth + 1));
                }
            }
            Value::Array(a) => {
                if a.len() > 128 {
                    return Err(ContractError::InvalidRequest);
                }
                stack.extend(a.iter().map(|v| (v, depth + 1)));
            }
            Value::String(s) if s.chars().count() > 24000 => {
                return Err(ContractError::InvalidRequest);
            }
            Value::Number(n)
                if (n.is_u64() && !n.is_i64()) || n.as_f64().is_none_or(|n| !n.is_finite()) =>
            {
                return Err(ContractError::InvalidRequest);
            }
            _ => {}
        }
    }
    if count > 8192
        || serde_json::to_vec(value)
            .map_err(|_| ContractError::InvalidRequest)?
            .len()
            > max_bytes
    {
        return Err(ContractError::InvalidRequest);
    }
    Ok(())
}

/// Validator for the deliberately small schema vocabulary in docking-v1.json.
/// It is not a general JSON Schema engine (no remote resolution or I/O).
pub fn validate(value: &Value, schema: &Value) -> Result<(), ContractError> {
    let bad = || ContractError::InvalidRequest;
    if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
        return if branches.iter().any(|s| validate(value, s).is_ok()) {
            Ok(())
        } else {
            Err(bad())
        };
    }
    let kind = schema["type"].as_str().unwrap_or("");
    let correct = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "" => true,
        _ => false,
    };
    if !correct
        || schema.get("const").is_some_and(|v| v != value)
        || schema
            .get("enum")
            .is_some_and(|v| !v.as_array().is_some_and(|a| a.contains(value)))
    {
        return Err(bad());
    }
    match kind {
        "object" => {
            let map = value.as_object().ok_or_else(bad)?;
            if map.len() > schema["maxProperties"].as_u64().unwrap_or(128) as usize {
                return Err(bad());
            }
            if let Some(required) = schema["required"].as_array()
                && required
                    .iter()
                    .any(|k| !map.contains_key(k.as_str().unwrap_or("")))
            {
                return Err(bad());
            }
            for (key, child) in map {
                if let Some(rule) = schema["properties"].get(key) {
                    validate(child, rule)?;
                    continue;
                }
                let mut matched = false;
                if let Some(patterns) = schema["patternProperties"].as_object() {
                    for (pattern, rule) in patterns {
                        if regex::Regex::new(pattern).map_err(|_| bad())?.is_match(key) {
                            validate(child, rule)?;
                            matched = true;
                        }
                    }
                }
                if !matched {
                    match schema.get("additionalProperties") {
                        Some(Value::Bool(false)) => return Err(bad()),
                        Some(rule @ Value::Object(_)) => validate(child, rule)?,
                        _ => {}
                    }
                }
            }
        }
        "array" => {
            let values = value.as_array().ok_or_else(bad)?;
            if values.len() < schema["minItems"].as_u64().unwrap_or(0) as usize
                || values.len()
                    > schema["maxItems"].as_u64().unwrap_or(MAX_JSON_BYTES as u64) as usize
            {
                return Err(bad());
            }
            for (i, v) in values.iter().enumerate() {
                validate(v, &schema["items"])?;
                if schema["uniqueItems"] == true && values[..i].contains(v) {
                    return Err(bad());
                }
            }
        }
        "string" => {
            let s = value.as_str().ok_or_else(bad)?;
            let min = schema["minLength"].as_u64().unwrap_or(0) as usize;
            if s.chars().count() < min
                || s.chars().count()
                    > schema["maxLength"]
                        .as_u64()
                        .unwrap_or(MAX_JSON_BYTES as u64) as usize
                || (min > 0 && s.trim().is_empty())
            {
                return Err(bad());
            }
        }
        "number" | "integer" => {
            let n = value.as_f64().ok_or_else(bad)?;
            if !n.is_finite()
                || n < schema["minimum"].as_f64().unwrap_or(f64::NEG_INFINITY)
                || n > schema["maximum"].as_f64().unwrap_or(f64::INFINITY)
            {
                return Err(bad());
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn operation_schema(operation: &str, direction: &str) -> Result<&'static Value, ContractError> {
    if !["decide", "validate_plan", "feedback"].contains(&operation) {
        return Err(ContractError::UnknownOperation);
    }
    Ok(&SCHEMA["$defs"][format!("{operation}_{direction}")])
}
pub fn validate_request(operation: &str, request: &Value) -> Result<(), ContractError> {
    let schema = operation_schema(operation, "input")?;
    bounded(request, MAX_JSON_BYTES)?;
    validate(request, schema)?;
    if operation == "decide" {
        let mut ids = std::collections::BTreeSet::new();
        for c in request["candidates"]
            .as_array()
            .ok_or(ContractError::InvalidRequest)?
        {
            if !ids.insert(c["id"].as_str().ok_or(ContractError::InvalidRequest)?) {
                return Err(ContractError::InvalidRequest);
            }
        }
    }
    Ok(())
}
pub fn describe() -> Value {
    let mut operations = Map::new();
    for name in ["decide", "validate_plan", "feedback"] {
        operations.insert(name.into(), json!({"input_schema": operation_schema(name, "input").unwrap(),
            "output_schema": operation_schema(name, "output").unwrap(), "required_capabilities": []}));
    }
    json!({"version": 1, "plugin_id": "switchbite", "operations": operations})
}
