#![cfg(feature = "integration")]

use std::path::PathBuf;

use ld_domain::Engine;
use ld_domain::clock::Stamp;
use ld_domain::config::{Config, ConnectionName, HistoryConfig, HistoryLimit};
use ld_domain::history::{Entry, Histories};
use ld_domain::ports::Store;
use ld_domain::saved::{QueryName, SavedQuery};
use ld_outbound::history::FileStore;

/// Under the target dir, so runs leave nothing behind and cannot collide.
#[allow(
    clippy::expect_used,
    reason = "allow-expect-in-tests only covers #[test]-attributed items, not helpers they call"
)]
fn scratch(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ignored = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

#[allow(clippy::expect_used, reason = "see scratch")]
fn name(name: &str) -> ConnectionName {
    ConnectionName::try_new(name).expect("a valid name")
}

#[allow(clippy::expect_used, reason = "see scratch")]
fn keeping(rows: u32) -> Config {
    Config {
        history: HistoryConfig {
            limit: HistoryLimit::try_new(rows).expect("in range"),
        },
        ..Config::default()
    }
}

fn entries(of: &[&str]) -> Vec<Entry> {
    (1_764_153_015..)
        .zip(of)
        .map(|(seconds, statement)| Entry {
            at: Some(Stamp::from_unix(seconds)),
            database: Some("shop".to_owned()),
            ..Entry::bare(statement, Engine::Postgres)
        })
        .collect()
}

fn statements<'a>(histories: &'a Histories, connection: &str) -> Vec<&'a str> {
    histories
        .of(&name(connection))
        .iter()
        .map(|entry| entry.statement.as_str())
        .collect()
}

#[tokio::test]
async fn what_one_run_saves_the_next_loads() {
    let path = scratch("history-roundtrip").join("state/history.toml");
    let store = FileStore::at(Some(path.clone()));

    store
        .save(&name("dev"), &entries(&["SELECT 2;", "SELECT\n1;"]))
        .await
        .expect("the first save creates the directory and the file");

    let (histories, diagnostics) = FileStore::at(Some(path)).load(&keeping(100)).await;

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        histories.of(&name("dev")),
        entries(&["SELECT 2;", "SELECT\n1;"])
    );
}

#[tokio::test]
async fn saving_one_connection_leaves_the_others_alone() {
    let path = scratch("history-others").join("history.toml");
    let store = FileStore::at(Some(path));

    store
        .save(&name("dev"), &entries(&["SELECT 1;"]))
        .await
        .expect("saved");
    store
        .save(&name("prod"), &entries(&["SELECT 2;"]))
        .await
        .expect("saved");
    store
        .save(&name("dev"), &entries(&["SELECT 3;"]))
        .await
        .expect("saved");

    let (histories, _) = store.load(&keeping(100)).await;

    assert_eq!(statements(&histories, "dev"), ["SELECT 3;"]);
    assert_eq!(statements(&histories, "prod"), ["SELECT 2;"]);
}

#[tokio::test]
async fn a_lower_limit_trims_on_load() {
    let path = scratch("history-limit").join("history.toml");
    let store = FileStore::at(Some(path));
    store
        .save(
            &name("dev"),
            &entries(&["SELECT 3;", "SELECT 2;", "SELECT 1;"]),
        )
        .await
        .expect("saved");

    let (histories, _) = store.load(&keeping(2)).await;

    assert_eq!(statements(&histories, "dev"), ["SELECT 3;", "SELECT 2;"]);
}

#[tokio::test]
async fn a_file_from_before_entries_had_times_loads_and_is_rewritten_in_the_new_form() {
    let path = scratch("history-bare").join("history.toml");
    std::fs::write(
        &path,
        "[connections.dev]\nentries = [\"SELECT 2;\", \"SELECT 1;\"]\n",
    )
    .expect("an older file");
    let store = FileStore::at(Some(path.clone()));

    let (histories, diagnostics) = store.load(&keeping(100)).await;

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        histories.of(&name("dev")),
        [
            Entry::bare("SELECT 2;", Engine::Postgres),
            Entry::bare("SELECT 1;", Engine::Postgres)
        ]
    );

    store
        .save(&name("dev"), histories.of(&name("dev")))
        .await
        .expect("saved");
    let rewritten = std::fs::read_to_string(&path).expect("written");

    assert!(
        rewritten.contains("{ engine = \"postgres\", statement = \"SELECT 2;\" },"),
        "{rewritten}"
    );
}

#[tokio::test]
async fn no_file_yet_is_an_empty_history_and_no_complaint() {
    let path = scratch("history-absent").join("history.toml");

    let (histories, diagnostics) = FileStore::at(Some(path)).load(&keeping(100)).await;

    assert!(histories.of(&name("dev")).is_empty());
    assert!(diagnostics.is_empty());
}

