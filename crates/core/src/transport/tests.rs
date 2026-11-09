use super::*;
use std::sync::atomic::Ordering;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn a_dialer_ref_is_compared_and_printed_by_identity() {
    let a: Arc<dyn Dialer> = Arc::new(TcpDialer::default());
    let b: Arc<dyn Dialer> = Arc::new(TcpDialer::default());
    assert_eq!(DialerRef(a.clone()), DialerRef(a.clone()));
    assert_ne!(DialerRef(a.clone()), DialerRef(b));
    assert!(format!("{:?}", DialerRef(a)).starts_with("Dialer@"));
}

#[test]
fn raw_text_is_the_far_ends_message_or_the_faults_detail() {
    let refused =
        DialError::Refused { host: "db".into(), port: 5432, reason: Refusal::Unreachable, detail: "no route".into() };
    assert_eq!(refused.raw(), "no route");
    assert_eq!(DialError::Failed(Fault::other("boom")).raw(), "boom");
    assert_eq!(DialError::NotOpen.raw(), "");
}

/// A loopback port nothing listens on. It is not one a listener on port 0 was given: once freed,
/// such a port can go to another socket asking for any port, or be a dial's own end (which
/// then connects to itself). This one is below the range systems hand out for either (32768
/// and up on Linux, 49152 and up on macOS and Windows), so it stays closed.
fn closed_port() -> u16 {
    (20_000..30_000).find(|&p| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()).expect("a free port")
}

#[tokio::test]
async fn the_tcp_dialer_connects_and_counts() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        s.write_all(&buf).await.unwrap();
    });
    let d = TcpDialer::default();
    let mut s = d.dial("127.0.0.1", port).await.expect("dial");
    s.write_all(b"ping").await.unwrap();
    let mut back = [0u8; 4];
    s.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"ping");
    server.await.unwrap();
    assert_eq!(d.dials.load(Ordering::SeqCst), 1);
    match d.dial("127.0.0.1", closed_port()).await {
        Err(DialError::Failed(f)) => {
            assert_eq!(f.kind, crate::fault::FaultKind::Io(std::io::ErrorKind::ConnectionRefused))
        }
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("connected to nothing"),
    }
    assert_eq!(d.dials.load(Ordering::SeqCst), 2);
}
