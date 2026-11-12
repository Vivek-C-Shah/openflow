//! Single-instance lock + tiny local control channel (`openflow stop`, `reload`).
//! A blocking accept() costs nothing while idle.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub const PORT: u16 = 47_118;

/// Returns `None` if another OpenFlow is already running.
pub fn bind() -> Option<TcpListener> {
    TcpListener::bind(("127.0.0.1", PORT)).ok()
}

pub fn serve(listener: TcpListener, handler: impl Fn(&str) -> String + Send + 'static) {
    std::thread::Builder::new()
        .name("control".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                let mut reader = BufReader::new(&stream);
                if reader.read_line(&mut line).is_ok() {
                    let reply = handler(line.trim());
                    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
                }
            }
        })
        .expect("spawn control thread");
}

pub fn send(cmd: &str) -> Result<String> {
    let mut s = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], PORT)), Duration::from_millis(500))
        .context("OpenFlow is not running")?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    s.write_all(format!("{cmd}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(&s).read_line(&mut reply)?;
    Ok(reply.trim().to_string())
}

pub fn is_running() -> bool {
    send("ping").map(|r| r == "pong").unwrap_or(false)
}