#[tokio::test]
async fn a_file_that_is_not_toml_is_reported_and_replaced_by_the_next_save() {
    let path = scratch("history-corrupt").join("history.toml");
    std::fs::write(&path, "[connections.dev\nentries = ").expect("a corrupt file");
    let store = FileStore::at(Some(path.clone()));

    let (histories, diagnostics) = store.load(&keeping(100)).await;

    assert!(histories.of(&name("dev")).is_empty());
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("replaced"));

    store
        .save(&name("dev"), &entries(&["SELECT 1;"]))
        .await
        .expect("a corrupt file does not block a save");
    let (histories, diagnostics) = store.load(&keeping(100)).await;

    assert!(diagnostics.is_empty());
    assert_eq!(statements(&histories, "dev"), ["SELECT 1;"]);
}

#[tokio::test]
async fn with_nowhere_to_write_a_save_fails_and_a_load_is_empty() {
    let store = FileStore::at(None);

    assert!(
        store
            .save(&name("dev"), &entries(&["SELECT 1;"]))
            .await
            .is_err()
    );
    assert!(store.load(&keeping(100)).await.1.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn the_file_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;

    let directory = scratch("history-mode").join("state");
    let path = directory.join("history.toml");

    FileStore::at(Some(path.clone()))
        .save(
            &name("dev"),
            &entries(&["CREATE ROLE x WITH PASSWORD = 'p';"]),
        )
        .await
        .expect("saved");

    let mode = |path: &PathBuf| {
        std::fs::metadata(path)
            .expect("exists")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&directory), 0o700);
}

#[allow(clippy::expect_used, reason = "see scratch")]
fn saved(of: &[(&str, &str)]) -> Vec<SavedQuery> {
    entries(
        &of.iter()
            .map(|(_, statement)| *statement)
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .zip(of)
    .map(|(entry, (called, _))| SavedQuery {
        name: QueryName::try_new(called).expect("a valid name"),
        entry,
    })
    .collect()
}

#[tokio::test]
async fn saved_queries_live_beside_the_history_and_load_back() {
    let directory = scratch("saved-roundtrip").join("state");
    let store = FileStore::at(Some(directory.join("history.toml")));
    let queries = saved(&[("pending", "SELECT *\nFROM orders;"), ("one", "SELECT 1;")]);

    store
        .save_queries(&name("dev"), &queries)
        .await
        .expect("the first save creates the file");
    store
        .save_queries(&name("prod"), &saved(&[("two", "SELECT 2;")]))
        .await
        .expect("a second connection joins it");

    let (loaded, diagnostics) = FileStore::at(Some(directory.join("history.toml")))
        .load_saved(&keeping(0))
        .await;

    assert!(directory.join("saved.toml").exists());
    assert!(!directory.join("history.toml").exists());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(loaded.of(&name("dev")), queries);
    assert_eq!(loaded.of(&name("prod")).len(), 1);
}

#[tokio::test]
async fn a_saved_file_that_does_not_parse_is_reported_and_not_overwritten() {
    let directory = scratch("saved-broken");
    let path = directory.join("saved.toml");
    std::fs::write(&path, "[connections.dev\n").expect("written");
    let store = FileStore::at(Some(directory.join("history.toml")));

    let (loaded, diagnostics) = store.load_saved(&keeping(100)).await;
    let saving = store
        .save_queries(&name("dev"), &saved(&[("one", "SELECT 1;")]))
        .await;

    assert!(loaded.is_empty());
    assert_eq!(diagnostics.len(), 1);
    assert!(saving.is_err());
    assert_eq!(
        std::fs::read_to_string(&path).expect("still there"),
        "[connections.dev\n"
    );
}

#[tokio::test]
async fn buffers_live_beside_the_history_and_an_empty_one_is_forgotten() {
    let directory = scratch("buffers-roundtrip").join("state");
    let store = FileStore::at(Some(directory.join("history.toml")));

    for (connection, text) in [
        ("dev", "SELECT *\nFROM \"orders\";\n"),
        ("prod", "SELECT 1;"),
        ("prod", ""),
    ] {
        store
            .save_buffer(&name(connection), text)
            .await
            .expect("written");
    }

    let (loaded, diagnostics) = FileStore::at(Some(directory.join("history.toml")))
        .load_buffers()
        .await;

    assert!(directory.join("buffers.toml").exists());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(loaded.of(&name("dev")), "SELECT *\nFROM \"orders\";\n");
    assert_eq!(loaded.of(&name("prod")), "");
}

#[tokio::test]
async fn a_buffers_file_that_does_not_parse_is_reported_and_not_overwritten() {
    let directory = scratch("buffers-broken");
    let path = directory.join("buffers.toml");
    std::fs::write(&path, "[connections.dev\n").expect("written");
    let store = FileStore::at(Some(directory.join("history.toml")));

    let (_loaded, diagnostics) = store.load_buffers().await;
    let saving = store.save_buffer(&name("dev"), "SELECT 1;").await;

    assert_eq!(diagnostics.len(), 1);
    assert!(saving.is_err());
    assert_eq!(
        std::fs::read_to_string(&path).expect("still there"),
        "[connections.dev\n"
    );
}
