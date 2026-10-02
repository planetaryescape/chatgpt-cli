//! Asking gpt-6-luna ("Luna") for JSON that fits a schema: the TS CLI's
//! `askLuna` (`src/classify/luna.ts` @ 1b8c950). With an OpenAI key, its
//! Responses API with a strict JSON schema; otherwise `codex exec -m
//! gpt-6-luna --output-schema`, sandboxed read-only with every tool
//! switched off. The input goes as `JSON.stringify` writes it. Luna's
//! calls aren't metered, as in the TS CLI.

use serde_json::Value;

use super::access::Access;

pub async fn ask(
    access: &Access,
    instructions: &str,
    input: &Value,
    schema: &Value,
) -> Result<Value, String> {
    let input = crate::js::stringify(input);
    if let Some(openai) = access.openai()? {
        let reply = openai
            .text(instructions, &input, Some(schema))
            .await
            .map_err(|error| error.to_string())?;
        return serde_json::from_str(&reply.text)
            .map_err(|_| "gpt-6-luna returned invalid JSON".to_owned());
    }
    let codex = access
        .which("codex")
        .ok_or("codex is required for gpt-6-luna.")?;
    let dir = tempfile::Builder::new()
        .prefix("chatgpt-cli-luna-")
        .tempdir()
        .map_err(|error| format!("couldn't make a scratch directory: {error}"))?;
    let schema_file = dir.path().join("schema.json");
    let output_file = dir.path().join("response.json");
    std::fs::write(&schema_file, crate::js::stringify(schema))
        .map_err(|error| format!("couldn't write the schema: {error}"))?;
    let mut args = super::summarise::codex_args();
    args.extend([
        "-C".to_owned(),
        dir.path().display().to_string(),
        "--output-schema".to_owned(),
        schema_file.display().to_string(),
        "-o".to_owned(),
        output_file.display().to_string(),
        instructions.to_owned(),
    ]);
    let finished = super::tools::run(
        "gpt-6-luna",
        &codex,
        &args,
        input.as_bytes(),
        dir.path(),
        access.path_var().as_deref(),
    )
    .await?;
    if finished.code != Some(0) {
        return Err(format!("gpt-6-luna {}", finished.exit()));
    }
    let text = std::fs::read_to_string(&output_file)
        .map_err(|_| "gpt-6-luna wrote no answer".to_owned())?;
    serde_json::from_str(&text).map_err(|_| "gpt-6-luna returned invalid JSON".to_owned())
}
