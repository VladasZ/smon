//! The set of consoles a daemon owns, and how a client names one of them.

use std::sync::{Arc, Mutex, RwLock, mpsc::channel};

use crate::{
    console::{Console, ConsoleSpec},
    log::ConsoleLog,
    runner::Runner,
    settings::eol_bytes,
};

/// What a client has to say to have a console taken over at runtime.
pub struct Adopt {
    pub device:  String,
    pub label:   Option<String>,
    pub baud:    u32,
    pub eol:     String,
    pub ring_kb: usize,
}

pub struct Registry {
    consoles:  RwLock<Vec<Arc<Console>>>,
    // Held for as long as the registry lives. A dropped runner would only
    // detach its thread, but keeping them makes the ownership honest.
    runners:   Mutex<Vec<Runner>>,
    retention: i64,
}

impl Registry {
    pub fn new(consoles: Vec<Arc<Console>>, retention: i64) -> Arc<Self> {
        Arc::new(Self {
            consoles: RwLock::new(consoles),
            runners: Mutex::new(Vec::new()),
            retention,
        })
    }

    pub fn all(&self) -> Vec<Arc<Console>> {
        self.consoles.read().unwrap().clone()
    }

    /// Take over a device that is not owned yet and start logging it.
    ///
    /// The first open has to succeed, so a device that is missing or already
    /// held by something else is refused here rather than turning into a
    /// console that silently never connects.
    ///
    /// # Errors
    /// Returns a message when the device is already owned, its end-of-line name
    /// is not one this understands, or the port cannot be opened.
    pub fn adopt(&self, wanted: Adopt) -> Result<Arc<Console>, String> {
        if let Some(existing) = self.consoles.read().unwrap().iter().find(|c| c.matches(&wanted.device)) {
            return Err(format!(
                "{} is already open here as {}",
                wanted.device,
                existing.name()
            ));
        }
        let eol = eol_bytes(&wanted.eol).map_err(|e| e.to_string())?;
        let name = wanted.label.clone().unwrap_or_else(|| wanted.device.clone());
        let log = ConsoleLog::open(&name, self.retention, None).map_err(|e| format!("{e:#}"))?;
        let (inject_tx, inject_rx) = channel();
        let console = Console::new(
            ConsoleSpec {
                device: wanted.device,
                label: wanted.label,
                baud: wanted.baud,
                eol,
                ring_cap: wanted.ring_kb.saturating_mul(1024),
                // A bridge listener is started with the server, so a console
                // adopted later cannot have one. Put it in the config for that.
                bridge: None,
            },
            log,
            inject_tx,
        );
        let runner = Runner::start(Arc::clone(&console), inject_rx, true).map_err(|e| format!("{e:#}"))?;

        self.runners.lock().unwrap().push(runner);
        self.consoles.write().unwrap().push(Arc::clone(&console));
        Ok(console)
    }

    /// Find the console a client is asking for.
    ///
    /// The name may be omitted when only one console exists, which keeps a
    /// single-port setup on a laptop free of ceremony. With several, naming one
    /// is required, because guessing which board to send a command to is the
    /// one mistake this must never make.
    ///
    /// # Errors
    /// Returns a message naming the available consoles when the query matches
    /// nothing, matches more than one, or is missing with several to choose
    /// from.
    pub fn resolve(&self, query: Option<&str>) -> Result<Arc<Console>, String> {
        let consoles = self.consoles.read().unwrap();
        let Some(query) = query else {
            return match consoles.as_slice() {
                [only] => Ok(Arc::clone(only)),
                [] => Err("no consoles are open".to_string()),
                _ => Err(format!("name a console, one of {}", self.names())),
            };
        };

        let mut found = consoles.iter().filter(|c| c.matches(query));
        let Some(first) = found.next() else {
            return Err(format!("no console '{query}', have {}", self.names()));
        };
        if found.next().is_some() {
            return Err(format!(
                "'{query}' matches more than one console of {}",
                self.names()
            ));
        }
        Ok(Arc::clone(first))
    }

