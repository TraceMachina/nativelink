# Delaying retries after throttled input downloads

Status: proposed design for
[TRA-1471](https://linear.app/tracemachina/issue/TRA-1471/nativelink-n12-design-hold-an-action-whose-input-fetch-timed-out-on-a).
This proposal does not change runtime behavior or add configuration fields.

When an input download exhausts its deadline during an upstream throttle,
retrying the action immediately on another cold worker adds demand to the
same store. Persist a short, increasing delay before another placement,
release the worker's execution reservation, and prefer a suitable worker
that recently downloaded part of the input tree. Keep the delay, affinity,
and total retry count bounded.

## Current behavior and prerequisites

The worker already retries transient input fetch failures locally within
`max_download_timeout`. Its local fast store retains successfully fetched
blobs between attempts. See `fetch_with_retry` and `prepare_action` in
[running_actions_manager.rs](../../nativelink-worker/src/running_actions_manager.rs).
The proposal begins after that local retry window is exhausted.

`RetryResult::RetryAfter` distinguishes a throttle inside
[retry.rs](../../nativelink-util/src/retry.rs), but an exhausted retry or
the outer download timeout can lose that distinction. A generic
`ResourceExhausted` also describes worker capacity or Redis memory
exhaustion. Neither the status code nor a substring in an error message
is enough to select this policy.

Worker results use a stream. Success from `execution_response` means the
update entered the stream, not that the scheduler persisted it; see
[worker_api_client_wrapper.rs](../../nativelink-worker/src/worker_api_client_wrapper.rs).
The scheduler currently recovers lost results through its executing sweep
and reports of running operations. A reliable deferred retry therefore
depends on an explicit acknowledgment protocol, not resending on a
successful stream write.

The matcher already supports `reservation_hold`. It reserves capacity for
an eligible action that cannot fit a busy worker. A deferred action must
not create or retain that hold. Implement this proposal after the
reservation and matching changes have settled, and preserve the existing
hard execution ceiling. Worker keepalives must not extend that ceiling.

## Carry a typed throttle cause

Add a typed input preparation outcome to the worker protocol. The proposed
cause is `upstream_throttled`, accompanied by a bounded remaining
`Retry-After` duration. It describes input preparation only: the worker
must not have started the action process.

Store adapters should mark throttles only from their structured provider
responses. Preserve the cause through store wrappers and retry exhaustion.
Keep attempt-local throttle state accessible to `prepare_action` even
when its timeout cancels the download future. A timeout qualifies only if
a fetch was still waiting on a confirmed throttle, or its last failure
exhausted a throttle retry budget. A throttle followed by successful
fetching and an unrelated disk failure must not qualify. For concurrent
fetches, retain bounded counters and the latest valid throttle deadline,
not an unbounded list of errors.

Missing inputs retain their existing missing-digest response. Permission
errors, disk failures, data loss, command execution timeouts, and generic
capacity errors retain their existing handling. Do not store the proposed
retry metadata in an action-cache result.

## Persist the delay before acknowledging it

Use a dispatch generation together with the operation and worker IDs to
identify a report. An operation can retry on the same worker, so the
operation ID alone cannot distinguish a duplicate report from a later
attempt. A result acknowledgment confirms the disposition of that exact
generation: persisted, superseded, or terminal.

On a qualifying report, the scheduler atomically changes the assigned
operation from Executing to an internal Deferred state. Persist the cause,
retry count, `eligible_at`, cumulative delay, and an optional affinity hint
in the awaited-action record. Expose Deferred to REAPI clients as Queued.
Duplicate reports for the same generation must return the original
acknowledgment without incrementing counters or extending the delay.

After persistence, acknowledge the report and release the scheduler's
reservation. The worker may clean the failed workspace immediately, but
retains the small report and includes its operation in the running report
until acknowledgment or a bounded delivery deadline. This prevents the
existing lost-result detector from returning it to the queue prematurely. Result
delivery retries need backoff, per-attempt deadlines, and a bounded pending
report queue. They must run outside the stream reader and scheduler worker
pool lock. If delivery cannot complete, reconnect and use the existing
bounded lost-result recovery; never retain a slot indefinitely.

If persistence fails, do not acknowledge success. A scheduler restart
after persistence but before acknowledgment must recover the same deferred
record and acknowledge a duplicate. A stale report must never change a
newer assignment or resurrect a completed or canceled operation.

## Make eligibility authoritative across replicas

Add an index ordered by `eligible_at` for Deferred records in both awaited
action database implementations. A background task moves due records back
to Queued using the existing version check. An atomic assignment check
must also reject a future `eligible_at`; a stale queue listing cannot
bypass the delay. Index updates and record transitions must have a recovery
path for a crash between them, as other indexed state transitions do.

Read due records in bounded batches and wake the matcher on successful
transitions. Schedule the next wake from the earliest deadline, with a
bounded periodic scan as a missed-notification fallback. Persist wall-clock
deadlines for restart and replica agreement; use monotonic local timers
for waits, recheck the deadline on wake, and tolerate backward clock steps.
An index temporarily missing a row must not leave it deferred forever.

Preserve original insertion time and priority when returning to Queued.
Cancellation and loss of all clients still retire a Deferred operation.
Release any `reservation_hold` when entering Deferred. Deferred operations
do not consume worker resources and do not count as currently executing.
Their delay does not reset client liveness or any overall operation budget.

## Bound retries and prefer a warm worker

Proposed initial policy, subject to review: disabled by default, a five
second initial delay, a sixty second delay cap, and a two minute cumulative
delay budget per operation. Double the policy delay after each qualifying
failure and apply jitter sampled once when persisting the transition.
Honor a provider's remaining `Retry-After` as a lower bound. If that lower
bound exceeds the remaining budget, terminate with the retryable failure
instead of scheduling earlier than requested.

Each failed dispatch consumes one `max_job_retries` attempt even when the
underlying status is `ResourceExhausted`; ordinary capacity declines keep
their current semantics. Repeated throttles therefore cannot create an
unlimited loop. A successful preparation clears the consecutive-throttle
count. Duplicate reports and scheduler restarts do not replenish budgets.

Initially, the previous worker is only a preference among currently
eligible candidates. Use the worker connection generation and a short
expiry to reject stale hints. Honor all platform properties, capacity,
health, draining state, and tenant or execution-scope boundaries first.
If the preferred worker is unavailable, choose another eligible worker
when the delay expires; affinity never extends the delay or reserves an
idle worker. Existing queue fairness takes precedence over locality.

A later extension may advertise bounded, expiring input-root hints. A
hint indicates possible local bytes, not proof that every input exists.
Do not issue filesystem `has` or remote CAS queries per candidate during
matching: that would recreate the fan-out this policy is meant to reduce.
Evicted inputs still follow the normal fetch and missing-input paths.

## Compatibility and implementation order

1. Add typed throttle propagation and tests, with scheduling unchanged.
2. Add dispatch generation and persisted-result acknowledgments behind
   negotiated worker capabilities. Test reconnects and duplicate reports.
3. Add the Deferred record, indexes, bounded wake task, and assignment
   eligibility checks. Deploy readers that understand the new record to
   every scheduler before enabling writers. Wire-field compatibility alone
   is insufficient when older schedulers share the database.
4. Enable the policy for a small worker pool, then add the optional locality
   preference. Old workers and disabled configurations retain current
   behavior. Rollback disables new deferrals and drains or migrates
   existing Deferred records before reverting to an older scheduler.

Generate the configuration reference and worker protocol code from their
sources when implementing these steps. Keep deployment and chart changes
in separate work.

## Acceptance checks

- Inject a throttle until input preparation times out. No worker receives
  the action before the persisted deadline; at most one receives it after.
- Cancel during the delay, restart the scheduler, lose an acknowledgment,
  and replay a report from an older dispatch. None resurrects an action,
  doubles a retry count, or holds a worker reservation forever.
- Run two schedulers against Redis with stale queue listings and an index
  update failure. Repeat against the memory backend. No early dispatch or
  permanently deferred action is allowed.
- Mix throttled downloads with successful downloads and local disk errors.
  Only the confirmed throttle outcome selects this policy. Missing inputs
  still reach the client with their digest.
- Exhaust the delay or job retry budget. The client receives a terminal
  operation containing the failure instead of another deferral.
- Expire an affinity hint, evict its inputs, or disconnect its worker.
  Other eligible workers remain usable without a new wait.
- Exercise mixed worker versions and rollback with deferred records.
  Existing running-operation reports and execution ceilings still work.

Record deferred queue depth, scheduled delay, retry outcomes, and affinity
hits using bounded cause labels. Compare dispatch attempts, input bytes,
and completion latency under an injected throttle before enabling the
policy broadly. A reduction in failures alone does not establish that the
extra delay reduced store demand.
