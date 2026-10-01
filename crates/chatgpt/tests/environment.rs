//! No HTTP client build may write the process environment
//! (docs/issues/impit-set-var-race.md). Two halves make that true:
//!
//! - `HttpClient::new` and every rebuild after a challenge refuse unless the
//!   variable already holds Chrome's order;
//! - the vendored impit writes the variable only when it differs.
//!
//! So a build that succeeds only ever reads it. These tests pin both halves.

#![allow(clippy::unwrap_used)]

use std::process::Command;

use chatgpt::http::{
    HttpClient, HttpError, PSEUDO_HEADER_ORDER_ENV, RetryPolicy, Secret, environment_prepared,
    pseudo_header_order,
};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

#[test]
fn the_test_environment_holds_chromes_order() {
    let config = std::fs::read_to_string(format!("{ROOT}/.cargo/config.toml")).unwrap();
    let expected = format!(
        "{PSEUDO_HEADER_ORDER_ENV} = {{ value = \"{}\", force = true }}",
        pseudo_header_order()
    );
    assert!(
        config.contains(&expected),
        ".cargo/config.toml must set {expected}"
    );
    assert!(environment_prepared(), "tests run with the [env] value");
}

/// Run in a child process without the variable, by
/// `a_client_is_refused_unless_the_environment_is_prepared`.
#[test]
#[ignore = "run by its parent test in a child process"]
fn child_builds_a_client_without_the_variable() {
    assert!(std::env::var_os(PSEUDO_HEADER_ORDER_ENV).is_none());
    let built = HttpClient::new(
        "http://127.0.0.1:9",
        Secret::new("a=b".into()),
        RetryPolicy::none(),
    );
    assert!(matches!(built, Err(HttpError::EnvironmentNotPrepared)));
    // Refusing must not have written it either.
    assert!(std::env::var_os(PSEUDO_HEADER_ORDER_ENV).is_none());
}

#[test]
fn a_client_is_refused_unless_the_environment_is_prepared() {
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_builds_a_client_without_the_variable",
            "--ignored",
            "--nocapture",
        ])
        .env_remove(PSEUDO_HEADER_ORDER_ENV)
        .status()
        .unwrap();
    assert!(status.success(), "the child test failed: {status}");
}

#[test]
fn the_vendored_impit_writes_the_variable_only_when_it_differs() {
    let source = std::fs::read_to_string(format!("{ROOT}/third_party/impit/src/impit.rs")).unwrap();
    let writes: Vec<usize> = source.match_indices("set_var(").map(|(at, _)| at).collect();
    assert_eq!(writes.len(), 1, "impit sets the environment in one place");
    let guard = source[..writes[0]]
        .rfind("if std::env::var(PSEUDOHEADERS_ORDER_ENV).ok().as_deref() != Some(order.as_str())")
        .expect("the set_var call is inside the chatgpt-cli guard");
    assert!(
        !source[guard..writes[0]].contains('}'),
        "the guard encloses the write"
    );
}

#[test]
fn the_lockfile_builds_impit_from_the_vendored_copy() {
    let lock = std::fs::read_to_string(format!("{ROOT}/Cargo.lock")).unwrap();
    let impit = lock
        .split("[[package]]")
        .find(|package| package.trim_start().starts_with("name = \"impit\"\n"))
        .expect("impit is in Cargo.lock");
    assert!(
        !impit.contains("source = "),
        "impit must come from third_party/impit (a path dependency has no source):{impit}"
    );
}

/// Run one `#[ignore]`d test of this file in a child process whose
/// environment holds `order` (or lacks the variable).
fn run_child(test: &str, order: Option<&str>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", test, "--ignored", "--nocapture"]);
    match order {
        Some(order) => command.env(PSEUDO_HEADER_ORDER_ENV, order),
        None => command.env_remove(PSEUDO_HEADER_ORDER_ENV),
    };
    let status = command.status().unwrap();
    assert!(
        status.success(),
        "{test} failed in its child process: {status}"
    );
}

const WRONG_ORDER: &str = ":path,:method";

/// Run by `a_mismatched_variable_is_refused_before_impit_could_write_it`.
#[test]
#[ignore = "run by its parent test in a child process"]
fn child_refuses_a_mismatched_variable_and_leaves_it_alone() {
    let built = HttpClient::new(
        "http://127.0.0.1:9",
        Secret::new("a=b".into()),
        RetryPolicy::none(),
    );
    assert!(matches!(built, Err(HttpError::EnvironmentNotPrepared)));
    // Unpatched impit would have overwritten it with Chrome's order.
    assert_eq!(std::env::var(PSEUDO_HEADER_ORDER_ENV).unwrap(), WRONG_ORDER);
}

#[test]
fn a_mismatched_variable_is_refused_before_impit_could_write_it() {
    run_child(
        "child_refuses_a_mismatched_variable_and_leaves_it_alone",
        Some(WRONG_ORDER),
    );
}

/// Run by `a_rebuild_after_a_challenge_leaves_the_environment_as_it_was`:
/// requests on a multi-threaded runtime, with a challenge forcing a client
/// rebuild while other tasks run requests.
#[test]
#[ignore = "run by its parent test in a child process"]
fn child_rebuilds_after_a_challenge() {
    use chatgpt::http::{RetryEvent, RetryReason};
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(403).insert_header("cf-mitigated", "challenge"))
            .up_to_n_times(3)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .with_priority(2)
            .mount(&server)
            .await;
        let events: Arc<Mutex<Vec<RetryEvent>>> = Arc::default();
        let seen = Arc::clone(&events);
        let policy = RetryPolicy {
            base_backoff: std::time::Duration::from_millis(1),
            ..RetryPolicy::default()
        };
        let http = Arc::new(
            HttpClient::new(server.uri(), Secret::new("a=b".into()), policy)
                .unwrap()
                .on_retry(move |event| seen.lock().unwrap().push(*event)),
        );
        let requests: Vec<_> = (0..4)
            .map(|_| {
                let http = Arc::clone(&http);
                tokio::spawn(async move { http.get("/x", &[]).await })
            })
            .collect();
        for request in requests {
            assert_eq!(request.await.unwrap().unwrap(), "ok");
        }
        let rebuilds = events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.reason == RetryReason::Challenge)
            .count();
        assert!(rebuilds >= 1, "no challenge rebuild happened");
    });
    assert_eq!(
        std::env::var(PSEUDO_HEADER_ORDER_ENV).unwrap(),
        pseudo_header_order()
    );
}

/// With the variable set before any thread (as `chatgpt daemon run` does),
/// a rebuild goes through the same guard as the first build, and the
/// vendored impit only reads the variable.
#[test]
fn a_rebuild_after_a_challenge_leaves_the_environment_as_it_was() {
    run_child(
        "child_rebuilds_after_a_challenge",
        Some(&pseudo_header_order()),
    );
}
