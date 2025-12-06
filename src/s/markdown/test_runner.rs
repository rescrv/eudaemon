//! Declarative test runner for markdown transformations.
//!
//! Loads test cases from TOML files and executes them against the markdown
//! transformation infrastructure.

use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::s::expr::{Parser, SExpr};
use crate::s::markdown::mutations::{
    append_child, graft, hoist, insert_after, insert_before, prepend_child, prune, replace_at,
};
use crate::s::markdown::{markdown_to_sexpr, sexpr_to_markdown};
use crate::s::nodeid::PathId;

/// Result of running a single test case.
#[derive(Debug)]
pub struct TestResult {
    /// Name of the test (filename::test_name).
    pub name: String,
    /// Whether the test passed.
    pub passed: bool,
    /// Whether the test was skipped.
    pub skipped: bool,
    /// Error message if the test failed.
    pub error: Option<String>,
}

impl TestResult {
    fn pass(name: String) -> Self {
        TestResult {
            name,
            passed: true,
            skipped: false,
            error: None,
        }
    }

    fn skip(name: String, reason: Option<String>) -> Self {
        TestResult {
            name,
            passed: true,
            skipped: true,
            error: reason,
        }
    }

    fn fail(name: String, error: String) -> Self {
        TestResult {
            name,
            passed: false,
            skipped: false,
            error: Some(error),
        }
    }
}

/// Metadata for a test suite.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    /// Version of the test format.
    pub version: String,
    /// Description of the test suite.
    #[serde(default)]
    pub description: Option<String>,
}

/// A single test case.
#[derive(Debug, Deserialize)]
pub struct TestCase {
    /// Unique test name.
    pub name: String,
    /// Description of what the test validates.
    #[serde(default)]
    pub description: Option<String>,

    // --- Inputs ---
    /// Input markdown string.
    #[serde(default)]
    pub before: Option<String>,
    /// Input s-expression string.
    #[serde(default)]
    pub before_sexpr: Option<String>,

    // --- Expected Outputs ---
    /// Expected s-expression after parsing `before`.
    #[serde(default)]
    pub sexpr: Option<String>,
    /// Expected markdown output.
    #[serde(default)]
    pub after: Option<String>,
    /// Expected s-expression after mutation.
    #[serde(default)]
    pub after_sexpr: Option<String>,

    // --- Comparison Mode ---
    /// How to compare outputs: "exact" (default) or "semantic".
    #[serde(default = "default_compare")]
    pub compare: String,

    // --- Mutations ---
    /// Single mutation to apply.
    #[serde(default)]
    pub mutation: Option<Mutation>,
    /// Multiple mutations to apply in sequence.
    #[serde(default)]
    pub mutations: Option<Vec<Mutation>>,

    // --- Error Expectation ---
    /// Expected error (mutually exclusive with success outputs).
    #[serde(default)]
    pub expected_error: Option<ExpectedError>,

    // --- Control ---
    /// Skip this test.
    #[serde(default)]
    pub skip: bool,
    /// Reason for skipping.
    #[serde(default)]
    pub skip_reason: Option<String>,
}

fn default_compare() -> String {
    "exact".to_string()
}

/// A mutation operation to apply.
#[derive(Debug, Deserialize, Clone)]
pub struct Mutation {
    /// Operation name: replace_at, prune, insert_before, insert_after,
    /// append_child, prepend_child, hoist, graft.
    pub op: String,
    /// Path for the operation.
    #[serde(default)]
    pub path: Option<String>,
    /// Node s-expression for operations that insert/replace.
    #[serde(default)]
    pub node: Option<String>,
    /// Parent path for append_child/prepend_child.
    #[serde(default)]
    pub parent: Option<String>,
    /// Delta for hoist operation.
    #[serde(default)]
    pub delta: Option<i32>,
    /// Source path for graft.
    #[serde(default)]
    pub source: Option<String>,
    /// Target parent path for graft.
    #[serde(default)]
    pub target_parent: Option<String>,
    /// Target index for graft.
    #[serde(default)]
    pub target_index: Option<usize>,
}

/// Expected error specification.
#[derive(Debug, Deserialize)]
pub struct ExpectedError {
    /// Error code to match.
    #[serde(default)]
    pub code: Option<String>,
    /// Error phase to match.
    #[serde(default)]
    pub phase: Option<String>,
    /// Substring that must appear in the error message.
    #[serde(default)]
    pub message_contains: Option<String>,
}

/// A complete test suite loaded from a TOML file.
#[derive(Debug, Deserialize)]
pub struct TestSuite {
    /// Suite metadata.
    pub metadata: Metadata,
    /// Test cases.
    #[serde(rename = "test")]
    pub tests: Vec<TestCase>,
}

