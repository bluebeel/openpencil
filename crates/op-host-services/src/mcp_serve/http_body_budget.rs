//! Process-wide ingress admission, held through dispatch rather than just reads.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{HttpRequest, McpServeError};

// Connection-count limits alone allow dozens of simultaneous 64 MiB bodies.
// This bounds buffered input, independently of document/render cache budgets.
static BODY_BUDGET: BodyBudget = BodyBudget::new(128 * 1024 * 1024);

#[derive(Debug)]
pub(super) struct BodyBudget {
    used: AtomicUsize,
    limit: usize,
}

impl BodyBudget {
    const fn new(limit: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            limit,
        }
    }

    fn reserve(&self, bytes: usize) -> Result<(), McpServeError> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|&next| next <= self.limit)
            })
            .map(|_| ())
            .map_err(|_| McpServeError::Framing {
                status: "503 Service Unavailable",
                message: "concurrent request bodies exceed the ingress memory budget; retry later"
                    .into(),
            })
    }
}

#[derive(Debug)]
#[must_use = "hold the body lease until dispatch has finished"]
pub(crate) struct BodyLease<'a> {
    budget: Option<&'a BodyBudget>,
    bytes: usize,
}

impl<'a> BodyLease<'a> {
    pub(super) fn new(budget: Option<&'a BodyBudget>, bytes: usize) -> Result<Self, McpServeError> {
        let mut lease = Self { budget, bytes: 0 };
        lease.grow(bytes)?;
        Ok(lease)
    }

    pub(super) fn grow(&mut self, bytes: usize) -> Result<(), McpServeError> {
        if let Some(budget) = self.budget {
            budget.reserve(bytes)?;
            self.bytes += bytes;
        }
        Ok(())
    }
}

impl Drop for BodyLease<'_> {
    fn drop(&mut self) {
        if let Some(budget) = self.budget {
            budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }
}

/// Body ownership may move into a converter; the separate lease stays in the
/// connection's stack frame through processing, errors, and response writing.
/// Refusal is explicit and happens before any mutation; clients decide retries.
pub(crate) fn read_admitted_http_request<S: Read + Write>(
    stream: &mut S,
) -> Result<Option<(HttpRequest, BodyLease<'static>)>, McpServeError> {
    read_with_budget(stream, &BODY_BUDGET)
}

fn read_with_budget<'a, S: Read + Write>(
    stream: &mut S,
    budget: &'a BodyBudget,
) -> Result<Option<(HttpRequest, BodyLease<'a>)>, McpServeError> {
    match super::read_http_request_with_budget(stream, Some(budget)) {
        Ok(request) => Ok(Some(request)),
        Err(McpServeError::Framing { status, message }) => {
            super::write_mcp_http_response_with_origin(
                stream,
                status,
                &super::rest_error_body(&message),
                None,
            )?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
