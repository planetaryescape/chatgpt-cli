//! Debug builds only: the debug `chatgpt` binary plays a fake `codex` or
//! `claude` for tests, as `chatgpt __fake-model-cli <log dir> <ok|slowok|fail|slowfail>
//! <codex|claude> <its arguments…>`, run from a two-line script named
//! `codex` or `claude` on a test's `PATH`. It answers as
//! the fake-chatgpt crate's `model_answers.rs` decides, and records each call (its
//! arguments, scratch paths made stable, and its stdin) in the log
//! directory, so a parity test can compare what each CLI sent.

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};

#[path = "../../fake-chatgpt/src/model_answers.rs"]
mod model_answers;

/// A scratch path the CLIs make afresh per call, made stable for
/// comparison: `<scratch>/<file>`.
fn stable(arg: &str) -> String {
    if arg.contains("chatgpt-cli-codex-") || arg.contains("chatgpt-cli-luna-") {
        let name = Path::new(arg)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.starts_with("chatgpt-cli-"))
            .unwrap_or_default();
        return format!("<scratch>/{name}");
    }
    arg.to_owned()
}

fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

pub fn run(args: &[String]) -> ExitCode {
    let [log, mode, tool, rest @ ..] = args else {
        eprintln!(
            "usage: chatgpt __fake-model-cli <log dir> <ok|slowok|fail|slowfail> <codex|claude> <args…>"
        );
        return ExitCode::from(2);
    };
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let mut env: Vec<String> = std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    env.sort();
    let record = json!({
        "tool": tool,
        "args": rest.iter().map(|arg| stable(arg)).collect::<Vec<_>>(),
        "stdin": stdin,
        "env": env,
    });
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let _ = std::fs::create_dir_all(log);
    let _ = std::fs::write(
        Path::new(log).join(format!("{tool}-{stamp}-{}.json", std::process::id())),
        record.to_string(),
    );
    if mode == "slowfail" || mode == "slowok" {
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
    if mode == "fail" || mode == "slowfail" {
        eprintln!("SENTINEL fake {tool} failure with private text");
        return ExitCode::from(1);
    }
    match tool.as_str() {
        "codex" => codex(rest, &stdin),
        "claude" => claude(&stdin),
        _ => ExitCode::from(2),
    }
}

fn codex(args: &[String], stdin: &str) -> ExitCode {
    let Some(out) = after(args, "-o") else {
        return ExitCode::from(2);
    };
    let answer = match after(args, "--output-schema") {
        Some(schema) => {
            let schema: Value = std::fs::read_to_string(schema)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Value::Null);
            let input: Value = serde_json::from_str(stdin).unwrap_or(Value::Null);
            model_answers::luna(&schema, &input).to_string()
        }
        None => model_answers::summary(stdin),
    };
    if std::fs::write(out, answer).is_err() {
        return ExitCode::from(1);
    }
    if args.iter().any(|arg| arg == "--json") {
        let usage = json!({
            "input_tokens": model_answers::INPUT_TOKENS,
            "cached_input_tokens": model_answers::CACHED_INPUT_TOKENS,
            "output_tokens": model_answers::OUTPUT_TOKENS,
            "reasoning_output_tokens": model_answers::REASONING_TOKENS,
        });
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{}", json!({ "type": "thread.started" }));
        let _ = writeln!(stdout, "not json");
        let _ = writeln!(
            stdout,
            "{}",
            json!({ "type": "turn.completed", "usage": usage })
        );
    }
    ExitCode::SUCCESS
}

fn claude(stdin: &str) -> ExitCode {
    let answer = json!({
        "type": "result",
        "is_error": false,
        "result": format!("Claude {}", model_answers::summary(stdin)),
        "total_cost_usd": model_answers::CLAUDE_COST_USD,
        "usage": {
            "input_tokens": model_answers::INPUT_TOKENS,
            "cache_read_input_tokens": 0,
            "cache_creation_input_tokens": 0,
            "output_tokens": model_answers::OUTPUT_TOKENS,
        },
    });
    println!("{answer}");
    ExitCode::SUCCESS
}