/// Loads a test suite from a TOML file.
fn load_test_suite(path: &Path) -> Result<TestSuite, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    toml::from_str(&content).map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
}

/// Runs all tests in a single TOML file.
pub fn run_test_file(path: &Path) -> Vec<TestResult> {
    let file_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let suite = match load_test_suite(path) {
        Ok(s) => s,
        Err(e) => {
            return vec![TestResult::fail(
                format!("{}::_load_", file_name),
                e.to_string(),
            )];
        }
    };

    suite
        .tests
        .into_iter()
        .map(|test| {
            let test_name = format!("{}::{}", file_name, test.name);
            run_single_test(&test_name, &test)
        })
        .collect()
}

/// Discovers and runs all *.toml files in a directory.
pub fn run_test_directory(dir: &Path) -> Vec<TestResult> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            return vec![TestResult::fail(
                format!("{}::_discover_", dir.display()),
                format!("Failed to read directory: {}", e),
            )];
        }
    };

    let mut results = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            results.extend(run_test_file(&path));
        }
    }

    results
}

/// Runs a single test case.
fn run_single_test(test_name: &str, test: &TestCase) -> TestResult {
    if test.skip {
        return TestResult::skip(test_name.to_string(), test.skip_reason.clone());
    }

    let result = execute_test(test);

    match result {
        Ok(()) => TestResult::pass(test_name.to_string()),
        Err(e) => TestResult::fail(test_name.to_string(), e),
    }
}

/// Executes a test case and returns Ok(()) on success or Err(message) on failure.
fn execute_test(test: &TestCase) -> Result<(), String> {
    // Determine test type and execute accordingly
    if test.expected_error.is_some() {
        execute_error_test(test)
    } else if test.before.is_some() && test.mutation.is_none() && test.mutations.is_none() {
        execute_roundtrip_test(test)
    } else if test.before_sexpr.is_some() && (test.mutation.is_some() || test.mutations.is_some()) {
        execute_mutation_test(test)
    } else if test.before.is_some() && (test.mutation.is_some() || test.mutations.is_some()) {
        execute_pipeline_test(test)
    } else {
        Err("Invalid test configuration: cannot determine test type".to_string())
    }
}

/// Executes a roundtrip test: markdown -> sexpr -> markdown.
fn execute_roundtrip_test(test: &TestCase) -> Result<(), String> {
    let before = test.before.as_ref().ok_or("Missing 'before' field")?;

    // Parse markdown to sexpr
    let parsed_sexpr = markdown_to_sexpr(before).map_err(|e| format!("Parse failed: {}", e))?;

    // Check sexpr if specified
    if let Some(expected_sexpr) = &test.sexpr {
        let expected = parse_sexpr(expected_sexpr)?;
        compare_sexpr(&parsed_sexpr, &expected, &test.compare, "sexpr")?;
    }

    // Render back to markdown
    let rendered = sexpr_to_markdown(&parsed_sexpr).map_err(|e| format!("Render failed: {}", e))?;

    // Check after (or compare to before if not specified)
    let expected_after = test.after.as_ref().unwrap_or(before);
    compare_markdown(&rendered, expected_after, &test.compare)?;

    Ok(())
}

/// Executes a mutation test: sexpr -> mutate -> sexpr.
fn execute_mutation_test(test: &TestCase) -> Result<(), String> {
    let before_sexpr_str = test
        .before_sexpr
        .as_ref()
        .ok_or("Missing 'before_sexpr' field")?;
    let mut doc = parse_sexpr(before_sexpr_str)?;

    // Apply mutations
    doc = apply_mutations(&doc, test)?;

    // Check after_sexpr if specified
    if let Some(expected_sexpr) = &test.after_sexpr {
        let expected = parse_sexpr(expected_sexpr)?;
        compare_sexpr(&doc, &expected, &test.compare, "after_sexpr")?;
    }

    // Check after (rendered markdown) if specified
    if let Some(expected_after) = &test.after {
        let rendered = sexpr_to_markdown(&doc).map_err(|e| format!("Render failed: {}", e))?;
        compare_markdown(&rendered, expected_after, &test.compare)?;
    }

    Ok(())
}

