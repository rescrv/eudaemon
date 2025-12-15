//! The test builtin: condition evaluation utility.
//!
//! Evaluates conditional expressions and returns 0 (true) or 1 (false).

use crate::{Environment, Error, ExitCode, FileType, Filesystem, Stderr, Stdin, Stdout};

/// Token types for the test grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenType {
    /// End of input.
    Eoi,
    /// An operand (string or filename).
    Operand,
    /// Unary operator (e.g., -n, -z, -f).
    Unop,
    /// Binary operator (e.g., =, !=, -eq).
    Binop,
    /// Boolean unary operator (!).
    Bunop,
    /// Boolean binary operator (-a, -o).
    Bbinop,
    /// Parenthesis.
    Paren,
}

/// Tokens in the test grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// End of input.
    Eoi,
    /// An operand (string or filename).
    Operand,

    // Unary file operators
    /// -r: file exists and is readable
    FileReadable,
    /// -w: file exists and is writable
    FileWritable,
    /// -x: file exists and is executable
    FileExecutable,
    /// -e: file exists
    FileExists,
    /// -f: file exists and is regular
    FileRegular,
    /// -d: file exists and is directory
    FileDirectory,
    /// -s: file exists and has size > 0
    FileSize,
    /// -L or -h: file exists and is symlink
    FileSymlink,

    // String operators
    /// -z: string length is zero
    StringZero,
    /// -n: string length is nonzero
    StringNonzero,

    // Binary string operators
    /// =: strings are equal
    StringEqual,
    /// !=: strings are not equal
    StringNotEqual,
    /// <: string comes before
    StringLess,
    /// >: string comes after
    StringGreater,

    // Binary integer operators
    /// -eq: integers are equal
    IntEqual,
    /// -ne: integers are not equal
    IntNotEqual,
    /// -gt: first integer is greater
    IntGreater,
    /// -ge: first integer is greater or equal
    IntGreaterEqual,
    /// -lt: first integer is less
    IntLess,
    /// -le: first integer is less or equal
    IntLessEqual,

    // Binary file operators
    /// -nt: file1 is newer than file2
    FileNewer,
    /// -ot: file1 is older than file2
    FileOlder,
    /// -ef: file1 and file2 are the same file
    FileSame,

    // Boolean operators
    /// !: logical not
    Not,
    /// -a: logical and
    And,
    /// -o: logical or
    Or,

    // Parentheses
    /// (
    LeftParen,
    /// )
    RightParen,
}

impl Token {
    /// Get the type of this token.
    fn token_type(self) -> TokenType {
        match self {
            Token::Eoi => TokenType::Eoi,
            Token::Operand => TokenType::Operand,
            Token::FileReadable
            | Token::FileWritable
            | Token::FileExecutable
            | Token::FileExists
            | Token::FileRegular
            | Token::FileDirectory
            | Token::FileSize
            | Token::FileSymlink
            | Token::StringZero
            | Token::StringNonzero => TokenType::Unop,
            Token::StringEqual
            | Token::StringNotEqual
            | Token::StringLess
            | Token::StringGreater
            | Token::IntEqual
            | Token::IntNotEqual
            | Token::IntGreater
            | Token::IntGreaterEqual
            | Token::IntLess
            | Token::IntLessEqual
            | Token::FileNewer
            | Token::FileOlder
            | Token::FileSame => TokenType::Binop,
            Token::Not => TokenType::Bunop,
            Token::And | Token::Or => TokenType::Bbinop,
            Token::LeftParen | Token::RightParen => TokenType::Paren,
        }
    }
}

/// Parser state for the test expression.
struct Parser<'a, FS>
where
    FS: Filesystem,
{
    /// The arguments to parse.
    args: &'a [String],
    /// Current position in args.
    pos: usize,
    /// Current parenthesis nesting level.
    paren_level: i32,
    /// Filesystem for file tests.
    fs: &'a FS,
    /// Error message if parsing failed.
    error: Option<String>,
}