    fn names(&self) -> String {
        self.consoles
            .read()
            .unwrap()
            .iter()
            .map(|c| c.name().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf, sync::mpsc::channel};

    use super::*;
    use crate::{console::ConsoleSpec, log::ConsoleLog, ring::DEFAULT_RING_CAP};

    // Tests run in parallel, so each one needs a log directory of its own.
    // Sharing one made a finishing test delete the directory another was still
    // writing into.
    struct Logs {
        dir: PathBuf,
    }

    impl Logs {
        fn new(name: &str) -> Logs {
            let dir = env::temp_dir().join(format!("smon-registry-test-{name}"));
            if dir.exists() {
                fs::remove_dir_all(&dir).unwrap();
            }
            Logs { dir }
        }

        fn console(&self, device: &str, label: Option<&str>) -> Arc<Console> {
            let log = ConsoleLog::open_in(self.dir.clone(), device, 0, None).unwrap();
            Console::new(
                ConsoleSpec {
                    device:   device.to_string(),
                    label:    label.map(str::to_string),
                    baud:     115_200,
                    eol:      b"\r\n".to_vec(),
                    ring_cap: DEFAULT_RING_CAP,
                    bridge:   None,
                },
                log,
                channel().0,
            )
        }
    }

    impl Drop for Logs {
        fn drop(&mut self) {
            if let Err(e) = fs::remove_dir_all(&self.dir) {
                eprintln!("could not clean {}: {e}", self.dir.display());
            }
        }
    }

    #[test]
    fn one_console_needs_no_name() {
        let logs = Logs::new("one");
        let registry = Registry::new(vec![logs.console("/dev/ttyUSB0", None)], 0);
        assert_eq!(registry.resolve(None).unwrap().device(), "/dev/ttyUSB0");
    }

    // Sending a command to a board nobody asked for is the worst thing this can
    // do, so several consoles and no name is an error, never a guess.
    #[test]
    fn several_consoles_refuse_to_guess() {
        let logs = Logs::new("guess");
        let registry = Registry::new(
            vec![
                logs.console("/dev/ttyUSB0", Some("first")),
                logs.console("/dev/ttyUSB2", Some("second")),
            ],
            0,
        );
        let error = registry.resolve(None).err().unwrap();
        assert!(error.contains("first"), "{error}");
        assert!(error.contains("second"), "{error}");
    }

    #[test]
    fn a_console_answers_to_its_label_and_its_device() {
        let logs = Logs::new("address");
        let registry = Registry::new(
            vec![
                logs.console("/dev/ttyUSB0", Some("first")),
                logs.console("/dev/ttyUSB2", Some("second")),
            ],
            0,
        );
        assert_eq!(registry.resolve(Some("second")).unwrap().device(), "/dev/ttyUSB2");
        assert_eq!(
            registry.resolve(Some("/dev/ttyUSB0")).unwrap().device(),
            "/dev/ttyUSB0"
        );
        assert_eq!(
            registry.resolve(Some("ttyUSB2")).unwrap().device(),
            "/dev/ttyUSB2"
        );
    }

    #[test]
    fn an_unknown_name_lists_what_there_is() {
        let logs = Logs::new("unknown");
        let registry = Registry::new(vec![logs.console("/dev/ttyUSB0", Some("first"))], 0);
        let error = registry.resolve(Some("nope")).err().unwrap();
        assert!(error.contains("nope"), "{error}");
        assert!(error.contains("first"), "{error}");
    }

    // A device already open cannot be taken over again. Without this the second
    // console would sit there never connecting, since the first still holds it.
    #[test]
    fn a_device_is_only_adopted_once() {
        let logs = Logs::new("adopt");
        let registry = Registry::new(vec![logs.console("/dev/ttyUSB0", Some("first"))], 0);
        let error = registry
            .adopt(Adopt {
                device:  "/dev/ttyUSB0".to_string(),
                label:   Some("again".to_string()),
                baud:    115_200,
                eol:     "crlf".to_string(),
                ring_kb: 64,
            })
            .err()
            .unwrap();
        assert!(error.contains("already open"), "{error}");
        assert!(error.contains("first"), "{error}");
        assert_eq!(registry.all().len(), 1);
    }

    // A device that cannot be opened is refused outright, rather than becoming a
    // console that silently never connects.
    #[test]
    fn adopting_a_device_that_is_not_there_fails() {
        let logs = Logs::new("adopt-missing");
        let registry = Registry::new(vec![logs.console("/dev/ttyUSB0", Some("first"))], 0);
        let error = registry
            .adopt(Adopt {
                device:  "/dev/does-not-exist".to_string(),
                label:   None,
                baud:    115_200,
                eol:     "crlf".to_string(),
                ring_kb: 64,
            })
            .err()
            .unwrap();
        assert!(error.contains("does-not-exist"), "{error}");
        assert_eq!(registry.all().len(), 1, "a failed adopt must add nothing");
    }

    // Two adapters of the same model land on paths that share a tail, so a short
    // query can be ambiguous. That must be said, not resolved to whichever came
    // first in the config.
    #[test]
    fn an_ambiguous_name_is_refused() {
        let logs = Logs::new("ambiguous");
        let registry = Registry::new(
            vec![
                logs.console("/dev/serial/by-path/a/ttyUSB0", Some("first")),
                logs.console("/dev/serial/by-path/b/ttyUSB0", Some("second")),
            ],
            0,
        );
        let error = registry.resolve(Some("ttyUSB0")).err().unwrap();
        assert!(error.contains("more than one"), "{error}");
    }
}
