//! The one-shot HTTP side door that sits next to /mcp.
//!
//! POST /call/<tool> with the JSON arguments as the body, an empty body meaning
//! no arguments. Same tools as MCP without the session handshake, so `smon call`
//! and curl stay one-liners.

use std::sync::Arc;

use axum::{
    Json as HttpJson,
    extract::{Path as UrlPath, State},
    http::StatusCode,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    console::Console,
    mcp::{
        AdoptReq, Cursor, ExpectReq, ExpectResult, ReadReq, ReadResult, RollReq, SendCtrlReq,
        SendReq, SnapshotReq, Which, log_result, status_of,
    },
    registry::Registry,
};

type CallError = (StatusCode, String);

pub async fn call_http(
    UrlPath(tool): UrlPath<String>,
    State(registry): State<Arc<Registry>>,
    body: String,
) -> Result<HttpJson<Value>, CallError> {
    let args = if body.trim().is_empty() { "{}" } else { body.as_str() };
    match tool.as_str() {
        "console_list" => {
            let all: Vec<_> = registry.all().iter().map(status_of).collect();
            respond(all)
        }
        "serial_send" => {
            let req: SendReq = parse_args(args)?;
            let console = resolve(&registry, req.console.as_deref())?;
            let cursor = console.send(req.text, req.newline).await.map_err(internal)?;
            respond(Cursor { cursor })
        }
        "serial_send_ctrl" => {
            let req: SendCtrlReq = parse_args(args)?;
            let console = resolve(&registry, req.console.as_deref())?;
            let ch = req
                .ctrl
                .chars()
                .next()
                .ok_or_else(|| bad_request("ctrl must be one character".to_string()))?;
            let cursor = console.send_ctrl(ch).await.map_err(internal)?;
            respond(Cursor { cursor })
        }
        "serial_read" => {
            let req: ReadReq = parse_args(args)?;
            let (data, cursor) = resolve(&registry, req.console.as_deref())?.read(req.cursor);
            respond(ReadResult { data, cursor })
        }
        "serial_expect" => {
            let req: ExpectReq = parse_args(args)?;
            let out = resolve(&registry, req.console.as_deref())?
                .expect(&req.pattern, req.timeout_ms, req.regex, req.cursor)
                .await
                .map_err(bad_request)?;
            respond(ExpectResult {
                matched:   out.matched,
                data:      out.data,
                cursor:    out.cursor,
                timed_out: out.timed_out,
            })
        }
        "serial_snapshot" => {
            let req: SnapshotReq = parse_args(args)?;
            respond(resolve(&registry, req.console.as_deref())?.snapshot(req.lines))
        }
        "serial_status" => {
            let req: Which = parse_args(args)?;
            respond(status_of(&resolve(&registry, req.console.as_deref())?))
        }
        "log_roll" => {
            let req: RollReq = parse_args(args)?;
            let info = resolve(&registry, req.console.as_deref())?
                .log_roll(req.tag.as_deref())
                .map_err(|e| internal(e.to_string()))?;
            respond(log_result(&info))
        }
        "console_adopt" => {
            let req: AdoptReq = parse_args(args)?;
            let console = registry.adopt(req.into()).map_err(bad_request)?;
            respond(status_of(&console))
        }
        "console_release" => {
            let req: Which = parse_args(args)?;
            let console = resolve(&registry, req.console.as_deref())?;
            if !console.release().await {
                return Err(internal(format!(
                    "{} did not let go of its device",
                    console.name()
                )));
            }
            respond(status_of(&console))
        }
        "console_hold" => {
            let req: Which = parse_args(args)?;
            let console = resolve(&registry, req.console.as_deref())?;
            console.hold();
            respond(status_of(&console))
        }
        "log_info" => {
            let req: Which = parse_args(args)?;
            let info = resolve(&registry, req.console.as_deref())?.log_info();
            respond(log_result(&info))
        }
        other => Err((StatusCode::NOT_FOUND, format!("unknown tool '{other}'"))),
    }
}

fn resolve(registry: &Registry, name: Option<&str>) -> Result<Arc<Console>, CallError> {
    registry.resolve(name).map_err(bad_request)
}

fn bad_request(e: String) -> CallError {
    (StatusCode::BAD_REQUEST, e)
}

fn internal(e: String) -> CallError {
    (StatusCode::INTERNAL_SERVER_ERROR, e)
}

fn parse_args<T: DeserializeOwned>(args: &str) -> Result<T, CallError> {
    serde_json::from_str(args).map_err(|e| bad_request(e.to_string()))
}

fn respond<T: Serialize>(value: T) -> Result<HttpJson<Value>, CallError> {
    serde_json::to_value(value)
        .map(HttpJson)
        .map_err(|e| internal(e.to_string()))
}
