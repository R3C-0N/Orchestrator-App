//! Faux `claude.exe` scriptable pour les tests du superviseur (plan 1.10,
//! point 3 ; jalon J6). Il parle stream-json comme le vrai CLI, sans réseau.
//!
//! Il ignore les arguments du vrai CLI (`-p`, `--input-format`…) et lit les
//! siens, passés par `LaunchOptions::extra_args` :
//!
//! | Option | Effet |
//! | - | - |
//! | `--fake-mode normal` | (défaut) répond à `initialize`, joue un tour par message, sort 0 à la fin de stdin |
//! | `--fake-mode die-immediately` | écrit sur stderr puis sort avec `--fake-exit-code` avant toute réponse |
//! | `--fake-mode no-init` | ne répond jamais à `initialize` (lit stdin jusqu'à la fin) |
//! | `--fake-mode die-mid-turn` | au premier message : début de tour, puis sortie avec `--fake-exit-code` |
//! | `--fake-mode orphan-stdout` | lance un petit-enfant qui hérite de stdout et dort 60 s, puis se comporte comme `die-mid-turn` |
//! | `--fake-mode close-stdin` | répond à `initialize`, puis ferme son stdin et dort 60 s (tuyau stdin cassé) |
//! | `--fake-mode sleeper` | (interne) dort 60 s |
//! | `--fake-exit-code N` | code de sortie des modes qui meurent (défaut 1) |
//! | `--fake-version V` | `claude_code_version` annoncée (défaut 2.1.325) |
//! | `--fake-stderr-lines N` | écrit N lignes sur stderr au démarrage |
//! | `--fake-huge-line N` | après `initialize`, écrit une ligne JSON de N octets |
//! | `--fake-bad-json` | après `initialize`, écrit une ligne illisible |
//! | `--fake-unknown` | émet des types inconnus (`type` et sous-type `system/*`) |
//! | `--fake-split` | texte avec caractères multioctets coupés entre deux écritures, fins de ligne CRLF |
//! | `--fake-interrupt-no-result` | accepte `interrupt` sans jamais envoyer de `result` |
//! | `--fake-ignore-stdin-eof` | ne sort pas à la fin de stdin (dort ; l'arrêt passe par le Job Object) |
//!
//! Le pid du petit-enfant (`orphan-stdout`) est écrit sur stderr :
//! `GRANDCHILD_PID <pid>`.

use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

const SESSION_ID: &str = "00000000-0000-4000-8000-00000000fa4e";

#[derive(Debug, Clone)]
struct Opts {
    mode: String,
    exit_code: i32,
    version: String,
    stderr_lines: usize,
    huge_line: usize,
    bad_json: bool,
    unknown: bool,
    split: bool,
    interrupt_no_result: bool,
    ignore_stdin_eof: bool,
}

fn parse_opts() -> Opts {
    let mut o = Opts {
        mode: "normal".into(),
        exit_code: 1,
        version: "2.1.325".into(),
        stderr_lines: 0,
        huge_line: 0,
        bad_json: false,
        unknown: false,
        split: false,
        interrupt_no_result: false,
        ignore_stdin_eof: false,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let val = || args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--fake-mode" => {
                o.mode = val();
                i += 1;
            }
            "--fake-exit-code" => {
                o.exit_code = val().parse().unwrap_or(1);
                i += 1;
            }
            "--fake-version" => {
                o.version = val();
                i += 1;
            }
            "--fake-stderr-lines" => {
                o.stderr_lines = val().parse().unwrap_or(0);
                i += 1;
            }
            "--fake-huge-line" => {
                o.huge_line = val().parse().unwrap_or(0);
                i += 1;
            }
            "--fake-bad-json" => o.bad_json = true,
            "--fake-unknown" => o.unknown = true,
            "--fake-split" => o.split = true,
            "--fake-interrupt-no-result" => o.interrupt_no_result = true,
            "--fake-ignore-stdin-eof" => o.ignore_stdin_eof = true,
            _ => {}
        }
        i += 1;
    }
    o
}

