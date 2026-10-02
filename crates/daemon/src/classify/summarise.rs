//! Summaries of long chats, for Jev to judge them from: the TS CLI's
//! `src/classify/summarise.ts` @ 1b8c950. The summarisers are tried in its
//! order, a failure falling through to the next:
//!
//! 1. OpenAI's API (`gpt-6-luna (API)`) with an OpenAI key, else `codex
//!    exec -m gpt-6-luna` (`gpt-6-luna`) when codex is on `PATH`;
//! 2. Anthropic's API (`claude-haiku (API)`) with an Anthropic key, else
//!    `claude -p --model haiku` (`claude-haiku`) when claude is.
//!
//! The subscription CLIs run from scratch directories so no project
//! instructions leak in, with tools and user config switched off
//! ([`super::tools`] keeps them away from our keys). Unlike the TS CLI,
//! a failure never quotes their output.

use std::path::PathBuf;

use serde_json::Value;

use super::access::Access;
use super::costs::{GPT_6_LUNA, price_of};

/// Bump when the instructions change, so cached summaries are made again.
pub const SUMMARY_PROMPT_VERSION: u32 = 9;

/// The summary exists only to let Jev judge a chat too long for its
/// window, so it keeps exactly the facts the questions depend on.
const INSTRUCTIONS: &str = include_str!("prompts/summary.txt");

/// Codex features switched off for every call: no tools.
pub const NO_TOOLS: &[&str] = &[
    "apps",
    "browser_use",
    "computer_use",
    "image_generation",
    "plugins",
    "multi_agent",
    "view_image",
    "unified_exec",
    "shell_tool",
];

/// `codex exec`'s arguments up to its workspace, as every call starts.
pub fn codex_args() -> Vec<String> {
    let mut args: Vec<String> = [
        "exec",
        "-m",
        "gpt-6-luna",
        "-c",
        "model_reasoning_effort=\"medium\"",
        "--skip-git-repo-check",
        "--ephemeral",
        "--sandbox",
        "read-only",
        "--ignore-user-config",
    ]
    .map(str::to_owned)
    .to_vec();
    for feature in NO_TOOLS {
        args.push("--disable".to_owned());
        args.push((*feature).to_owned());
    }
    args
}

enum Summariser {
    OpenAi(model_api::OpenAi),
    Codex(PathBuf),
    Anthropic(model_api::Anthropic),
    Claude(PathBuf),
}

impl Summariser {
    fn name(&self) -> &'static str {
        match self {
            Self::OpenAi(_) => "gpt-6-luna (API)",
            Self::Codex(_) => "gpt-6-luna",
            Self::Anthropic(_) => "claude-haiku (API)",
            Self::Claude(_) => "claude-haiku",
        }
    }
}

/// `installed()`: at most one per provider, in order.
fn installed(access: &Access) -> Result<Vec<Summariser>, String> {
    let mut found = Vec::new();
    if let Some(openai) = access.openai()? {
        found.push(Summariser::OpenAi(openai));
    } else if let Some(codex) = access.which("codex") {
        found.push(Summariser::Codex(codex));
    }
    if let Some(anthropic) = access.anthropic()? {
        found.push(Summariser::Anthropic(anthropic));
    } else if let Some(claude) = access.which("claude") {
        found.push(Summariser::Claude(claude));
    }
    Ok(found)
}

/// `summariserNames()`.
pub fn names(access: &Access) -> Result<Vec<&'static str>, String> {
    Ok(installed(access)?.iter().map(Summariser::name).collect())
}

/// `summariserAvailable()`.
pub fn available(access: &Access) -> Result<bool, String> {
    Ok(!installed(access)?.is_empty())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub summary: String,
    pub model: String,
    pub usd: f64,
    pub tokens: u64,
}

struct Ran {
    text: String,
    usd: f64,
    tokens: u64,
}

/// `summarise(title, transcript)`.
pub async fn summarise(access: &Access, title: &str, transcript: &str) -> Result<Summary, String> {
    let mut errors = Vec::new();
    let input = format!("Title: {title}\n\n{transcript}");
    let summarisers = installed(access)?;
    if summarisers.is_empty() {
        return Err("Configure an OpenAI or Anthropic API key, or install codex or claude.".into());
    }
    for summariser in &summarisers {
        match run(summariser, access, &input).await {
            Ok(ran) => {
                let summary = chatgpt_core::js::trim(&ran.text);
                if !summary.is_empty() {
                    return Ok(Summary {
                        summary: summary.to_owned(),
                        model: summariser.name().to_owned(),
                        usd: ran.usd,
                        tokens: ran.tokens,
                    });
                }
                errors.push(format!("{}: empty summary", summariser.name()));
            }
            Err(error) => errors.push(format!("{}: {error}", summariser.name())),
        }
    }
    Err(format!("No summariser succeeded. {}", errors.join(" | ")))
}

