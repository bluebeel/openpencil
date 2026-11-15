use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use op_ai::chat_provider::{ChatDelta, ChatProvider, ChatRequest, StopReason};
use op_editor_core::{HostedModelRoute, HostedProtocol};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::{HostedRuntimeError, Session};

pub(super) struct HostedProvider {
    pub session: Session,
    pub epoch: u64,
    pub subject: String,
    pub route: HostedModelRoute,
}

impl ChatProvider for HostedProvider {
    fn provider_label(&self) -> &str {
        self.route.display_name()
    }
    fn hosted_session_epoch(&self) -> Option<u64> {
        Some(self.session.auth_epoch)
    }
    fn supports_cancellable_send(&self) -> bool {
        true
    }
    fn supports_evidence_only_send(&self) -> bool {
        true
    }
    fn send(&self, request: ChatRequest) -> Box<dyn Iterator<Item = ChatDelta> + Send> {
        self.send_cancellable(request, Arc::new(AtomicBool::new(false)))
    }
    fn send_cancellable(
        &self,
        request: ChatRequest,
        cancel: Arc<AtomicBool>,
    ) -> Box<dyn Iterator<Item = ChatDelta> + Send> {
        if cancel.load(Ordering::Acquire) {
            return Box::new(
                [ChatDelta::Done {
                    stop_reason: StopReason::Aborted,
                }]
                .into_iter(),
            );
        }
        // Reject unsupported input before private authorization or any network
        // work. The caller can retain its draft and choose a compatible model.
        let (path, body) = match request_body(&self.route, request) {
            Ok(body) => body,
            Err(error) => {
                return Box::new([ChatDelta::Error(error.to_string())].into_iter());
            }
        };
        let session = self.session.clone();
        let epoch = self.epoch;
        let subject = self.subject.clone();
        let route = self.route.clone();
        let (tx, rx) = mpsc::channel(64);
        let worker_cancel = cancel.clone();
        let task = op_chat_agent::runtime::shared_runtime().spawn(async move {
            let operation = async {
                if worker_cancel.load(Ordering::Acquire) {
                    return Err(HostedRuntimeError::Cancelled);
                }
                let access = super::access_for(&session, epoch).await?;
                if access.subject() != subject {
                    return Err(HostedRuntimeError::SessionChanged);
                }
                super::check_session(&session, epoch)?;
                // Catalog membership is checked again immediately before the paid request.
                {
                    let state = super::STATE.lock().unwrap_or_else(|p| p.into_inner());
                    if !state.routes.iter().any(|candidate| candidate == &route) {
                        return Err(HostedRuntimeError::RouteUnavailable);
                    }
                }
                if worker_cancel.load(Ordering::Acquire) {
                    return Err(HostedRuntimeError::Cancelled);
                }
                let client = super::client()?;
                let mut builder = client
                    .post(format!("{}{path}", access.gateway_base_url()))
                    .bearer_auth(access.bearer())
                    .json(&body);
                if route.protocol() == HostedProtocol::AnthropicMessages {
                    builder = builder.header("anthropic-version", "2023-06-01");
                }
                let response = builder
                    .send()
                    .await
                    .map_err(|_| HostedRuntimeError::Network)?;
                if !response.status().is_success() {
                    return Err(HostedRuntimeError::GatewayRejected);
                }
                super::stream::pump(response, &tx, route.protocol()).await
            };
            let result = tokio::select! {
                result = operation => result,
                _ = tx.closed() => return,
                error = watch_session(&session, epoch) => Err(error),
                _ = watch_cancel(&worker_cancel) => Err(HostedRuntimeError::Cancelled),
            };
            match result {
                Ok(true) => {}
                Ok(false) => {
                    let _ = tx
                        .send(ChatDelta::Error(
                            "Hosted gateway stream ended before completion".into(),
                        ))
                        .await;
                }
                Err(error) => {
                    let _ = tx.send(ChatDelta::Error(error.to_string())).await;
                }
            }
        });
        Box::new(op_chat_agent::blocking_recv::BlockingRecvIter::cancellable(
            rx, cancel, task,
        ))
    }
}

async fn watch_cancel(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub(super) async fn watch_session(session: &Session, epoch: u64) -> HostedRuntimeError {
    loop {
        if let Err(error) = super::check_session(session, epoch) {
            return error;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub(super) fn request_body(
    route: &HostedModelRoute,
    request: ChatRequest,
) -> Result<(&'static str, Value), HostedRuntimeError> {
    if !request.attachments.is_empty() {
        return Err(HostedRuntimeError::UnsupportedAttachments);
    }
    let mut system_prompt = request.system_prompt;
    let has_system = !system_prompt.trim().is_empty();
    let openai_system = has_system && route.protocol() == HostedProtocol::OpenAiCompatible;
    let mut messages = Vec::with_capacity(
        request
            .history
            .len()
            .saturating_add(1 + usize::from(openai_system)),
    );
    if openai_system {
        messages.push(message("system", std::mem::take(&mut system_prompt)));
    }
    for (role, content) in request.history {
        messages.push(message(role.as_str(), content));
    }
    messages.push(message("user", request.user_message));
    let mut body = serde_json::Map::new();
    body.insert("model".into(), Value::String(route.model_id().into()));
    body.insert("stream".into(), Value::Bool(true));
    body.insert(
        "max_tokens".into(),
        request.max_output_tokens.clamp(1, 65536).into(),
    );
    body.insert("messages".into(), Value::Array(messages));
    let mut body = Value::Object(body);
    let path = match route.protocol() {
        HostedProtocol::AnthropicMessages => {
            if has_system {
                body["system"] = Value::String(system_prompt);
            }
            "/messages"
        }
        HostedProtocol::OpenAiCompatible => {
            body["stream_options"] = json!({"include_usage": true});
            op_chat_agent::backoff::apply_reasoning_wire_control(
                &mut body,
                route.model_id(),
                request.thinking == op_ai::chat_provider::ThinkingMode::Disabled,
            );
            "/chat/completions"
        }
        HostedProtocol::OpenAiResponses => return Err(HostedRuntimeError::UnsupportedProtocol),
    };
    Ok((path, body))
}

fn message(role: &str, content: String) -> Value {
    let mut message = serde_json::Map::new();
    message.insert("role".into(), Value::String(role.into()));
    message.insert("content".into(), Value::String(content));
    Value::Object(message)
}
