use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::tool_runtime::{IssueFinderToolInvocation, IssueFinderToolRuntime};
use crate::tool_specs::{list_tool_specs_for_profile, ToolProfile};

pub async fn serve_stdio(runtime: IssueFinderToolRuntime, profile: ToolProfile) -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    while let Some(line) = input.next_line().await? {
        let request: Value = serde_json::from_str(&line).context("invalid MCP JSON-RPC message")?;
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let response = match method {
            "initialize" => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "issue-finder", "version": env!("CARGO_PKG_VERSION") }
            }),
            "tools/list" => {
                let specs = list_tool_specs_for_profile(profile);
                json!({
                    "tools": specs.tools.into_iter().map(|spec| json!({
                        "name": transport_name(&spec.name),
                        "description": spec.description,
                        "inputSchema": spec.input_schema,
                    })).collect::<Vec<_>>()
                })
            }
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(Value::Null);
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let semantic = semantic_name(name);
                let invocation = IssueFinderToolInvocation {
                    call_id: format!("mcp:{id}"),
                    turn_id: None,
                    tool_name: semantic,
                    arguments: params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                };
                let result = runtime.execute(invocation).await;
                json!({
                    "content": result.content_items.into_iter().map(|item| match item {
                        crate::tool_runtime::IssueFinderContentItem::InputText { text } => json!({"type":"text","text":text})
                    }).collect::<Vec<_>>(),
                    "structuredContent": result.structured_content,
                    "isError": !result.success,
                })
            }
            _ => {
                write_message(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}),
                )
                .await?;
                continue;
            }
        };
        write_message(
            &mut output,
            &json!({"jsonrpc":"2.0","id":id,"result":response}),
        )
        .await?;
    }
    Ok(())
}

fn transport_name(semantic: &str) -> String {
    semantic
        .replace("issue-finder.", "issue_finder_")
        .replace('-', "_")
}

fn semantic_name(transport: &str) -> String {
    transport
        .strip_prefix("issue_finder_")
        .map(|name| format!("issue-finder.{name}"))
        .unwrap_or_else(|| transport.to_string())
}

async fn write_message(output: &mut tokio::io::Stdout, value: &Value) -> Result<()> {
    output.write_all(value.to_string().as_bytes()).await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    Ok(())
}