/// Écrit des octets bruts sur stdout et vide le tampon.
fn out_bytes(b: &[u8]) {
    let mut o = io::stdout().lock();
    // Un tuyau cassé n'est pas une erreur du faux CLI : il s'arrête.
    if o.write_all(b).and_then(|_| o.flush()).is_err() {
        std::process::exit(0);
    }
}

fn out(v: &Value) {
    let mut s = v.to_string();
    s.push('\n');
    out_bytes(s.as_bytes());
}

fn err_line(s: &str) {
    let mut e = io::stderr().lock();
    let _ = writeln!(e, "{s}");
    let _ = e.flush();
}

fn stream(event: Value) -> Value {
    json!({"type": "stream_event", "event": event, "session_id": SESSION_ID,
           "parent_tool_use_id": null, "uuid": uuid::Uuid::new_v4().to_string()})
}

fn initialize_response(request_id: &str, o: &Opts) -> Value {
    json!({"type": "control_response", "response": {
        "subtype": "success", "request_id": request_id,
        "response": {
            "commands": [{"name": "review", "description": "Revue", "argumentHint": ""}],
            "agents": [{"name": "Explore", "description": "Exploration"}],
            "models": [{"value": "haiku", "resolvedModel": "claude-haiku-5", "displayName": "Haiku"}],
            "claude_code_version": o.version,
            "current_permission_mode": "bypassPermissions",
            "capabilities": [],
            "pid": std::process::id(),
        }
    }})
}

fn system_init(o: &Opts) -> Value {
    json!({"type": "system", "subtype": "init", "cwd": ".", "session_id": SESSION_ID,
           "tools": ["Bash"], "mcp_servers": [], "model": "claude-haiku-5",
           "permissionMode": "bypassPermissions", "slash_commands": ["review"],
           "skills": [], "claude_code_version": o.version,
           "capabilities": ["ask_user_question_unanswered_v1"],
           "uuid": uuid::Uuid::new_v4().to_string()})
}

fn emit_unknown() {
    out(&json!({"type": "mystery_event", "payload": {"x": 1}}));
    out(&json!({"type": "system", "subtype": "mystery_subtype", "session_id": SESSION_ID}));
}

/// Début d'un tour : `system/init`, `message_start`, bloc texte ouvert.
fn begin_turn(o: &Opts, msg_id: &str) {
    out(&system_init(o));
    out(&stream(json!({"type": "message_start", "message": {
        "model": "claude-haiku-5", "id": msg_id, "type": "message",
        "role": "assistant", "content": []}})));
    out(&stream(json!({"type": "content_block_start", "index": 0,
                       "content_block": {"type": "text", "text": ""}})));
}

fn text_delta(text: &str) -> Value {
    stream(json!({"type": "content_block_delta", "index": 0,
                  "delta": {"type": "text_delta", "text": text}}))
}

/// Un tour complet qui répond `text`.
fn play_turn(o: &Opts, n: u32) {
    let msg_id = format!("msg_fake_{n}");
    begin_turn(o, &msg_id);
    if o.unknown {
        emit_unknown();
    }
    let text = if o.split {
        // « é€😀 » coupé au milieu de chaque caractère multioctet, et la ligne
        // terminée par CRLF.
        let line = text_delta("é€😀").to_string();
        let bytes = line.as_bytes();
        let cut = line.find('é').unwrap() + 1; // au milieu de « é »
        let cut2 = line.find('😀').unwrap() + 2; // au milieu de « 😀 »
        out_bytes(&bytes[..cut]);
        sleep(Duration::from_millis(50));
        out_bytes(&bytes[cut..cut2]);
        sleep(Duration::from_millis(50));
        out_bytes(&bytes[cut2..]);
        out_bytes(b"\r\n");
        "é€😀".to_string()
    } else {
        out(&text_delta("Bonjour"));
        "Bonjour".to_string()
    };
    out(&stream(json!({"type": "content_block_stop", "index": 0})));
    out(&json!({"type": "assistant", "message": {
        "model": "claude-haiku-5", "id": msg_id, "type": "message", "role": "assistant",
        "content": [{"type": "text", "text": text}]},
        "session_id": SESSION_ID, "parent_tool_use_id": null,
        "uuid": uuid::Uuid::new_v4().to_string()}));
    out(&stream(json!({"type": "message_stop"})));
    out(&result("success", &text));
}

