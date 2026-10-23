//! Official rmcp server. Domain objects are returned directly as structuredContent.
use crate::{
    Plugin,
    contracts::{self, ContractError},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt, model::*, service::RequestContext,
    transport::async_rw::AsyncRwTransport,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub const MAX_FRAME_BYTES: usize = 131_072;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Docking,
    Cli,
}
#[derive(Clone)]
pub struct McpServer {
    plugin: Plugin,
    permits: Arc<tokio::sync::Semaphore>,
    decisions: Arc<tokio::sync::Semaphore>,
    profile: Profile,
}
impl McpServer {
    pub fn new(plugin: Plugin) -> Self {
        Self::with_profile(plugin, Profile::Docking)
    }
    pub fn with_profile(plugin: Plugin, profile: Profile) -> Self {
        Self {
            plugin,
            permits: Arc::new(tokio::sync::Semaphore::new(if profile == Profile::Cli {
                4
            } else {
                1
            })),
            decisions: Arc::new(tokio::sync::Semaphore::new(1)),
            profile,
        }
    }
    async fn invoke(&self, operation: String, request: Value) -> CallToolResponse {
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            return CallToolResult::structured_error(json!({"error":{"code":"busy","message":"Plugin call limit reached; wait for pending calls to settle."}})).into();
        };
        let decision = if operation == "decide" {
            match self.decisions.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => return CallToolResult::structured_error(json!({"error":{"code":"busy","message":"A decision is in progress; wait for it to settle."}})).into(),
            }
        } else {
            None
        };
        let plugin = self.plugin.clone();
        match tokio::task::spawn_blocking(move || {
            let (_permit, _decision) = (permit, decision);
            plugin.invoke(&operation, &request)
        })
        .await
        {
            Ok(Ok(value)) => CallToolResult::structured(value).into(),
            Ok(Err(e)) => error(e),
            Err(_) => error(ContractError::JudgeFailed),
        }
    }
}
fn tools() -> Vec<Tool> {
    vec![Tool::new("describe","Describe docking API 1 and its schemas.",json!({"type":"object","properties":{},"additionalProperties":false}).as_object().unwrap().clone()),
        Tool::new("invoke","Return a proposal from decide, validate_plan or feedback; never execute host work.",
            json!({"type":"object","properties":{"operation":{"type":"string","enum":["decide","validate_plan","feedback"]},
                "request":{"type":"object"}},"required":["operation","request"],"additionalProperties":false}).as_object().unwrap().clone())]
}
fn error(e: ContractError) -> CallToolResponse {
    CallToolResult::structured_error(json!({"error":{"code":e.code(),"message":e.to_string()}}))
        .into()
}
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("Switchbite", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(if self.profile == Profile::Cli {
            "Use switchbite_decide to propose worker assignments, switchbite_validate_plan for supplied DAGs, switchbite_feedback for outcome proposals, and switchbite_status for configuration presence. Never execute or claim completion based only on a proposal."
        } else { "Proposal-only docking API 1. Host owns authorization and execution." }.into());
        info
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: if self.profile == Profile::Cli {
                crate::cli_tools::tools()
            } else {
                tools()
            },
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        call: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Value::Object(call.arguments.unwrap_or_default());
        if self.profile == Profile::Cli {
            if call.name == "switchbite_status" {
                return Ok(if args.as_object().unwrap().is_empty() {
                    CallToolResult::structured(crate::cli_tools::status(&self.plugin)).into()
                } else {
                    error(ContractError::InvalidRequest)
                });
            }
            let Some((_, operation, _)) = crate::cli_tools::OPERATIONS
                .iter()
                .find(|(name, _, _)| *name == call.name.as_ref())
            else {
                return Ok(error(ContractError::UnknownOperation));
            };
            let request = match crate::cli_tools::normalize(operation, &args) {
                Ok(v) => v,
                Err(e) => return Ok(error(e)),
            };
            return Ok(self.invoke((*operation).into(), request).await);
        }
        match call.name.as_ref() {
            "describe" if args.as_object().unwrap().is_empty() => {
                Ok(CallToolResult::structured(self.plugin.describe()).into())
            }
            "invoke" => {
                if args.as_object().unwrap().len() != 2
                    || !args["operation"].is_string()
                    || !args["request"].is_object()
                {
                    return Ok(error(ContractError::InvalidRequest));
                }
                let op = args["operation"].as_str().unwrap().to_owned();
                let request = args["request"].clone();
                Ok(self.invoke(op, request).await)
            }
            "describe" => Ok(error(ContractError::InvalidRequest)),
            _ => Ok(error(ContractError::UnknownOperation)),
        }
    }
}

