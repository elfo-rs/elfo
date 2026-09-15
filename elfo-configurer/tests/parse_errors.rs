//! Config parsing errors reported by the configurer actor.

use elfo_configurer::ReloadConfigs;
use elfo_core::{Topology, config::AnyConfig, messages::Terminate};
use std::{
    fs, io,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tempdir::TempDir;
use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan;

const SECRET_CONFIG: &str = "[database]\npassword = \"do-not-log-this\" @";

/// The message part comes from `toml` itself, so the test does not pin its wording.
fn expected_error() -> String {
    let mut error = toml::from_str::<toml::Table>(SECRET_CONFIG).unwrap_err();
    // In case `error.message()` starts using it.
    error.set_input(None);
    format!("TOML parse error at 2:30: {}", error.message())
}

fn config_file(content: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new("elfo_configurer_test").unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, content).unwrap();
    (dir, path)
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapturedLogs {
    fn assert_parse_error(&self, expected_error: &str) {
        let bytes = self.0.lock().unwrap().clone();
        let logs = String::from_utf8_lossy(&bytes);
        assert!(!logs.contains("do-not-log-this"), "{logs}");
        assert!(logs.contains("invalid config"), "{logs}");
        assert!(logs.contains(expected_error), "{logs}");
    }
}

fn capture_logs() -> (tracing::subscriber::DefaultGuard, CapturedLogs) {
    let logs = CapturedLogs::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_span_events(FmtSpan::FULL)
        .with_writer(move || writer.clone())
        .finish();
    (tracing::subscriber::set_default(subscriber), logs)
}

#[tokio::test]
async fn startup_parse_error_omits_source_line_with_secret() {
    let expected_error = expected_error();
    let (_guard, logs) = capture_logs();
    let (_dir, path) = config_file(SECRET_CONFIG);

    let topology = Topology::empty();
    topology
        .local("system.configurers")
        .entrypoint()
        .mount(elfo_configurer::from_path(&topology, &path));

    let error = elfo_core::init::try_start(topology).await.unwrap_err();
    assert_eq!(error.errors.len(), 1);
    assert_eq!(error.errors[0].group, "system.configurers");
    assert_eq!(error.errors[0].reason, expected_error);
    assert!(!error.errors[0].reason.contains("do-not-log-this"));
    logs.assert_parse_error(&expected_error);
}

#[tokio::test]
async fn reload_parse_error_omits_source_line_with_secret() {
    let expected_error = expected_error();
    let (_guard, logs) = capture_logs();
    // Start with valid TOML so that only the reload fails.
    let (_dir, path) = config_file("");

    let blueprint = elfo_configurer::from_path(&Topology::empty(), &path);
    let proxy = elfo_test::proxy(blueprint, AnyConfig::default()).await;
    proxy.request(ReloadConfigs::default()).await.unwrap();

    fs::write(&path, SECRET_CONFIG).unwrap();
    let rejected = proxy.request(ReloadConfigs::default()).await.unwrap_err();
    assert_eq!(rejected.errors.len(), 1);
    assert_eq!(rejected.errors[0].group, "subject");
    assert_eq!(rejected.errors[0].reason, expected_error);
    assert!(!rejected.errors[0].reason.contains("do-not-log-this"));

    proxy.send(Terminate::default()).await;
    proxy.finished().await;
    logs.assert_parse_error(&expected_error);
}
