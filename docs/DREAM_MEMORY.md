# Milestone 3 memory design

The operator-side store owns creation and revision of research memory; Dream and
Lucid receive only a read-only validated retrieval capability. No CLI or production
memory is wired to this subsystem. Test stores must be in-memory or temporary.

## Approval boundary

A host-configured `OperatorVerifier` authenticates the operator's credential and
approves the entire `MemoryRevisionRequest`: operation, memory ID, expected
revision, new content or reason, and (for promotion) evidence and evidence run.
Its attestation must match the operator identity and the declared promotion
approval reference. A reference or claimed operator name alone is not authority.
No permissive production verifier is supplied. The host's authentication service
is a trust boundary; the library does not implement a login or credential store.
`MemoryActor` is a caller-declared role check for defense in depth; it is not
authentication. The configured verifier and credential establish authority.

The verifier issues an opaque, non-cloneable, non-deserializable revision permit,
bound to that store's authority instance. Model, Dream, Lucid and Witness actors
are rejected independently of what their text or approval references say. The
store consumes the permit, checks state/revision again in the transaction, and
records the authenticated approval reference with the resulting revision. Approval
references must not be reusable after a successful write. Credentials and raw
verifier diagnostics never become memory records.

## Persistence and retrieval invariants

- A memory item retains its original source provenance and source-input
  fingerprint. Both are immutable across edits, promotions, retirement and rollback.
- Initial content is Candidate at revision zero. Operator edits are allowed only
  while Candidate, create a new revision, and retain speculative trust.
- Promotion requires Candidate, authenticated operator approval, nonblank evidence,
  a recorded completed evidence run distinct from the immutable source run,
  matching task/input provenance, and one atomic database transaction.
- Retirement removes Validated content from retrieval. It preserves revision history.
- Rollback restores an earlier revision's content as Candidate, never as Validated.
  Restored content requires fresh operator approval and evidence before re-promotion.
- Stale revisions, replayed approvals, absent evidence runs and partial writes fail
  closed. History and evidence-run records are append-only.
- Retrieval selects only Validated rows and excludes the current task, source run,
  and identical source input fingerprint, even if a caller renamed the task.
  Existing FNV fingerprints correlate trimmed input bytes; they are not integrity
  proofs, semantic similarity scores or secret-hiding cryptographic hashes.

The initial Claude relay incorrectly said sbh-store already used SQLite. Chris
approved adding exactly rusqlite 0.32.1 with bundled SQLite to sbh-store; the root
and core crates acquire no direct database dependency. Existing JSONL stores stay
separate. Schema version 1 is created transactionally and unknown versions are
rejected. Every mutation uses `BEGIN IMMEDIATE` and parameterized queries; errors
drop the transaction. There is no default database path. On Unix, explicit file
opening enforces `0600` permissions for both new and existing database files before
SQLite opens them.

`MemoryStore::record_run` admits trusted evaluator-owned run records, never raw
model output. Promotion checks structural validity, completion, matching task and
trimmed source-input fingerprint, a distinct evidence run ID, and at least one
observed passing test result in any arm. This is a minimal structural gate: a run
where only CleanBaseline passed also qualifies. The operator must judge whether
the evidence actually supports the memory; a passing result alone does not
establish causality or intervention benefit.
Run records and approval references are append-only, including failed runs.

`DreamRequest::memory_recombination` accepts IDs only. Configure a controller with
`with_memory_reader(store.reader())` to enable the optional separate arm. The
reader selects current Validated heads in one read transaction and automatically
excludes source task, source run and source input. Caller JSON cannot fabricate
a Validated fragment. Normal Dream branches and Lucid have no memory writer.
Content retains speculative trust even after operator validation; lifecycle state
does not relabel generated prose as observed evidence.

Retrieval is a snapshot at the start of each Dream invocation. Retirement before
retrieval takes effect immediately; retirement during an already-running research
invocation does not revoke its in-memory snapshot. No controller output writes
back to the store. The database and configured verifier are trusted host resources;
these API boundaries do not protect against a host replacing its own SQLite file.

Remaining risks include baseline-only passing runs satisfying the structural
evidence gate; scoped operator review must establish actual support. Snapshot
revocation during inference and a WAL/journal mode deployment decision remain
deferred until a real deployment path exists.
