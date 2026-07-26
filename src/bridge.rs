//! A raw byte bridge onto a console, for programs that speak to a serial port
//! directly.
//!
//! This is what a tool like pyserial reaches with `socket://127.0.0.1:port`. It
//! exists so nothing has to be handed the device node itself: the console keeps
//! its buffer and its log, and whatever the other program says to the board is
//! recorded like any other input instead of vanishing.

use std::sync::Arc;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::broadcast::error::RecvError,
};

use crate::console::{Console, ConsoleEvent};

/// Accept connections forever, one console per port. Loopback only, for the
/// same reason the rest of the endpoint is: a console is never put on a network.
pub async fn serve(console: Arc<Console>, port: u16) {
    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("smon: bridge for {} cannot bind {port}: {e}", console.name());
            return;
        }
    };
    println!(
        "smon: console {} bridged raw on 127.0.0.1:{port}",
        console.name()
    );
    loop {
        match listener.accept().await {
            Ok((socket, _)) => {
                tokio::spawn(pipe(Arc::clone(&console), socket));
            }
            Err(e) => {
                eprintln!("smon: bridge for {} stopped: {e}", console.name());
                return;
            }
        }
    }
}

async fn pipe(console: Arc<Console>, socket: TcpStream) {
    let mut events = console.subscribe();
    let (mut from_client, mut to_client) = socket.into_split();
    let mut buf = [0u8; 4096];
    console.push_system("a raw bridge client attached");

    loop {
        tokio::select! {
            event = events.recv() => match event {
                // Only received bytes go out. An echo of the client's own input
                // would come back as if the board had said it.
                Ok(ConsoleEvent::Rx(bytes)) => {
                    if to_client.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            read = from_client.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Err(e) = console.queue_raw(buf[..n].to_vec()) {
                        console.push_system(&e);
                        break;
                    }
                }
            },
        }
    }
    console.push_system("the raw bridge client left");
}
