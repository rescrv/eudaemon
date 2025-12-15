//! Debug log analyzer for eudaemonfs.
//!
//! This tool reads debug logs produced by LoggingFilesystem and LoggingBlockDevice,
//! parses the S-expressions, and allows filtering and analysis using a Lisp program.
//!
//! # Log Format
//!
//! Each line in the log is an S-expression of the form:
//! ```text
//! (seq <sequence-number> <operation>)
//! ```
//!
//! Operations include:
//! - `(block-read <block> #x<hash>)` - Block device read
//! - `(block-write <block> #x<hash>)` - Block device write
//! - `(fs-open "<path>" fd:<n> created:<bool>)` - File open
//! - `(fs-close fd:<n>)` - File close
//! - `(fs-read fd:<n> offset:<n> len:<n> #x<hash>)` - File read
//! - `(fs-write fd:<n> offset:<n> len:<n> #x<hash>)` - File write
//! - `(fs-seek fd:<n> pos:<n>)` - File seek
//! - `(fs-truncate fd:<n> size:<n>)` - File truncate
//! - And more filesystem operations...

use std::fs::File;
use std::io::{BufRead, BufReader, Read};

use arrrg::CommandLine;
use arrrg_derive::CommandLine;
use lispdown::{Parser, SExpr, Vm};

/// Command line arguments for eudaemon-fs-debug.
#[derive(CommandLine, Debug, Default, Eq, PartialEq)]
struct Args {
    /// Path to the debug log file (use - for stdin).
    #[arrrg(optional, "Path to debug log file (default: stdin)")]
    log: Option<String>,

    /// Filter expression to apply to each log entry.
    /// The expression receives the parsed S-expression as 'entry'.
    #[arrrg(optional, "Filter expression (receives 'entry')")]
    filter: Option<String>,

    /// Lisp program file to run over the log.
    /// The program receives the entire log as a list bound to 'log'.
    #[arrrg(optional, "Lisp program file to run")]
    program: Option<String>,

    /// Show only operations matching this type (e.g., "fs-write", "block-read").
    #[arrrg(optional, "Show only operations of this type")]
    op_type: Option<String>,

    /// Show raw parsed S-expressions.
    #[arrrg(flag, "Show raw S-expressions")]
    raw: bool,

    /// Count operations by type.
    #[arrrg(flag, "Count operations by type")]
    count: bool,
}

/// Parses a single log line into an S-expression.
fn parse_log_line(line: &str) -> Option<SExpr> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut parser = Parser::new(trimmed);
    parser.parse().ok()
}

/// Extracts the operation type from a log entry.
fn get_op_type(entry: &SExpr) -> Option<&str> {
    // Format: (seq <n> <operation>)
    if let SExpr::List(outer) = entry
        && outer.len() >= 3
        && let SExpr::List(inner) = &outer[2]
        && let Some(SExpr::Atom(op)) = inner.first()
    {
        return Some(op.as_str());
    }
    None
}

/// Extracts the sequence number from a log entry.
fn get_seq_num(entry: &SExpr) -> Option<u64> {
    if let SExpr::List(outer) = entry
        && outer.len() >= 2
        && let SExpr::Atom(num_str) = &outer[1]
    {
        return num_str.parse().ok();
    }
    None
}

/// Counts operations by type.
fn count_operations(entries: &[SExpr]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for entry in entries {
        if let Some(op_type) = get_op_type(entry) {
            *counts.entry(op_type.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// Reads log entries from a reader.
fn read_log_entries<R: Read>(reader: R) -> Vec<SExpr> {
    let buf_reader = BufReader::new(reader);
    buf_reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| parse_log_line(&line))
        .collect()
}

fn main() {
    let (args, _free) =
        Args::from_command_line_relaxed("eudaemon-fs-debug: analyze filesystem debug logs");

    // Read log entries
    let entries: Vec<SExpr> = if let Some(ref path) = args.log {
        if path == "-" {
            read_log_entries(std::io::stdin())
        } else {
            let file = File::open(path).unwrap_or_else(|e| {
                eprintln!("Error opening {}: {}", path, e);
                std::process::exit(1);
            });
            read_log_entries(file)
        }
    } else {
        read_log_entries(std::io::stdin())
    };

    // Filter by operation type if specified
    let filtered: Vec<&SExpr> = if let Some(ref op_type) = args.op_type {
        entries
            .iter()
            .filter(|e| get_op_type(e) == Some(op_type.as_str()))
            .collect()
    } else {
        entries.iter().collect()
    };

    // Count operations if requested
    if args.count {
        let counts = count_operations(&entries);
        println!("Operation counts:");
        for (op, count) in &counts {
            println!("  {}: {}", op, count);
        }
        println!("Total: {}", entries.len());
        return;
    }

    // Run program if specified
    if let Some(ref program_path) = args.program {
        let program = std::fs::read_to_string(program_path).unwrap_or_else(|e| {
            eprintln!("Error reading {}: {}", program_path, e);
            std::process::exit(1);
        });

        // Build the log as a list S-expression
        let log_list = SExpr::List(
            std::iter::once(SExpr::Atom("list".to_string()))
                .chain(filtered.iter().map(|e| (*e).clone()))
                .collect(),
        );

        // Create VM and bind log
        let mut vm = Vm::new();
        vm.register_builtins();

        // Parse and evaluate the program
        // First, wrap the program to bind 'log'
        let wrapped = format!("(let ((log {})) {})", log_list, program);
        let mut parser = Parser::new(&wrapped);
        match parser.parse() {
            Ok(expr) => match vm.eval(&expr) {
                Ok(result) => println!("{}", result),
                Err(e) => {
                    eprintln!("Evaluation error: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Parse error: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    // Apply filter expression if specified
    if let Some(ref filter_expr) = args.filter {
        let mut vm = Vm::new();
        vm.register_builtins();

        for entry in &filtered {
            // Wrap the filter to bind 'entry'
            let wrapped = format!("(let ((entry {})) {})", entry, filter_expr);
            let mut parser = Parser::new(&wrapped);
            match parser.parse() {
                Ok(expr) => match vm.eval(&expr) {
                    Ok(result) => {
                        // If result is truthy (not nil/false), print the entry
                        if result != SExpr::Atom("nil".to_string())
                            && result != SExpr::Atom("false".to_string())
                        {
                            if args.raw {
                                println!("{}", entry);
                            } else {
                                print_entry(entry);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Filter error on {:?}: {}", get_seq_num(entry), e);
                    }
                },
                Err(e) => {
                    eprintln!("Filter parse error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        return;
    }

    // Default: print all entries
    for entry in &filtered {
        if args.raw {
            println!("{}", entry);
        } else {
            print_entry(entry);
        }
    }
}

/// Prints a log entry in a human-readable format.
fn print_entry(entry: &SExpr) {
    if let Some(seq) = get_seq_num(entry)
        && let SExpr::List(outer) = entry
        && outer.len() >= 3
    {
        println!("[{:6}] {}", seq, outer[2]);
        return;
    }
    println!("{}", entry);
}
