//! JSON to S-expression conversion and vice versa

use crate::s::expr::{Parser, SExpr};
use serde_json::Value;

/// Converts JSON string to S-expression string representation.
/// Maps JSON types to explicitly tagged S-expressions:
/// - Object: (obj ("key" value) ...)
/// - Array: (arr value value ...)
/// - String: "string"
/// - Number: 123 or 12.34
/// - Boolean: #t or #f
/// - Null: null
pub fn json_to_sexpr(s: &str) -> Result<String, String> {
    let json_value: Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    let sexpr = json_value_to_sexpr(&json_value);
    Ok(sexpr.to_string())
}

fn json_value_to_sexpr(value: &Value) -> SExpr {
    match value {
        Value::Null => SExpr::Atom("null".to_string()),
        Value::Bool(b) => SExpr::Atom(if *b { "#t" } else { "#f" }.to_string()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SExpr::Atom(i.to_string())
            } else if let Some(f) = n.as_f64() {
                SExpr::Atom(f.to_string())
            } else {
                SExpr::Atom(n.to_string())
            }
        }
        Value::String(s) => SExpr::Atom(format!("\"{}\"", escape_string(s))),
        Value::Array(arr) => {
            let mut elements = vec![SExpr::Atom("arr".to_string())];
            for item in arr {
                elements.push(json_value_to_sexpr(item));
            }
            SExpr::List(elements)
        }
        Value::Object(obj) => {
            let mut elements = vec![SExpr::Atom("obj".to_string())];
            for (key, val) in obj {
                let pair = SExpr::List(vec![
                    SExpr::Atom(format!("\"{}\"", escape_string(key))),
                    json_value_to_sexpr(val),
                ]);
                elements.push(pair);
            }
            SExpr::List(elements)
        }
    }
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Converts S-expression string to JSON string.
/// Expects explicitly tagged S-expressions as produced by json_to_sexpr.
/// Applies implicit null filtering: nulls in arrays are removed.
pub fn sexpr_to_json(s: &str) -> Result<String, String> {
    let mut parser = Parser::new(s);
    let sexpr = parser.parse()?;
    let json_value = sexpr_to_json_value(&sexpr)?;
    serde_json::to_string(&json_value).map_err(|e| e.to_string())
}

fn sexpr_to_json_value(sexpr: &SExpr) -> Result<Value, String> {
    match sexpr {
        SExpr::Atom(s) => {
            if s == "null" {
                Ok(Value::Null)
            } else if s == "#t" {
                Ok(Value::Bool(true))
            } else if s == "#f" {
                Ok(Value::Bool(false))
            } else if s.starts_with('"') && s.ends_with('"') {
                let unquoted = &s[1..s.len() - 1];
                Ok(Value::String(unescape_string(unquoted)))
            } else if let Ok(i) = s.parse::<i64>() {
                Ok(serde_json::json!(i))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(serde_json::json!(f))
            } else {
                Err(format!("Cannot convert atom '{}' to JSON", s))
            }
        }
        SExpr::List(list) => {
            if list.is_empty() {
                return Err("Empty list cannot be converted to JSON".to_string());
            }

            let tag = match &list[0] {
                SExpr::Atom(s) => s.as_str(),
                _ => return Err("First element of list must be a tag atom".to_string()),
            };

            match tag {
                "obj" => {
                    let mut map = serde_json::Map::new();
                    for item in list.iter().skip(1) {
                        match item {
                            SExpr::List(pair) if pair.len() == 2 => {
                                let key = match &pair[0] {
                                    SExpr::Atom(k) if k.starts_with('"') && k.ends_with('"') => {
                                        unescape_string(&k[1..k.len() - 1])
                                    }
                                    _ => {
                                        return Err(
                                            "Object key must be a quoted string".to_string()
                                        );
                                    }
                                };
                                let value = sexpr_to_json_value(&pair[1])?;
                                map.insert(key, value);
                            }
                            _ => return Err("Object entries must be (key value) pairs".to_string()),
                        }
                    }
                    Ok(Value::Object(map))
                }
                "arr" => {
                    let mut arr = Vec::new();
                    for item in list.iter().skip(1) {
                        let value = sexpr_to_json_value(item)?;
                        // Implicit null filtering: skip nulls in arrays
                        if !value.is_null() {
                            arr.push(value);
                        }
                    }
                    Ok(Value::Array(arr))
                }
                _ => Err(format!("Unknown tag '{}'", tag)),
            }
        }
    }
}

