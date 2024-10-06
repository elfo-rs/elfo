#![allow(dead_code)] // TODO: combine tests into "it/*"

// For tests without `elfo::test::proxy`.
pub(crate) fn setup_logger() {
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
}
