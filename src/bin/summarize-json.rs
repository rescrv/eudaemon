use std::collections::HashMap;

use serde_json::Value;
use std::io::{self, BufRead};

#[derive(Default, Debug)]
struct Accumulator {
    count: u64,
    children: HashMap<String, Accumulator>,
}

impl Accumulator {
    fn add(&mut self, meta: &Value) {
        if let Value::Object(obj) = meta {
            for (key, value) in obj.iter() {
                let child = self.children.entry(key.clone()).or_default();
                child.add(value);
            }
        } else {
            self.count += 1;
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    let mut accumulator = Accumulator::default();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let input: Value = serde_json::from_str(&line)?;
        let meta = input.get("meta").ok_or("Input must have a 'meta' field")?;
        accumulator.add(meta);
    }

    println!("{accumulator:?}");
    Ok(())
}
