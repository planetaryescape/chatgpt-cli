//! `archive`, `unarchive`, `delete`, `rename` and `title`, as the TS CLI's
//! (`src/cli.ts`, `src/commands/mutate.ts` and `check.ts` @ 1b8c950) run
//! them: the daemon resolves the chats and runs the Jev guard; the preview
//! and the confirmation happen here; then the daemon changes exactly the
//! chats shown.

use std::process::ExitCode;

use chatgpt_core::Paths;
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{
    ChatAction, Outcome, Request, ResponseData, Row, Selection, SessionChoice, Target,
};

use crate::args::{ChangeArgs, RenameArgs};
use crate::export_cmd::{read_stdin, stdin_ids};
use crate::output::{ProgressLines, note, unexpected};
use crate::prompt;
use crate::reads::{filter, stale_note, text_row};

/// How many chats a preview lists before "… and N more".
pub const PREVIEW_ROWS: usize = 25;

/// Send `request`, drawing the daemon's progress lines as they come.
pub async fn ask_showing_progress(
    paths: &Paths,
    request: Request,
) -> Result<ResponseData, ClientError> {
    let mut progress = ProgressLines::new();
    let answer = chatgpt_launcher::ask(paths, request, |event| progress.show(event)).await;
    progress.close();
    answer
}

/// The ids as given: `None` without any (the filters choose), the ids read
/// from stdin for a lone `-`.
pub fn given_ids(ids: Vec<String>) -> Result<Option<Vec<String>>, ClientError> {
    match ids.as_slice() {
        [] => Ok(None),
        [dash] if dash == "-" => Ok(Some(stdin_ids(&read_stdin()?))),
        _ => Ok(Some(ids)),
    }
}

/// The chats the daemon resolved, after the stale-index note.
pub async fn select(paths: &Paths, selection: Selection) -> Result<Vec<Row>, ClientError> {
    let request = Request::Select {
        selection: Box::new(selection),
    };
    let ResponseData::Rows(answer) = chatgpt_launcher::ask(paths, request, |_| {}).await? else {
        return Err(unexpected());
    };
    stale_note(&answer.synced_at);
    Ok(answer.rows)
}

/// Preview lines: the first [`PREVIEW_ROWS`], then how many more.
pub fn preview(lines: impl Iterator<Item = String>, total: usize) {
    for line in lines.take(PREVIEW_ROWS) {
        note(&line);
    }
    if total > PREVIEW_ROWS {
        note(&format!("… and {} more", total - PREVIEW_ROWS));
    }
}

/// Print a bulk change's failures; exit 1 if there were any.
pub fn finish(outcome: &Outcome) -> ExitCode {
    for failure in &outcome.failures {
        eprintln!("failed: {failure}");
    }
    if outcome.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

pub fn targets(rows: &[Row]) -> Vec<Target> {
    rows.iter()
        .map(|row| Target {
            id: row.id.clone(),
            title: row.title.clone(),
        })
        .collect()
}

pub async fn change(
    paths: &Paths,
    action: ChatAction,
    args: ChangeArgs,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let mut chosen = filter(args.filters);
    // Unarchive works on archived chats unless the caller asked otherwise.
    if action == ChatAction::Unarchive && !chosen.all {
        chosen.archived = true;
    }
    let applying_suggestions =
        action != ChatAction::Unarchive && chosen.suggest.as_deref() == Some(action.as_str());
    let selection = Selection {
        ids: given_ids(args.ids)?,
        filter: chosen,
        pinned: args.pinned,
        exclude_unsure: applying_suggestions,
        allow_unfiltered: false,
    };
    let mut rows = select(paths, selection).await?;
    // Explicit ids bypass filters, so the Jev check stays a second guard.
    if args.check || applying_suggestions {
        if action == ChatAction::Unarchive {
            return Err(crate::reads::invalid(
                "--check applies to archive and delete only.",
            ));
        }
        let access = crate::classify_cmd::model_access();
        let request = Request::JevCheck {
            action,
            ids: rows.iter().map(|row| row.id.clone()).collect(),
            api_key: access.typesafe.clone(),
            session: session.clone(),
            access,
            yes: args.yes,
        };
        let ResponseData::Approved { ids } =
            crate::classify_cmd::ask_showing_progress_and_asking(paths, request).await?
        else {
            return Err(unexpected());
        };
        let approved: std::collections::HashSet<String> = ids.into_iter().collect();
        rows.retain(|row| approved.contains(&row.id));
    }
    apply(paths, action, &rows, args.dry_run, args.yes, session).await
}

/// `applyAction`: preview, confirm, change.
async fn apply(
    paths: &Paths,
    action: ChatAction,
    rows: &[Row],
    dry_run: bool,
    yes: bool,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let count = rows.len();
    if count == 0 {
        note("Nothing matched.");
        return Ok(ExitCode::SUCCESS);
    }
    preview(rows.iter().map(text_row), count);
    let name = action.as_str();
    if dry_run {
        note(&format!("dry run: would {name} {count} conversation(s)."));
        return Ok(ExitCode::SUCCESS);
    }
    let confirmed = yes
        || if action == ChatAction::Delete {
            prompt::confirm_count(
                &format!(
                    "Permanently delete {count} conversation(s)? This cannot be undone. Type {count} to confirm: "
                ),
                count,
            )?
        } else {
            prompt::confirm(&format!("{name} {count} conversation(s)? [y/N] "))?
        };
    if !confirmed {
        note("Cancelled.");
        return Ok(ExitCode::SUCCESS);
    }
    let request = Request::Mutate {
        action,
        targets: targets(rows),
        session,
    };
    let ResponseData::Outcome(outcome) = ask_showing_progress(paths, request).await? else {
        return Err(unexpected());
    };
    Ok(finish(&outcome))
}

pub async fn rename(
    paths: &Paths,
    args: RenameArgs,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let request = Request::Rename {
        reference: args.id,
        title: args.title.clone(),
        archived: args.archived,
        all: args.all,
        session,
    };
    let ResponseData::Renamed {
        old_title,
        synced_at,
        ..
    } = chatgpt_launcher::ask(paths, request, |_| {}).await?
    else {
        return Err(unexpected());
    };
    stale_note(&synced_at);
    note(&format!("renamed \"{old_title}\" → \"{}\"", args.title));
    Ok(ExitCode::SUCCESS)
}

pub async fn title(paths: &Paths, args: RenameArgs) -> Result<ExitCode, ClientError> {
    let request = Request::SetTitle {
        reference: args.id,
        title: args.title.clone(),
        archived: args.archived,
        all: args.all,
    };
    let ResponseData::TitleSaved { id, synced_at } =
        chatgpt_launcher::ask(paths, request, |_| {}).await?
    else {
        return Err(unexpected());
    };
    stale_note(&synced_at);
    note(&format!(
        "Local title saved for {id}: {}",
        chatgpt_core::js::trim(&args.title)
    ));
    Ok(ExitCode::SUCCESS)
}
