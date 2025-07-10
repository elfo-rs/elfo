//! A smoke integration test for the OpenMetrics telemeter.

use eyre::Result;
use toml::toml;

#[tokio::test]
async fn it_works() -> Result<()> {
    let config = toml! {
        sink = "OpenMetrics"
        listen = "127.0.0.1:9042"
    };

    let blueprint = elfo_telemeter::init();
    let _proxy = elfo_test::proxy(blueprint, config).await;

    // Get metrics with GZIP compression

    let client = reqwest::Client::builder().gzip(true).build()?;
    let content = client
        .get("http://127.0.0.1:9042/metrics")
        .send()
        .await?
        .text()
        .await?;

    let some_expected_parts = [
        r#"elfo_actor_status_changes_total{actor_group="system.init",status="Initializing"}"#,
        r#"elfo_active_actors{actor_group="subject",status="Normal"}"#,
        r#"elfo_sent_messages_total{actor_group="subject",message="Render",protocol="elfo-telemeter"}"#,
        r#"elfo_busy_time_seconds{actor_group="subject",quantile="0.75"}"#,
        r#"elfo_message_waiting_time_seconds_min{actor_group="subject"}"#,
        "# TYPE elfo_sent_messages_total counter",
        "# TYPE elfo_active_actors gauge",
        "# TYPE elfo_busy_time_seconds summary",
        "EOF",
    ];

    println!("Metrics content (first request):\n{content}");

    for part in some_expected_parts {
        assert!(content.contains(part), "not found: {part}");
    }

    // Get metrics without compression

    let content = reqwest::Client::builder()
        .gzip(false)
        .build()?
        .get("http://127.0.0.1:9042/metrics")
        .send()
        .await?
        .text()
        .await?;

    println!("Metrics content (second request):\n{content}");

    for part in some_expected_parts {
        assert!(content.contains(part), "not found: {part}");
    }

    // Not supported methods

    let response = client.post("http://127.0.0.1:9042/metrics").send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);

    // Not supported endpoints

    let response = client.get("http://127.0.0.1:9042/something").send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    Ok(())
}
