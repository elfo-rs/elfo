//! A smoke integration test for the dumper.

use std::{fs, time::Duration};

use serde::Deserialize;
use tempdir::TempDir;
use toml::toml;

use elfo_core::{
    config::AnyConfig,
    messages::{Terminate, UpdateConfig},
};

#[tokio::test(start_paused = true)]
async fn it_works() {
    let tmp_dir = TempDir::new("elfo_dumper_test").unwrap();
    let tmp_path = tmp_dir.path().join("first.dump").display().to_string();
    let tmp_path_2 = tmp_path.clone();

    let config = toml! {
        path = tmp_path_2
    };

    let blueprint = elfo_dumper::new();
    let mut proxy = elfo_test::proxy(blueprint, config).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let content = fs::read_to_string(&tmp_path).unwrap();
    let some_expected_parts = [
        r#""g":"system.configurers","k":"_""#,
        r#"cl":"internal","mn":"UpdateConfig","mp":"elfo-core","mk":"Regular"#,
        r#"d":"In","cl":"internal","mn":"DumpingTick","mp":"elfo-dumper","mk":"Regular"#,
        "first.dump",
    ];

    println!("First file content:\n{content}");
    for part in some_expected_parts {
        assert!(content.contains(part), "not found: {part}");
    }

    // Register a new class

    elfo_core::dumping::Dumper::new("custom");

    // Update the config

    let tmp_path = tmp_dir.path().join("second.dump").display().to_string();
    let tmp_path_2 = tmp_path.clone();

    let config = toml! {
        path = tmp_path_2
    };

    proxy.sync().await;
    proxy
        .send(UpdateConfig::new(AnyConfig::deserialize(config).unwrap()))
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let content = fs::read_to_string(&tmp_path).unwrap();
    let some_expected_parts = [
        r#"cl":"internal","mn":"StartDumperForClass","mp":"elfo-dumper","mk":"Regular","m":"custom"#,
        r#"d":"Out","cl":"internal","mn":"UpdateConfig","mp":"elfo-core","mk":"Regular"#,
        r#"cl":"internal","mn":"ConfigUpdated","mp":"elfo-core"#,
        r#"d":"In","cl":"internal","mn":"DumpingTick","mp":"elfo-dumper","mk":"Regular"#,
        "second.dump",
    ];

    println!("Second file content:\n{content}");
    for part in some_expected_parts {
        assert!(content.contains(part), "not found: {part}");
    }

    // Graceful termination

    proxy.send(Terminate::default()).await;
    proxy.finished().await;
}
