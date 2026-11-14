use super::*;
use crate::mcp_serve::HttpRequest;

fn request() -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/api/figma/convert".into(),
        body: "not json".into(),
        host: None,
        origin: None,
        token: None,
        content_type: None,
        authorization: None,
        cookie: None,
        query: None,
    }
}

#[test]
fn conversion_refuses_before_decode_and_recovers_after_release() {
    let active = AtomicBool::new(false);
    let lease = ConversionLease::acquire(&active).unwrap();
    let mut output = Vec::new();
    serve_conversion_with_gate(
        &mut output,
        &request(),
        Some("not json".into()),
        None,
        &active,
    )
    .unwrap();
    let response = String::from_utf8(output).unwrap();
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable"));
    assert!(!response.contains("bad convert request"));
    drop(lease);
    let mut output = Vec::new();
    serve_conversion_with_gate(&mut output, &request(), None, None, &active).unwrap();
    let response = String::from_utf8(output).unwrap();
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(response.contains("bad convert request"));
    assert!(!active.load(Ordering::Acquire));
}

#[test]
fn conversion_holds_admission_through_write_and_releases_on_io_failure() {
    struct FailingWriter<'a>(&'a AtomicBool);
    impl Write for FailingWriter<'_> {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            assert!(self.0.load(Ordering::Acquire));
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "peer disconnected",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let active = AtomicBool::new(false);
    assert!(serve_conversion_with_gate(
        &mut FailingWriter(&active),
        &request(),
        None,
        None,
        &active
    )
    .is_err());
    assert!(!active.load(Ordering::Acquire));
}

#[test]
fn conversion_admission_releases_on_handler_unwind() {
    let active = AtomicBool::new(false);
    assert!(std::panic::catch_unwind(|| {
        let _lease = ConversionLease::acquire(&active).unwrap();
        panic!("converter panicked");
    })
    .is_err());
    assert!(ConversionLease::acquire(&active).is_some());
}
