//! Tests the process-wide system time source in an isolated test executable.

#![cfg(feature = "system-time-source")]

use std::time::SystemTime as StdSystemTime;

use elfo_utils::time::{SystemTime, set_system_time_source};

#[test]
fn global_time_source() {
    let before = SystemTime::from(StdSystemTime::now());
    let now = SystemTime::now();
    let after = SystemTime::from(StdSystemTime::now());
    assert!(before <= now && now <= after);

    let expected = SystemTime::from_unix_time_nanos(42);
    assert!(set_system_time_source(|| SystemTime::from_unix_time_nanos(
        42
    )));
    assert_eq!(SystemTime::now(), expected);
    assert_eq!(
        std::thread::spawn(SystemTime::now).join().unwrap(),
        expected
    );

    assert!(!set_system_time_source(|| SystemTime::UNIX_EPOCH));
    assert_eq!(SystemTime::now(), expected);

    #[cfg(feature = "test-util")]
    {
        use std::time::Duration;

        elfo_utils::time::with_system_time_mock(|mock| {
            assert_eq!(SystemTime::now(), SystemTime::UNIX_EPOCH);
            mock.advance(Duration::from_nanos(7));
            assert_eq!(SystemTime::now().to_unix_time_nanos(), 7);
            assert_eq!(
                std::thread::spawn(SystemTime::now).join().unwrap(),
                expected
            );
        });
        assert_eq!(SystemTime::now(), expected);
    }
}
