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
    // Nothing listens there any more.
    match d.dial("127.0.0.1", port).await {
        Err(DialError::Failed(f)) => {
            assert_eq!(f.kind, crate::fault::FaultKind::Io(std::io::ErrorKind::ConnectionRefused))
        }
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("connected to nothing"),
    }
    assert_eq!(d.dials.load(Ordering::SeqCst), 2);
}
