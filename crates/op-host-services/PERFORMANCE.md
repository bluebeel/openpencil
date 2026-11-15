# Headless performance counters

`GET /api/mcp/metrics` returns durable cohort counters without acquiring the
document writer lock. It remains available while an authoring operation is busy
or the document needs recovery. Normal request origin, authentication and read
scope checks still apply.

`durableGroups` belongs to the connection's authority. Its claimed/saved batch
histograms contain occupied bins only. Local and managed daemons additionally
return process-wide `incrementalSaves` counters. Online tenants do not receive
those process-wide counters. No document content, filenames, account IDs or
resident registry traversal is included.

The fields are independent atomic counters, not one transactional snapshot.
Measure wall time, CPU and memory around the actual operation separately. Read
these counters after a barrier to distinguish committed cohorts, isolated
fallbacks, capture/budget refusals and full versus incremental saves. A counter
read does not establish whether a timed-out mutation committed.

`GET /api/mcp/server` retains its existing health/configuration response, which
also includes state-dependent details and may wait for the document writer.