fn result(subtype: &str, text: &str) -> Value {
    json!({"type": "result", "subtype": subtype, "is_error": false, "result": text,
           "duration_ms": 10, "num_turns": 1, "total_cost_usd": 0.0,
           "session_id": SESSION_ID, "permission_denials": [],
           "uuid": uuid::Uuid::new_v4().to_string()})
}

fn spawn_grandchild() {
    let exe = std::env::current_exe().expect("current_exe");
    // stdout et stderr hérités : le petit-enfant garde les deux tuyaux ouverts.
    let child = Command::new(exe)
        .args(["--fake-mode", "sleeper"])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("petit-enfant");
    err_line(&format!("GRANDCHILD_PID {}", child.id()));
    std::mem::forget(child);
}

/// Ferme le handle de stdin : l'écrivain du superviseur verra un tuyau cassé.
fn close_stdin() {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        let h = io::stdin().as_raw_handle();
        // SAFETY : stdin n'est plus lu après cet appel ; le `File` prend le
        // handle et le ferme à sa destruction.
        drop(unsafe { std::fs::File::from_raw_handle(h) });
    }
}

fn main() {
    let o = parse_opts();
    match o.mode.as_str() {
        "sleeper" => {
            sleep(Duration::from_secs(60));
            return;
        }
        "die-immediately" => {
            err_line("fake_claude: erreur fatale au démarrage");
            std::process::exit(o.exit_code);
        }
        "orphan-stdout" => spawn_grandchild(),
        _ => {}
    }
    for i in 0..o.stderr_lines {
        err_line(&format!(
            "fake_claude: ligne de stderr {i} ................................................"
        ));
    }

    let stdin = io::stdin();
    let mut turns = 0u32;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            err_line(&format!("fake_claude: ligne illisible sur stdin : {line}"));
            continue;
        };
        match v["type"].as_str() {
            Some("control_request") => {
                let id = v["request_id"].as_str().unwrap_or_default().to_string();
                match v["request"]["subtype"].as_str() {
                    Some("initialize") => {
                        if o.mode == "no-init" {
                            continue;
                        }
                        out(&initialize_response(&id, &o));
                        if o.mode == "close-stdin" {
                            close_stdin();
                            sleep(Duration::from_secs(60));
                            return;
                        }
                        if o.unknown {
                            emit_unknown();
                        }
                        if o.bad_json {
                            out_bytes(b"{ceci n'est pas du JSON\n");
                        }
                        if o.huge_line > 0 {
                            let pad = "x".repeat(o.huge_line);
                            out(&json!({"type": "mystery_huge", "pad": pad}));
                        }
                    }
                    Some("interrupt") => {
                        out(&json!({"type": "control_response", "response": {
                            "subtype": "success", "request_id": id, "response": {}}}));
                        if !o.interrupt_no_result {
                            out(&result("error_during_execution", ""));
                        }
                    }
                    Some("set_model") => {
                        let m = v["request"]["model"].as_str().unwrap_or("haiku");
                        out(&json!({"type": "control_response", "response": {
                            "subtype": "success", "request_id": id,
                            "response": {"model": format!("claude-{m}")}}}));
                    }
                    other => {
                        out(&json!({"type": "control_response", "response": {
                            "subtype": "error", "request_id": id,
                            "error": format!("sous-type non géré : {other:?}")}}));
                    }
                }
            }
            Some("user") => {
                turns += 1;
                match o.mode.as_str() {
                    "die-mid-turn" | "orphan-stdout" => {
                        begin_turn(&o, "msg_fake_dying");
                        out(&text_delta("Je commence…"));
                        err_line("fake_claude: plantage en plein tour");
                        std::process::exit(o.exit_code);
                    }
                    _ if o.interrupt_no_result => {
                        // Tour ouvert, jamais fini : sert au test d'interruption.
                        begin_turn(&o, "msg_fake_open");
                        out(&text_delta("Un long tour…"));
                    }
                    _ => play_turn(&o, turns),
                }
            }
            _ => {}
        }
    }
    if o.ignore_stdin_eof {
        sleep(Duration::from_secs(60));
    }
}
