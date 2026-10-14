//! Loopback JSON-lines control server: one request per line, one reply per line.
//! This is the transport the MCP server (`deckcraft-cli mcp --connect`) wraps.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use deckcraft_ui_egui::ControlRequest;
use serde_json::{Value, json};

pub fn start(port: u16, ctx: egui::Context) -> Receiver<ControlRequest> {
    match start_with(port, ctx, None) {
        Some((_, rx)) => rx,
        None => channel::<ControlRequest>().1,
    }
}

/// Listen on 127.0.0.1:`port` (0: a free port the system picks). With a `token`, a request
/// that doesn't carry it (`"token": "…"`) is refused: the Claude pane's channel answers only
/// the Claude it started. Returns the port and the requests.
pub fn start_with(port: u16, ctx: egui::Context, token: Option<String>) -> Option<(u16, Receiver<ControlRequest>)> {
    let (tx, rx) = channel::<ControlRequest>();
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log::error!("control server failed to bind 127.0.0.1:{port}: {e}");
            return None;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    log::info!("control server listening on 127.0.0.1:{port}");
    let token = token.map(std::sync::Arc::new);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            let ctx = ctx.clone();
            let token = token.clone();
            std::thread::spawn(move || serve(stream, tx, ctx, token.as_deref().map(String::as_str)));
        }
    });
    Some((port, rx))
}

/// Does the request carry the channel's token (when it has one)?
fn authorised(msg: &Value, token: Option<&str>) -> bool {
    match token {
        None => true,
        Some(t) => msg
            .get("token")
            .and_then(Value::as_str)
            .is_some_and(|given| given.len() == t.len() && given.bytes().zip(t.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0),
    }
}

fn serve(stream: TcpStream, tx: Sender<ControlRequest>, ctx: egui::Context, token: Option<&str>) {
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    for line in BufReader::new(read).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) if !authorised(&msg, token) => {
                json!({"id": msg.get("id").cloned().unwrap_or(Value::Null), "ok": false, "error": "this control channel needs its token"})
            }
            Ok(msg) => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                let (req, rrx) = ControlRequest::new(method, params);
                if tx.send(req).is_err() {
                    break;
                }
                ctx.request_repaint();
                let mut r = rrx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"}));
                if let Some(o) = r.as_object_mut() {
                    o.insert("id".into(), id);
                }
                r
            }
            Err(e) => json!({"ok": false, "error": format!("bad JSON: {e}")}),
        };
        if writeln!(out, "{reply}").is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tokened_channel_answers_only_requests_with_its_token() {
        let req = |t: Option<&str>| match t {
            Some(t) => json!({"id": 1, "method": "ui.inspect", "token": t}),
            None => json!({"id": 1, "method": "ui.inspect"}),
        };
        assert!(authorised(&req(None), None), "the --control channel takes no token");
        assert!(authorised(&req(Some("abc123")), Some("abc123")));
        for wrong in [None, Some(""), Some("abc12"), Some("abc124"), Some("abc1234")] {
            assert!(!authorised(&req(wrong), Some("abc123")), "{wrong:?}");
        }
    }
}
