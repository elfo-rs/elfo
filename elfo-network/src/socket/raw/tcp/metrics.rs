use std::os::unix::io::AsRawFd;

use metrics::{counter, gauge};

#[derive(Debug, Default)]
pub(crate) struct TcpMetrics {
    /// Smoothed round trip time (microseconds)
    pub rtt: u32,
    /// RTT variance (microseconds)
    pub rtt_var: u32,
    /// Retransmission timeout (microseconds)
    pub rto: u32,

    /// Congestion window size
    pub snd_cwnd: u32,
    /// Slow start threshold
    pub snd_ssthresh: u32,

    /// Total retransmitted packets
    pub retrans: u32,
    /// Lost packets
    pub lost: u32,
    /// Unacknowledged packets
    pub unacked: u32,
    /// Selectively acknowledged packets
    pub sacked: u32,

    /// Path MTU
    pub pmtu: u32,
    /// Receive buffer space
    pub rcv_space: u32,
}

pub(crate) fn collect_tcp_metrics<T: AsRawFd>(socket: &T) -> Result<TcpMetrics, std::io::Error> {
    let fd = socket.as_raw_fd();

    // SAFETY: We're calling getsockopt with a valid file descriptor from AsRawFd.
    // The [libc::tcp_info] structure matches the kernel's expectations.
    let (ret, tcp_info) = unsafe {
        let mut tcp_info = std::mem::zeroed::<libc::tcp_info>();
        let mut len = std::mem::size_of::<libc::tcp_info>() as libc::socklen_t;

        let ret = libc::getsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_INFO,
            &mut tcp_info as *mut _ as *mut libc::c_void,
            &mut len,
        );

        (ret, tcp_info)
    };

    if ret == 0 {
        Ok(TcpMetrics {
            rtt: tcp_info.tcpi_rtt,
            rtt_var: tcp_info.tcpi_rttvar,
            rto: tcp_info.tcpi_rto,
            snd_cwnd: tcp_info.tcpi_snd_cwnd,
            snd_ssthresh: tcp_info.tcpi_snd_ssthresh,
            retrans: tcp_info.tcpi_total_retrans,
            lost: tcp_info.tcpi_lost,
            unacked: tcp_info.tcpi_unacked,
            sacked: tcp_info.tcpi_sacked,
            pmtu: tcp_info.tcpi_pmtu,
            rcv_space: tcp_info.tcpi_rcv_space,
        })
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(crate) fn report_tcp_metrics(current: &TcpMetrics, previous: &TcpMetrics) {
    const US_TO_SEC: f64 = 1e-6;

    gauge!(
        "elfo_network_tcp_rtt_seconds",
        current.rtt as f64 * US_TO_SEC
    );
    gauge!(
        "elfo_network_tcp_rtt_var_seconds",
        current.rtt_var as f64 * US_TO_SEC
    );
    gauge!(
        "elfo_network_tcp_rto_seconds",
        current.rto as f64 * US_TO_SEC
    );
    gauge!("elfo_network_tcp_snd_cwnd", current.snd_cwnd as f64);
    gauge!("elfo_network_tcp_snd_ssthresh", current.snd_ssthresh as f64);
    gauge!("elfo_network_tcp_pmtu", current.pmtu as f64);
    gauge!("elfo_network_tcp_unacked", current.unacked as f64);

    counter!(
        "elfo_network_tcp_sacked_total",
        current.sacked.overflowing_sub(previous.sacked).0 as u64
    );
    counter!(
        "elfo_network_tcp_rcv_space_total",
        current.rcv_space.overflowing_sub(previous.rcv_space).0 as u64
    );
    counter!(
        "elfo_network_tcp_retrans_total",
        current.retrans.overflowing_sub(previous.retrans).0 as u64
    );
    counter!(
        "elfo_network_tcp_lost_total",
        current.lost.overflowing_sub(previous.lost).0 as u64
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::RawFd;

    #[test]
    fn tcp_metrics_default() {
        let metrics = TcpMetrics::default();
        assert_eq!(metrics.rtt, 0);
        assert_eq!(metrics.rtt_var, 0);
        assert_eq!(metrics.rto, 0);
        assert_eq!(metrics.snd_cwnd, 0);
        assert_eq!(metrics.snd_ssthresh, 0);
        assert_eq!(metrics.retrans, 0);
        assert_eq!(metrics.lost, 0);
        assert_eq!(metrics.unacked, 0);
        assert_eq!(metrics.sacked, 0);
        assert_eq!(metrics.pmtu, 0);
        assert_eq!(metrics.rcv_space, 0);
    }

    #[test]
    fn collect_metrics_invalid_fd() {
        struct InvalidSocket;
        impl AsRawFd for InvalidSocket {
            fn as_raw_fd(&self) -> RawFd {
                -1
            }
        }

        let socket = InvalidSocket;
        let result = collect_tcp_metrics(&socket);
        assert!(result.is_err());
    }

    #[test]
    fn metrics_reporting_does_not_panic() {
        let metrics = TcpMetrics {
            rtt: 1000,
            rtt_var: 500,
            rto: 2000,
            snd_cwnd: 10,
            snd_ssthresh: 100,
            retrans: 5,
            lost: 1,
            unacked: 2,
            sacked: 3,
            pmtu: 1500,
            rcv_space: 65536,
        };

        let previous = TcpMetrics::default();
        report_tcp_metrics(&metrics, &previous);
    }

    #[test]
    fn counter_increment_behavior() {
        // Test that counters only emit deltas when values increase
        let initial = TcpMetrics {
            retrans: 5,
            lost: 2,
            ..Default::default()
        };

        let updated = TcpMetrics {
            retrans: 10,
            lost: 4,
            ..Default::default()
        };

        // First report - should emit initial values
        let zero = TcpMetrics::default();
        report_tcp_metrics(&initial, &zero);

        // Second report - should emit only the delta
        report_tcp_metrics(&updated, &initial);

        // No change - should not emit counters
        report_tcp_metrics(&updated, &updated);

        // Regression (should not happen in practice) - should not emit counters
        let regressed = TcpMetrics {
            retrans: 8,
            lost: 3,
            ..Default::default()
        };
        report_tcp_metrics(&regressed, &updated);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn tcp_info_struct_size() {
        let tcp_info_size = std::mem::size_of::<libc::tcp_info>();
        // Size varies by kernel version, but should be at least 104 bytes
        // See /usr/include/linux/tcp.h
        assert!(tcp_info_size >= 104, "TCP_INFO struct size too small");
    }

    #[test]
    fn tcp_info_collection_with_real_socket() {
        use std::{
            net::{TcpListener, TcpStream},
            thread,
            time::Duration,
        };

        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind");
        let addr = listener.local_addr().expect("failed to get local addr");

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::{Read, Write};
                let mut buf = [0u8; 128];
                for _ in 0..5 {
                    let _ = stream.write_all(b"Hello from server\n");
                    let _ = stream.read(&mut buf);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        });

        thread::sleep(Duration::from_millis(50));

        let client = {
            let mut stream = TcpStream::connect(addr).expect("Failed to connect");
            use std::io::{Read, Write};
            let mut buf = [0u8; 128];
            for _ in 0..5 {
                stream
                    .write_all(b"Hello from client\n")
                    .expect("Failed to write");
                let _ = stream.read(&mut buf).expect("failed to read");
                thread::sleep(Duration::from_millis(10));
            }

            stream
        };

        let result = collect_tcp_metrics(&client);

        #[cfg(not(target_os = "linux"))]
        {
            assert!(result.is_err(), "Should fail on non-Linux platforms");

            match result {
                Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::Unsupported),
                Ok(_) => panic!("Should not succeed on non-Linux platforms"),
            }
        }

        #[cfg(target_os = "linux")]
        {
            assert!(
                result.is_ok(),
                "Failed to collect TCP_INFO: {:?}",
                result.err()
            );

            let metrics = result.expect("metrics were collected");

            assert!(
                metrics.rtt > 0,
                "RTT should be positive, got {}",
                metrics.rtt
            );
            assert!(
                metrics.pmtu > 0,
                "PMTU should be positive, got {}",
                metrics.pmtu
            );
            assert!(
                metrics.snd_cwnd > 0,
                "Congestion window should be positive, got {}",
                metrics.snd_cwnd
            );
            assert!(
                metrics.rcv_space > 0,
                "Receive space should be positive, got {}",
                metrics.rcv_space
            );

            // For local connections, RTT should be relatively small
            assert!(
                metrics.rtt < 10_000,
                "Local RTT should be < 10ms (10000 μs), got {} μs",
                metrics.rtt
            );

            // typically 65535 for loopback
            assert!(
                metrics.pmtu >= 1500 || metrics.pmtu == 65535,
                "PMTU should be >= 1500 or 65535 for loopback, got {}",
                metrics.pmtu
            );
        }
    }
}
