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
    control::Control,
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
    State(control): State<Arc<Control>>,
    body: String,
) -> Result<HttpJson<Value>, CallError> {
    let args = if body.trim().is_empty() { "{}" } else { body.as_str() };
    match tool.as_str() {
        "console_list" => {
            let all: Vec<_> = registry.all().iter().map(status_of).collect();
            respond(all)
        }
        // Deliberately absent from the MCP router. Standing a process down drops
        // every console and every viewer on it, and an agent cannot know that
        // someone is watching one in another terminal.
        "smon_info" => respond(control.info(registry.all().len())),
        "smon_restart" => {
            let info = control.info(registry.all().len());
            control.request_stop();
            respond(info)
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

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        sync::{Arc, mpsc},
        thread::sleep,
        time::{Duration, Instant},
    };


    use crate::{
        client::call,
        console::{Console, ConsoleSpec},
        control::{Control, Info, Role},
        log::ConsoleLog,
        mcp,
        registry::Registry,
        ring::DEFAULT_RING_CAP,
    };

    // The whole update flow rests on this: a running smon must report what it is
    // and then actually let its port go when told, or the replacement process
    // cannot bind and the machine is left with nothing serving.
    #[test]
    fn a_restart_request_reports_the_process_and_frees_the_port() {
        let dir = env::temp_dir().join("smon-restart-test");
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        let log = ConsoleLog::open_in(dir.clone(), "COMTEST", 0, None).unwrap();
        let console = Console::new(
            ConsoleSpec {
                device:   "COMTEST".to_string(),
                label:    Some("under-test".to_string()),
                baud:     9600,
                eol:      b"\r\n".to_vec(),
                ring_cap: DEFAULT_RING_CAP,
                bridge:   None,
            },
            log,
            mpsc::channel().0,
        );
        let registry = Registry::new(vec![Arc::clone(&console)], 0);
        let control = Arc::new(Control::new(Role::Daemon));

        let (ready_tx, ready_rx) = mpsc::channel();
        let server = mcp::spawn(
            "127.0.0.1:0".parse().unwrap(),
            registry,
            Arc::clone(&control),
            ready_tx,
        );
        let addr = ready_rx.recv().unwrap().unwrap();

        let info: Info = serde_json::from_str(&call(addr, "smon_info", "{}").unwrap()).unwrap();
        assert_eq!(info.role, Role::Daemon);
        assert_eq!(info.consoles, 1);
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.pid, std::process::id());

        // Answered before standing down, so the caller learns which process it
        // just stopped rather than losing the reply to the shutdown.
        let told: Info =
            serde_json::from_str(&call(addr, "smon_restart", "{}").unwrap()).unwrap();
        assert_eq!(told.pid, info.pid);
        assert!(control.stopping());

        let deadline = Instant::now() + Duration::from_secs(5);
        while call(addr, "smon_info", "{}").is_ok() {
            assert!(Instant::now() < deadline, "the server never let {addr} go");
            sleep(Duration::from_millis(20));
        }

        drop(server);
        fs::remove_dir_all(&dir).unwrap();
    }
}
