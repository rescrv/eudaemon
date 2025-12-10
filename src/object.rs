use crate::error::{SError, SResult};
use crate::eval::Env;
use crate::expr::SExpr;
use crate::json::unescape_string;

/// Extract string content from a quoted atom like `"hello"` -> `hello`
/// and unescape any escape sequences
fn atom_to_string(atom: &str) -> String {
    if atom.starts_with('"') && atom.ends_with('"') && atom.len() >= 2 {
        unescape_string(&atom[1..atom.len() - 1])
    } else {
        atom.to_string()
    }
}

/// Get a value from an object by key
pub fn get(value: &SExpr, key: &str) -> SExpr {
    match value {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0]
                && tag == "obj"
            {
                // Search for the key in the object
                for item in items.iter().skip(1) {
                    if let SExpr::List(pair) = item
                        && pair.len() == 2
                        && let SExpr::Atom(k) = &pair[0]
                        && atom_to_string(k) == key
                    {
                        return pair[1].clone();
                    }
                }
            }
            SExpr::Atom("null".to_string())
        }
        _ => SExpr::Atom("null".to_string()),
    }
}

/// Get keys from an object or array
pub fn keys(value: &SExpr) -> SExpr {
    match value {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0] {
                if tag == "obj" {
                    let keys: Vec<SExpr> = items
                        .iter()
                        .skip(1)
                        .filter_map(|item| {
                            if let SExpr::List(pair) = item
                                && pair.len() == 2
                            {
                                return Some(pair[0].clone());
                            }
                            None
                        })
                        .collect();
                    return SExpr::List(
                        std::iter::once(SExpr::Atom("arr".to_string()))
                            .chain(keys)
                            .collect(),
                    );
                } else if tag == "arr" {
                    let indices: Vec<SExpr> = (0..items.len() - 1)
                        .map(|i| SExpr::Atom(i.to_string()))
                        .collect();
                    return SExpr::List(
                        std::iter::once(SExpr::Atom("arr".to_string()))
                            .chain(indices)
                            .collect(),
                    );
                }
            }
            SExpr::Atom("null".to_string())
        }
        _ => SExpr::Atom("null".to_string()),
    }
}

/// Get values from an object or array
pub fn values(value: &SExpr) -> SExpr {
    match value {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0] {
                if tag == "obj" {
                    let vals: Vec<SExpr> = items
                        .iter()
                        .skip(1)
                        .filter_map(|item| {
                            if let SExpr::List(pair) = item
                                && pair.len() == 2
                            {
                                return Some(pair[1].clone());
                            }
                            None
                        })
                        .collect();
                    return SExpr::List(
                        std::iter::once(SExpr::Atom("arr".to_string()))
                            .chain(vals)
                            .collect(),
                    );
                } else if tag == "arr" {
                    return value.clone();
                }
            }
            SExpr::Atom("null".to_string())
        }
        _ => SExpr::Atom("null".to_string()),
    }
}

/// Associate a key-value pair in an object
pub fn assoc(value: &SExpr, key: &str, new_value: SExpr) -> SExpr {
    match value {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0]
                && tag == "obj"
            {
                let mut result = vec![SExpr::Atom("obj".to_string())];
                let mut found = false;

                for item in items.iter().skip(1) {
                    if let SExpr::List(pair) = item
                        && pair.len() == 2
                        && let SExpr::Atom(k) = &pair[0]
                        && atom_to_string(k) == key
                    {
                        result.push(SExpr::List(vec![
                            SExpr::Atom(format!("\"{}\"", key)),
                            new_value.clone(),
                        ]));
                        found = true;
                        continue;
                    }
                    result.push(item.clone());
                }

                if !found {
                    result.push(SExpr::List(vec![
                        SExpr::Atom(format!("\"{}\"", key)),
                        new_value,
                    ]));
                }

                return SExpr::List(result);
            }
            SExpr::Atom("null".to_string())
        }
        SExpr::Atom(s) if s == "null" => SExpr::List(vec![
            SExpr::Atom("obj".to_string()),
            SExpr::List(vec![SExpr::Atom(format!("\"{}\"", key)), new_value]),
        ]),
        _ => SExpr::Atom("null".to_string()),
    }
}

