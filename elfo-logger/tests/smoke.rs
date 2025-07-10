//! A smoke integration test for the logger.

use std::fs;

use serde::Deserialize;
use tempdir::TempDir;
use toml::toml;

use elfo_core::{
    config::AnyConfig,
    messages::{Terminate, UpdateConfig},
};

#[tokio::test]
async fn it_works() {
    let tmp_dir = TempDir::new("elfo_logger_test").unwrap();
    let tmp_path = tmp_dir.path().join("first.log").display().to_string();
    let tmp_path_2 = tmp_path.clone();

    let config = toml! {
        sink = "File"
        path = tmp_path_2
    };

    let blueprint = elfo_logger::init();
    let mut proxy = elfo_test::proxy(blueprint, config).await;
    proxy.sync().await;

    let content = fs::read_to_string(&tmp_path).unwrap();
    let some_expected_parts = [
        "system.configurers/_ - status changed",
        "configs are updated",
        "status=Normal",
        "INFO [",
    ];

    println!("First file content:\n{content}");
    for part in some_expected_parts {
        assert!(content.contains(part), "not found: {part}");
    }

    proxy.send(elfo_logger::ReopenLogFile::default()).await;

    // Update the config

    let tmp_path = tmp_dir.path().join("second.log").display().to_string();
    let tmp_path_2 = tmp_path.clone();

    let config = toml! {
        sink = "File"
        path = tmp_path_2
        format.with_location = true
        format.with_module = true
        format.colorization = "Always"
        targets.crate.max_level = "Debug"
    };

    proxy
        .send(UpdateConfig::new(AnyConfig::deserialize(config).unwrap()))
        .await;
    proxy.sync().await;

    let content = fs::read_to_string(&tmp_path).unwrap();
    let some_expected_parts = ["subject/_", "config updated", "_location", "_module"];

    println!("Second file content:\n{content}");
    for part in some_expected_parts {
        assert!(content.contains(part), "not found: {part}");
    }

    // Graceful termination

    proxy.send(Terminate::default()).await;
    proxy.finished().await;
}
