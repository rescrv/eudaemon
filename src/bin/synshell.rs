//! Interactive REPL for synshell.

use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use synshell::Environment;
use synshell::Shell;

fn main() {
    let env: Environment<_, _, _> = Environment::default();
    let mut shell = Shell::new(env);
    let mut rl = DefaultEditor::new().expect("Failed to create editor");

    loop {
        match rl.readline("synshell> ") {
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str());
                match shell.run(line) {
                    Ok(exit_code) => {
                        if exit_code.code() != 0 {
                            eprintln!("process exited: {}", exit_code.code());
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {:?}", e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(err) => {
                eprintln!("error: {:?}", err);
                break;
            }
        }
    }
}
