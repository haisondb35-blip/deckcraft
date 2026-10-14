//! The Claude pane: ask Claude to work on the open presentation, as Copilot does in Office.
//!
//! The pane runs Claude Code (`claude`, installed and signed in on this computer) in print mode
//! with one MCP server, `deckcraft-cli mcp --connect`, attached to this window through a control
//! channel of its own: a loopback port chosen by the system and a random token per run, so only
//! the Claude this pane starts can drive the app. Claude sees and changes the presentation the
//! person sees, with the same commands as the ribbon (each one undoable), and nothing else: the
//! MCP configuration is strict, Claude Code's own file, shell and web tools are refused, and it
//! runs in an empty folder of its own.
//!
//! Replies stream in as Claude Code's `stream-json` lines ([`parse_line`]); the conversation
//! continues across questions (`--resume`) until New chat.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::control::ControlRequest;

/// Starts a control channel for the pane: given the token requests must carry, the port it
/// listens on (127.0.0.1) and the requests it receives. The desktop app provides it.
pub type ControlStarter = Box<dyn Fn(&egui::Context, &str) -> Option<(u16, Receiver<ControlRequest>)>>;

/// What starts Claude for a question (the real one runs `claude`; tests use a fake).
pub type Launcher = Arc<dyn Fn(Launch) -> Result<Running, String> + Send + Sync>;

/// One question for Claude.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub prompt: String,
    /// The conversation to continue.
    pub session: Option<String>,
    /// The MCP configuration file (the DeckCraft server, connected to this window).
    pub mcp_config: PathBuf,
    /// The folder Claude runs in.
    pub work_dir: PathBuf,
}

/// A question being answered: its events, and how to stop it.
pub struct Running {
    pub events: Receiver<Event>,
    pub stop: Box<dyn FnOnce() + Send>,
}

/// What Claude Code reports while it answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The conversation's id (to continue it).
    Session(String),
    /// Text of the reply.
    Text(String),
    /// A tool it is using (without the `mcp__deckcraft__` prefix).
    Step(String),
    /// Finished, with the error if it failed.
    Done(Option<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Claude,
    Step,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub role: Role,
    pub text: String,
}

/// The longest conversation kept on screen.
const MAX_MESSAGES: usize = 400;
/// The longest line read from Claude Code (a slide image in a tool result can be large).
const MAX_LINE: u64 = 16 << 20;

/// Appended to Claude Code's system prompt.
pub const SYSTEM_PROMPT: &str = "You are the assistant inside DeckCraft, a presentation editor compatible with PowerPoint. \
The presentation the user has open is in the running app: work on it only through the deckcraft tools, which act on exactly what the user sees \
(every change can be undone with Ctrl+Z). Look before you change: inspect the document or slide, and render a slide to check your result. \
Prefer the app's commands (list_commands, run_command) over pointer input. Reply in the user's language, briefly, and say what you changed.";

/// Claude Code's own tools the pane refuses: Claude works on the presentation, not the computer.
pub const REFUSED_TOOLS: &[&str] =
    &["Bash", "BashOutput", "KillShell", "Edit", "MultiEdit", "Write", "NotebookEdit", "Read", "Glob", "Grep", "LS", "WebFetch", "WebSearch", "Task"];

pub struct Assistant {
    pub messages: Vec<Msg>,
    pub input: String,
    /// The conversation being continued.
    pub session: Option<String>,
    running: Option<Running>,
    /// The pane's control channel: requests from the Claude it started.
    pub control_rx: Option<Receiver<ControlRequest>>,
    port: Option<u16>,
    token: String,
    pub starter: Option<ControlStarter>,
    pub launcher: Launcher,
    /// `deckcraft-cli` (default: beside the running executable).
    pub cli: Option<PathBuf>,
    work_dir: Option<PathBuf>,
}

impl Default for Assistant {
    fn default() -> Self {
        Assistant {
            messages: Vec::new(),
            input: String::new(),
            session: None,
            running: None,
            control_rx: None,
            port: None,
            token: random_token(),
            starter: None,
            launcher: Arc::new(launch_claude),
            cli: None,
            work_dir: None,
        }
    }
}