/// Only framing/duplicate-member protection precedes rmcp. rmcp owns JSON-RPC,
/// protocol negotiation, initialize/initialized, tool dispatch and responses.
async fn guarded_input<R: AsyncRead + Unpin>(
    reader: R,
    mut writer: impl AsyncWrite + Unpin,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            break;
        }
        let len = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if line.len() + len > MAX_FRAME_BYTES {
            break;
        }
        line.extend_from_slice(&available[..len]);
        reader.consume(len);
        if line.last() == Some(&b'\n') {
            if contracts::parse_json(&line, MAX_FRAME_BYTES).is_err() {
                break;
            }
            writer.write_all(&line).await?;
            line.clear();
        }
    }
    writer.shutdown().await
}
pub async fn serve<R, W>(
    plugin: Plugin,
    reader: R,
    writer: W,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    serve_with_profile(plugin, reader, writer, Profile::Docking).await
}
pub async fn serve_with_profile<R, W>(
    plugin: Plugin,
    reader: R,
    writer: W,
    profile: Profile,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let (checked, forward) = tokio::io::duplex(MAX_FRAME_BYTES);
    let guard = tokio::spawn(guarded_input(reader, forward));
    let transport = AsyncRwTransport::<RoleServer, _, _>::new(checked, writer)
        .with_max_line_length(MAX_FRAME_BYTES);
    let result = async {
        McpServer::with_profile(plugin, profile)
            .serve(transport)
            .await?
            .waiting()
            .await?;
        Ok(())
    }
    .await;
    guard.abort();
    result
}
pub async fn stdio(plugin: Plugin) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serve(plugin, tokio::io::stdin(), tokio::io::stdout()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    fn value(response: CallToolResponse) -> Value {
        match response {
            CallToolResponse::Complete(result) => serde_json::to_value(result).unwrap(),
            _ => panic!("Unexpected non-final tool response"),
        }
    }
    #[tokio::test]
    async fn cli_readonly_feedback_runs_during_a_decision_without_parallel_provider_work() {
        let (entered, wait_entered) = mpsc::channel();
        let (release, wait_release) = mpsc::channel();
        let wait_release = Mutex::new(wait_release);
        let judge = move |_: &Value| {
            let _ = entered.send(());
            let _ = wait_release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3));
            Err(ContractError::JudgeFailed)
        };
        let server = Arc::new(McpServer::with_profile(
            Plugin::with_judge(judge),
            Profile::Cli,
        ));
        let request = json!({"version":1,"task":{"id":"t","objective":"Translate a reply","requirements":[],"context_refs":[],"extensions":{}},
            "candidates":[{"id":"translator","description":"Translates replies","capabilities":[],"available":true,"extensions":{}}],"preferences":"Preserve tone"});
        let running = {
            let server = server.clone();
            let request = request.clone();
            tokio::spawn(async move { server.invoke("decide".into(), request).await })
        };
        tokio::task::spawn_blocking(move || wait_entered.recv_timeout(Duration::from_secs(2)))
            .await
            .unwrap()
            .unwrap();
        let busy = value(server.invoke("decide".into(), request).await);
        assert_eq!(busy["isError"], true);
        assert_eq!(busy["structuredContent"]["error"]["code"], "busy");
        let feedback = value(server.invoke("feedback".into(),json!({"version":1,"task_id":"t","candidate_id":"translator","outcome":"succeeded","evidence":["receipt:observed"]})).await);
        assert_eq!(feedback["isError"], false);
        assert_eq!(feedback["structuredContent"]["operation"], "feedback");
        release.send(()).unwrap();
        assert!(value(running.await.unwrap())["isError"].as_bool().unwrap());
    }
}