/// Dissociate a key from an object
pub fn dissoc(value: &SExpr, key: &str) -> SExpr {
    match value {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0]
                && tag == "obj"
            {
                let mut result = vec![SExpr::Atom("obj".to_string())];

                for item in items.iter().skip(1) {
                    if let SExpr::List(pair) = item
                        && pair.len() == 2
                        && let SExpr::Atom(k) = &pair[0]
                        && atom_to_string(k) == key
                    {
                        continue;
                    }
                    result.push(item.clone());
                }

                return SExpr::List(result);
            }
            SExpr::Atom("null".to_string())
        }
        _ => SExpr::Atom("null".to_string()),
    }
}

/// Merge multiple objects
pub fn merge(objects: &[SExpr]) -> SExpr {
    let mut result_map: Vec<(String, SExpr)> = Vec::new();

    for obj in objects {
        if let SExpr::List(items) = obj
            && !items.is_empty()
            && let SExpr::Atom(tag) = &items[0]
            && tag == "obj"
        {
            for item in items.iter().skip(1) {
                if let SExpr::List(pair) = item
                    && pair.len() == 2
                    && let SExpr::Atom(k) = &pair[0]
                {
                    let key = atom_to_string(k);
                    // Remove existing entry with this key
                    result_map.retain(|(existing_key, _)| existing_key != &key);
                    result_map.push((key, pair[1].clone()));
                }
            }
        }
    }

    let mut result = vec![SExpr::Atom("obj".to_string())];
    for (key, value) in result_map {
        result.push(SExpr::List(vec![
            SExpr::Atom(format!("\"{}\"", key)),
            value,
        ]));
    }
    SExpr::List(result)
}

/// Constructs an object from key-value pairs.
pub fn builtin_obj(args: &[SExpr]) -> SResult<SExpr> {
    Ok(SExpr::List(
        std::iter::once(SExpr::Atom("obj".to_string()))
            .chain(args.iter().cloned())
            .collect(),
    ))
}

/// Constructs an array from values.
pub fn builtin_arr(args: &[SExpr]) -> SResult<SExpr> {
    Ok(SExpr::List(
        std::iter::once(SExpr::Atom("arr".to_string()))
            .chain(args.iter().cloned())
            .collect(),
    ))
}

/// Gets a value from an object by key.
pub fn builtin_get(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("get")
            .with_code("wrong-argument-count")
            .with_message("get requires exactly two arguments")
            .with_atom_field("expected", 2)
            .with_atom_field("received", args.len()));
    }
    let key = match &args[1] {
        SExpr::Atom(s) => atom_to_string(s),
        _ => {
            return Err(SError::new("get")
                .with_code("invalid-key-type")
                .with_message("Key must be a string atom")
                .with_field("key", args[1].clone()));
        }
    };
    Ok(get(&args[0], &key))
}