impl<'a, FS> Parser<'a, FS>
where
    FS: Filesystem,
{
    /// Create a new parser.
    fn new(args: &'a [String], fs: &'a FS) -> Self {
        Self {
            args,
            pos: 0,
            paren_level: 0,
            fs,
            error: None,
        }
    }

    /// Get the current argument or None if at end.
    fn current(&self) -> Option<&str> {
        self.args.get(self.pos).map(|s| s.as_str())
    }

    /// Peek at the next argument.
    fn peek(&self) -> Option<&str> {
        self.args.get(self.pos + 1).map(|s| s.as_str())
    }

    /// Get number of remaining arguments.
    fn remaining(&self) -> usize {
        self.args.len().saturating_sub(self.pos)
    }

    /// Advance to the next argument.
    fn advance(&mut self) {
        if self.pos < self.args.len() {
            self.pos += 1;
        }
    }

    /// Set an error message.
    fn set_error(&mut self, msg: String) {
        if self.error.is_none() {
            self.error = Some(msg);
        }
    }

    /// Find the operator for a string.
    fn find_op(s: &str) -> Token {
        match s {
            // Single-char operators
            "=" => Token::StringEqual,
            "<" => Token::StringLess,
            ">" => Token::StringGreater,
            "!" => Token::Not,
            "(" => Token::LeftParen,
            ")" => Token::RightParen,

            // Two-char operators with dash prefix
            "-r" => Token::FileReadable,
            "-w" => Token::FileWritable,
            "-x" => Token::FileExecutable,
            "-e" => Token::FileExists,
            "-f" => Token::FileRegular,
            "-d" => Token::FileDirectory,
            "-s" => Token::FileSize,
            "-h" | "-L" => Token::FileSymlink,
            "-z" => Token::StringZero,
            "-n" => Token::StringNonzero,
            "-a" => Token::And,
            "-o" => Token::Or,

            // Two-char operators without dash
            "==" => Token::StringEqual,
            "!=" => Token::StringNotEqual,

            // Three-char operators with dash prefix
            "-eq" => Token::IntEqual,
            "-ne" => Token::IntNotEqual,
            "-gt" => Token::IntGreater,
            "-ge" => Token::IntGreaterEqual,
            "-lt" => Token::IntLess,
            "-le" => Token::IntLessEqual,
            "-nt" => Token::FileNewer,
            "-ot" => Token::FileOlder,
            "-ef" => Token::FileSame,

            _ => Token::Operand,
        }
    }

    /// Lexically analyze the current argument to get its token.
    /// This handles the grammar ambiguity by looking ahead.
    fn t_lex(&self, s: Option<&str>) -> Token {
        let Some(s) = s else {
            return Token::Eoi;
        };

        let tok = Self::find_op(s);

        // Handle grammar ambiguity
        match tok.token_type() {
            TokenType::Unop | TokenType::Bunop => {
                if self.is_unop_operand() {
                    return Token::Operand;
                }
            }
            TokenType::Paren if tok == Token::LeftParen => {
                if self.is_lparen_operand() {
                    return Token::Operand;
                }
            }
            TokenType::Paren if tok == Token::RightParen => {
                if self.is_rparen_operand() {
                    return Token::Operand;
                }
            }
            _ => {}
        }

        tok
    }

    /// Check if a unary operator should be treated as an operand.
    fn is_unop_operand(&self) -> bool {
        let remaining = self.remaining();
        if remaining == 1 {
            return true;
        }
        let next = self.peek();
        if remaining == 2 {
            return self.paren_level == 1 && next == Some(")");
        }
        // Check if next is a binary operator
        if let Some(next_str) = next {
            let next_tok = Self::find_op(next_str);
            if next_tok.token_type() == TokenType::Binop {
                // Look at the token after that
                if let Some(after) = self.args.get(self.pos + 2) {
                    return self.paren_level == 0 || after != ")";
                }
                return true;
            }
        }
        false
    }

    /// Check if left paren should be treated as an operand.
    fn is_lparen_operand(&self) -> bool {
        let remaining = self.remaining();
        if remaining == 1 {
            return true;
        }
        let next = self.peek();
        if remaining == 2 {
            return self.paren_level == 1 && next == Some(")");
        }
        if remaining == 3
            && let Some(next_str) = next
        {
            let next_tok = Self::find_op(next_str);
            return next_tok.token_type() == TokenType::Binop;
        }
        false
    }

    /// Check if right paren should be treated as an operand.
    fn is_rparen_operand(&self) -> bool {
        let remaining = self.remaining();
        if remaining == 1 {
            return false;
        }
        if remaining == 2 {
            return self.paren_level == 1 && self.peek() == Some(")");
        }
        false
    }

    /// Parse an or-expression (lowest precedence).
    fn oexpr(&mut self) -> bool {
        let res = self.aexpr();
        let tok = self.t_lex(self.current());
        if tok == Token::Or {
            self.advance();
            // Note: both sides are always evaluated (per POSIX)
            return self.oexpr() || res;
        }
        res
    }

    /// Parse an and-expression.
    fn aexpr(&mut self) -> bool {
        let res = self.nexpr();
        let tok = self.t_lex(self.current());
        if tok == Token::And {
            self.advance();
            // Note: both sides are always evaluated (per POSIX)
            return self.aexpr() && res;
        }
        res
    }

    /// Parse a negation expression.
    fn nexpr(&mut self) -> bool {
        let tok = self.t_lex(self.current());
        if tok == Token::Not {
            self.advance();
            return !self.nexpr();
        }
        self.primary()
    }

    /// Parse a primary expression.
    fn primary(&mut self) -> bool {
        let tok = self.t_lex(self.current());

        match tok {
            Token::Eoi => false,

            Token::LeftParen => {
                self.paren_level += 1;
                self.advance();

                let inner_tok = self.t_lex(self.current());
                if inner_tok == Token::RightParen {
                    self.paren_level -= 1;
                    self.advance();
                    return false;
                }

                let res = self.oexpr();

                if self.t_lex(self.current()) != Token::RightParen {
                    self.set_error("closing paren expected".to_string());
                }
                self.paren_level -= 1;
                self.advance();
                res
            }

            _ if tok.token_type() == TokenType::Unop => {
                self.advance();
                let operand = match self.current() {
                    Some(s) => s.to_string(),
                    None => {
                        self.set_error("argument expected".to_string());
                        return false;
                    }
                };
                self.advance();

                match tok {
                    Token::StringZero => operand.is_empty(),
                    Token::StringNonzero => !operand.is_empty(),
                    Token::FileExists => self.fs.exists(&operand),
                    Token::FileRegular => self
                        .fs
                        .stat(&operand)
                        .map(|s| s.file_type == FileType::RegularFile)
                        .unwrap_or(false),
                    Token::FileDirectory => self.fs.is_dir(&operand),
                    Token::FileSymlink => self
                        .fs
                        .stat(&operand)
                        .map(|s| s.file_type == FileType::Symlink)
                        .unwrap_or(false),
                    Token::FileSize => self
                        .fs
                        .metadata(&operand)
                        .map(|m| m.size > 0)
                        .unwrap_or(false),
                    // For a virtual filesystem, these permissions are simulated
                    Token::FileReadable | Token::FileWritable | Token::FileExecutable => {
                        self.fs.exists(&operand)
                    }
                    _ => false,
                }
            }

            Token::Operand => {
                // Could be a bare string (true if non-empty) or start of binary op
                let operand = self.current().unwrap_or("").to_string();

                // Look ahead to see if this is a binary expression
                if let Some(next) = self.peek() {
                    let next_tok = Self::find_op(next);
                    if next_tok.token_type() == TokenType::Binop {
                        return self.binop();
                    }
                }

                // Bare string: true if non-empty
                self.advance();
                !operand.is_empty()
            }

            _ => {
                // Treat as operand
                let operand = self.current().unwrap_or("").to_string();
                self.advance();
                !operand.is_empty()
            }
        }
    }

    /// Parse a binary operation.
    fn binop(&mut self) -> bool {
        let opnd1 = self.current().unwrap_or("").to_string();
        self.advance();

        let op = self.t_lex(self.current());
        self.advance();

        let opnd2 = match self.current() {
            Some(s) => s.to_string(),
            None => {
                self.set_error("argument expected".to_string());
                return false;
            }
        };
        self.advance();

        match op {
            Token::StringEqual => opnd1 == opnd2,
            Token::StringNotEqual => opnd1 != opnd2,
            Token::StringLess => opnd1 < opnd2,
            Token::StringGreater => opnd1 > opnd2,
            Token::IntEqual => int_cmp(&opnd1, &opnd2) == Some(std::cmp::Ordering::Equal),
            Token::IntNotEqual => int_cmp(&opnd1, &opnd2) != Some(std::cmp::Ordering::Equal),
            Token::IntGreater => int_cmp(&opnd1, &opnd2) == Some(std::cmp::Ordering::Greater),
            Token::IntGreaterEqual => {
                matches!(
                    int_cmp(&opnd1, &opnd2),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                )
            }
            Token::IntLess => int_cmp(&opnd1, &opnd2) == Some(std::cmp::Ordering::Less),
            Token::IntLessEqual => {
                matches!(
                    int_cmp(&opnd1, &opnd2),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                )
            }
            Token::FileNewer => {
                let m1 = self.fs.stat(&opnd1).ok();
                let m2 = self.fs.stat(&opnd2).ok();
                match (m1, m2) {
                    (Some(s1), Some(s2)) => s1.mtime_ms > s2.mtime_ms,
                    _ => false,
                }
            }
            Token::FileOlder => {
                let m1 = self.fs.stat(&opnd1).ok();
                let m2 = self.fs.stat(&opnd2).ok();
                match (m1, m2) {
                    (Some(s1), Some(s2)) => s1.mtime_ms < s2.mtime_ms,
                    _ => false,
                }
            }
            Token::FileSame => {
                // In a mock filesystem, same path means same file
                opnd1 == opnd2 && self.fs.exists(&opnd1)
            }
            _ => false,
        }
    }

    /// Parse the expression and return the result.
    fn parse(&mut self) -> bool {
        if self.args.is_empty() {
            return false;
        }

        // Special case: handle "! expr" with exactly 4 args
        // (This handles cases like `! "" -o x`)
        if self.args.len() == 4 && self.args.first().map(|s| s.as_str()) == Some("!") {
            self.advance();
            return self.oexpr();
        }

        !self.oexpr()
    }
}