fn unescape_string(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    '\\' => result.push('\\'),
                    '"' => result.push('"'),
                    _ => {
                        result.push('\\');
                        result.push(next);
                    }
                }
            } else {
                result.push('\\');
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_json_null() {
        let json = "null";
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, "null");
    }

    #[test]
    fn converts_json_bool_true() {
        let json = "true";
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, "#t");
    }

    #[test]
    fn converts_json_bool_false() {
        let json = "false";
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, "#f");
    }

    #[test]
    fn converts_json_integer() {
        let json = "42";
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, "42");
    }

    #[test]
    fn converts_json_float() {
        let json = "3.14159";
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, "3.14159");
    }

    #[test]
    fn converts_json_string() {
        let json = r#""hello""#;
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, r#""hello""#);
    }

    #[test]
    fn converts_json_string_with_escapes() {
        let json = r#""hello\nworld""#;
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, r#""hello\nworld""#);
    }

    #[test]
    fn converts_json_array() {
        let json = r#"[1, 2, 3]"#;
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, "(arr 1 2 3)");
    }

    #[test]
    fn converts_json_empty_array() {
        let json = r#"[]"#;
        let sexpr = json_to_sexpr(json).unwrap();
        assert_eq!(sexpr, "(arr)");
    }

    #[test]
    fn converts_json_object() {
        let json = r#"{"id": 1, "name": "Bob"}"#;
        let sexpr = json_to_sexpr(json).unwrap();
        // JSON objects may not preserve order, but we can parse it back
        assert!(sexpr.contains("(obj"));
        assert!(sexpr.contains(r#"("id" 1)"#));
        assert!(sexpr.contains(r#"("name" "Bob")"#));
    }

    #[test]
    fn converts_json_nested_structure() {
        let json = r#"{"id": 1, "tags": ["a", "b"]}"#;
        let sexpr = json_to_sexpr(json).unwrap();
        assert!(sexpr.contains("(obj"));
        assert!(sexpr.contains(r#"("id" 1)"#));
        assert!(sexpr.contains(r#"("tags" (arr "a" "b"))"#));
    }

    #[test]
    fn round_trip_null() {
        let json = "null";
        let sexpr = json_to_sexpr(json).unwrap();
        let json_out = sexpr_to_json(&sexpr).unwrap();
        assert_eq!(json_out, json);
    }

    #[test]
    fn round_trip_bool() {
        let json = "true";
        let sexpr = json_to_sexpr(json).unwrap();
        let json_out = sexpr_to_json(&sexpr).unwrap();
        assert_eq!(json_out, json);
    }

    #[test]
    fn round_trip_number() {
        let json = "42";
        let sexpr = json_to_sexpr(json).unwrap();
        let json_out = sexpr_to_json(&sexpr).unwrap();
        assert_eq!(json_out, json);
    }

    #[test]
    fn round_trip_string() {
        let json = r#""hello""#;
        let sexpr = json_to_sexpr(json).unwrap();
        let json_out = sexpr_to_json(&sexpr).unwrap();
        assert_eq!(json_out, json);
    }

    #[test]
    fn round_trip_array() {
        let json = r#"[1,2,3]"#;
        let sexpr = json_to_sexpr(json).unwrap();
        let json_out = sexpr_to_json(&sexpr).unwrap();
        assert_eq!(json_out, json);
    }

    #[test]
    fn round_trip_object() {
        let json = r#"{"id":1}"#;
        let sexpr = json_to_sexpr(json).unwrap();
        let json_out = sexpr_to_json(&sexpr).unwrap();
        // Parse both to compare values since order might differ
        let original: Value = serde_json::from_str(json).unwrap();
        let result: Value = serde_json::from_str(&json_out).unwrap();
        assert_eq!(original, result);
    }

    #[test]
    fn round_trip_complex() {
        let json = r#"{"id":1,"tags":["a","b"],"active":true}"#;
        let sexpr = json_to_sexpr(json).unwrap();
        let json_out = sexpr_to_json(&sexpr).unwrap();
        let original: Value = serde_json::from_str(json).unwrap();
        let result: Value = serde_json::from_str(&json_out).unwrap();
        assert_eq!(original, result);
    }

    #[test]
    fn implicit_null_filtering() {
        let sexpr = "(arr 1 null 3)";
        let json = sexpr_to_json(sexpr).unwrap();
        assert_eq!(json, "[1,3]");
    }

    #[test]
    fn null_filtering_preserves_order() {
        let sexpr = "(arr null 1 null 2 null 3 null)";
        let json = sexpr_to_json(sexpr).unwrap();
        assert_eq!(json, "[1,2,3]");
    }

    #[test]
    fn objects_preserve_null_values() {
        let sexpr = r#"(obj ("id" 1) ("name" null))"#;
        let json = sexpr_to_json(sexpr).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["id"], 1);
        assert!(parsed["name"].is_null());
    }

    #[test]
    fn spec_example() {
        let json = r#"{"id": 1, "tags": ["a", "b"]}"#;
        let sexpr = json_to_sexpr(json).unwrap();

        // Verify the S-expression contains the expected structure
        assert!(sexpr.contains("(obj"));
        assert!(sexpr.contains(r#"("id" 1)"#));
        assert!(sexpr.contains(r#"("tags" (arr "a" "b"))"#));

        // Verify round-trip
        let json_out = sexpr_to_json(&sexpr).unwrap();
        let original: Value = serde_json::from_str(json).unwrap();
        let result: Value = serde_json::from_str(&json_out).unwrap();
        assert_eq!(original, result);
    }
}
