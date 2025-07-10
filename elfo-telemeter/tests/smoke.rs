//! A smoke integration test for the OpenMetrics telemeter.

use toml::toml;

#[tokio::test]
async fn it_works() {
    let config = toml! {
        sink = "OpenMetrics"
        listen = "127.0.0.1:9042"
    };

    let blueprint = elfo_telemeter::init();
    let _proxy = elfo_test::proxy(blueprint, config).await;

    let content = reqwest::get("http://127.0.0.1:9042/metrics")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    let some_expected_parts = [
        r#"elfo_actor_status_changes_total{actor_group="system.init",status="Initializing"} 0"#,
        r#"elfo_active_actors{actor_group="subject",status="Normal"} 1"#,
        r#"elfo_sent_messages_total{actor_group="subject",message="Render",protocol="elfo-telemeter"} 0"#,
        r#"elfo_busy_time_seconds{actor_group="subject",quantile="0.75"}"#,
        r#"elfo_message_handling_time_seconds{actor_group="system.configurers",message="<Startup>",quantile="0.75"}"#,
        r#"elfo_message_waiting_time_seconds_min{actor_group="subject"}"#,
        "# TYPE elfo_sent_messages_total counter",
        "# TYPE elfo_active_actors gauge",
        "# TYPE elfo_busy_time_seconds summary",
        "EOF",
    ];

    for part in some_expected_parts {
        assert!(content.contains(part), "not found: {part}");
    }
}