/// Executes a pipeline test: markdown -> sexpr -> mutate -> markdown.
fn execute_pipeline_test(test: &TestCase) -> Result<(), String> {
    let before = test.before.as_ref().ok_or("Missing 'before' field")?;

    // Parse markdown to sexpr
    let mut doc = markdown_to_sexpr(before).map_err(|e| format!("Parse failed: {}", e))?;

    // Check intermediate sexpr if specified
    if let Some(expected_sexpr) = &test.sexpr {
        let expected = parse_sexpr(expected_sexpr)?;
        compare_sexpr(&doc, &expected, &test.compare, "sexpr")?;
    }

    // Apply mutations
    doc = apply_mutations(&doc, test)?;

    // Check after_sexpr if specified
    if let Some(expected_sexpr) = &test.after_sexpr {
        let expected = parse_sexpr(expected_sexpr)?;
        compare_sexpr(&doc, &expected, &test.compare, "after_sexpr")?;
    }

    // Check after (rendered markdown)
    if let Some(expected_after) = &test.after {
        let rendered = sexpr_to_markdown(&doc).map_err(|e| format!("Render failed: {}", e))?;
        compare_markdown(&rendered, expected_after, &test.compare)?;
    }

    Ok(())
}

/// Executes an error test: expects an operation to fail with a specific error.
fn execute_error_test(test: &TestCase) -> Result<(), String> {
    let expected_error = test
        .expected_error
        .as_ref()
        .ok_or("Missing 'expected_error' field")?;

    // Try to execute the test and expect it to fail
    let result = if test.before_sexpr.is_some() {
        execute_mutation_expecting_error(test)
    } else if test.before.is_some() {
        execute_parse_expecting_error(test)
    } else {
        return Err("Error test requires 'before' or 'before_sexpr'".to_string());
    };

    match result {
        Ok(()) => Err("Expected an error but operation succeeded".to_string()),
        Err(error_str) => {
            // Check if error matches expectations
            // Error format: (error (phase xyz) (code abc) ...)
            // Values may or may not be quoted depending on the error type
            if let Some(code) = &expected_error.code
                && !error_str.contains(&format!("(code {})", code))
                && !error_str.contains(&format!("(code \"{}\")", code))
            {
                return Err(format!(
                    "Expected error code '{}' but got: {}",
                    code, error_str
                ));
            }
            if let Some(phase) = &expected_error.phase
                && !error_str.contains(&format!("(phase {})", phase))
                && !error_str.contains(&format!("(phase \"{}\")", phase))
            {
                return Err(format!(
                    "Expected error phase '{}' but got: {}",
                    phase, error_str
                ));
            }
            if let Some(msg) = &expected_error.message_contains
                && !error_str.contains(msg)
            {
                return Err(format!(
                    "Expected error to contain '{}' but got: {}",
                    msg, error_str
                ));
            }
            Ok(())
        }
    }
}

/// Executes a mutation expecting it to fail.
fn execute_mutation_expecting_error(test: &TestCase) -> Result<(), String> {
    let before_sexpr_str = test
        .before_sexpr
        .as_ref()
        .ok_or("Missing 'before_sexpr' field")?;
    let doc = parse_sexpr(before_sexpr_str)?;

    // Apply mutations - we expect this to fail
    apply_mutations(&doc, test)?;

    Ok(())
}

/// Executes a parse expecting it to fail.
fn execute_parse_expecting_error(test: &TestCase) -> Result<(), String> {
    let before = test.before.as_ref().ok_or("Missing 'before' field")?;
    markdown_to_sexpr(before).map_err(|e| e.to_string())?;
    Ok(())
}

/// Applies mutations from a test case to a document.
fn apply_mutations(doc: &SExpr, test: &TestCase) -> Result<SExpr, String> {
    let mut current = doc.clone();

    if let Some(mutation) = &test.mutation {
        current = apply_single_mutation(&current, mutation)?;
    }

    if let Some(mutations) = &test.mutations {
        for mutation in mutations {
            current = apply_single_mutation(&current, mutation)?;
        }
    }

    Ok(current)
}