async fn run(summariser: &Summariser, access: &Access, input: &str) -> Result<Ran, String> {
    match summariser {
        Summariser::OpenAi(openai) => {
            let reply = openai
                .text(INSTRUCTIONS, input, None)
                .await
                .map_err(|error| error.to_string())?;
            Ok(Ran {
                usd: price_of(
                    GPT_6_LUNA,
                    reply.input_tokens,
                    reply.cached_input_tokens,
                    reply.output_tokens,
                ),
                tokens: reply.input_tokens + reply.output_tokens,
                text: reply.text,
            })
        }
        Summariser::Anthropic(anthropic) => {
            let reply = anthropic
                .text(INSTRUCTIONS, input)
                .await
                .map_err(|error| error.to_string())?;
            Ok(Ran {
                usd: (reply.input_tokens + reply.output_tokens * 5) as f64 / 1e6,
                tokens: reply.input_tokens + reply.output_tokens,
                text: reply.text,
            })
        }
        Summariser::Codex(codex) => run_codex(codex, access, input).await,
        Summariser::Claude(claude) => run_claude(claude, access, input).await,
    }
}

/// `codex exec`, whose `--json` events carry the token usage on
/// `turn.completed`; the summary is the file it writes.
async fn run_codex(codex: &std::path::Path, access: &Access, input: &str) -> Result<Ran, String> {
    let dir = tempfile::Builder::new()
        .prefix("chatgpt-cli-codex-")
        .tempdir()
        .map_err(|error| format!("couldn't make a scratch directory: {error}"))?;
    let out_file = dir.path().join("summary.txt");
    let mut args = codex_args();
    args.extend([
        "-C".to_owned(),
        dir.path().display().to_string(),
        "-o".to_owned(),
        out_file.display().to_string(),
        "--json".to_owned(),
        format!("{INSTRUCTIONS}\n\nThe conversation is in the <stdin> block."),
    ]);
    let finished = super::tools::run(
        "codex",
        codex,
        &args,
        input.as_bytes(),
        dir.path(),
        access.path_var().as_deref(),
    )
    .await?;
    if finished.code != Some(0) {
        return Err(format!("codex {}", finished.exit()));
    }
    // Several turns can complete; the last one's usage counts. A malformed
    // event line only costs us the usage figure.
    let events = String::from_utf8_lossy(&finished.stdout);
    let mut usage = Value::Null;
    for line in events
        .lines()
        .filter(|line| line.contains("\"turn.completed\""))
    {
        if let Ok(event) = serde_json::from_str::<Value>(line)
            && let Some(found) = event.get("usage")
        {
            usage = found.clone();
        }
    }
    let count = |name: &str| {
        usage
            .get(name)
            .and_then(Value::as_f64)
            .map_or(0, |n| n.max(0.0) as u64)
    };
    let input_tokens = count("input_tokens");
    // Reasoning tokens are billed as output.
    let output_tokens = count("output_tokens") + count("reasoning_output_tokens");
    let text =
        std::fs::read_to_string(&out_file).map_err(|_| "codex wrote no summary".to_owned())?;
    Ok(Ran {
        text,
        usd: price_of(
            GPT_6_LUNA,
            input_tokens,
            count("cached_input_tokens"),
            output_tokens,
        ),
        tokens: input_tokens + output_tokens,
    })
}

/// `claude -p`, which reports its own API-equivalent cost.
async fn run_claude(claude: &std::path::Path, access: &Access, input: &str) -> Result<Ran, String> {
    // Haiku is plenty for summarising. `--effort low` made it ignore the
    // system prompt (observed 2026-09-27); medium and high behave. Without
    // the setting and MCP flags, every call carries ~33k tokens of context.
    let args: Vec<String> = [
        "-p",
        "--model",
        "haiku",
        "--output-format",
        "json",
        "--tools",
        "",
        "--no-session-persistence",
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--effort",
        "medium",
        "--system-prompt",
        INSTRUCTIONS,
    ]
    .map(str::to_owned)
    .to_vec();
    let finished = super::tools::run(
        "claude -p",
        claude,
        &args,
        input.as_bytes(),
        &std::env::temp_dir(),
        access.path_var().as_deref(),
    )
    .await?;
    let Ok(parsed) = serde_json::from_slice::<Value>(&finished.stdout) else {
        return Err(format!(
            "claude -p {} without a JSON answer",
            finished.exit()
        ));
    };
    if parsed.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err("claude -p failed".to_owned());
    }
    let usage = parsed.get("usage").cloned().unwrap_or(Value::Null);
    let count = |name: &str| {
        usage
            .get(name)
            .and_then(Value::as_f64)
            .map_or(0, |n| n.max(0.0) as u64)
    };
    Ok(Ran {
        text: parsed
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        usd: parsed
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        tokens: count("input_tokens")
            + count("cache_read_input_tokens")
            + count("cache_creation_input_tokens")
            + count("output_tokens"),
    })
}
