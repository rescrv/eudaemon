use crate::s::expr::SExpr;
use crate::s::json::unescape_string;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::s::expr::Parser;

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
}