impl Drop for Assistant {
    fn drop(&mut self) {
        self.stop();
        if let Some(d) = &self.work_dir {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// 128 random bits as hex, from the standard library's randomly keyed hasher.
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let part = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u64(salt);
        h.write_u32(std::process::id());
        h.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

impl Assistant {
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    fn push(&mut self, role: Role, text: impl Into<String>) {
        self.messages.push(Msg { role, text: text.into() });
        if self.messages.len() > MAX_MESSAGES {
            let extra = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(..extra);
        }
    }

    /// Start a new conversation.
    pub fn new_chat(&mut self) {
        self.stop();
        self.messages.clear();
        self.session = None;
    }

    /// Stop the answer in progress.
    pub fn stop(&mut self) {
        if let Some(r) = self.running.take() {
            (r.stop)();
            self.push(Role::Error, "Stopped.");
        }
    }

    /// The folder Claude runs in and the MCP configuration that connects it to this window,
    /// starting the pane's control channel the first time.
    fn prepare(&mut self, ctx: &egui::Context) -> Result<(PathBuf, PathBuf), String> {
        if self.port.is_none() {
            let starter = self.starter.as_ref().ok_or("the Claude pane needs the DeckCraft desktop app")?;
            let (port, rx) = starter(ctx, &self.token).ok_or("couldn't open a control channel for Claude")?;
            self.port = Some(port);
            self.control_rx = Some(rx);
        }
        let port = self.port.ok_or("no control channel")?;
        let dir = match &self.work_dir {
            Some(d) => d.clone(),
            None => {
                let d = std::env::temp_dir().join(format!("deckcraft-claude-{}", std::process::id()));
                std::fs::create_dir_all(&d).map_err(|e| format!("couldn't create {}: {e}", d.display()))?;
                self.work_dir = Some(d.clone());
                d
            }
        };
        let cli = self.cli.clone().or_else(cli_path).ok_or("deckcraft-cli wasn't found next to DeckCraft; reinstall DeckCraft")?;
        let config = mcp_config(&cli, port, &self.token);
        let path = dir.join("mcp.json");
        std::fs::write(&path, config.to_string()).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
        Ok((dir, path))
    }

    /// Ask Claude what's in the input box.
    pub fn send(&mut self, ctx: &egui::Context) {
        let prompt = self.input.trim().to_string();
        if prompt.is_empty() || self.busy() {
            return;
        }
        self.input.clear();
        self.push(Role::User, prompt.clone());
        let launched = self
            .prepare(ctx)
            .and_then(|(work_dir, mcp_config)| (self.launcher)(Launch { prompt, session: self.session.clone(), mcp_config, work_dir }));
        match launched {
            Ok(r) => self.running = Some(r),
            Err(e) => self.push(Role::Error, e),
        }
    }

    /// Take in what Claude has reported since the last frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(run) = &self.running else { return };
        let mut events = Vec::new();
        let mut gone = false;
        loop {
            match run.events.try_recv() {
                Ok(e) => events.push(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
        let mut done = gone;
        for e in events {
            match e {
                Event::Session(id) => self.session = Some(id),
                Event::Text(t) => match self.messages.last_mut() {
                    Some(m) if m.role == Role::Claude => {
                        m.text.push_str("\n\n");
                        m.text.push_str(&t);
                    }
                    _ => self.push(Role::Claude, t),
                },
                Event::Step(s) => self.push(Role::Step, s),
                Event::Done(err) => {
                    if let Some(e) = err {
                        self.push(Role::Error, e);
                    }
                    done = true;
                }
            }
        }
        if done {
            self.running = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

/// `deckcraft-cli` beside the running executable.
fn cli_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) { "deckcraft-cli.exe" } else { "deckcraft-cli" };
    let p = exe.parent()?.join(name);
    p.is_file().then_some(p)
}

/// The MCP configuration: the DeckCraft server, connected to this window with its token.
pub fn mcp_config(cli: &Path, port: u16, token: &str) -> Value {
    json!({
        "mcpServers": {
            "deckcraft": {
                "type": "stdio",
                "command": cli.to_string_lossy(),
                "args": ["mcp", "--connect", format!("127.0.0.1:{port}")],
                "env": { "DECKCRAFT_CONTROL_TOKEN": token },
            }
        }
    })
}

/// Claude Code's command-line arguments for a question (the prompt goes in on stdin).
pub fn claude_args(l: &Launch) -> Vec<String> {
    let mut a: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--strict-mcp-config", "--allowedTools", "mcp__deckcraft"]
        .into_iter()
        .map(String::from)
        .collect();
    a.push("--mcp-config".into());
    a.push(l.mcp_config.to_string_lossy().into_owned());
    a.push("--disallowedTools".into());
    a.extend(REFUSED_TOOLS.iter().map(|t| t.to_string()));
    a.push("--append-system-prompt".into());
    a.push(SYSTEM_PROMPT.into());
    if let Some(s) = &l.session {
        a.push("--resume".into());
        a.push(s.clone());
    }
    a
}

/// Where Claude Code is: `DECKCRAFT_CLAUDE`, else `claude` on the PATH (on Windows the native
/// `claude.exe`, else npm's `claude.cmd`).
pub fn find_claude() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DECKCRAFT_CLAUDE").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd"] } else { &["claude"] };
    let path = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    names.iter().find_map(|n| dirs.iter().map(|d| d.join(n)).find(|p| p.is_file()))
}

/// The events in one `stream-json` line (none for lines the pane doesn't show).
pub fn parse_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => s(&v, "session_id").map(Event::Session).into_iter().collect(),
        Some("assistant") => v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                        Some("text") => s(p, "text").filter(|t| !t.trim().is_empty()).map(|t| Event::Text(t.trim().to_string())),
                        Some("tool_use") => s(p, "name").map(|n| Event::Step(n.trim_start_matches("mcp__deckcraft__").to_string())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Some("result") => {
            let mut out: Vec<Event> = s(&v, "session_id").map(Event::Session).into_iter().collect();
            let failed = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let why = s(&v, "result").or_else(|| s(&v, "subtype")).unwrap_or_else(|| "Claude couldn't finish".into());
            out.push(Event::Done(failed.then_some(why)));
            out
        }
        _ => Vec::new(),
    }
}

/// Run Claude Code for a question.
fn launch_claude(l: Launch) -> Result<Running, String> {
    let claude = find_claude()
        .ok_or("Claude Code isn't installed: install it (https://claude.com/claude-code), run `claude` once to sign in, then ask again")?;
    let mut cmd = std::process::Command::new(&claude);
    cmd.args(claude_args(&l))
        .current_dir(&l.work_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // CREATE_NO_WINDOW: no console window flashes up.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start Claude Code ({}): {e}", claude.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let prompt = l.prompt.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
        });
    }
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let child = Arc::new(Mutex::new(child));
    let (tx, rx) = channel();
    let err_text = Arc::new(Mutex::new(String::new()));
    if let Some(mut e) = stderr {
        let err_text = err_text.clone();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = (&mut e).take(64 << 10).read_to_string(&mut s);
            if let Ok(mut t) = err_text.lock() {
                *t = s;
            }
        });
    }
    let waiter = child.clone();
    std::thread::spawn(move || read_events(stdout, &tx, &waiter, &err_text));
    let killer = child;
    Ok(Running {
        events: rx,
        stop: Box::new(move || {
            if let Ok(mut c) = killer.lock() {
                let _ = c.kill();
            }
        }),
    })
}

