//! Checks metrics produced per actor key.

use eyre::Result;
use toml::toml;

#[tokio::test]
async fn it_groups_by_telemetry_key() -> Result<()> {
    let config = toml! {
        sink = "OpenMetrics"
        listen = "127.0.0.1:9043"

        [system.telemetry]
        per_actor_group = false
        per_actor_key = [".*", "rewritten"]
    };

    let blueprint = elfo_telemeter::init();
    let _proxy = elfo_test::proxy(blueprint, config).await;

    let content = reqwest::get("http://127.0.0.1:9043/metrics")
        .await?
        .text()
        .await?;

    println!("Metrics content:\n{content}");

    let expected = [
        r#"elfo_message_waiting_time_seconds_min{actor_group="subject",actor_key="rewritten"}"#,
        r#"elfo_busy_time_seconds{actor_group="subject",actor_key="rewritten",quantile="0.75"}"#,
    ];

    for part in expected {
        assert!(content.contains(part), "not found: {part}");
    }

    // Metrics per group are disabled for the subject.
    assert!(!content.contains(r#"elfo_busy_time_seconds{actor_group="subject",quantile="0.75"}"#));

    Ok(())
}
