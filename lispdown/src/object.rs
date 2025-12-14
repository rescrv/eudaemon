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

/// Get a value from an object by key.
///
/// Works with any tagged list containing key-value pairs, e.g.:
/// - `(obj ("key" value) ...)`
/// - `(section ("key" value) ...)`
///
/// The tag must be an unquoted atom (not a string).
pub fn get(value: &SExpr, key: &str) -> SExpr {
    match value {
        SExpr::List(items) if !items.is_empty() => {
            if let SExpr::Atom(tag) = &items[0] {
                // Skip if tag looks like a quoted string
                if tag.starts_with('"') {
                    return SExpr::Atom("null".to_string());
                }
                // Search for the key in the key-value pairs
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
    fn get_works_with_section_tag() {
        let value = parse(r#"(section ("level" 1) ("title" "Hello") ("slug" "hello"))"#);
        let result = get(&value, "slug");
        assert_eq!(result.to_string(), r#""hello""#);
        println!("DEBUG get_works_with_section_tag: {}", result);
    }

    #[test]
    fn get_works_with_any_tag() {
        let value = parse(r#"(custom-tag ("foo" "bar") ("baz" 42))"#);
        assert_eq!(get(&value, "foo").to_string(), r#""bar""#);
        assert_eq!(get(&value, "baz").to_string(), "42");
        println!("DEBUG get_works_with_any_tag passed");
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
