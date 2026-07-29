//! Command line parsing.

use std::{iter::Peekable, net::SocketAddr};

use anyhow::{Context, Result, bail};

use crate::{
    settings::{DEFAULT_BIND, eol_bytes},
    update::Request,
};

pub const USAGE: &str = r#"smon - minimalistic TUI serial monitor

Usage: smon [options]
       smon daemon [--config <path>] [--mcp <host:port>]
       smon list [--host <ssh target>]
       smon call <tool> [json-args] [--host <ssh target>]
       smon update [version] [--from-source] [--yes]

Subcommands:
  daemon                     own every console in the config file and serve them
  list                       list the consoles a running smon owns
  call <tool> [json]         call one tool, for example
                             smon call serial_status
                             smon call serial_send '{"console":"left","text":"reboot"}'
  update [version]           install the newest release, or the version named.
                             This restarts the daemon and kills every other smon
                             running here, so open sessions end.

Options:
  --from-source            build the update with cargo instead of downloading a
                           prebuilt binary
  --yes                    do not ask before killing running smon processes
  --eol <cr|lf|crlf|none>  line ending appended to sent lines, default crlf
  --config <path>          daemon config file, default the first of $SMON_CONFIG,
                           <config dir>/smon/daemon.toml, /etc/smon/daemon.toml
  --mcp <host:port>        endpoint to serve on, or to talk to, default 127.0.0.1:4123.
                           A standalone TUI moves up to the next free port when
                           this one is taken, the daemon does not.
  --host <ssh target>      attach to the smon on another machine, over an ssh
                           tunnel this opens itself, for example --host pi
  -h, --help               show this help
  -V, --version            show the version"#;

#[derive(Debug)]
pub enum Cli {
    Run {
        eol:  Vec<u8>,
        mcp:  SocketAddr,
        /// An ssh target whose smon to attach to, instead of this machine's.
        host: Option<String>,
    },
    Daemon {
        config: Option<String>,
        /// None leaves the choice to the config file.
        mcp:    Option<SocketAddr>,
    },
    List {
        mcp:  SocketAddr,
        host: Option<String>,
    },
    Call {
        mcp:  SocketAddr,
        host: Option<String>,
        tool: String,
        args: String,
    },
    Update {
        /// Where to look for the smon processes this update has to stand down.
        mcp:     SocketAddr,
        request: Request,
    },
    Help,
    Version,
}

#[derive(Default)]
struct Flags {
    eol:         Option<String>,
    mcp:         Option<String>,
    config:      Option<String>,
    host:        Option<String>,
    from_source: bool,
    yes:         bool,
}

/// # Errors
/// Returns an error for an unknown argument, a flag without its value, a flag
/// that means nothing to the chosen subcommand, or an unparsable address.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Cli> {
    let mut flags = Flags::default();
    let mut words: Vec<String> = Vec::new();
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "-V" | "--version" => return Ok(Cli::Version),
            "--eol" => flags.eol = Some(value(&mut args, "--eol")?),
            "--mcp" => flags.mcp = Some(value(&mut args, "--mcp")?),
            "--config" => flags.config = Some(value(&mut args, "--config")?),
            "--host" => flags.host = Some(value(&mut args, "--host")?),
            "--from-source" => flags.from_source = true,
            "--yes" => flags.yes = true,
            other => {
                if let Some(v) = other.strip_prefix("--eol=") {
                    flags.eol = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--mcp=") {
                    flags.mcp = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--config=") {
                    flags.config = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--host=") {
                    flags.host = Some(v.to_string());
                } else if other.starts_with('-') {
                    bail!("unknown argument '{other}', try --help");
                } else {
                    words.push(other.to_string());
                }
            }
        }
    }

    let (command, rest) = words.split_first().map_or(("", &[][..]), |(c, r)| (c.as_str(), r));
    if command != "update" {
        let named = if command.is_empty() {
            "the monitor"
        } else {
            command
        };
        deny_flag(flags.from_source, "--from-source", named)?;
        deny_flag(flags.yes, "--yes", named)?;
    }
    match command {
        "update" => {
            deny(flags.eol.as_ref(), "--eol", "update")?;
            deny(flags.config.as_ref(), "--config", "update")?;
            // The tunnel only carries the side door, it cannot put a file on
            // the far machine, so updating one is a separate feature.
            deny(flags.host.as_ref(), "--host", "update")?;
            if rest.len() > 1 {
                bail!("update takes at most one version");
            }
            Ok(Cli::Update {
                mcp:     endpoint(flags.mcp.as_deref())?,
                request: Request {
                    version:     rest.first().cloned(),
                    from_source: flags.from_source,
                    yes:         flags.yes,
                },
            })
        }
        "daemon" => {
            reject(rest, "daemon takes no arguments")?;
            // Each console names its own line ending in the config, so a single
            // flag for all of them would be a lie.
            deny(flags.eol.as_ref(), "--eol", "daemon")?;
            // The daemon serves, it does not reach out to another machine.
            deny(flags.host.as_ref(), "--host", "daemon")?;
            Ok(Cli::Daemon {
                config: flags.config,
                mcp:    flags.mcp.as_deref().map(address).transpose()?,
            })
        }
        "list" => {
            reject(rest, "list takes no arguments")?;
            deny(flags.eol.as_ref(), "--eol", "list")?;
            deny(flags.config.as_ref(), "--config", "list")?;
            Ok(Cli::List {
                mcp:  endpoint(flags.mcp.as_deref())?,
                host: flags.host,
            })
        }
        "call" => {
            deny(flags.eol.as_ref(), "--eol", "call")?;
            deny(flags.config.as_ref(), "--config", "call")?;
            let (tool, rest) = rest.split_first().context("usage: smon call <tool> [json-args]")?;
            if rest.len() > 1 {
                bail!("too many arguments for call");
            }
            Ok(Cli::Call {
                mcp:  endpoint(flags.mcp.as_deref())?,
                host: flags.host,
                tool: tool.clone(),
                args: rest.first().cloned().unwrap_or_else(|| "{}".to_string()),
            })
        }
        "" => {
            deny(flags.config.as_ref(), "--config", "the monitor")?;
            Ok(Cli::Run {
                eol:  eol_bytes(flags.eol.as_deref().unwrap_or("crlf"))?,
                mcp:  endpoint(flags.mcp.as_deref())?,
                host: flags.host,
            })
        }
        other => bail!("unknown subcommand '{other}', try --help"),
    }
}

