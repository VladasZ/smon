use std::{env, net::SocketAddr};

use anyhow::Result;

use crate::cli::{Cli, USAGE};

mod attach;
mod attached;
mod bridge;
mod cli;
mod client;
mod config;
mod console;
mod control;
mod daemon;
mod draw;
mod frame;
mod http;
mod log;
mod mcp;
mod picker;
mod probe;
mod registry;
mod remote;
mod ring;
mod runner;
mod session;
mod settings;
mod tui;
mod tunnel;
mod ui;
mod update;
mod wsl;

fn main() -> Result<()> {
    match cli::parse(env::args().skip(1))? {
        Cli::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Cli::Version => {
            println!("smon {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cli::Call {
            mcp,
            host,
            tool,
            args,
        } => {
            let reached = reach(mcp, host.as_deref())?;
            print_call_result(&client::call(reached.addr, &tool, &args)?);
            Ok(())
        }
        Cli::List { mcp, host } => {
            let reached = reach(mcp, host.as_deref())?;
            client::list(reached.addr)
        }
        Cli::Update { mcp, request } => update::update(&request, mcp),
        Cli::Daemon { config, mcp } => daemon::run(config.as_deref(), mcp),
        Cli::Run { eol, mcp, host } => {
            let reached = reach(mcp, host.as_deref())?;
            tui::run(eol, reached.addr, reached.tunnel.is_some())
        }
    }
}

/// Where to talk to smon, and the tunnel keeping that reachable. The tunnel
/// closes when this is dropped, so it is held for as long as the address is
/// used.
struct Reached {
    addr:   SocketAddr,
    tunnel: Option<tunnel::Tunnel>,
}

fn reach(mcp: SocketAddr, host: Option<&str>) -> Result<Reached> {
    let Some(host) = host else {
        return Ok(Reached {
            addr:   mcp,
            tunnel: None,
        });
    };
    let tunnel = tunnel::open(host, mcp)?;
    Ok(Reached {
        addr:   tunnel.addr(),
        tunnel: Some(tunnel),
    })
}

// A tool result is JSON. Print bare strings raw, snapshot text mostly, and
// everything else pretty, so agents and humans both read it directly.
fn print_call_result(body: &str) {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(serde_json::Value::String(text)) => println!("{text}"),
        Ok(value) => println!("{value:#}"),
        Err(_) => println!("{body}"),
    }
}
