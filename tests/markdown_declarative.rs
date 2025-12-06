//! Integration test that runs all declarative markdown tests.

use std::path::Path;

use agentkb::run_test_directory;

#[test]
fn run_all_markdown_tests() {
    let results = run_test_directory(Path::new("tests/markdown"));

    let mut passed = 0;
    let mut skipped = 0;
    let mut failed = Vec::new();

    for result in &results {
        if result.skipped {
            skipped += 1;
            println!("SKIP: {}", result.name);
            if let Some(reason) = &result.error {
                println!("  Reason: {}", reason);
            }
        } else if result.passed {
            passed += 1;
            println!("PASS: {}", result.name);
        } else {
            failed.push(result);
        }
    }

    for result in &failed {
        println!("FAIL: {}", result.name);
        if let Some(err) = &result.error {
            println!("  Error: {}", err);
        }
    }

    println!();
    println!(
        "Results: {} passed, {} skipped, {} failed",
        passed,
        skipped,
        failed.len()
    );

    assert!(
        failed.is_empty(),
        "{} test(s) failed out of {} total",
        failed.len(),
        results.len()
    );
}