/// Applies a single mutation to a document.
fn apply_single_mutation(doc: &SExpr, mutation: &Mutation) -> Result<SExpr, String> {
    match mutation.op.as_str() {
        "replace_at" => {
            let path = parse_path(mutation.path.as_ref().ok_or("replace_at requires 'path'")?)?;
            let node = parse_sexpr(mutation.node.as_ref().ok_or("replace_at requires 'node'")?)?;
            replace_at(doc, &path, node).map_err(|e| e.to_string())
        }
        "prune" => {
            let path = parse_path(mutation.path.as_ref().ok_or("prune requires 'path'")?)?;
            prune(doc, &path).map_err(|e| e.to_string())
        }
        "insert_before" => {
            let path = parse_path(
                mutation
                    .path
                    .as_ref()
                    .ok_or("insert_before requires 'path'")?,
            )?;
            let node = parse_sexpr(
                mutation
                    .node
                    .as_ref()
                    .ok_or("insert_before requires 'node'")?,
            )?;
            insert_before(doc, &path, node).map_err(|e| e.to_string())
        }
        "insert_after" => {
            let path = parse_path(
                mutation
                    .path
                    .as_ref()
                    .ok_or("insert_after requires 'path'")?,
            )?;
            let node = parse_sexpr(
                mutation
                    .node
                    .as_ref()
                    .ok_or("insert_after requires 'node'")?,
            )?;
            insert_after(doc, &path, node).map_err(|e| e.to_string())
        }
        "append_child" => {
            let parent = parse_path(
                mutation
                    .parent
                    .as_ref()
                    .ok_or("append_child requires 'parent'")?,
            )?;
            let node = parse_sexpr(
                mutation
                    .node
                    .as_ref()
                    .ok_or("append_child requires 'node'")?,
            )?;
            append_child(doc, &parent, node).map_err(|e| e.to_string())
        }
        "prepend_child" => {
            let parent = parse_path(
                mutation
                    .parent
                    .as_ref()
                    .ok_or("prepend_child requires 'parent'")?,
            )?;
            let node = parse_sexpr(
                mutation
                    .node
                    .as_ref()
                    .ok_or("prepend_child requires 'node'")?,
            )?;
            prepend_child(doc, &parent, node).map_err(|e| e.to_string())
        }
        "hoist" => {
            let path = parse_path(mutation.path.as_ref().ok_or("hoist requires 'path'")?)?;
            let delta = mutation.delta.ok_or("hoist requires 'delta'")?;
            hoist(doc, &path, delta).map_err(|e| e.to_string())
        }
        "graft" => {
            let source = parse_path(mutation.source.as_ref().ok_or("graft requires 'source'")?)?;
            let target_parent = parse_path(
                mutation
                    .target_parent
                    .as_ref()
                    .ok_or("graft requires 'target_parent'")?,
            )?;
            let target_index = mutation
                .target_index
                .ok_or("graft requires 'target_index'")?;
            graft(doc, &source, &target_parent, target_index).map_err(|e| e.to_string())
        }
        _ => Err(format!("Unknown mutation operation: {}", mutation.op)),
    }
}

/// Parses an s-expression string.
fn parse_sexpr(s: &str) -> Result<SExpr, String> {
    Parser::new(s)
        .parse()
        .map_err(|e| format!("Failed to parse s-expression: {}", e))
}

/// Parses a path string.
fn parse_path(s: &str) -> Result<PathId, String> {
    PathId::parse(s).map_err(|e| format!("Failed to parse path: {}", e))
}

/// Compares two s-expressions based on the comparison mode.
fn compare_sexpr(
    actual: &SExpr,
    expected: &SExpr,
    mode: &str,
    field_name: &str,
) -> Result<(), String> {
    match mode {
        "exact" => {
            let actual_str = actual.to_string();
            let expected_str = expected.to_string();
            if actual_str != expected_str {
                Err(format!(
                    "{} mismatch (exact):\n  expected: {}\n  actual:   {}",
                    field_name, expected_str, actual_str
                ))
            } else {
                Ok(())
            }
        }
        "semantic" => {
            // For semantic comparison, the s-expressions should be equal
            if actual != expected {
                Err(format!(
                    "{} mismatch (semantic):\n  expected: {}\n  actual:   {}",
                    field_name, expected, actual
                ))
            } else {
                Ok(())
            }
        }
        _ => Err(format!("Unknown comparison mode: {}", mode)),
    }
}

/// Compares two markdown strings based on the comparison mode.
fn compare_markdown(actual: &str, expected: &str, mode: &str) -> Result<(), String> {
    match mode {
        "exact" => {
            let actual_normalized = normalize_markdown(actual);
            let expected_normalized = normalize_markdown(expected);
            if actual_normalized != expected_normalized {
                Err(format!(
                    "Markdown mismatch (exact):\n  expected: {:?}\n  actual:   {:?}",
                    expected_normalized, actual_normalized
                ))
            } else {
                Ok(())
            }
        }
        "semantic" => {
            // For semantic comparison, parse both and compare ASTs
            let actual_sexpr =
                markdown_to_sexpr(actual).map_err(|e| format!("Failed to parse actual: {}", e))?;
            let expected_sexpr = markdown_to_sexpr(expected)
                .map_err(|e| format!("Failed to parse expected: {}", e))?;
            if actual_sexpr != expected_sexpr {
                Err(format!(
                    "Markdown mismatch (semantic):\n  expected AST: {}\n  actual AST:   {}",
                    expected_sexpr, actual_sexpr
                ))
            } else {
                Ok(())
            }
        }
        _ => Err(format!("Unknown comparison mode: {}", mode)),
    }
}