/// Gets all keys from an object or array.
pub fn builtin_keys(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("keys")
            .with_code("wrong-argument-count")
            .with_message("keys requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    Ok(keys(&args[0]))
}

/// Gets all values from an object or array.
pub fn builtin_values(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 1 {
        return Err(SError::new("values")
            .with_code("wrong-argument-count")
            .with_message("values requires exactly one argument")
            .with_atom_field("expected", 1)
            .with_atom_field("received", args.len()));
    }
    Ok(values(&args[0]))
}

/// Associates a key-value pair in an object.
pub fn builtin_assoc(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 3 {
        return Err(SError::new("assoc")
            .with_code("wrong-argument-count")
            .with_message("assoc requires exactly three arguments")
            .with_atom_field("expected", 3)
            .with_atom_field("received", args.len()));
    }
    let key = match &args[1] {
        SExpr::Atom(s) => atom_to_string(s),
        _ => {
            return Err(SError::new("assoc")
                .with_code("invalid-key-type")
                .with_message("Key must be a string atom")
                .with_field("key", args[1].clone()));
        }
    };
    Ok(assoc(&args[0], &key, args[2].clone()))
}

/// Dissociates a key from an object.
pub fn builtin_dissoc(args: &[SExpr]) -> SResult<SExpr> {
    if args.len() != 2 {
        return Err(SError::new("dissoc")
            .with_code("wrong-argument-count")
            .with_message("dissoc requires exactly two arguments")
            .with_atom_field("expected", 2)
            .with_atom_field("received", args.len()));
    }
    let key = match &args[1] {
        SExpr::Atom(s) => atom_to_string(s),
        _ => {
            return Err(SError::new("dissoc")
                .with_code("invalid-key-type")
                .with_message("Key must be a string atom")
                .with_field("key", args[1].clone()));
        }
    };
    Ok(dissoc(&args[0], &key))
}

/// Merges multiple objects into one.
pub fn builtin_merge(args: &[SExpr]) -> SResult<SExpr> {
    Ok(merge(args))
}

/// Registers JSON/object manipulation functions in the environment.
///
/// Registers these functions:
/// - `obj`: Constructor for objects `(obj ("key" value) ...)`
/// - `arr`: Constructor for arrays `(arr value ...)`
/// - `get`: Get value by key `(get obj "key")`
/// - `keys`: Get keys from object/array `(keys obj)`
/// - `values`: Get values from object/array `(values obj)`
/// - `assoc`: Associate key-value in object `(assoc obj "key" value)`
/// - `dissoc`: Remove key from object `(dissoc obj "key")`
/// - `merge`: Merge multiple objects `(merge obj1 obj2 ...)`
pub fn register_json_builtins(env: &mut Env) {
    env.def_fn("obj", builtin_obj);
    env.def_fn("arr", builtin_arr);
    env.def_fn("get", builtin_get);
    env.def_fn("keys", builtin_keys);
    env.def_fn("values", builtin_values);
    env.def_fn("assoc", builtin_assoc);
    env.def_fn("dissoc", builtin_dissoc);
    env.def_fn("merge", builtin_merge);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Parser;

    fn parse(s: &str) -> SExpr {
        let mut parser = Parser::new(s);
        parser.parse().unwrap()
    }

    #[test]
    fn get_returns_value_for_key() {
        let value = parse(r#"(obj ("name" "Alice") ("age" 30))"#);
        let result = get(&value, "name");
        assert_eq!(result.to_string(), r#""Alice""#);
    }

    #[test]
    fn get_returns_null_for_missing_key() {
        let value = parse(r#"(obj ("name" "Alice"))"#);
        let result = get(&value, "missing");
        assert_eq!(result.to_string(), "null");
    }

    #[test]
    fn keys_returns_object_keys() {
        let value = parse(r#"(obj ("a" 1) ("b" 2))"#);
        let result = keys(&value);
        assert_eq!(result.to_string(), r#"(arr "a" "b")"#);
    }

    #[test]
    fn values_returns_object_values() {
        let value = parse(r#"(obj ("a" 1) ("b" 2))"#);
        let result = values(&value);
        assert_eq!(result.to_string(), "(arr 1 2)");
    }

    #[test]
    fn assoc_adds_new_key() {
        let value = parse(r#"(obj ("a" 1))"#);
        let result = assoc(&value, "b", SExpr::Atom("2".to_string()));
        assert_eq!(result.to_string(), r#"(obj ("a" 1) ("b" 2))"#);
    }

    #[test]
    fn assoc_updates_existing_key() {
        let value = parse(r#"(obj ("a" 1) ("b" 2))"#);
        let result = assoc(&value, "a", SExpr::Atom("10".to_string()));
        assert_eq!(result.to_string(), r#"(obj ("a" 10) ("b" 2))"#);
    }

    #[test]
    fn dissoc_removes_key() {
        let value = parse(r#"(obj ("a" 1) ("b" 2))"#);
        let result = dissoc(&value, "a");
        assert_eq!(result.to_string(), r#"(obj ("b" 2))"#);
    }

    #[test]
    fn merge_combines_objects() {
        let obj1 = parse(r#"(obj ("a" 1) ("b" 2))"#);
        let obj2 = parse(r#"(obj ("b" 3) ("c" 4))"#);
        let result = merge(&[obj1, obj2]);
        assert_eq!(result.to_string(), r#"(obj ("a" 1) ("b" 3) ("c" 4))"#);
    }

    #[test]
    fn get_returns_null_for_non_object() {
        let value = parse("not-an-object");
        assert_eq!(get(&value, "key"), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn get_returns_null_for_empty_list() {
        let value = parse("()");
        assert_eq!(get(&value, "key"), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn get_returns_null_for_array() {
        let value = parse("(arr 1 2 3)");
        assert_eq!(get(&value, "key"), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn get_with_nested_value() {
        let value = parse(r#"(obj ("nested" (obj ("inner" 42))))"#);
        let nested = get(&value, "nested");
        let inner = get(&nested, "inner");
        assert_eq!(inner, SExpr::Atom("42".to_string()));
    }

    #[test]
    fn keys_returns_null_for_atom() {
        let value = parse("atom");
        assert_eq!(keys(&value), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn keys_returns_null_for_empty_list() {
        let value = parse("()");
        assert_eq!(keys(&value), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn keys_returns_indices_for_array() {
        let value = parse("(arr a b c)");
        assert_eq!(keys(&value).to_string(), "(arr 0 1 2)");
    }

    #[test]
    fn keys_returns_empty_for_empty_object() {
        let value = parse("(obj)");
        assert_eq!(keys(&value).to_string(), "(arr)");
    }

    #[test]
    fn values_returns_null_for_atom() {
        let value = parse("atom");
        assert_eq!(values(&value), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn values_returns_null_for_empty_list() {
        let value = parse("()");
        assert_eq!(values(&value), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn values_returns_array_unchanged() {
        let value = parse("(arr 1 2 3)");
        assert_eq!(values(&value).to_string(), "(arr 1 2 3)");
    }

    #[test]
    fn values_returns_empty_for_empty_object() {
        let value = parse("(obj)");
        assert_eq!(values(&value).to_string(), "(arr)");
    }

    #[test]
    fn assoc_returns_null_for_non_object() {
        let value = parse("not-an-object");
        assert_eq!(
            assoc(&value, "key", SExpr::Atom("1".to_string())),
            SExpr::Atom("null".to_string())
        );
    }

    #[test]
    fn assoc_returns_null_for_array() {
        let value = parse("(arr 1 2 3)");
        assert_eq!(
            assoc(&value, "key", SExpr::Atom("1".to_string())),
            SExpr::Atom("null".to_string())
        );
    }

    #[test]
    fn assoc_creates_object_from_null() {
        let value = parse("null");
        let result = assoc(&value, "key", SExpr::Atom("42".to_string()));
        assert_eq!(result.to_string(), r#"(obj ("key" 42))"#);
    }

    #[test]
    fn assoc_to_empty_object() {
        let value = parse("(obj)");
        let result = assoc(&value, "key", SExpr::Atom("42".to_string()));
        assert_eq!(result.to_string(), r#"(obj ("key" 42))"#);
    }

    #[test]
    fn dissoc_returns_null_for_non_object() {
        let value = parse("not-an-object");
        assert_eq!(dissoc(&value, "key"), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn dissoc_returns_null_for_array() {
        let value = parse("(arr 1 2 3)");
        assert_eq!(dissoc(&value, "key"), SExpr::Atom("null".to_string()));
    }

    #[test]
    fn dissoc_missing_key_unchanged() {
        let value = parse(r#"(obj ("a" 1))"#);
        let result = dissoc(&value, "missing");
        assert_eq!(result.to_string(), r#"(obj ("a" 1))"#);
    }

    #[test]
    fn dissoc_last_key_returns_empty_object() {
        let value = parse(r#"(obj ("only" 1))"#);
        let result = dissoc(&value, "only");
        assert_eq!(result.to_string(), "(obj)");
    }

    #[test]
    fn merge_empty_returns_empty_object() {
        let result = merge(&[]);
        assert_eq!(result.to_string(), "(obj)");
    }

    #[test]
    fn merge_single_object() {
        let obj = parse(r#"(obj ("a" 1))"#);
        let result = merge(&[obj]);
        assert_eq!(result.to_string(), r#"(obj ("a" 1))"#);
    }

    #[test]
    fn merge_skips_non_objects() {
        let obj = parse(r#"(obj ("a" 1))"#);
        let non_obj = parse("not-an-object");
        let result = merge(&[obj, non_obj]);
        assert_eq!(result.to_string(), r#"(obj ("a" 1))"#);
    }

    #[test]
    fn merge_three_objects() {
        let obj1 = parse(r#"(obj ("a" 1))"#);
        let obj2 = parse(r#"(obj ("b" 2))"#);
        let obj3 = parse(r#"(obj ("c" 3))"#);
        let result = merge(&[obj1, obj2, obj3]);
        assert_eq!(result.to_string(), r#"(obj ("a" 1) ("b" 2) ("c" 3))"#);
    }

    #[test]
    fn merge_later_values_override() {
        let obj1 = parse(r#"(obj ("x" 1))"#);
        let obj2 = parse(r#"(obj ("x" 2))"#);
        let obj3 = parse(r#"(obj ("x" 3))"#);
        let result = merge(&[obj1, obj2, obj3]);
        assert_eq!(result.to_string(), r#"(obj ("x" 3))"#);
    }

    #[test]
    fn keys_preserves_order() {
        let value = parse(r#"(obj ("z" 1) ("a" 2) ("m" 3))"#);
        assert_eq!(keys(&value).to_string(), r#"(arr "z" "a" "m")"#);
    }

    #[test]
    fn values_preserves_order() {
        let value = parse(r#"(obj ("z" 1) ("a" 2) ("m" 3))"#);
        assert_eq!(values(&value).to_string(), "(arr 1 2 3)");
    }
}
