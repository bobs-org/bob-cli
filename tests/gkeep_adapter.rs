//! `bob gkeep` adapter harness smoke tests: the shared `FakeAdapter`.

mod gkeep_support;

use std::{
    io::Write,
    process::{Command, Stdio},
    time::Instant,
};

use gkeep_support::{
    archive_ok, error_response, exchange_ok, note, ping_ok, snapshot_ok,
    FakeAdapter, GkeepEnv,
};

/// Pipe `request` into the fake adapter and collect its output.
///
/// A just-written executable can report transient `ETXTBSY` under
/// concurrent load; that one error retries like `spawn_adapter`.
fn run_fake(path: &std::path::Path, request: &str) -> std::process::Output {
    let mut attempts = 0;
    let mut child = loop {
        match Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => break child,
            Err(error)
                if error.raw_os_error() == Some(26) && attempts < 100 =>
            {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => panic!("spawn fake adapter: {error}"),
        }
    };
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(request.as_bytes())
        .expect("write request");
    child.wait_with_output().expect("wait for fake adapter")
}

#[test]
fn fake_adapter_serves_ping_and_records_the_call() {
    let env = GkeepEnv::new("bob-cli-gkeep-fake-ping");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("ping", &ping_ok());

    let output = run_fake(fake.path(), r#"{"protocol":1,"op":"ping"}"#);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), ping_ok());
    assert_eq!(fake.request("ping", 1), r#"{"protocol":1,"op":"ping"}"#);
    assert_eq!(fake.argv("ping", 1), "");
    assert_eq!(fake.call_count(), 1);
}

#[test]
fn fake_adapter_prefers_nth_responses() {
    let env = GkeepEnv::new("bob-cli-gkeep-fake-nth");
    let fake = FakeAdapter::new(&env, "adapter");
    let first = snapshot_ok(
        "bryanbugyi34@gmail.com",
        vec![note("First").id("n1").build()],
    );
    let second = snapshot_ok(
        "bryanbugyi34@gmail.com",
        vec![note("Second").id("n2").build()],
    );
    fake.respond("snapshot", &first);
    fake.respond_nth("snapshot", 2, &second);

    let one = run_fake(fake.path(), r#"{"protocol":1,"op":"snapshot"}"#);
    let two = run_fake(fake.path(), r#"{"protocol":1,"op":"snapshot"}"#);
    assert_eq!(String::from_utf8_lossy(&one.stdout), first);
    assert_eq!(String::from_utf8_lossy(&two.stdout), second);
    assert_eq!(fake.call_count(), 2);
}

#[test]
fn fake_adapter_exit_is_a_crash_without_output() {
    let env = GkeepEnv::new("bob-cli-gkeep-fake-exit");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("archive", &archive_ok(vec![("n1", "archived")]));
    fake.set_exit("archive", 3);

    let output =
        run_fake(fake.path(), r#"{"protocol":1,"op":"archive","notes":[]}"#);
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());

    fake.clear_exit("archive");
    let output =
        run_fake(fake.path(), r#"{"protocol":1,"op":"archive","notes":[]}"#);
    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
}

#[test]
fn fake_adapter_honors_sleep() {
    let env = GkeepEnv::new("bob-cli-gkeep-fake-sleep");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("ping", &ping_ok());
    fake.set_sleep("ping", "0.2");

    let started = Instant::now();
    let output = run_fake(fake.path(), r#"{"protocol":1,"op":"ping"}"#);
    assert!(output.status.success());
    assert!(
        started.elapsed().as_millis() >= 150,
        "the sleep file delays the response"
    );
}

#[test]
fn fake_adapter_serves_errors_and_exchange() {
    let env = GkeepEnv::new("bob-cli-gkeep-fake-misc");
    let fake = FakeAdapter::new(&env, "adapter");
    fake.respond("snapshot", &error_response("auth", "login expired"));
    fake.respond("exchange", &exchange_ok("aas_et/new-token"));

    let denied = run_fake(fake.path(), r#"{"protocol":1,"op":"snapshot"}"#);
    assert!(denied.status.success());
    assert_eq!(
        String::from_utf8_lossy(&denied.stdout),
        error_response("auth", "login expired")
    );

    let exchanged = run_fake(
        fake.path(),
        r#"{"protocol":1,"op":"exchange","email":"a@b.c"}"#,
    );
    assert!(exchanged.status.success());
    assert_eq!(
        String::from_utf8_lossy(&exchanged.stdout),
        exchange_ok("aas_et/new-token")
    );
}

#[test]
fn note_builder_shapes_a_keep_note() {
    let value = note("Hardware store")
        .id("list-9")
        .created("2026-09-26T08:02:00Z")
        .edited("2026-09-27T09:00:00Z")
        .pinned()
        .label("errands")
        .list(vec![
            ("wood screws", false, false),
            ("sandpaper", true, false),
        ])
        .attachment("image", Some("RECEIPT TOTAL 12.99"))
        .url("https://keep.google.com/u/0/#NOTE/list-9")
        .build();

    assert_eq!(value["id"], serde_json::json!("list-9"));
    assert_eq!(value["kind"], serde_json::json!("list"));
    assert_eq!(
        value["content"]["title"],
        serde_json::json!("Hardware store")
    );
    assert_eq!(value["content"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        value["content"]["items"][1]["checked"],
        serde_json::json!(true)
    );
    assert_eq!(value["pinned"], serde_json::json!(true));
    assert_eq!(value["labels"], serde_json::json!(["errands"]));
    assert_eq!(
        value["attachments"][0]["extracted_text"],
        serde_json::json!("RECEIPT TOTAL 12.99")
    );
    assert_eq!(value["created"], serde_json::json!("2026-09-26T08:02:00Z"));
    assert_eq!(value["edited"], serde_json::json!("2026-09-27T09:00:00Z"));
    assert!(value["url"].as_str().unwrap().contains("keep.google.com"));
}
