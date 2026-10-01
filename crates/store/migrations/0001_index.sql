-- The TS CLI's tables (src/index/*.ts @ 1b8c950), column for column, so the
-- import from its index.db is a straight copy and verdicts read the same rows.
create table conversations (
    id text primary key,
    title text not null,
    create_time text not null,
    update_time text not null,
    is_archived integer not null,
    pinned integer not null,
    project_id text
);

create table meta (key text primary key, value text not null);

create table local_titles (
    id text primary key,
    update_time text not null,
    version integer not null,
    source text not null check (source in ('luna', 'manual')),
    title text not null,
    theme text not null default '',
    updated_at text not null
);

create table transcripts (
    id text primary key,
    update_time text not null,
    render_version integer not null,
    markdown text not null,
    turns integer not null,
    approx_tokens integer not null
);

create table summaries (
    id text primary key,
    update_time text not null,
    prompt_version integer not null,
    summary text not null,
    model text not null
);

create table judgments (
    id text primary key,
    update_time text not null,
    version text not null,
    content_kind text not null,
    answers text not null,
    classified_at text not null,
    topic text generated always as (json_extract(answers, '$.topic.choice')) virtual
);

create table deep_judgments (
    id text primary key,
    update_time text not null,
    questions_version text not null,
    version text not null,
    answers text not null,
    classified_at text not null
);

create table luna_judgments (
    id text primary key,
    update_time text not null,
    questions_version text not null,
    deep_version text not null,
    version integer not null,
    suggestion text not null,
    brainstorm text,
    reason text not null,
    classified_at text not null
);

create table memory_judgments (
    id text primary key,
    input_hash text not null,
    version text not null,
    system_one text not null,
    system_two text,
    classified_at text not null
);
