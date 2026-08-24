# `base-telemetry-client`

Collection and transport for Base node telemetry. Builds a
[`NodeReport`](../telemetry-types) from the local machine and delivers it to an
ingest endpoint.

The crate is deliberately client-agnostic. Nothing here knows about the
consensus node specifically, so the execution node and the snapshot download
command can reuse it without a move.

## Overview

- **`TelemetryConfig`**: enable flag, endpoint, intervals, and identity path.
  Reporting is inert unless both `enabled` is set and an endpoint is configured.
- **`TelemetryId`**: a random v4 `UUID`, minted on first run and persisted so a
  restart keeps the same identity.
- **`HardwareCollector`**: cloud vs bare metal, CPU, memory, disk, and whether
  the data directory sits on network storage. Degrades field-by-field to `None`
  rather than failing.
- **`LatencySampler`**: accumulates head-lag point samples and the high-water
  mark between reports.
- **`NodeReportBuilder`**: assembles a whole `NodeReport` from a `NodeIdentity`
  fixed at startup plus the head and network snapshots of the moment.
- **`ReportSink`**: the delivery seam. `HttpReportSink` POSTs JSON; tests
  substitute a mock.
- **`TelemetryReporter`**: bounded queue in front of a sink, with a background
  task that retries with backoff.
- **`DeliveryStreak`**: counts consecutive delivery failures so an outage costs
  one warning at its start and one at its end.

## Operational contract

The client is best-effort by construction, because a telemetry outage must never
degrade a node:

- It never blocks startup and never blocks a hot path. `enqueue` is a
  `try_send`; a full queue drops the report and increments a counter rather than
  applying backpressure to the caller.
- It backs off on failure and logs one warning when delivery starts failing and
  one when it recovers, not one per attempt and not one per reporting cycle. A
  node pointed at an endpoint that never comes back stays quiet enough to run
  for weeks.
- It honors `HTTPS_PROXY` and `NO_PROXY`, which comes free with `reqwest` as
  long as nothing calls `.no_proxy()`.

## Identity

`TelemetryId::load_or_create` writes to a caller-supplied path and logs a
first-run disclosure banner the first time it mints an ID.

The ID is reliable within a reporting window and not across months, which is all
anything downstream depends on. A restart preserves it because the file does. A
rebuild on a fresh volume does not, and no metric here survives that.

The identity file must never be packaged into a snapshot. If it were, every node
that snap-syncs would share one ID and the fleet would collapse into a single
row.

## License

Licensed under the [MIT License](https://github.com/base/base/blob/main/LICENSE).