fn value(args: &mut Peekable<impl Iterator<Item = String>>, flag: &str) -> Result<String> {
    args.next().with_context(|| format!("{flag} needs a value"))
}

fn reject(rest: &[String], message: &str) -> Result<()> {
    if rest.is_empty() {
        return Ok(());
    }
    bail!("{message}")
}

fn deny(flag: Option<&String>, name: &str, command: &str) -> Result<()> {
    if flag.is_none() {
        return Ok(());
    }
    bail!("{name} means nothing to {command}")
}

fn deny_flag(set: bool, name: &str, command: &str) -> Result<()> {
    if !set {
        return Ok(());
    }
    bail!("{name} means nothing to {command}")
}

fn endpoint(text: Option<&str>) -> Result<SocketAddr> {
    address(text.unwrap_or(DEFAULT_BIND))
}

fn address(text: &str) -> Result<SocketAddr> {
    text.parse()
        .with_context(|| format!("invalid address '{text}', expected host:port like {DEFAULT_BIND}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Cli> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn unknown_or_incomplete_arguments_are_rejected() {
        assert!(parse_args(&["--elo=cr"]).is_err());
        assert!(parse_args(&["extra"]).is_err());
        assert!(parse_args(&["--eol"]).is_err());
        assert!(parse_args(&["--eol", "tab"]).is_err());
        assert!(parse_args(&["--mcp", "not-an-addr"]).is_err());
    }

    #[test]
    fn eol_and_mcp_parse_in_both_forms() {
        let Ok(Cli::Run { eol, mcp, .. }) = parse_args(&["--eol=cr", "--mcp", "127.0.0.1:9000"]) else {
            panic!("expected Cli::Run");
        };
        assert_eq!(eol, b"\r");
        assert_eq!(mcp.port(), 9000);
    }

    #[test]
    fn defaults_apply_with_no_arguments() {
        let Ok(Cli::Run { eol, mcp, .. }) = parse_args(&[]) else {
            panic!("expected Cli::Run");
        };
        assert_eq!(eol, b"\r\n");
        assert_eq!(mcp.to_string(), DEFAULT_BIND);
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert!(matches!(parse_args(&["--help"]), Ok(Cli::Help)));
        assert!(matches!(parse_args(&["-V", "--bogus"]), Ok(Cli::Version)));
    }

    #[test]
    fn call_and_list_subcommands_parse() {
        let Ok(Cli::Call { mcp, tool, args, .. }) = parse_args(&["call", "serial_status"]) else {
            panic!("expected Cli::Call");
        };
        assert_eq!(mcp.to_string(), DEFAULT_BIND);
        assert_eq!(tool, "serial_status");
        assert_eq!(args, "{}");
        assert!(matches!(parse_args(&["list"]), Ok(Cli::List { .. })));
        assert!(parse_args(&["call"]).is_err());
        assert!(parse_args(&["list", "extra"]).is_err());
        assert!(parse_args(&["call", "a", "{}", "extra"]).is_err());
    }

    #[test]
    fn call_takes_json_and_an_endpoint() {
        let Ok(Cli::Call { mcp, tool, args, .. }) =
            parse_args(&["call", "serial_send", r#"{"text":"x"}"#, "--mcp=127.0.0.1:5000"])
        else {
            panic!("expected Cli::Call");
        };
        assert_eq!(mcp.port(), 5000);
        assert_eq!(tool, "serial_send");
        assert_eq!(args, r#"{"text":"x"}"#);
    }

    #[test]
    fn daemon_takes_a_config_and_an_optional_bind() {
        let Ok(Cli::Daemon { config, mcp }) = parse_args(&["daemon", "--config", "/etc/x.toml"]) else {
            panic!("expected Cli::Daemon");
        };
        assert_eq!(config.as_deref(), Some("/etc/x.toml"));
        assert_eq!(mcp, None);

        let Ok(Cli::Daemon { mcp, .. }) = parse_args(&["daemon", "--mcp", "0.0.0.0:1"]) else {
            panic!("expected Cli::Daemon");
        };
        assert_eq!(mcp.map(|a| a.port()), Some(1));
    }

    // A flag that cannot mean anything must be refused, never accepted and
    // ignored, or a daemon would silently run with a line ending nobody set.
    #[test]
    fn flags_that_mean_nothing_to_a_subcommand_are_refused() {
        assert!(parse_args(&["daemon", "--eol", "cr"]).is_err());
        assert!(parse_args(&["daemon", "--host", "pi"]).is_err());
        assert!(parse_args(&["list", "--config", "x"]).is_err());
        assert!(parse_args(&["call", "t", "--eol", "cr"]).is_err());
        assert!(parse_args(&["--config", "x"]).is_err());
    }

    #[test]
    fn host_reaches_another_machine() {
        let Ok(Cli::Run { host, .. }) = parse_args(&["--host", "pi"]) else {
            panic!("expected Cli::Run");
        };
        assert_eq!(host.as_deref(), Some("pi"));

        let Ok(Cli::List { host, .. }) = parse_args(&["list", "--host=pi"]) else {
            panic!("expected Cli::List");
        };
        assert_eq!(host.as_deref(), Some("pi"));
    }

    #[test]
    fn update_takes_an_optional_version_and_its_own_flags() {
        let Ok(Cli::Update { mcp, request }) = parse_args(&["update"]) else {
            panic!("expected Cli::Update");
        };
        assert_eq!(mcp.to_string(), DEFAULT_BIND);
        assert_eq!(request.version, None);
        assert!(!request.from_source);
        assert!(!request.yes);

        let Ok(Cli::Update { request, .. }) = parse_args(&["update", "v0.1.2", "--from-source", "--yes"])
        else {
            panic!("expected Cli::Update");
        };
        assert_eq!(request.version.as_deref(), Some("v0.1.2"));
        assert!(request.from_source);
        assert!(request.yes);
    }

    #[test]
    fn update_refuses_a_second_version_and_flags_it_cannot_use() {
        assert!(parse_args(&["update", "v0.1.2", "v0.1.3"]).is_err());
        assert!(parse_args(&["update", "--eol", "cr"]).is_err());
        assert!(parse_args(&["update", "--config", "x"]).is_err());
    }

    // The tunnel carries the side door only, it cannot put a file on the far
    // machine, so accepting --host would update the wrong one silently.
    #[test]
    fn update_refuses_a_remote_host() {
        let error = parse_args(&["update", "--host", "pi"]).unwrap_err().to_string();
        assert!(error.contains("--host"), "{error}");
    }

    #[test]
    fn the_update_flags_mean_nothing_to_other_subcommands() {
        assert!(parse_args(&["--yes"]).is_err());
        assert!(parse_args(&["--from-source"]).is_err());
        assert!(parse_args(&["daemon", "--yes"]).is_err());
        assert!(parse_args(&["list", "--from-source"]).is_err());
        assert!(parse_args(&["call", "serial_status", "--yes"]).is_err());
    }

    #[test]
    fn an_unknown_subcommand_is_named_in_the_error() {
        let error = parse_args(&["deamon"]).unwrap_err().to_string();
        assert!(error.contains("deamon"), "{error}");
    }
}
