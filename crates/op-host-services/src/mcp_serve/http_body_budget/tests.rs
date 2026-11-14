use super::*;
use std::io::{Cursor, Read, Write};

struct Peer {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl Peer {
    fn new(body: &[u8]) -> Self {
        let mut input = format!(
            "POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        input.extend_from_slice(body);
        Self {
            input: Cursor::new(input),
            output: Vec::new(),
        }
    }
}

impl Read for Peer {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(bytes)
    }
}

impl Write for Peer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn ingress_refuses_before_body_and_lease_survives_owned_transfer() {
    let budget = BodyBudget::new(6);
    let mut first = Peer::new(b"1234");
    let (request, lease) = read_with_budget(&mut first, &budget).unwrap().unwrap();
    let moved_body = request.body;
    let mut second = Peer::new(b"567");
    assert!(read_with_budget(&mut second, &budget).unwrap().is_none());
    assert_eq!(
        second.input.position() as usize,
        second.input.get_ref().len() - 3
    );
    assert!(String::from_utf8_lossy(&second.output).starts_with("HTTP/1.1 503 Service Unavailable"));
    assert_eq!(budget.used.load(Ordering::Acquire), 4);
    assert_eq!(moved_body, "1234");
    drop(lease);
    let mut retry = Peer::new(b"567");
    assert!(read_with_budget(&mut retry, &budget).unwrap().is_some());
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
}

#[test]
fn ingress_releases_after_truncation_and_unwinding() {
    let budget = BodyBudget::new(6);
    let mut truncated = Peer::new(b"1234");
    truncated.input.get_mut().pop();
    assert!(read_with_budget(&mut truncated, &budget).is_err());
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
    let result = std::panic::catch_unwind(|| {
        let _lease = BodyLease::new(Some(&budget), 6).unwrap();
        panic!("handler aborted");
    });
    assert!(result.is_err());
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
}

#[test]
fn ingress_charges_invalid_utf8_expansion_and_matches_legacy_bytes() {
    let invalid = b"a\xf0\x90\x80z\xff\xff\xe2\x82";
    let budget = BodyBudget::new(invalid.len() * 4);
    let mut peer = Peer::new(invalid);
    let (request, lease) = read_with_budget(&mut peer, &budget).unwrap().unwrap();
    assert_eq!(request.body, String::from_utf8_lossy(invalid));
    assert_eq!(budget.used.load(Ordering::Acquire), invalid.len() * 4);
    assert!(request.body.capacity() <= invalid.len() * 3);
    drop(lease);
    let too_small = BodyBudget::new(invalid.len() * 4 - 1);
    assert!(read_with_budget(&mut Peer::new(invalid), &too_small)
        .unwrap()
        .is_none());
    assert_eq!(too_small.used.load(Ordering::Acquire), 0);
}

#[test]
fn ingress_valid_utf8_body_keeps_bounded_allocation_for_uneven_lengths() {
    for length in [0, 1, 65535, 65536, 65537, 160001] {
        let bytes = vec![b'x'; length];
        let budget = BodyBudget::new(length);
        let (request, lease) = read_with_budget(&mut Peer::new(&bytes), &budget)
            .unwrap()
            .unwrap();
        assert_eq!(request.body.as_bytes(), bytes);
        assert!(request.body.capacity() <= length);
        assert_eq!(budget.used.load(Ordering::Acquire), length);
        drop(lease);
        assert_eq!(budget.used.load(Ordering::Acquire), 0);
    }
}

#[test]
fn ingress_atomic_admission_never_overcommits_under_contention() {
    let budget = BodyBudget::new(7);
    let live = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..32 {
            scope.spawn(|| {
                for _ in 0..1000 {
                    if let Ok(lease) = BodyLease::new(Some(&budget), 1) {
                        assert!(live.fetch_add(1, Ordering::SeqCst) < 7);
                        std::thread::yield_now();
                        live.fetch_sub(1, Ordering::SeqCst);
                        drop(lease);
                    }
                }
            });
        }
    });
    assert_eq!(budget.used.load(Ordering::Acquire), 0);
    assert_eq!(live.load(Ordering::SeqCst), 0);
}