/// Normalizes markdown for comparison by trimming leading/trailing whitespace.
fn normalize_markdown(s: &str) -> String {
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_test_suite() {
        let toml_content = r#"
[metadata]
version = "1.0"
description = "Test suite"

[[test]]
name = "simple"
before = "Hello"
"#;

        let suite: TestSuite = toml::from_str(toml_content).unwrap();
        assert_eq!(suite.metadata.version, "1.0");
        assert_eq!(suite.tests.len(), 1);
        assert_eq!(suite.tests[0].name, "simple");
        println!("DEBUG: Parsed test suite successfully");
    }

    #[test]
    fn parse_mutation() {
        let toml_content = r#"
[metadata]
version = "1.0"

[[test]]
name = "mutation_test"
before_sexpr = '(doc (h1 "Title"))'
mutation = { op = "replace_at", path = "1", node = '(h2 "New")' }
after_sexpr = '(doc (h2 "New"))'
"#;

        let suite: TestSuite = toml::from_str(toml_content).unwrap();
        assert!(suite.tests[0].mutation.is_some());
        let mutation = suite.tests[0].mutation.as_ref().unwrap();
        assert_eq!(mutation.op, "replace_at");
        assert_eq!(mutation.path, Some("1".to_string()));
        println!("DEBUG: Parsed mutation successfully");
    }

    #[test]
    fn parse_expected_error() {
        let toml_content = r#"
[metadata]
version = "1.0"

[[test]]
name = "error_test"
before_sexpr = '(doc (h1 "Title"))'
mutation = { op = "prune", path = "99" }
expected_error = { code = "index-out-of-bounds" }
"#;

        let suite: TestSuite = toml::from_str(toml_content).unwrap();
        assert!(suite.tests[0].expected_error.is_some());
        let expected = suite.tests[0].expected_error.as_ref().unwrap();
        assert_eq!(expected.code, Some("index-out-of-bounds".to_string()));
        println!("DEBUG: Parsed expected_error successfully");
    }

    #[test]
    fn execute_simple_roundtrip() {
        let test = TestCase {
            name: "simple".to_string(),
            description: None,
            before: Some("Hello, world!".to_string()),
            before_sexpr: None,
            sexpr: Some(r#"(doc (p "Hello, world!"))"#.to_string()),
            after: None,
            after_sexpr: None,
            compare: "semantic".to_string(),
            mutation: None,
            mutations: None,
            expected_error: None,
            skip: false,
            skip_reason: None,
        };

        let result = execute_test(&test);
        println!("DEBUG: roundtrip result = {:?}", result);
        assert!(result.is_ok(), "Test failed: {:?}", result);
    }

    #[test]
    fn execute_simple_mutation() {
        let test = TestCase {
            name: "mutation".to_string(),
            description: None,
            before: None,
            before_sexpr: Some(r#"(doc (h1 "Old") (p "Content"))"#.to_string()),
            sexpr: None,
            after: None,
            after_sexpr: Some(r#"(doc (h2 "New") (p "Content"))"#.to_string()),
            compare: "exact".to_string(),
            mutation: Some(Mutation {
                op: "replace_at".to_string(),
                path: Some("1".to_string()),
                node: Some(r#"(h2 "New")"#.to_string()),
                parent: None,
                delta: None,
                source: None,
                target_parent: None,
                target_index: None,
            }),
            mutations: None,
            expected_error: None,
            skip: false,
            skip_reason: None,
        };

        let result = execute_test(&test);
        println!("DEBUG: mutation result = {:?}", result);
        assert!(result.is_ok(), "Test failed: {:?}", result);
    }

    #[test]
    fn execute_expected_error() {
        let test = TestCase {
            name: "error".to_string(),
            description: None,
            before: None,
            before_sexpr: Some(r#"(doc (h1 "Title"))"#.to_string()),
            sexpr: None,
            after: None,
            after_sexpr: None,
            compare: "exact".to_string(),
            mutation: Some(Mutation {
                op: "prune".to_string(),
                path: Some("99".to_string()),
                node: None,
                parent: None,
                delta: None,
                source: None,
                target_parent: None,
                target_index: None,
            }),
            mutations: None,
            expected_error: Some(ExpectedError {
                code: Some("index-out-of-bounds".to_string()),
                phase: None,
                message_contains: None,
            }),
            skip: false,
            skip_reason: None,
        };

        let result = execute_test(&test);
        println!("DEBUG: error test result = {:?}", result);
        assert!(result.is_ok(), "Test failed: {:?}", result);
    }
}
