use claudius::{
    Anthropic, ContentBlock, MessageCreateParams, MessageParam, Model, ToolChoice, ToolParam,
    ToolUnionParam,
};
use serde_json::{Value, json};
use std::io::{self, BufRead};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let anthropic = Anthropic::new(None)?;

    let stdin = io::stdin();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let input: Value = serde_json::from_str(&line)?;
        let text = input
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("Input must have a 'text' field")?;

        let metadata = call_llm_with_tool(&anthropic, text).await?;

        let mut output = input.clone();
        output["meta"] = metadata;

        println!("{}", serde_json::to_string(&output)?);
    }

    Ok(())
}

async fn call_llm_with_tool(
    anthropic: &Anthropic,
    text: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let tool = ToolParam::new(
        "doit".to_string(),
        json!({
            "type": "object",
            "properties": {
              "metadata": {
                "type": "object",
              },
            },
            "required": ["metadata"]
        }),
    )
    .with_description("doit".to_string());

    let params = MessageCreateParams::new(
        4096,
        vec![MessageParam::user(format!(
            "Extract metadata from this writing:

<writing>
{text}
</writing>

Remember: Extract metadata from the writing above.
"
        ))],
        Model::Custom("claude-sonnet-4-5-20250929".to_string()),
    )
    .with_tools(vec![ToolUnionParam::CustomTool(tool)])
    .with_tool_choice(ToolChoice::tool("doit"));

    let response = anthropic.send(params).await?;

    for content_block in &response.content {
        if let ContentBlock::ToolUse(tool_use) = content_block
            && tool_use.name == "doit"
            && let Some(metadata) = tool_use.input.get("metadata")
        {
            return Ok(metadata.clone());
        }
    }

    Err("No tool_use block found in response".into())
}