fn read_events(stdout: Option<std::process::ChildStdout>, tx: &Sender<Event>, child: &Arc<Mutex<std::process::Child>>, err: &Arc<Mutex<String>>) {
    let mut finished = false;
    if let Some(out) = stdout {
        let mut reader = BufReader::new(out);
        loop {
            let mut buf = Vec::new();
            match reader.by_ref().take(MAX_LINE).read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    for e in parse_line(String::from_utf8_lossy(&buf).trim()) {
                        finished |= matches!(e, Event::Done(_));
                        if tx.send(e).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }
    let status = child.lock().ok().and_then(|mut c| c.wait().ok());
    if !finished {
        // Ended without a result: say why, from its error output when it gave one.
        std::thread::sleep(std::time::Duration::from_millis(100));
        let text = err.lock().map(|t| t.trim().lines().take(3).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let why = if !text.is_empty() {
            text
        } else if status.is_some_and(|s| s.success()) {
            "Claude ended without an answer".to_string()
        } else {
            "Claude Code stopped; run `claude` in a terminal once to check it is signed in".to_string()
        };
        let _ = tx.send(Event::Done(Some(why)));
    }
}

/// The pane's contents (inside the task pane frame).
pub fn pane(app: &mut crate::SlideApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get(ui.ctx());
    let a = &mut app.assistant;
    a.poll(ui.ctx());
    let input_h = 96.0;
    let list_h = (ui.available_height() - input_h).max(80.0);
    egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        if a.messages.is_empty() {
            ui.label(egui::RichText::new("Ask Claude to build or change slides, rewrite text, or check the presentation. Changes appear here as Claude makes them and can be undone.").color(t.text_dim));
        }
        for m in &a.messages {
            match m.role {
                Role::User => {
                    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(&m.text);
                    });
                }
                Role::Claude => {
                    ui.label(&m.text);
                }
                Role::Step => {
                    ui.label(egui::RichText::new(format!("· {}", m.text)).small().color(t.text_dim));
                }
                Role::Error => {
                    ui.label(egui::RichText::new(&m.text).color(egui::Color32::from_rgb(196, 43, 28)));
                }
            }
            ui.add_space(6.0);
        }
        if a.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Claude is working…").color(t.text_dim));
            });
        }
    });
    ui.separator();
    let busy = a.busy();
    let edit = egui::TextEdit::multiline(&mut a.input).hint_text("Ask Claude…").desired_rows(2).desired_width(f32::INFINITY);
    let resp = ui.add_enabled(!busy, edit);
    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    if enter {
        // Enter sends; Shift+Enter is a new line. Drop the newline the key typed.
        while a.input.ends_with('\n') {
            a.input.pop();
        }
    }
    let mut send = enter;
    ui.horizontal(|ui| {
        if a.busy() {
            if ui.button("Stop").clicked() {
                a.stop();
            }
        } else if ui.add_enabled(!a.input.trim().is_empty(), egui::Button::new("Send")).clicked() {
            send = true;
        }
        if ui.add_enabled(!a.messages.is_empty(), egui::Button::new("New chat")).clicked() {
            a.new_chat();
        }
    });
    if send {
        a.send(ui.ctx());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_json_lines_become_events() {
        assert_eq!(parse_line(r#"{"type":"system","subtype":"init","session_id":"abc","tools":[]}"#), vec![Event::Session("abc".into())]);
        let a = r#"{"type":"assistant","message":{"content":[{"type":"text","text":" Đã thêm slide. "},{"type":"tool_use","name":"mcp__deckcraft__add_slide","input":{}}]}}"#;
        assert_eq!(parse_line(a), vec![Event::Text("Đã thêm slide.".into()), Event::Step("add_slide".into())]);
        assert_eq!(
            parse_line(r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"abc"}"#),
            vec![Event::Session("abc".into()), Event::Done(None)]
        );
        assert_eq!(parse_line(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#), vec![Event::Done(Some("error_max_turns".into()))]);
        for junk in ["", "not json", "{}", r#"{"type":"user"}"#, r#"{"type":"assistant","message":{}}"#, "[1,2]"] {
            assert!(parse_line(junk).is_empty(), "{junk}");
        }
    }

    #[test]
    fn claude_gets_only_the_deckcraft_tools() {
        let l = Launch { prompt: "x".into(), session: Some("s1".into()), mcp_config: PathBuf::from("m.json"), work_dir: PathBuf::from(".") };
        let a = claude_args(&l);
        let has = |pair: [&str; 2]| a.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert!(a.contains(&"--strict-mcp-config".to_string()));
        assert!(has(["--allowedTools", "mcp__deckcraft"]) && has(["--mcp-config", "m.json"]) && has(["--resume", "s1"]));
        for t in ["Bash", "Read", "Write", "Edit", "WebFetch"] {
            assert!(a.contains(&t.to_string()), "{t} refused");
        }
        assert!(!a.iter().any(|x| x == "x"), "the prompt goes in on stdin, not the command line");
        let cfg = mcp_config(Path::new("C:/DeckCraft/deckcraft-cli.exe"), 4567, "tok");
        assert_eq!(cfg.pointer("/mcpServers/deckcraft/args/2").and_then(Value::as_str), Some("127.0.0.1:4567"));
        assert_eq!(cfg.pointer("/mcpServers/deckcraft/env/DECKCRAFT_CONTROL_TOKEN").and_then(Value::as_str), Some("tok"));
    }

    /// The pane end to end with a stand-in for Claude Code: the question goes out with the MCP
    /// configuration for this window's channel, and the reply, its steps and the conversation id
    /// come back into the pane.
    #[test]
    fn a_question_comes_back_as_a_reply_with_its_steps() {
        let mut app = crate::SlideApp::new(deckcraft_engine::Session::new(), crate::Services::default());
        let (_ctl_tx, ctl_rx) = channel();
        let ctl_rx = Mutex::new(Some(ctl_rx));
        let tokens = Arc::new(Mutex::new(Vec::new()));
        let seen = tokens.clone();
        app.assistant.starter = Some(Box::new(move |_, token| {
            if let Ok(mut t) = seen.lock() {
                t.push(token.to_string());
            }
            ctl_rx.lock().ok().and_then(|mut r| r.take()).map(|rx| (4567, rx))
        }));
        app.assistant.cli = Some(PathBuf::from("deckcraft-cli"));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = asked.clone();
        app.assistant.launcher = Arc::new(move |l: Launch| {
            let config = std::fs::read_to_string(&l.mcp_config).map_err(|e| e.to_string())?;
            if let Ok(mut a) = log.lock() {
                a.push((l.prompt.clone(), l.session.clone(), config));
            }
            let (tx, rx) = channel();
            for e in [Event::Session("s1".into()), Event::Step("add_slide".into()), Event::Text("Đã thêm slide tiêu đề.".into()), Event::Done(None)]
            {
                let _ = tx.send(e);
            }
            Ok(Running { events: rx, stop: Box::new(|| {}) })
        });
        let ctx = egui::Context::default();
        app.assistant.input = "Thêm slide tiêu đề".into();
        app.assistant.send(&ctx);
        app.assistant.poll(&ctx);
        let roles: Vec<Role> = app.assistant.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::User, Role::Step, Role::Claude]);
        assert_eq!(app.assistant.messages[2].text, "Đã thêm slide tiêu đề.");
        assert!(!app.assistant.busy() && app.assistant.input.is_empty());
        assert!(app.assistant.control_rx.is_some(), "the pane's channel is open");
        // A second question continues the conversation, on the same channel.
        app.assistant.input = "Đổi màu nền".into();
        app.assistant.send(&ctx);
        app.assistant.poll(&ctx);
        let asked = asked.lock().map(|a| a.clone()).unwrap_or_default();
        assert_eq!(asked.len(), 2);
        assert_eq!((asked[0].1.as_deref(), asked[1].1.as_deref()), (None, Some("s1")));
        let token = tokens.lock().map(|t| t.clone()).unwrap_or_default();
        assert_eq!(token.len(), 1, "one channel for the app's run");
        assert!(asked[0].2.contains("127.0.0.1:4567") && asked[0].2.contains(&token[0]), "{}", asked[0].2);
        app.assistant.new_chat();
        assert!(app.assistant.messages.is_empty() && app.assistant.session.is_none());
    }

    #[test]
    fn without_the_desktop_app_the_pane_says_so() {
        let mut a = Assistant::default();
        a.input = "hello".into();
        a.send(&egui::Context::default());
        assert_eq!(a.messages.last().map(|m| m.role), Some(Role::Error));
        assert!(!a.busy());
    }

    #[test]
    fn tokens_are_long_and_differ() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