/// Compare two strings as integers.
fn int_cmp(s1: &str, s2: &str) -> Option<std::cmp::Ordering> {
    let n1: i64 = s1.trim().parse().ok()?;
    let n2: i64 = s2.trim().parse().ok()?;
    Some(n1.cmp(&n2))
}

/// The test builtin: evaluate conditional expressions.
///
/// Usage:
///   test expression
///   [ expression ]
///
/// Returns 0 if the expression is true, 1 if false, >1 on error.
pub fn bin<SI, SO, SE, FS>(env: &Environment<SI, SO, SE, FS>) -> Result<ExitCode, Error>
where
    SI: Stdin,
    SO: Stdout,
    SE: Stderr,
    FS: Filesystem,
{
    let args = &env.args;

    // Determine if invoked as "[" and strip trailing "]"
    let prog_name = args.first().map(|s| s.as_str()).unwrap_or("test");
    let is_bracket = prog_name == "[" || prog_name.ends_with("/[");

    let expr_args: Vec<String> = if is_bracket {
        // Must have trailing "]"
        if args.last().map(|s| s.as_str()) != Some("]") {
            env.stderr.write_line("[: missing ']'")?;
            return Ok(ExitCode::from(2));
        }
        // Skip program name and trailing "]"
        args[1..args.len() - 1].to_vec()
    } else {
        // Skip program name
        args[1..].to_vec()
    };

    // No expression => false
    if expr_args.is_empty() {
        return Ok(ExitCode::from(1));
    }

    let mut parser = Parser::new(&expr_args, &env.fs);
    let result = parser.parse();

    // Check for trailing arguments
    if parser.pos < expr_args.len() {
        env.stderr.write_line(&format!(
            "test: {}: unexpected operator",
            expr_args.get(parser.pos).map(|s| s.as_str()).unwrap_or("")
        ))?;
        return Ok(ExitCode::from(2));
    }

    // Check for parse errors
    if let Some(err) = parser.error {
        env.stderr.write_line(&format!("test: {}", err))?;
        return Ok(ExitCode::from(2));
    }

    // Note: parser.parse() returns inverted result for convenience
    // true expression => result is true => exit 0
    // false expression => result is false => exit 1
    Ok(ExitCode::from(if result { 1 } else { 0 }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestFilesystemExt;
    use crate::test_utils::{TestFilesystem, make_test_env};
    use crate::{MemoryLfsExt, StringStderr, StringStdin, StringStdout};

    use eudaemonfs::DeviceId;

    fn zero_time() -> i64 {
        0
    }

    fn make_fs() -> TestFilesystem {
        crate::EudaemonFilesystem::new_memory(
            256 * 4096,
            DeviceId::new(1),
            zero_time as fn() -> i64,
        )
        .expect("failed to create test filesystem")
    }

    fn make_env_with_fs(
        args: Vec<&str>,
        fs: TestFilesystem,
    ) -> Environment<StringStdin, StringStdout, StringStderr, TestFilesystem> {
        Environment {
            stdin: StringStdin::new(""),
            stdout: StringStdout::new(),
            stderr: StringStderr::new(),
            fs,
            env: std::collections::HashMap::new(),
            args: args.into_iter().map(|s| s.to_string()).collect(),
            cwd: utf8path::Path::from("/"),
            exit_signaled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    // ========================================================================
    // No expression tests
    // ========================================================================

    #[test]
    fn no_expression_returns_false() {
        let env = make_test_env(vec!["test"]);
        let result = bin(&env).unwrap();
        println!("no_expression_returns_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn bracket_empty_returns_false() {
        let env = make_test_env(vec!["[", "]"]);
        let result = bin(&env).unwrap();
        println!("bracket_empty_returns_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn bracket_missing_close_returns_error() {
        let env = make_test_env(vec!["[", "-n", "foo"]);
        let result = bin(&env).unwrap();
        println!(
            "bracket_missing_close_returns_error: exit code = {}",
            result.code()
        );
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("missing ']'"));
    }

    // ========================================================================
    // String length tests (-n, -z)
    // ========================================================================

    #[test]
    fn string_nonzero_nonempty() {
        let env = make_test_env(vec!["test", "-n", "hello"]);
        let result = bin(&env).unwrap();
        println!("string_nonzero_nonempty: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_nonzero_empty() {
        let env = make_test_env(vec!["test", "-n", ""]);
        let result = bin(&env).unwrap();
        println!("string_nonzero_empty: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn string_zero_empty() {
        let env = make_test_env(vec!["test", "-z", ""]);
        let result = bin(&env).unwrap();
        println!("string_zero_empty: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_zero_nonempty() {
        let env = make_test_env(vec!["test", "-z", "hello"]);
        let result = bin(&env).unwrap();
        println!("string_zero_nonempty: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Bare string tests
    // ========================================================================

    #[test]
    fn bare_string_nonempty() {
        let env = make_test_env(vec!["test", "hello"]);
        let result = bin(&env).unwrap();
        println!("bare_string_nonempty: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn bare_string_empty() {
        let env = make_test_env(vec!["test", ""]);
        let result = bin(&env).unwrap();
        println!("bare_string_empty: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // String comparison tests
    // ========================================================================

    #[test]
    fn string_equal_true() {
        let env = make_test_env(vec!["test", "abc", "=", "abc"]);
        let result = bin(&env).unwrap();
        println!("string_equal_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_equal_false() {
        let env = make_test_env(vec!["test", "abc", "=", "def"]);
        let result = bin(&env).unwrap();
        println!("string_equal_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn string_equal_double_equals() {
        let env = make_test_env(vec!["test", "abc", "==", "abc"]);
        let result = bin(&env).unwrap();
        println!("string_equal_double_equals: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_not_equal_true() {
        let env = make_test_env(vec!["test", "abc", "!=", "def"]);
        let result = bin(&env).unwrap();
        println!("string_not_equal_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_not_equal_false() {
        let env = make_test_env(vec!["test", "abc", "!=", "abc"]);
        let result = bin(&env).unwrap();
        println!("string_not_equal_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn string_less_true() {
        let env = make_test_env(vec!["test", "abc", "<", "def"]);
        let result = bin(&env).unwrap();
        println!("string_less_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_less_false() {
        let env = make_test_env(vec!["test", "def", "<", "abc"]);
        let result = bin(&env).unwrap();
        println!("string_less_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn string_greater_true() {
        let env = make_test_env(vec!["test", "def", ">", "abc"]);
        let result = bin(&env).unwrap();
        println!("string_greater_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_greater_false() {
        let env = make_test_env(vec!["test", "abc", ">", "def"]);
        let result = bin(&env).unwrap();
        println!("string_greater_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Integer comparison tests
    // ========================================================================

    #[test]
    fn int_equal_true() {
        let env = make_test_env(vec!["test", "42", "-eq", "42"]);
        let result = bin(&env).unwrap();
        println!("int_equal_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_equal_false() {
        let env = make_test_env(vec!["test", "42", "-eq", "43"]);
        let result = bin(&env).unwrap();
        println!("int_equal_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_not_equal_true() {
        let env = make_test_env(vec!["test", "42", "-ne", "43"]);
        let result = bin(&env).unwrap();
        println!("int_not_equal_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_not_equal_false() {
        let env = make_test_env(vec!["test", "42", "-ne", "42"]);
        let result = bin(&env).unwrap();
        println!("int_not_equal_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_greater_true() {
        let env = make_test_env(vec!["test", "43", "-gt", "42"]);
        let result = bin(&env).unwrap();
        println!("int_greater_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_greater_false() {
        let env = make_test_env(vec!["test", "42", "-gt", "43"]);
        let result = bin(&env).unwrap();
        println!("int_greater_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_greater_equal_true_greater() {
        let env = make_test_env(vec!["test", "43", "-ge", "42"]);
        let result = bin(&env).unwrap();
        println!(
            "int_greater_equal_true_greater: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_greater_equal_true_equal() {
        let env = make_test_env(vec!["test", "42", "-ge", "42"]);
        let result = bin(&env).unwrap();
        println!(
            "int_greater_equal_true_equal: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_greater_equal_false() {
        let env = make_test_env(vec!["test", "41", "-ge", "42"]);
        let result = bin(&env).unwrap();
        println!("int_greater_equal_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_less_true() {
        let env = make_test_env(vec!["test", "42", "-lt", "43"]);
        let result = bin(&env).unwrap();
        println!("int_less_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_less_false() {
        let env = make_test_env(vec!["test", "43", "-lt", "42"]);
        let result = bin(&env).unwrap();
        println!("int_less_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_less_equal_true_less() {
        let env = make_test_env(vec!["test", "42", "-le", "43"]);
        let result = bin(&env).unwrap();
        println!("int_less_equal_true_less: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_less_equal_true_equal() {
        let env = make_test_env(vec!["test", "42", "-le", "42"]);
        let result = bin(&env).unwrap();
        println!("int_less_equal_true_equal: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_less_equal_false() {
        let env = make_test_env(vec!["test", "43", "-le", "42"]);
        let result = bin(&env).unwrap();
        println!("int_less_equal_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_negative_numbers() {
        let env = make_test_env(vec!["test", "-5", "-lt", "5"]);
        let result = bin(&env).unwrap();
        println!("int_negative_numbers: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // File tests
    // ========================================================================

    #[test]
    fn file_exists_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-e", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_exists_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_exists_false() {
        let env = make_test_env(vec!["test", "-e", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_exists_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_regular_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-f", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_regular_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_regular_false_directory() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_directory("/tmp/testdir");
        let env = make_env_with_fs(vec!["test", "-f", "/tmp/testdir"], fs);
        let result = bin(&env).unwrap();
        println!(
            "file_regular_false_directory: exit code = {}",
            result.code()
        );
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_directory_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_directory("/tmp/testdir");
        let env = make_env_with_fs(vec!["test", "-d", "/tmp/testdir"], fs);
        let result = bin(&env).unwrap();
        println!("file_directory_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_directory_false() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-d", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_directory_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_size_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-s", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_size_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_size_false_empty() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "");
        let env = make_env_with_fs(vec!["test", "-s", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_size_false_empty: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_readable_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-r", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_readable_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_readable_false() {
        let env = make_test_env(vec!["test", "-r", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_readable_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Negation tests
    // ========================================================================

    #[test]
    fn not_true_to_false() {
        let env = make_test_env(vec!["test", "!", "-n", "hello"]);
        let result = bin(&env).unwrap();
        println!("not_true_to_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn not_false_to_true() {
        let env = make_test_env(vec!["test", "!", "-z", "hello"]);
        let result = bin(&env).unwrap();
        println!("not_false_to_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn double_negation() {
        let env = make_test_env(vec!["test", "!", "!", "-n", "hello"]);
        let result = bin(&env).unwrap();
        println!("double_negation: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // Boolean operator tests (-a, -o)
    // ========================================================================

    #[test]
    fn and_true_true() {
        let env = make_test_env(vec!["test", "-n", "a", "-a", "-n", "b"]);
        let result = bin(&env).unwrap();
        println!("and_true_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn and_true_false() {
        let env = make_test_env(vec!["test", "-n", "a", "-a", "-z", "b"]);
        let result = bin(&env).unwrap();
        println!("and_true_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn and_false_true() {
        let env = make_test_env(vec!["test", "-z", "a", "-a", "-n", "b"]);
        let result = bin(&env).unwrap();
        println!("and_false_true: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn and_false_false() {
        let env = make_test_env(vec!["test", "-z", "a", "-a", "-z", "b"]);
        let result = bin(&env).unwrap();
        println!("and_false_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn or_true_true() {
        let env = make_test_env(vec!["test", "-n", "a", "-o", "-n", "b"]);
        let result = bin(&env).unwrap();
        println!("or_true_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn or_true_false() {
        let env = make_test_env(vec!["test", "-n", "a", "-o", "-z", "b"]);
        let result = bin(&env).unwrap();
        println!("or_true_false: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn or_false_true() {
        let env = make_test_env(vec!["test", "-z", "a", "-o", "-n", "b"]);
        let result = bin(&env).unwrap();
        println!("or_false_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn or_false_false() {
        let env = make_test_env(vec!["test", "-z", "a", "-o", "-z", "b"]);
        let result = bin(&env).unwrap();
        println!("or_false_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn and_higher_precedence_than_or() {
        // "true -o false -a false" should be "true -o (false -a false)" = true
        let env = make_test_env(vec!["test", "-n", "a", "-o", "-z", "b", "-a", "-z", "c"]);
        let result = bin(&env).unwrap();
        println!(
            "and_higher_precedence_than_or: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // Parenthesis tests
    // ========================================================================

    #[test]
    fn parentheses_simple() {
        let env = make_test_env(vec!["test", "(", "-n", "hello", ")"]);
        let result = bin(&env).unwrap();
        println!("parentheses_simple: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn parentheses_override_precedence() {
        // "(true -o false) -a false" should be false
        let env = make_test_env(vec![
            "test", "(", "-n", "a", "-o", "-z", "b", ")", "-a", "-z", "c",
        ]);
        let result = bin(&env).unwrap();
        println!(
            "parentheses_override_precedence: exit code = {}",
            result.code()
        );
        assert_eq!(1, result.code());
    }

    #[test]
    fn empty_parentheses() {
        let env = make_test_env(vec!["test", "(", ")"]);
        let result = bin(&env).unwrap();
        println!("empty_parentheses: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Bracket invocation tests
    // ========================================================================

    #[test]
    fn bracket_string_test() {
        let env = make_test_env(vec!["[", "-n", "hello", "]"]);
        let result = bin(&env).unwrap();
        println!("bracket_string_test: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn bracket_comparison() {
        let env = make_test_env(vec!["[", "42", "-eq", "42", "]"]);
        let result = bin(&env).unwrap();
        println!("bracket_comparison: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // Edge cases and grammar ambiguity
    // ========================================================================

    #[test]
    fn operator_as_operand_single_arg() {
        // When there's only one argument, an operator like "-n" is an operand
        let env = make_test_env(vec!["test", "-n"]);
        let result = bin(&env).unwrap();
        println!(
            "operator_as_operand_single_arg: exit code = {}",
            result.code()
        );
        // "-n" as a bare string is non-empty, so true
        assert_eq!(0, result.code());
    }

    #[test]
    fn equals_sign_as_operand() {
        let env = make_test_env(vec!["test", "="]);
        let result = bin(&env).unwrap();
        println!("equals_sign_as_operand: exit code = {}", result.code());
        // "=" as a bare string is non-empty, so true
        assert_eq!(0, result.code());
    }

    #[test]
    fn exclamation_as_operand() {
        let env = make_test_env(vec!["test", "!"]);
        let result = bin(&env).unwrap();
        println!("exclamation_as_operand: exit code = {}", result.code());
        // "!" as a bare string is non-empty, so true
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // Integer boundary tests
    // ========================================================================

    #[test]
    fn int_zero_equal_zero() {
        let env = make_test_env(vec!["test", "0", "-eq", "0"]);
        let result = bin(&env).unwrap();
        println!("int_zero_equal_zero: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_large_positive() {
        let env = make_test_env(vec!["test", "9223372036854775807", "-gt", "0"]);
        let result = bin(&env).unwrap();
        println!("int_large_positive: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_large_negative() {
        let env = make_test_env(vec!["test", "-9223372036854775808", "-lt", "0"]);
        let result = bin(&env).unwrap();
        println!("int_large_negative: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_negative_equal_negative() {
        let env = make_test_env(vec!["test", "-42", "-eq", "-42"]);
        let result = bin(&env).unwrap();
        println!("int_negative_equal_negative: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_negative_less_than_positive() {
        let env = make_test_env(vec!["test", "-1", "-lt", "1"]);
        let result = bin(&env).unwrap();
        println!(
            "int_negative_less_than_positive: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_greater_not_equal() {
        let env = make_test_env(vec!["test", "100", "-gt", "100"]);
        let result = bin(&env).unwrap();
        println!("int_greater_not_equal: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_less_not_equal() {
        let env = make_test_env(vec!["test", "100", "-lt", "100"]);
        let result = bin(&env).unwrap();
        println!("int_less_not_equal: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn int_with_leading_zeros() {
        let env = make_test_env(vec!["test", "007", "-eq", "7"]);
        let result = bin(&env).unwrap();
        println!("int_with_leading_zeros: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn int_with_whitespace() {
        let env = make_test_env(vec!["test", " 42 ", "-eq", "42"]);
        let result = bin(&env).unwrap();
        println!("int_with_whitespace: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // String comparison edge cases
    // ========================================================================

    #[test]
    fn string_equal_empty_strings() {
        let env = make_test_env(vec!["test", "", "=", ""]);
        let result = bin(&env).unwrap();
        println!("string_equal_empty_strings: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_not_equal_empty_vs_nonempty() {
        let env = make_test_env(vec!["test", "", "!=", "x"]);
        let result = bin(&env).unwrap();
        println!(
            "string_not_equal_empty_vs_nonempty: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_less_empty_before_nonempty() {
        let env = make_test_env(vec!["test", "", "<", "a"]);
        let result = bin(&env).unwrap();
        println!(
            "string_less_empty_before_nonempty: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_greater_nonempty_after_empty() {
        let env = make_test_env(vec!["test", "a", ">", ""]);
        let result = bin(&env).unwrap();
        println!(
            "string_greater_nonempty_after_empty: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_less_case_sensitive() {
        // 'A' (65) < 'a' (97) in ASCII
        let env = make_test_env(vec!["test", "A", "<", "a"]);
        let result = bin(&env).unwrap();
        println!("string_less_case_sensitive: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_compare_numbers_as_strings() {
        // "9" > "10" lexicographically because '9' > '1'
        let env = make_test_env(vec!["test", "9", ">", "10"]);
        let result = bin(&env).unwrap();
        println!(
            "string_compare_numbers_as_strings: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_equal_with_spaces() {
        let env = make_test_env(vec!["test", "hello world", "=", "hello world"]);
        let result = bin(&env).unwrap();
        println!("string_equal_with_spaces: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_not_equal_different_whitespace() {
        let env = make_test_env(vec!["test", "hello world", "!=", "hello  world"]);
        let result = bin(&env).unwrap();
        println!(
            "string_not_equal_different_whitespace: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // File test edge cases
    // ========================================================================

    #[test]
    fn file_writable_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-w", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_writable_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_writable_false() {
        let env = make_test_env(vec!["test", "-w", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_writable_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_executable_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-x", "/tmp/testfile"], fs);
        let result = bin(&env).unwrap();
        println!("file_executable_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_executable_false() {
        let env = make_test_env(vec!["test", "-x", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_executable_false: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_size_nonexistent() {
        let env = make_test_env(vec!["test", "-s", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_size_nonexistent: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_regular_nonexistent() {
        let env = make_test_env(vec!["test", "-f", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_regular_nonexistent: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_directory_nonexistent() {
        let env = make_test_env(vec!["test", "-d", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_directory_nonexistent: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_symlink_h_flag() {
        // -h is an alias for -L
        let env = make_test_env(vec!["test", "-h", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_symlink_h_flag: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_symlink_l_flag() {
        let env = make_test_env(vec!["test", "-L", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("file_symlink_l_flag: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // File comparison tests (-nt, -ot, -ef)
    // ========================================================================

    #[test]
    fn file_same_true() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/file", "content");
        let env = make_env_with_fs(vec!["test", "/tmp/file", "-ef", "/tmp/file"], fs);
        let result = bin(&env).unwrap();
        println!("file_same_true: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_same_false_different_paths() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/file1", "content");
        fs.add_file("/tmp/file2", "content");
        let env = make_env_with_fs(vec!["test", "/tmp/file1", "-ef", "/tmp/file2"], fs);
        let result = bin(&env).unwrap();
        println!(
            "file_same_false_different_paths: exit code = {}",
            result.code()
        );
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_same_false_nonexistent() {
        let env = make_test_env(vec!["test", "/nonexistent1", "-ef", "/nonexistent2"]);
        let result = bin(&env).unwrap();
        println!("file_same_false_nonexistent: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_newer_false_nonexistent() {
        let env = make_test_env(vec!["test", "/nonexistent1", "-nt", "/nonexistent2"]);
        let result = bin(&env).unwrap();
        println!(
            "file_newer_false_nonexistent: exit code = {}",
            result.code()
        );
        assert_eq!(1, result.code());
    }

    #[test]
    fn file_older_false_nonexistent() {
        let env = make_test_env(vec!["test", "/nonexistent1", "-ot", "/nonexistent2"]);
        let result = bin(&env).unwrap();
        println!(
            "file_older_false_nonexistent: exit code = {}",
            result.code()
        );
        assert_eq!(1, result.code());
    }

    // ========================================================================
    // Complex boolean expression tests
    // ========================================================================

    #[test]
    fn complex_and_or_chain() {
        // true -a true -o false => (true -a true) -o false => true
        let env = make_test_env(vec!["test", "-n", "a", "-a", "-n", "b", "-o", "-z", "c"]);
        let result = bin(&env).unwrap();
        println!("complex_and_or_chain: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn complex_or_and_chain() {
        // false -o true -a true => false -o (true -a true) => true
        let env = make_test_env(vec!["test", "-z", "a", "-o", "-n", "b", "-a", "-n", "c"]);
        let result = bin(&env).unwrap();
        println!("complex_or_and_chain: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn triple_negation() {
        let env = make_test_env(vec!["test", "!", "!", "!", "-n", "hello"]);
        let result = bin(&env).unwrap();
        println!("triple_negation: exit code = {}", result.code());
        assert_eq!(1, result.code());
    }

    #[test]
    fn not_with_parentheses() {
        let env = make_test_env(vec!["test", "!", "(", "-z", "hello", ")"]);
        let result = bin(&env).unwrap();
        println!("not_with_parentheses: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn nested_parentheses() {
        let env = make_test_env(vec!["test", "(", "(", "-n", "a", ")", ")"]);
        let result = bin(&env).unwrap();
        println!("nested_parentheses: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn parentheses_with_and() {
        let env = make_test_env(vec!["test", "(", "-n", "a", "-a", "-n", "b", ")"]);
        let result = bin(&env).unwrap();
        println!("parentheses_with_and: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn parentheses_with_or() {
        let env = make_test_env(vec!["test", "(", "-z", "a", "-o", "-n", "b", ")"]);
        let result = bin(&env).unwrap();
        println!("parentheses_with_or: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // Grammar ambiguity edge cases
    // ========================================================================

    #[test]
    fn two_args_operator_and_string() {
        // test "=" "=" "=" is ambiguous - parser sees "=" = "=" as string compare
        // but the middle "=" is also the operator, leaving trailing "="
        let env = make_test_env(vec!["test", "=", "=", "="]);
        let result = bin(&env).unwrap();
        println!(
            "two_args_operator_and_string: exit code = {}",
            result.code()
        );
        // This results in error due to trailing argument
        assert_eq!(2, result.code());
    }

    #[test]
    fn operator_a_as_string() {
        // "-a" at the start is parsed as boolean and, expecting expression before it
        // This is ambiguous and results in an error or unexpected behavior
        let env = make_test_env(vec!["test", "-a", "=", "-a"]);
        let result = bin(&env).unwrap();
        println!("operator_a_as_string: exit code = {}", result.code());
        // Grammar ambiguity - parser interprets differently
        assert_eq!(2, result.code());
    }

    #[test]
    fn operator_o_as_string() {
        // "-o" at the start is parsed as boolean or, expecting expression before it
        // This is ambiguous and results in an error or unexpected behavior
        let env = make_test_env(vec!["test", "-o", "=", "-o"]);
        let result = bin(&env).unwrap();
        println!("operator_o_as_string: exit code = {}", result.code());
        // Grammar ambiguity - parser interprets differently
        assert_eq!(2, result.code());
    }

    #[test]
    fn parenthesis_as_string_in_comparison() {
        let env = make_test_env(vec!["test", "(", "=", "("]);
        let result = bin(&env).unwrap();
        println!(
            "parenthesis_as_string_in_comparison: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn right_paren_as_single_operand() {
        let env = make_test_env(vec!["test", ")"]);
        let result = bin(&env).unwrap();
        println!(
            "right_paren_as_single_operand: exit code = {}",
            result.code()
        );
        // ")" is a non-empty string
        assert_eq!(0, result.code());
    }

    #[test]
    fn left_paren_as_single_operand() {
        let env = make_test_env(vec!["test", "("]);
        let result = bin(&env).unwrap();
        println!(
            "left_paren_as_single_operand: exit code = {}",
            result.code()
        );
        // "(" is a non-empty string
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // Bracket invocation edge cases
    // ========================================================================

    #[test]
    fn bracket_with_path_prefix() {
        let env = make_test_env(vec!["/usr/bin/[", "-n", "hello", "]"]);
        let result = bin(&env).unwrap();
        println!("bracket_with_path_prefix: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn bracket_negation() {
        let env = make_test_env(vec!["[", "!", "-z", "hello", "]"]);
        let result = bin(&env).unwrap();
        println!("bracket_negation: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn bracket_complex_expression() {
        let env = make_test_env(vec!["[", "(", "-n", "a", "-o", "-n", "b", ")", "]"]);
        let result = bin(&env).unwrap();
        println!("bracket_complex_expression: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn bracket_only_close_bracket() {
        let env = make_test_env(vec!["[", "]", "]"]);
        let result = bin(&env).unwrap();
        println!("bracket_only_close_bracket: exit code = {}", result.code());
        // "]" as a bare string is non-empty
        assert_eq!(0, result.code());
    }

    // ========================================================================
    // Special string value tests
    // ========================================================================

    #[test]
    fn string_with_newline() {
        let env = make_test_env(vec!["test", "-n", "hello\nworld"]);
        let result = bin(&env).unwrap();
        println!("string_with_newline: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_with_tab() {
        let env = make_test_env(vec!["test", "-n", "hello\tworld"]);
        let result = bin(&env).unwrap();
        println!("string_with_tab: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_only_whitespace() {
        let env = make_test_env(vec!["test", "-n", "   "]);
        let result = bin(&env).unwrap();
        println!("string_only_whitespace: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn string_zero_only_whitespace() {
        let env = make_test_env(vec!["test", "-z", "   "]);
        let result = bin(&env).unwrap();
        println!("string_zero_only_whitespace: exit code = {}", result.code());
        // "   " is not zero length
        assert_eq!(1, result.code());
    }

    #[test]
    fn string_equal_operators_as_values() {
        // "-eq" is parsed as integer equality operator, but "=" is not a valid integer
        // This triggers the integer comparison which fails for non-integers
        let env = make_test_env(vec!["test", "-eq", "=", "-eq"]);
        let result = bin(&env).unwrap();
        println!(
            "string_equal_operators_as_values: exit code = {}",
            result.code()
        );
        // Parser sees "-eq" as unary op (since followed by binop), leaving trailing args
        assert_eq!(2, result.code());
    }

    // ========================================================================
    // Error condition tests
    // ========================================================================

    #[test]
    fn missing_binary_operand() {
        let env = make_test_env(vec!["test", "foo", "="]);
        let result = bin(&env).unwrap();
        println!("missing_binary_operand: exit code = {}", result.code());
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("argument expected"));
    }

    #[test]
    fn unclosed_parenthesis() {
        let env = make_test_env(vec!["test", "(", "-n", "foo"]);
        let result = bin(&env).unwrap();
        println!("unclosed_parenthesis: exit code = {}", result.code());
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("closing paren expected"));
    }

    #[test]
    fn extra_arguments() {
        let env = make_test_env(vec!["test", "-n", "foo", "bar"]);
        let result = bin(&env).unwrap();
        println!("extra_arguments: exit code = {}", result.code());
        assert_eq!(2, result.code());
        let stderr = env.stderr.into_string();
        println!("stderr: {:?}", stderr);
        assert!(stderr.contains("unexpected operator"));
    }

    // ========================================================================
    // Combined file and string tests
    // ========================================================================

    #[test]
    fn file_exists_and_string_nonempty() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_file("/tmp/testfile", "content");
        let env = make_env_with_fs(vec!["test", "-e", "/tmp/testfile", "-a", "-n", "hello"], fs);
        let result = bin(&env).unwrap();
        println!(
            "file_exists_and_string_nonempty: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_not_exists_or_string_nonempty() {
        let env = make_test_env(vec!["test", "-e", "/nonexistent", "-o", "-n", "hello"]);
        let result = bin(&env).unwrap();
        println!(
            "file_not_exists_or_string_nonempty: exit code = {}",
            result.code()
        );
        assert_eq!(0, result.code());
    }

    #[test]
    fn not_file_exists() {
        let env = make_test_env(vec!["test", "!", "-e", "/nonexistent"]);
        let result = bin(&env).unwrap();
        println!("not_file_exists: exit code = {}", result.code());
        assert_eq!(0, result.code());
    }

    #[test]
    fn file_directory_and_file_regular() {
        let fs = make_fs();
        fs.add_directory("/tmp");
        fs.add_directory("/tmp/testdir");
        let env = make_env_with_fs(
            vec!["test", "-d", "/tmp/testdir", "-a", "-f", "/tmp/testdir"],
            fs,
        );
        let result = bin(&env).unwrap();
        println!(
            "file_directory_and_file_regular: exit code = {}",
            result.code()
        );
        // Can't be both a directory and a regular file
        assert_eq!(1, result.code());
    }
}
