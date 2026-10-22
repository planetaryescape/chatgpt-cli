//! The model path, both CLIs, side by side: `configure`, `classify`,
//! `titles`, `memory classify` and the Jev guard run in the TypeScript CLI
//! and in the Rust CLI, in the same order, each against its own fakes —
//! chatgpt.com, TypeSafe, OpenAI and Anthropic — and their own stand-in
//! `codex` and `claude` on `PATH`. Their notes, steps, cost lines,
//! prompts, failures and data are compared, the model requests they send
//! (Jev bodies, OpenAI and Anthropic bodies, and the subscription CLIs'
//! arguments and stdin, byte for byte), and the verdicts, titles and memory
//! classifications they end with.
//!
//! Every scenario also states what it expects to have happened: the exit
//! status each side must give, the Jev requests and model calls it must
//! make, and the verdicts, titles and memory classifications both indexes
//! must hold. Agreeing is not enough: two CLIs that both did nothing would
//! agree.
//!
//! The scenarios are the v0.1.5 harness's
//! (`git show v0.1.5:crates/cli/tests/parity_classify.rs`), ported to
//! today's CLI and shaped like the write-path harnesses.
//!
//! Needs bun and the TypeScript CLI's checkout (`CHATGPT_TS_REPO`); skips
//! without them.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};

use fake_chatgpt::models::{ANTHROPIC_KEY, FakeModelApi, OPENAI_KEY};
use fake_chatgpt::typesafe::{API_KEY, FakeTypeSafe};
use fake_chatgpt::{Chat, FakeChatGpt};
use serde_json::{Value, json};
use support::{
    Env, PARITY_PLUGIN, Ts, bun_for, copy_chats, model_calls, normalize, ts_index, ts_repo,
};

/// The v0.1.5 harness's chat set: the chats Jev judges differently (a
/// confident delete, an unsure one, a kept brainstorm, a product one), a
/// long one and a longer one (both summarised), a pinned one (left out),
/// an archived one, and one with a project.
fn chats() -> Vec<Chat> {
    let mut long = Chat::new("e-long", "Long essay", "2024-03-01T05:00:00.000000Z");
    long.text = "word ".repeat(12_000);
    let mut long_guard = Chat::new("g-long", "Long junk diary", "2024-03-01T01:00:00.000000Z");
    long_guard.text = "line ".repeat(13_000);
    let mut pinned = Chat::new("f-pinned", "Pinned junk", "2024-03-01T04:00:00.000000Z");
    pinned.pinned = true;
    let mut project = Chat::new(
        "d-product",
        "My product plan",
        "2024-03-01T07:00:00.000000Z",
    );
    project.gizmo_id = Some("g-p-1".into());
    let mut huge = Chat::new("h-huge", "Huge log", "2024-03-01T03:00:00.000000Z");
    huge.text = "word ".repeat(400_100);
    huge.archived = true;
    vec![
        Chat::new("a-junk", "Junk ping", "2024-03-01T10:00:00.000000Z"),
        Chat::new("b-maybe", "Maybe receipt", "2024-03-01T09:00:00.000000Z"),
        Chat::new("c-idea", "Book idea", "2024-03-01T08:00:00.000000Z"),
        project,
        long,
        pinned,
        Chat::new(
            "i-notitle",
            "A notitle junk chat",
            "2024-03-01T02:00:00.000000Z",
        ),
        long_guard,
        huge,
    ]
}

/// The saved memories both fakes hold: one worth keeping, one from a chat,
/// one about moving and one about a trip.
fn memories() -> Vec<Value> {
    vec![
        json!({ "id": "mem-tea", "content": "Prefers green tea, \"not\" coffee", "updated_at": "2026-09-01T00:00:00Z", "status": "active", "conversation_id": null, "labels": null }),
        json!({ "id": "mem-old", "content": "Is planning last year's trip to Rome", "updated_at": "2026-09-02T00:00:00Z", "status": "active", "conversation_id": "c-idea" }),
        json!({ "id": "mem-move", "content": "Will move house soon", "updated_at": "2026-09-03T00:00:00Z", "status": "active", "conversation_id": null }),
        json!({ "id": "mem-trip", "content": "Is planning a trip to Rome", "updated_at": "2026-09-04T00:00:00Z", "status": "active", "conversation_id": null }),
    ]
}

/// The state both fakes start in: the chats and the memories.
fn set_up(fake: &FakeChatGpt) {
    let mut state = fake.state();
    state.chats = chats();
    state.memories = Some(memories());
}

/// One side's model fakes: Jev, OpenAI, Anthropic, and the stand-in
/// `codex` and `claude` on its `PATH`.
struct Models {
    typesafe: FakeTypeSafe,
    openai: FakeModelApi,
    anthropic: FakeModelApi,
    tools: PathBuf,
    log: PathBuf,
}

impl Models {
    fn start(dir: &Path) -> Self {
        let tools = dir.join("tools");
        let log = dir.join("model-calls");
        support::fake_tools(&tools, &log, "ok", &["codex", "claude"]);
        Self {
            typesafe: FakeTypeSafe::start(),
            openai: FakeModelApi::openai(),
            anthropic: FakeModelApi::anthropic(),
            tools,
            log,
        }
    }

    /// The stand-in tools, in the given mode: `codex` and `claude`, or
    /// none of them.
    fn set_tools(&self, mode: &str, tools: &[&str]) {
        support::set_tools(&self.tools, &self.log, mode, tools);
    }

    /// The stand-ins a scenario runs with. Each scenario sets its own, so
    /// a failing `codex` never outlives the scenario that wanted it.
    fn set_stand_ins(&self, stand_ins: StandIns) {
        match stand_ins {
            StandIns::Answer => self.set_tools("ok", &["codex", "claude"]),
            StandIns::Absent => self.set_tools("ok", &[]),
            StandIns::CodexFails => {
                self.set_tools("fail", &["codex"]);
                support::fake_tools(&self.tools, &self.log, "ok", &["claude"]);
            }
            StandIns::BothFail => self.set_tools("fail", &["codex", "claude"]),
        }
    }

    /// How the fake OpenAI and Anthropic answer a scenario. Each scenario
    /// sets its own, as it sets its stand-ins, so a failure never outlives
    /// the scenario that wanted it.
    fn set_apis(&self, apis: Apis) {
        let (openai, anthropic) = match apis {
            Apis::Answer => (None, None),
            Apis::OpenAiFails => (Some(500), None),
            Apis::BothFail => (Some(500), Some(500)),
        };
        self.openai.state().fail = openai;
        self.anthropic.state().fail = anthropic;
    }

    /// The subscription CLIs' calls, as comparable text, sorted (they run
    /// concurrently).
    fn tool_calls(&self) -> Vec<String> {
        let mut calls: Vec<String> = model_calls(&self.log)
            .into_iter()
            .map(|call| {
                json!({ "tool": call["tool"], "args": call["args"], "stdin": call["stdin"] })
                    .to_string()
            })
            .collect();
        calls.sort();
        calls
    }

