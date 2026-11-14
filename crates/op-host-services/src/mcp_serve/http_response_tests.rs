use super::*;

#[test]
fn large_payload_is_borrowed_and_sent_with_headers_in_one_vector() {
    struct Observer<'a> {
        body: &'a [u8],
        writes: usize,
        flushes: usize,
    }
    impl Write for Observer<'_> {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            panic!("vectored writer should receive headers and body together")
        }
        fn write_vectored(&mut self, parts: &[IoSlice<'_>]) -> std::io::Result<usize> {
            self.writes += 1;
            assert_eq!(parts.len(), 2);
            let header = std::str::from_utf8(&parts[0]).unwrap();
            assert!(header.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(header.contains(&format!("Content-Length: {}\r\n", self.body.len())));
            assert!(header.contains("Access-Control-Allow-Origin: https://example.test\r\n"));
            assert!(header.ends_with("\r\n\r\n"));
            assert_eq!(
                parts[1].as_ptr(),
                self.body.as_ptr(),
                "body must not be copied"
            );
            assert_eq!(parts[1].len(), self.body.len());
            Ok(parts[0].len() + parts[1].len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }
    let body = "x".repeat(8 * 1024 * 1024) + "α";
    let mut writer = Observer {
        body: body.as_bytes(),
        writes: 0,
        flushes: 0,
    };
    write_mcp_http_response_with_origin(&mut writer, "200 OK", &body, Some("https://example.test"))
        .unwrap();
    assert_eq!((writer.writes, writer.flushes), (1, 1));
}

#[test]
fn interrupted_short_writes_preserve_the_complete_wire_response() {
    struct ShortWriter {
        bytes: Vec<u8>,
        interrupt: bool,
        flushed: bool,
    }
    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.interrupt {
                self.interrupt = false;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let count = bytes.len().min(7);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushed = true;
            Ok(())
        }
    }
    for body in ["", "{\"value\":\"αβ\"}"] {
        let mut writer = ShortWriter {
            bytes: Vec::new(),
            interrupt: true,
            flushed: false,
        };
        write_mcp_http_response_with_origin(&mut writer, "200 OK", body, None).unwrap();
        let expected = format!(
            "HTTP/1.1 200 OK\r\n\
             Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n\
             Access-Control-Allow-Headers: Content-Type, mcp-session-id, X-OpenPencil-Token, Authorization\r\n\
             Access-Control-Expose-Headers: mcp-session-id\r\n\
             mcp-session-id: openpencil\r\n\
             Cache-Control: no-store\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        assert_eq!(writer.bytes, expected.as_bytes());
        assert!(writer.flushed);
    }
}

#[test]
fn zero_and_failed_writes_are_reported_without_flushing() {
    struct BrokenWriter {
        zero: bool,
    }
    impl Write for BrokenWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            if self.zero {
                Ok(0)
            } else {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            panic!("failed writes must not flush")
        }
    }
    for zero in [false, true] {
        let error =
            write_mcp_http_response_with_origin(&mut BrokenWriter { zero }, "200 OK", "body", None)
                .unwrap_err();
        assert!(
            matches!(error, McpServeError::Io(ref message) if message.starts_with("http write:"))
        );
    }
}
