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
    let active = SharedWorkGate::new();
    let lease = active.try_conversion().unwrap();
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
    assert!(active.try_conversion().is_some());
}

#[test]
fn conversion_holds_admission_through_write_and_releases_on_io_failure() {
    struct FailingWriter<'a>(&'a SharedWorkGate);
    impl Write for FailingWriter<'_> {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            assert!(self.0.try_render().is_none());
            assert!(self.0.try_conversion().is_none());
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "peer disconnected",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let active = SharedWorkGate::new();
    assert!(serve_conversion_with_gate(
        &mut FailingWriter(&active),
        &request(),
        None,
        None,
        &active
    )
    .is_err());
    assert!(active.try_conversion().is_some());
}

#[test]
fn conversion_admission_releases_on_handler_unwind() {
    let active = SharedWorkGate::new();
    assert!(std::panic::catch_unwind(|| {
        let _lease = active.try_conversion().unwrap();
        panic!("converter panicked");
    })
    .is_err());
    assert!(active.try_conversion().is_some());
}

#[test]
fn raster_response_refuses_conversion_before_decode() {
    let gate = SharedWorkGate::new();
    let raster = gate.try_render().unwrap();
    let mut output = Vec::new();
    serve_conversion_with_gate(&mut output, &request(), None, None, &gate).unwrap();
    let response = String::from_utf8(output).unwrap();
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable"));
    assert!(!response.contains("bad convert request"));
    drop(raster);
    assert!(gate.try_conversion().is_some());
}