    /// The environment each subscription CLI was run with, by name: the
    /// Rust CLI runs them with a sanitised environment, the TS CLI with
    /// its whole one (`docs/issues/classify-followups.md`).
    fn tool_environments(&self) -> Vec<Vec<String>> {
        let mut envs: Vec<Vec<String>> = model_calls(&self.log)
            .into_iter()
            .map(|call| {
                call["env"]
                    .as_array()
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        envs.sort();
        envs
    }
}

/// The bodies a fake recorded, sorted: the two sides' calls run in their
/// own order.
fn sorted(bodies: &[String]) -> Vec<String> {
    let mut bodies = bodies.to_vec();
    bodies.sort();
    bodies
}

/// The exit code, stdout and stderr of one Rust CLI run, with `stdin`
/// what the command reads (a key, for `configure`).
fn rust(
    env: &Env,
    args: &[&str],
    stdin: &str,
    openai: bool,
    anthropic: bool,
) -> (Option<i32>, String, String) {
    use std::io::Write;
    let mut command = env.std_cmd();
    if openai {
        command.env("OPENAI_API_KEY", OPENAI_KEY);
    }
    if anthropic {
        command.env("ANTHROPIC_API_KEY", ANTHROPIC_KEY);
    }
    let mut child = command
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    let output = child.wait_with_output().unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// stderr without what varies run to run: the running counts keep their
/// label, count and cost, a summary loses its duration, `Done in …` its
/// duration, and each side's own config path becomes `<config>`.
fn steady(stderr: &str, config: &Path) -> String {
    let stderr = stderr.replace(&config.display().to_string(), "<config>");
    let mut out = String::new();
    for line in normalize(&stderr).split('\n') {
        let line = match line.strip_prefix("Done in ") {
            Some(rest) => format!(
                "Done in <t>. {}",
                rest.split_once(". ").map_or("", |(_, r)| r)
            ),
            None => line.to_owned(),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// One run of one CLI: what it printed.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// What differs between the two sides, if anything. The exit codes are
/// compared as success or failure, because the Rust CLI's invalid-input
/// errors exit 2 where the TS CLI exits 1 (documented in
/// `docs/issues/classify-followups.md`).
fn difference(ts: &Run, rust: &Run) -> Option<String> {
    let mut out = Vec::new();
    if (ts.code == Some(0)) != (rust.code == Some(0)) {
        out.push(format!("exit: TS {:?}, Rust {:?}", ts.code, rust.code));
    }
    if ts.stdout != rust.stdout {
        out.push(format!(
            "stdout\nTS: {:?}\nRust: {:?}",
            ts.stdout, rust.stdout
        ));
    }
    if ts.stderr != rust.stderr {
        out.push(format!(
            "stderr\nTS:\n{}\nRust:\n{}",
            ts.stderr, rust.stderr
        ));
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

/// The stand-in `codex` and `claude` a scenario runs with.
#[derive(Debug, Clone, Copy, PartialEq)]
enum StandIns {
    /// Both on the `PATH`, both answering.
    Answer,
    /// Neither on the `PATH`: the Anthropic API is the only summariser
    /// left.
    Absent,
    /// Both on the `PATH`; `codex` fails, so a summary falls back to
    /// `claude`.
    CodexFails,
    /// Both on the `PATH`, both failing: a summary has nowhere to go.
    BothFail,
}

/// How the fake OpenAI and Anthropic answer, when a scenario has their
/// keys. A failure is a 500 whose body, `{"error":"secret"}`, the Rust CLI
/// must never quote.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Apis {
    /// Both answer.
    Answer,
    /// OpenAI fails, so a summary falls back to Anthropic.
    OpenAiFails,
    /// Both fail: a summary has nowhere to go.
    BothFail,
}

/// The `failed:` line each side prints when every summariser failed. The
/// TS CLI quotes each provider's body and each tool's stderr; the Rust CLI
/// says only how each failed (`docs/issues/classify-followups.md`,
/// bullet 1). It is the one line of stderr the two sides may differ in.
struct Failure {
    ts: &'static str,
    rust: &'static str,
}

/// One scenario: the command, and what it must do.
struct Scenario {
    args: &'static [&'static str],
    /// What the command reads from stdin.
    stdin: &'static str,
    /// Whether both sides get the OpenAI key: with it they call the fake
    /// OpenAI, without it the stand-in `codex`.
    with_openai: bool,
    /// Whether both sides get the Anthropic key: with it they call the
    /// fake Anthropic, without it the stand-in `claude`.
    with_anthropic: bool,
    /// The stand-in subscription CLIs on the `PATH`, and how they answer.
    stand_ins: StandIns,
    /// How the fake OpenAI and Anthropic answer.
    apis: Apis,
    /// The exit status the TS CLI must give.
    ts_code: i32,
    /// The exit status the Rust CLI must give: 2 where the TS CLI gives 1
    /// (invalid input; `docs/issues/classify-followups.md`).
    rust_code: i32,
    /// The stdout both must print, byte for byte, when it is known.
    stdout: Option<&'static str>,
    /// The Jev requests this scenario must make, on each side.
    jev: usize,
    /// The subscription CLI calls this scenario must make, on each side.
    tools: usize,
    /// The OpenAI requests this scenario must make, on each side.
    openai: usize,
    /// The Anthropic requests this scenario must make, on each side.
    anthropic: usize,
    /// When every summariser fails: the line each side must print instead
    /// of the other's.
    failure: Option<Failure>,
}

/// The scenarios, in the order they run. The order matters: each one
/// builds on what the last left behind, as the v0.1.5 harness's did.
///
/// The counts are what each side must have asked and called by then: the
/// Jev requests, the subscription CLIs' calls, and the model API
/// requests. The exit statuses are the TS CLI's and the Rust CLI's: the
/// Rust CLI exits 2 where the TS CLI exits 1 for invalid input
/// (`docs/issues/classify-followups.md`).
fn scenarios() -> Vec<Scenario> {
    vec![
        // configure: status, a key from stdin, removal, the errors.
        Scenario {
            args: &["configure"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["configure", "anthropic"],
            stdin: "  ant-from-stdin \n",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["configure", "openai"],
            stdin: "sk-from-stdin",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["configure"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["configure", "openai", "--remove"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["configure", "jev", "--remove"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // No provider to remove.
        Scenario {
            args: &["configure", "--remove"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 2,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // An unknown provider.
        Scenario {
            args: &["configure", "grok"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 2,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // A blank key.
        Scenario {
            args: &["configure", "jev"],
            stdin: "   ",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 2,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["configure", "anthropic", "--remove"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // The Jev guard asks Jev about each chat and summarises the long
        // one, so it needs the stand-in model CLIs as well as the fake
        // TypeSafe. It runs before anything else here, so the chats it
        // judges are ones nothing has judged yet: two Jev requests and
        // one summary through `codex`.
        Scenario {
            args: &["delete", "g-long", "a-junk", "--check", "-n"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 2,
            tools: 1,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // classify: a filter, then everything (summaries, follow-up, Luna
        // and titles), then again (all cached), then --redo.
        // The titles of `i-notitle` fail on both sides, so both exit 1.
        Scenario {
            args: &["classify", "--title", "junk", "-y"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 1,
            stdout: Some(""),
            jev: 1,
            tools: 5,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["classify", "-y"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 1,
            stdout: Some(""),
            jev: 5,
            tools: 7,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["classify"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 1,
            stdout: Some(""),
            jev: 0,
            tools: 1,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["classify", "c-idea", "e-long", "--redo"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 2,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // OpenAI fails: the long chat's summary, made again, falls back to
        // Anthropic. Nothing else asks OpenAI (the chat already has its
        // title, and Jev keeps it without Luna), so its failure costs only
        // the attempt, and neither side says it failed.
        Scenario {
            args: &["classify", "e-long", "--redo", "-y"],
            stdin: "",
            with_openai: true,
            with_anthropic: true,
            stand_ins: StandIns::Answer,
            apis: Apis::OpenAiFails,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 1,
            tools: 0,
            openai: 1,
            anthropic: 1,
            failure: None,
        },
        // Both APIs fail: the long chat has no summary, so Jev isn't asked
        // about it and both sides report the summarisers' failures.
        Scenario {
            args: &["classify", "e-long", "--redo", "-y"],
            stdin: "",
            with_openai: true,
            with_anthropic: true,
            stand_ins: StandIns::Answer,
            apis: Apis::BothFail,
            ts_code: 1,
            rust_code: 1,
            stdout: Some(""),
            jev: 0,
            tools: 0,
            openai: 1,
            anthropic: 1,
            failure: Some(Failure {
                ts: r#"failed: e-long Long essay: No summariser succeeded. gpt-6-luna (API): 127.0.0.1 returned 500: {"error":"secret"} | claude-haiku (API): 127.0.0.1 returned 500: {"error":"secret"}"#,
                rust: "failed: e-long Long essay: No summariser succeeded. gpt-6-luna (API): 127.0.0.1 returned 500 | claude-haiku (API): 127.0.0.1 returned 500",
            }),
        },
        // Both subscription CLIs fail: the same, through `codex` and
        // `claude`.
        Scenario {
            args: &["classify", "e-long", "--redo", "-y"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::BothFail,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 1,
            stdout: Some(""),
            jev: 0,
            tools: 2,
            openai: 0,
            anthropic: 0,
            failure: Some(Failure {
                ts: "failed: e-long Long essay: No summariser succeeded. gpt-6-luna: codex exited 1: SENTINEL fake codex failure with private text | claude-haiku: claude -p exited 1: SENTINEL fake claude failure with private text",
                rust: "failed: e-long Long essay: No summariser succeeded. gpt-6-luna: codex exited 1 | claude-haiku: claude -p exited 1 without a JSON answer",
            }),
        },
        // `codex` fails: the long chat's summary, made again, falls back
        // to `claude`. Nothing else asks `codex` (the chat already has its
        // title, and Jev keeps it without Luna), so its failure costs only
        // the attempt. The scenarios after this one ask `codex` again and
        // get an answer.
        Scenario {
            args: &["classify", "e-long", "--redo", "-y"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::CodexFails,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 1,
            tools: 2,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["classify", "--title", "junk", "--pinned", "--limit", "1"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // An unknown chat.
        Scenario {
            args: &["classify", "nope"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 2,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // A bad age.
        Scenario {
            args: &["classify", "--older-than", "3x"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 2,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // titles.
        Scenario {
            args: &["titles"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 1,
            stdout: Some(""),
            jev: 0,
            tools: 1,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["titles", "--title", "idea|maybe", "--redo"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 0,
            tools: 1,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["titles", "f-pinned"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 1,
            stdout: Some(""),
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // memory classify: four quick passes, and one memory's Luna
        // review through `codex`.
        Scenario {
            args: &["memory", "classify"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 4,
            tools: 1,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["memory", "classify", "--format", "json"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &[
                "memory",
                "classify",
                "--format",
                "csv",
                "--suggest",
                "delete",
            ],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["memory", "classify", "--format", "ids", "--limit", "2"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["memory", "classify", "--suggest", "review"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // An unknown format.
        Scenario {
            args: &["memory", "classify", "--format", "xml"],
            stdin: "",
            with_openai: false,
            with_anthropic: false,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 1,
            rust_code: 2,
            stdout: None,
            jev: 0,
            tools: 0,
            openai: 0,
            anthropic: 0,
            failure: None,
        },
        // With API keys: OpenAI and Anthropic instead of the CLIs (the
        // long chat's summary is made again).
        Scenario {
            args: &["classify", "e-long", "--redo", "-y"],
            stdin: "",
            with_openai: true,
            with_anthropic: true,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 1,
            tools: 0,
            openai: 1,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["titles", "c-idea", "d-product", "--redo"],
            stdin: "",
            with_openai: true,
            with_anthropic: true,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 0,
            tools: 0,
            openai: 1,
            anthropic: 0,
            failure: None,
        },
        Scenario {
            args: &["memory", "classify", "--redo", "--format", "ids"],
            stdin: "",
            with_openai: true,
            with_anthropic: true,
            stand_ins: StandIns::Answer,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: None,
            jev: 4,
            tools: 0,
            openai: 1,
            anthropic: 0,
            failure: None,
        },
        // The Anthropic API alone: no OpenAI key and no `codex` on the
        // PATH, so Anthropic is the only summariser left and the long
        // chat's summary comes from it.
        Scenario {
            args: &["classify", "e-long", "--redo", "-y"],
            stdin: "",
            with_openai: false,
            with_anthropic: true,
            stand_ins: StandIns::Absent,
            apis: Apis::Answer,
            ts_code: 0,
            rust_code: 0,
            stdout: Some(""),
            jev: 1,
            tools: 0,
            openai: 0,
            anthropic: 1,
            failure: None,
        },
    ]
}

/// The two CLIs, their two fake chatgpt.coms and their two sets of model
/// fakes, set up alike: the same chats, the same memories, and an index
/// each. `None` when the TypeScript CLI's checkout or a `bun` that runs is
/// missing.
struct Parity {
    env: Env,
    ts: Ts,
    rust_models: Models,
    ts_models: Models,
    rust_config: PathBuf,
    ts_config: PathBuf,
    rust_index: PathBuf,
    ts_db_path: PathBuf,
}

fn parity() -> Option<Parity> {
    let Some(repo) = ts_repo() else {
        eprintln!(
            "skipping the TS parity harness: set CHATGPT_TS_REPO to the TypeScript CLI's checkout"
        );
        return None;
    };
    let mut env = Env::with_fake(Vec::new());
    set_up(env.fake());
    let ts_fake = FakeChatGpt::start(Vec::new());
    set_up(&ts_fake);
    let work = env.home.path().join("parity");
    let rust_models = Models::start(&work.join("rust"));
    let ts_models = Models::start(&work.join("ts"));
    env.tools = rust_models.tools.clone();
    env.model_log = rust_models.log.clone();
    env.extra_env.extend([
        ("TYPESAFE_API_KEY".to_owned(), API_KEY.to_owned()),
        (
            "TYPESAFE_BASE_URL".to_owned(),
            rust_models.typesafe.url.clone(),
        ),
        (
            "CHATGPT_TEST_OPENAI_URL".to_owned(),
            rust_models.openai.url.clone(),
        ),
        (
            "CHATGPT_TEST_ANTHROPIC_URL".to_owned(),
            rust_models.anthropic.url.clone(),
        ),
    ]);
    // The Rust index, synced from its own fake. The background indexer's
    // batch reads fail, so `classify` downloads the transcripts itself, as
    // the TS CLI (with none cached) does.
    env.fake().state().fail_batch = u32::try_from(chats().len().div_ceil(10)).unwrap();
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    assert_eq!(
        env.fake().state().fail_batch,
        0,
        "the indexer tried every chat"
    );
    let rust_index = env.data_dir().join("chatgpt.db");

    // The TS CLI's own index: the same chats.
    let ts_home = work.join("home");
    std::fs::create_dir_all(&ts_home).unwrap();
    let ts_config = work.join("config");
    std::fs::create_dir_all(&ts_config).unwrap();
    std::fs::create_dir_all(work.join("cache")).unwrap();
    let ts_data_home = work.join("data");
    let ts_db_path = ts_index(&ts_data_home);
    assert_eq!(copy_chats(&rust_index, &ts_db_path), chats().len());
    // `CHATGPT_PARITY_SEED_DIFFERENCE=1` breaks the TS side before the
    // scenarios, to show that the harness fails with both sides printed
    // when the two CLIs differ. A pinned chat's title: it is never
    // judged, so the difference is in the notes, not in a model request.
    if std::env::var_os("CHATGPT_PARITY_SEED_DIFFERENCE").is_some() {
        rusqlite::Connection::open(&ts_db_path)
            .unwrap()
            .execute(
                "update conversations set title = 'Pinned junk (seeded)' where id = 'f-pinned'",
                [],
            )
            .unwrap();
    }

    let plugin = work.join("plugin.ts");
    std::fs::write(&plugin, PARITY_PLUGIN).unwrap();
    // The plugin only fires when the file it patches is loaded, so the
    // files it patches are read here first: if either changed, the TS CLI
    // would reach the real service.
    let client = std::fs::read_to_string(repo.join("src/api/client.ts")).unwrap();
    assert!(
        client.contains("const BASE = \"https://chatgpt.com\";"),
        "src/api/client.ts changed: update the parity plugin"
    );
    let model_api = std::fs::read_to_string(repo.join("src/classify/model-api.ts")).unwrap();
    assert!(
        model_api.contains("\"https://api.openai.com/v1/responses\""),
        "src/classify/model-api.ts changed: update the parity plugin"
    );
    assert!(
        model_api.contains("\"https://api.anthropic.com/v1/messages\""),
        "src/classify/model-api.ts changed: update the parity plugin"
    );
    let Some(bun) = bun_for(&ts_home) else {
        eprintln!("skipping the TS parity harness: no bun on PATH runs with a synthetic HOME");
        return None;
    };
    let ts = Ts {
        bun,
        repo,
        work: work.clone(),
        data_home: ts_data_home,
        home: ts_home,
        plugin,
        tools: ts_models.tools.clone(),
        fake_url: ts_fake.url.clone(),
        typesafe_url: Some(ts_models.typesafe.url.clone()),
        openai_url: Some(ts_models.openai.url.clone()),
        anthropic_url: Some(ts_models.anthropic.url.clone()),
        openai_key: false,
        anthropic_key: false,
    };
    let rust_config = env.home.path().join("xdg-config");
    Some(Parity {
        env,
        ts,
        rust_models,
        ts_models,
        rust_config,
        ts_config,
        rust_index,
        ts_db_path,
    })
}

/// The summary instructions both CLIs send, `claude` as its system prompt.
const SUMMARY_PROMPT: &str = include_str!("../../daemon/src/classify/prompts/summary.txt");

/// What one side must have done when `codex` failed on `e-long`'s
/// summary: tried `codex`, then `claude` with the same chat and the summary
/// instructions, saved `claude`'s summary, judged the chat from it, and
/// said nothing of `codex`'s failure, whose stderr carries the fake's
/// sentinel. `calls` are the scenario's tool calls, oldest first.
fn fell_back_to_claude(
    side: &str,
    calls: &[Value],
    db: &rusqlite::Connection,
    jev: &[String],
    stderr: &str,
) {
    tried_codex_then_claude(side, calls);
    assert_eq!(
        calls[1]["args"],
        json!([
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
            SUMMARY_PROMPT,
        ]),
        "{side}: claude's arguments"
    );
    judged_from(
        "claude-haiku",
        "Claude Summary of Title: Long essay",
        side,
        db,
        jev,
        stderr,
    );
}

/// One side's summary calls to the subscription CLIs in a scenario,
/// oldest first: `codex`, then `claude` with the same long chat.
fn tried_codex_then_claude(side: &str, calls: &[Value]) {
    let tools: Vec<&str> = calls
        .iter()
        .map(|call| call["tool"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(tools, ["codex", "claude"], "{side}: the summarisers tried");
    let (codex, claude) = (&calls[0], &calls[1]);
    let stdin = claude["stdin"].as_str().unwrap_or_default();
    assert!(
        stdin.starts_with("Title: Long essay\n\n") && stdin.contains("word word"),
        "{side}: claude was not sent the long chat"
    );
    assert_eq!(
        codex["stdin"], claude["stdin"],
        "{side}: claude was sent another chat than codex"
    );
}

/// What one side must have done when OpenAI failed on `e-long`'s summary:
/// asked OpenAI, then Anthropic with the same chat, saved Anthropic's
/// summary, judged the chat from it, and said nothing of OpenAI's failure.
/// `openai` and `anthropic` are what the side's fakes held before the
/// scenario.
fn fell_back_to_anthropic(
    side: &str,
    models: &Models,
    (openai, anthropic): (usize, usize),
    db: &rusqlite::Connection,
    jev: &[String],
    stderr: &str,
) {
    asked_openai_then_anthropic(side, models, (openai, anthropic));
    judged_from(
        "claude-haiku (API)",
        "Summary of Title: Long essay",
        side,
        db,
        jev,
        stderr,
    );
}

/// One side's summary requests to the model APIs in a scenario: OpenAI
/// once, then Anthropic once, about the long chat, each with the summary
/// instructions. The fakes stamp when each request arrived, so the order
/// is observed across the two servers. `openai` and `anthropic` are what
/// the fakes held before the scenario.
fn asked_openai_then_anthropic(side: &str, models: &Models, (openai, anthropic): (usize, usize)) {
    let openai_state = models.openai.state();
    let anthropic_state = models.anthropic.state();
    assert_eq!(
        openai_state.bodies.len() - openai,
        1,
        "{side}: OpenAI requests"
    );
    assert_eq!(
        anthropic_state.bodies.len() - anthropic,
        1,
        "{side}: Anthropic requests"
    );
    assert!(
        openai_state.arrived[openai] < anthropic_state.arrived[anthropic],
        "{side}: Anthropic was asked before OpenAI"
    );
    let asked: Value = serde_json::from_str(&openai_state.bodies[openai]).unwrap();
    let fallback: Value = serde_json::from_str(&anthropic_state.bodies[anthropic]).unwrap();
    let input = asked["input"].as_str().unwrap_or_default();
    assert!(
        input.starts_with("Title: Long essay\n\n") && input.contains("word word"),
        "{side}: OpenAI was not sent the long chat"
    );
    assert_eq!(
        asked["instructions"], SUMMARY_PROMPT,
        "{side}: OpenAI's instructions"
    );
    assert_eq!(
        fallback.pointer("/messages/0/content"),
        Some(&json!(input)),
        "{side}: Anthropic was sent another chat than OpenAI"
    );
    assert_eq!(
        fallback["system"], SUMMARY_PROMPT,
        "{side}: Anthropic's instructions"
    );
}

/// The end of a fallback, on one side: `model`'s `summary` saved for
/// `e-long`, Jev asked about it once, and the chat judged from it. The
/// fallback is quiet on both sides: only the cost line names the
/// summariser that answered.
fn judged_from(
    model: &str,
    summary: &str,
    side: &str,
    db: &rusqlite::Connection,
    jev: &[String],
    stderr: &str,
) {
    let saved: (String, String) = db
        .query_row(
            "select summary, model from summaries where id = 'e-long'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        saved,
        (summary.to_owned(), model.to_owned()),
        "{side}: the saved summary"
    );
    assert_eq!(jev.len(), 1, "{side}: Jev requests");
    assert!(
        jev[0].contains(summary),
        "{side}: Jev was not asked about {model}'s summary"
    );
    let judged: String = db
        .query_row(
            "select content_kind from judgments where id = 'e-long'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(judged, "summary", "{side}: what Jev judged e-long from");
    assert!(
        stderr.contains(&format!("  summaries ({model}) ")),
        "{side}: {stderr}"
    );
    assert!(!stderr.contains("failed"), "{side}: {stderr}");
    assert!(!stderr.contains("SENTINEL"), "{side}: {stderr}");
    assert!(!stderr.contains("secret"), "{side}: {stderr}");
}

/// One chat's saved judgment, whole: what it was judged from, the
/// answers, and when.
fn judgment(db: &rusqlite::Connection, id: &str) -> Option<(String, String, String, String)> {
    db.query_row(
        "select version, content_kind, answers, classified_at from judgments where id = ?",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .ok()
}

/// What one side must have left when every summariser failed on
/// `e-long`: no summary, and the judgment it had before, not one made
/// from a summary.
fn nothing_saved(
    side: &str,
    db: &rusqlite::Connection,
    before: Option<&(String, String, String, String)>,
) {
    let summaries: u32 = db
        .query_row(
            "select count(*) from summaries where id = 'e-long'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(summaries, 0, "{side}: a summary was saved");
    assert!(before.is_some(), "{side}: e-long had no judgment to keep");
    assert_eq!(
        judgment(db, "e-long").as_ref(),
        before,
        "{side}: e-long's judgment changed"
    );
}

/// `stderr` with the `failed:` line one side must print, `line`, in
/// place of the line itself: the two sides word it differently on
/// purpose, and everything else is compared.
fn expected_failure(side: &str, stderr: &str, line: &str) -> String {
    let found = stderr.split('\n').filter(|shown| *shown == line).count();
    assert_eq!(found, 1, "{side}: expected the line\n{line}\nin\n{stderr}");
    stderr.replace(line, "<every summariser failed>")
}

/// The rows `list --json --all` prints, parsed.
fn rows(stdout: &str) -> Vec<Value> {
    serde_json::from_str(stdout).unwrap()
}

/// What each chat ended up as: its verdict, whether Luna reviewed it,
/// whether it is still unsure, and the title it shows.
fn verdicts(listed: &[Value]) -> Vec<(String, String, bool, bool, String)> {
    listed
        .iter()
        .map(|row| {
            let jev = &row["jev"];
            (
                row["id"].as_str().unwrap_or_default().to_owned(),
                jev["suggestion"].as_str().unwrap_or_default().to_owned(),
                jev["luna"].as_bool().unwrap_or(false),
                jev["unsure"].as_bool().unwrap_or(false),
                row["display_title"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// What each saved memory ended up as: its suggestion and the stage it
/// reached.
fn classifications(stdout: &str) -> Vec<(String, String, String)> {
    let listed: Vec<Value> = serde_json::from_str(stdout).unwrap();
    listed
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap_or_default().to_owned(),
                row["suggestion"].as_str().unwrap_or_default().to_owned(),
                row["stage"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// The TypeScript CLI in a pseudo-terminal, as `support::in_terminal` runs
/// the Rust one: the prompt appears, the answer is typed, and everything
/// the terminal showed is returned.
fn ts_in_terminal(ts: &Ts, args: &[&str], prompt: &str, answer: &str) -> (Option<i32>, String) {
    use std::io::{Read, Write};
    let template = ts.command(args);
    let mut command = std::process::Command::new("script");
    command
        .arg("-q")
        .arg("/dev/null")
        .arg(template.get_program())
        .args(template.get_args())
        .current_dir(&ts.repo)
        .env_clear();
    for (name, value) in template.get_envs() {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let shown = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let shown = std::sync::Arc::clone(&shown);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                shown.lock().unwrap().extend_from_slice(&buffer[..read]);
            }
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !String::from_utf8_lossy(&shown.lock().unwrap()).contains(prompt) {
        assert!(
            std::time::Instant::now() < deadline,
            "the TS CLI never prompted: {}",
            String::from_utf8_lossy(&shown.lock().unwrap())
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(format!("{answer}\n").as_bytes()).unwrap();
    let status = child.wait().unwrap();
    drop(stdin);
    reader.join().unwrap();
    (
        status.code(),
        String::from_utf8_lossy(&shown.lock().unwrap()).replace("\r\n", "\n"),
    )
}

/// The Rust side's download of the 2 MB chat must not cost what the
/// search index's work costs: the TypeScript CLI does the same work in
/// 0.1 s, and the regression this pins cost 18 s on an idle machine and
/// 42 s to 1 m 18 s on a loaded one
/// (`docs/issues/parity-classify-transcript-download-cost.md`). Ten
/// seconds is well under the smallest of those, and a hundred times what
/// the same work costs now.
const DOWNLOAD_BOUND_MS: u64 = 10_000;

/// The duration a "Downloaded N transcript(s) (Xs)" note carries, in
/// milliseconds.
fn downloaded_ms(shown: &str) -> Option<u64> {
    let line = shown.lines().find(|line| line.contains("Downloaded "))?;
    let at = line.rfind(" (")?;
    let text = line[at + 2..].trim_end_matches(')');
    let (minutes, rest) = match text.split_once('m') {
        Some((minutes, rest)) => (minutes.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let seconds: f64 = rest.trim_end_matches('s').parse().ok()?;
    Some(minutes * 60_000 + (seconds * 1000.0).round() as u64)
}

/// A terminal transcript as text: no carriage returns or escape sequences
/// (the TS CLI colours `failed:` lines), a live line's redraws as separate
/// lines, and neither a step's opening line nor its running counts: at a
/// terminal the TS CLI opens a step with its zero count where the Rust CLI
/// shows the label (`docs/issues/classify-followups.md`).
fn plain(shown: &str) -> String {
    let mut out = String::new();
    let mut chars = shown.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {}
            '\u{1b}' => {
                let mut last = None;
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() {
                            last = Some(next);
                            break;
                        }
                    }
                }
                // A cleared line starts a redraw.
                if last == Some('K') && !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The notes of a terminal transcript, without the live progress lines:
/// what both CLIs said, in order.
fn notes(shown: &str, config: &Path) -> Vec<String> {
    plain(shown)
        .lines()
        .map(|line| line.replace(&config.display().to_string(), "<config>"))
        .filter(|line| !line.contains(['█', '░']))
        // A step's opening line: the Rust CLI shows its label where the
        // TS CLI draws its bar at zero
        // (`docs/issues/classify-followups.md`).
        .filter(|line| !line.ends_with('…'))
        // A duration: how long a step took is not the same on the two
        // sides.
        .map(|line| match line.rfind(" (") {
            Some(at)
                if line.ends_with(')')
                    && line[at + 2..line.len() - 1]
                        .chars()
                        .all(|c| c.is_ascii_digit() || "smh.".contains(c)) =>
            {
                line[..at].to_owned()
            }
            _ => line,
        })
        .map(|line| match line.strip_prefix("Done in ") {
            Some(rest) => format!(
                "Done in <t>. {}",
                rest.split_once(". ").map_or("", |(_, r)| r)
            ),
            None => line,
        })
        .collect()
}

#[test]
fn the_model_path_prints_sends_and_saves_as_the_ts_cli_does() {
    let Some(parity) = parity() else {
        return;
    };
    let Parity {
        env,
        mut ts,
        rust_models,
        ts_models,
        rust_config,
        ts_config,
        rust_index,
        ts_db_path,
    } = parity;

    let mut compared = 0;
    for scenario in scenarios() {
        let args = scenario.args;
        // The long chat's summary is made again: through the API, or
        // through `claude` when `codex` fails, or not at all when both fail.
        if (scenario.with_openai
            || scenario.with_anthropic
            || matches!(
                scenario.stand_ins,
                StandIns::CodexFails | StandIns::BothFail
            ))
            && args.first() == Some(&"classify")
        {
            for db in [
                env.index_db(),
                rusqlite::Connection::open(&ts_db_path).unwrap(),
            ] {
                db.execute("delete from summaries where id = 'e-long'", [])
                    .unwrap();
            }
        }
        ts.openai_key = scenario.with_openai;
        ts.anthropic_key = scenario.with_anthropic;
        rust_models.set_stand_ins(scenario.stand_ins);
        ts_models.set_stand_ins(scenario.stand_ins);
        rust_models.set_apis(scenario.apis);
        ts_models.set_apis(scenario.apis);
        // The seeded difference reaches the first fallback too, and first:
        // the TS side's fallback fails as well, so it has no summariser
        // left.
        if (scenario.stand_ins == StandIns::CodexFails || scenario.apis == Apis::OpenAiFails)
            && std::env::var_os("CHATGPT_PARITY_SEED_DIFFERENCE").is_some()
        {
            ts_models.set_tools("fail", &["codex", "claude"]);
            ts_models.set_apis(Apis::BothFail);
        }
        let rust_judged = judgment(&env.index_db(), "e-long");
        let ts_judged = judgment(&rusqlite::Connection::open(&ts_db_path).unwrap(), "e-long");
        let rust_before = rust_models.typesafe.calls();
        let ts_before = ts_models.typesafe.calls();
        let rust_tools_before = model_calls(&rust_models.log).len();
        let ts_tools_before = model_calls(&ts_models.log).len();
        let rust_openai_before = rust_models.openai.calls();
        let ts_openai_before = ts_models.openai.calls();
        let rust_anthropic_before = rust_models.anthropic.calls();
        let ts_anthropic_before = ts_models.anthropic.calls();

        let (ts_code, ts_out, ts_err) = ts.run_with_stdin(args, scenario.stdin);
        let (rust_code, rust_out, rust_err) = rust(
            &env,
            args,
            scenario.stdin,
            scenario.with_openai,
            scenario.with_anthropic,
        );
        env.wait_for_indexer();

        // The proof: the two sides, side by side.
        eprintln!(
            "======== {args:?} (openai key: {}, anthropic key: {}, stand-ins: {:?}, APIs: {:?})",
            scenario.with_openai, scenario.with_anthropic, scenario.stand_ins, scenario.apis
        );
        eprintln!("--- TS exit: {ts_code:?}  stdout: {ts_out:?}");
        eprintln!("--- TS stderr:\n{ts_err}");
        eprintln!("--- Rust exit: {rust_code:?}  stdout: {rust_out:?}");
        eprintln!("--- Rust stderr:\n{rust_err}");

        let mut ts_run = Run {
            code: ts_code,
            stdout: ts_out.replace(&ts_config.display().to_string(), "<config>"),
            stderr: steady(&ts_err, &ts_config),
        };
        let mut rust_run = Run {
            code: rust_code,
            stdout: rust_out.replace(&rust_config.display().to_string(), "<config>"),
            stderr: steady(&rust_err, &rust_config),
        };
        // Every summariser failed: each side prints its own line, and the
        // rest of stderr is compared as usual.
        if let Some(failure) = &scenario.failure {
            ts_run.stderr = expected_failure("TS", &ts_run.stderr, failure.ts);
            rust_run.stderr = expected_failure("Rust", &rust_run.stderr, failure.rust);
        }
        // The Rust CLI never quotes a provider's body or a tool's stderr;
        // the TS CLI does, when a failure surfaces.
        for secret in ["secret", "SENTINEL"] {
            assert!(
                !rust_err.contains(secret),
                "{args:?}: the Rust CLI quoted {secret:?}:\n{rust_err}"
            );
        }
        if scenario.failure.is_some() {
            assert!(
                ts_err.contains("secret") || ts_err.contains("SENTINEL"),
                "{args:?}: the TS CLI no longer quotes the failure:\n{ts_err}"
            );
        }
        let found = difference(&ts_run, &rust_run);
        assert!(
            found.is_none(),
            "{args:?} differs:\n{}",
            found.unwrap_or_default()
        );
        assert_eq!(ts_code, Some(scenario.ts_code), "{args:?} TS exit");
        assert_eq!(rust_code, Some(scenario.rust_code), "{args:?} Rust exit");
        if let Some(stdout) = scenario.stdout {
            assert_eq!(ts_run.stdout, stdout, "{args:?} TS stdout");
            assert_eq!(rust_run.stdout, stdout, "{args:?} Rust stdout");
        }

        // The requests each side made, and the ones it must have made.
        let rust_jev = rust_models.typesafe.calls() - rust_before;
        let ts_jev = ts_models.typesafe.calls() - ts_before;
        assert_eq!(rust_jev, ts_jev, "{args:?} Jev requests");
        let rust_tools = model_calls(&rust_models.log).len() - rust_tools_before;
        let ts_tools = model_calls(&ts_models.log).len() - ts_tools_before;
        assert_eq!(rust_tools, ts_tools, "{args:?} codex and claude calls");
        let rust_openai = rust_models.openai.calls() - rust_openai_before;
        let ts_openai = ts_models.openai.calls() - ts_openai_before;
        assert_eq!(rust_openai, ts_openai, "{args:?} OpenAI requests");
        let rust_anthropic = rust_models.anthropic.calls() - rust_anthropic_before;
        let ts_anthropic = ts_models.anthropic.calls() - ts_anthropic_before;
        assert_eq!(rust_anthropic, ts_anthropic, "{args:?} Anthropic requests");
        assert_eq!(rust_jev, scenario.jev, "{args:?} Jev requests");
        assert_eq!(
            rust_tools, scenario.tools,
            "{args:?} codex and claude calls"
        );
        assert_eq!(rust_openai, scenario.openai, "{args:?} OpenAI requests");
        assert_eq!(
            rust_anthropic, scenario.anthropic,
            "{args:?} Anthropic requests"
        );
        if scenario.apis == Apis::OpenAiFails {
            fell_back_to_anthropic(
                "Rust",
                &rust_models,
                (rust_openai_before, rust_anthropic_before),
                &env.index_db(),
                &rust_models.typesafe.state().bodies[rust_before..],
                &rust_err,
            );
            fell_back_to_anthropic(
                "TS",
                &ts_models,
                (ts_openai_before, ts_anthropic_before),
                &rusqlite::Connection::open(&ts_db_path).unwrap(),
                &ts_models.typesafe.state().bodies[ts_before..],
                &ts_err,
            );
        }
        if scenario.apis == Apis::BothFail {
            asked_openai_then_anthropic(
                "Rust",
                &rust_models,
                (rust_openai_before, rust_anthropic_before),
            );
            asked_openai_then_anthropic("TS", &ts_models, (ts_openai_before, ts_anthropic_before));
        }
        if scenario.stand_ins == StandIns::BothFail {
            tried_codex_then_claude("Rust", &model_calls(&rust_models.log)[rust_tools_before..]);
            tried_codex_then_claude("TS", &model_calls(&ts_models.log)[ts_tools_before..]);
        }
        if scenario.failure.is_some() {
            nothing_saved("Rust", &env.index_db(), rust_judged.as_ref());
            nothing_saved(
                "TS",
                &rusqlite::Connection::open(&ts_db_path).unwrap(),
                ts_judged.as_ref(),
            );
        }
        if scenario.stand_ins == StandIns::CodexFails {
            fell_back_to_claude(
                "Rust",
                &model_calls(&rust_models.log)[rust_tools_before..],
                &env.index_db(),
                &rust_models.typesafe.state().bodies[rust_before..],
                &rust_err,
            );
            fell_back_to_claude(
                "TS",
                &model_calls(&ts_models.log)[ts_tools_before..],
                &rusqlite::Connection::open(&ts_db_path).unwrap(),
                &ts_models.typesafe.state().bodies[ts_before..],
                &ts_err,
            );
        }

        compared += 1;
    }

    // The config files, written the same way.
    let rust_config_file = rust_config.join("chatgpt-cli/config.json");
    let ts_config_file = ts_config.join("chatgpt-cli/config.json");
    assert_eq!(
        std::fs::read_to_string(&rust_config_file).unwrap(),
        std::fs::read_to_string(&ts_config_file).unwrap(),
        "the config files differ"
    );

    // The same model requests, byte for byte.
    let jev = sorted(&rust_models.typesafe.state().bodies);
    assert!(jev.len() > 10, "{}", jev.len());
    assert_eq!(
        jev,
        sorted(&ts_models.typesafe.state().bodies),
        "the two CLIs asked Jev different questions"
    );
    let tool_calls = rust_models.tool_calls();
    assert!(tool_calls.len() > 5, "{}", tool_calls.len());
    assert_eq!(
        tool_calls,
        ts_models.tool_calls(),
        "the two CLIs sent the subscription CLIs different calls"
    );
    // The one difference the harness does not compare, because it is
    // deliberate: the Rust CLI runs `codex` and `claude` with only what
    // they need to find their own login, the TS CLI with its whole
    // environment, API keys included
    // (`docs/issues/classify-followups.md`, bullet 2).
    let rust_envs = rust_models.tool_environments();
    let ts_envs = ts_models.tool_environments();
    assert!(!rust_envs.is_empty());
    for env in &rust_envs {
        for secret in [
            "TYPESAFE_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "CHATGPT_TEST_COOKIE",
        ] {
            assert!(
                !env.iter().any(|name| name == secret),
                "{secret} leaked into a subscription CLI: {env:?}"
            );
        }
    }
    assert!(
        ts_envs
            .iter()
            .any(|env| env.iter().any(|name| name == "TYPESAFE_API_KEY")),
        "the TS CLI did not pass its whole environment: {ts_envs:?}"
    );
    let openai = sorted(&rust_models.openai.state().bodies);
    assert!(!openai.is_empty());
    assert_eq!(
        openai,
        sorted(&ts_models.openai.state().bodies),
        "the two CLIs sent OpenAI different requests"
    );
    assert_eq!(
        sorted(&rust_models.anthropic.state().bodies),
        sorted(&ts_models.anthropic.state().bodies),
        "the two CLIs sent Anthropic different requests"
    );

    // The same verdicts, titles and memory classifications.
    let (_, ts_list, _) = ts.run(&["list", "--json", "--all"]);
    let (rust_code, rust_list, _) = rust(&env, &["list", "--json", "--all"], "", false, false);
    assert_eq!(rust_code, Some(0));
    assert_eq!(rows(&rust_list), rows(&ts_list), "list --json --all");
    let rust_verdicts = verdicts(&rows(&rust_list));
    let ts_verdicts = verdicts(&rows(&ts_list));
    assert_eq!(rust_verdicts, ts_verdicts, "the verdicts differ");
    eprintln!("--- the verdicts: {rust_verdicts:#?}");
    // What both indexes must hold: Jev's verdicts, Luna's reviews, and
    // the local titles. `f-pinned` and `h-huge` were never judged: the
    // first is pinned and the second archived.
    assert_eq!(
        rust_verdicts,
        vec![
            (
                "a-junk".to_owned(),
                "delete".to_owned(),
                false,
                false,
                "Luna: Junk ping".to_owned()
            ),
            (
                "b-maybe".to_owned(),
                "archive".to_owned(),
                false,
                false,
                "Luna: Maybe receipt".to_owned()
            ),
            (
                "c-idea".to_owned(),
                "keep".to_owned(),
                false,
                false,
                "Luna: Book idea".to_owned()
            ),
            (
                "d-product".to_owned(),
                "keep".to_owned(),
                true,
                false,
                "Luna: My product plan".to_owned()
            ),
            (
                "e-long".to_owned(),
                "keep".to_owned(),
                false,
                false,
                "Luna: Long essay".to_owned()
            ),
            (
                "f-pinned".to_owned(),
                String::new(),
                false,
                false,
                "Pinned junk".to_owned()
            ),
            (
                "h-huge".to_owned(),
                String::new(),
                false,
                false,
                "Huge log".to_owned()
            ),
            (
                "i-notitle".to_owned(),
                "delete".to_owned(),
                false,
                false,
                "A notitle junk chat".to_owned()
            ),
            (
                "g-long".to_owned(),
                "delete".to_owned(),
                false,
                false,
                "Luna: Long junk diary".to_owned()
            ),
        ]
    );
    let (_, ts_memories, _) = ts.run(&["memory", "classify", "--format", "json"]);
    let (rust_code, rust_memories, _) = rust(
        &env,
        &["memory", "classify", "--format", "json"],
        "",
        false,
        false,
    );
    assert_eq!(rust_code, Some(0));
    assert_eq!(
        classifications(&rust_memories),
        classifications(&ts_memories),
        "the memory classifications differ"
    );
    eprintln!(
        "--- the memory classifications: {:#?}",
        classifications(&rust_memories)
    );
    // The memory of a kept thing stays, the ones that are gone or that
    // Luna wants a look at are marked as such.
    assert_eq!(
        classifications(&rust_memories),
        vec![
            ("mem-tea".to_owned(), "keep".to_owned(), "quick".to_owned()),
            ("mem-old".to_owned(), "delete".to_owned(), "deep".to_owned()),
            (
                "mem-move".to_owned(),
                "review".to_owned(),
                "deep".to_owned()
            ),
            (
                "mem-trip".to_owned(),
                "delete".to_owned(),
                "deep".to_owned()
            ),
        ]
    );
    // The local titles both indexes hold, and the summaries both cached.
    let expected_titles = vec![
        ("a-junk".to_owned(), "Luna: Junk ping".to_owned()),
        ("b-maybe".to_owned(), "Luna: Maybe receipt".to_owned()),
        ("c-idea".to_owned(), "Luna: Book idea".to_owned()),
        ("d-product".to_owned(), "Luna: My product plan".to_owned()),
        ("e-long".to_owned(), "Luna: Long essay".to_owned()),
        ("g-long".to_owned(), "Luna: Long junk diary".to_owned()),
    ];
    for (side, db) in [
        (
            "Rust index",
            &rusqlite::Connection::open(&rust_index).unwrap(),
        ),
        (
            "TS index",
            &rusqlite::Connection::open(&ts_db_path).unwrap(),
        ),
    ] {
        let titles: Vec<(String, String)> = db
            .prepare("select id, title from local_titles order by id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(titles, expected_titles, "{side} local titles");
        // `i-notitle`'s title failed on both sides, and the two long
        // chats are the two summaries.
        let summaries: Vec<String> = db
            .prepare("select id from summaries order by id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(
            summaries,
            vec!["e-long".to_owned(), "g-long".to_owned()],
            "{side} summaries"
        );
    }

    let anthropic = sorted(&rust_models.anthropic.state().bodies);
    assert!(!anthropic.is_empty(), "the Anthropic API was never called");
    eprintln!(
        "TS parity: {compared} model-path commands matched; {} Jev, {} codex/claude, {} OpenAI and {} Anthropic requests identical",
        jev.len(),
        tool_calls.len(),
        openai.len(),
        anthropic.len()
    );

    // The question before a large batch of summaries, answered no at a
    // terminal: both CLIs ask it, and both skip the long chat.
    ts.openai_key = false;
    ts.anthropic_key = false;
    // The stand-in CLIs are back: the last scenario took them off the
    // PATH to reach the Anthropic API.
    env.set_tools("ok", &["codex", "claude"]);
    ts_models.set_tools("ok", &["codex", "claude"]);
    let args = ["classify", "h-huge", "--archived"];
    let (rust_code, rust_shown) = support::in_terminal(&env, &args, None, "Go ahead? [y/N] ", "n");
    let (ts_code, ts_shown) = ts_in_terminal(&ts, &args, "Go ahead? [y/N] ", "n");
    eprintln!("======== {args:?} (at a terminal)");
    eprintln!("--- TS exit: {ts_code:?}\n{ts_shown}");
    eprintln!("--- Rust exit: {rust_code:?}\n{rust_shown}");
    let rust_notes = notes(&rust_shown, &rust_config);
    let ts_notes = notes(&ts_shown, &ts_config);
    assert_eq!(rust_code, ts_code, "the terminal run's exit status");
    assert_eq!(rust_notes, ts_notes, "the terminal run's notes");
    // The download must not cost what the search index's work costs.
    let rust_download = downloaded_ms(&rust_shown).expect("the Rust side's download note");
    let ts_download = downloaded_ms(&ts_shown).expect("the TS side's download note");
    eprintln!("======== the download: Rust {rust_download}ms, TS {ts_download}ms");
    assert!(
        rust_download <= DOWNLOAD_BOUND_MS,
        "the Rust side's download took {rust_download}ms (the bound is {DOWNLOAD_BOUND_MS}ms): \
         the classification pass is doing the search index's work again"
    );
    let rust_plain = plain(&rust_shown);
    let ts_plain = plain(&ts_shown);
    assert!(
        rust_plain.contains(
            "Go ahead? [y/N] n\nSkipping those in step 3; everything else will still be judged.\n"
        ),
        "{rust_plain}"
    );
    assert!(
        ts_plain.contains(
            "Go ahead? [y/N] n\nSkipping those in step 3; everything else will still be judged.\n"
        ),
        "{ts_plain}"
    );
    assert!(
        rust_plain.contains("failed: h-huge Huge log: no current summary; run classify first.\n"),
        "{rust_plain}"
    );
    assert!(
        ts_plain.contains("failed: h-huge Huge log: no current summary; run classify first.\n"),
        "{ts_plain}"
    );
}

/// The TypeScript CLI must not be able to reach a real model service: the
/// OpenAI and Anthropic base URLs are hard-coded in its source, so the
/// plugin points them at their fakes, and the keys it carries are the
/// fakes'. The environment it runs with is checked as it is built, and a
/// command that asks OpenAI is shown to reach the fake, not a real
/// service.
#[test]
fn the_ts_cli_cannot_reach_a_real_model_service() {
    let Some(parity) = parity() else {
        return;
    };
    let Parity {
        env,
        ts,
        ts_models,
        rust_models,
        ..
    } = parity;

    let environment = |ts: &Ts| -> Vec<(String, Option<String>)> {
        ts.command(&["--help"])
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    };
    let value = |env: &[(String, Option<String>)], name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };

    // The sealed environment: the fake's model APIs, and the fake's keys
    // only when the harness asks for them.
    let mut with_keys = ts.clone();
    with_keys.openai_key = true;
    with_keys.anthropic_key = true;
    let sealed = environment(&with_keys);
    assert_eq!(
        value(&sealed, "PARITY_OPENAI_BASE"),
        Some(Some(ts_models.openai.url.clone())),
        "the TS CLI's OpenAI base URL is not the fake's"
    );
    assert_eq!(
        value(&sealed, "PARITY_ANTHROPIC_BASE"),
        Some(Some(ts_models.anthropic.url.clone())),
        "the TS CLI's Anthropic base URL is not the fake's"
    );
    assert_eq!(
        value(&sealed, "OPENAI_API_KEY"),
        Some(Some(OPENAI_KEY.to_owned())),
        "the TS CLI's OpenAI key is not the fake's"
    );
    assert_eq!(
        value(&sealed, "ANTHROPIC_API_KEY"),
        Some(Some(ANTHROPIC_KEY.to_owned())),
        "the TS CLI's Anthropic key is not the fake's"
    );
    // Without the keys, it has none: it falls back on the subscription
    // CLIs, and a key of the developer's could never reach a provider.
    let without = environment(&ts);
    for name in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"] {
        assert_eq!(
            value(&without, name),
            Some(None),
            "the TS CLI's environment still carries {name}"
        );
    }
    // Without a fake, the model APIs point at an address nothing
    // answers, never at the real provider.
    let unreachable = Ts {
        openai_url: None,
        anthropic_url: None,
        ..ts.clone()
    };
    let sealed = environment(&unreachable);
    for name in ["PARITY_OPENAI_BASE", "PARITY_ANTHROPIC_BASE"] {
        assert_eq!(
            value(&sealed, name),
            Some(Some(support::UNREACHABLE.to_owned())),
            "the TS CLI's {name} could reach the real service"
        );
    }

    // And the end to end: a command that asks OpenAI reaches the fake,
    // which records the request. Had the URL stayed the hard-coded one,
    // the fake would have seen nothing and the request would have carried
    // the chat to api.openai.com.
    let mut with_keys = ts.clone();
    with_keys.openai_key = true;
    with_keys.anthropic_key = true;
    let before = ts_models.openai.calls();
    let (code, _out, err) = with_keys.run(&["classify", "c-idea", "--redo", "-y"]);
    let after = ts_models.openai.calls();
    eprintln!("--- the TS CLI asked the fake OpenAI {after} time(s)");
    assert!(
        after > before,
        "the TS CLI's OpenAI request did not reach the fake (exit {code:?}): {err}"
    );
    // The request carried the chat, not a key: the fake answers only the
    // fake's key, so a request it answered had that key.
    assert!(
        ts_models.openai.state().bodies[before..]
            .iter()
            .all(|body| body.contains("Book idea")),
        "the TS CLI's OpenAI request did not carry the chat it asked about"
    );
    // The Rust side is sealed the same way: its model APIs answer only the
    // fake's key, and the fake saw its request too.
    let rust_before = rust_models.openai.calls();
    let (code, _out, err) = rust(
        &env,
        &["classify", "c-idea", "--redo", "-y"],
        "",
        true,
        true,
    );
    assert!(
        rust_models.openai.calls() > rust_before,
        "the Rust CLI's OpenAI request did not reach the fake (exit {code:?}): {err}"
    );
}
