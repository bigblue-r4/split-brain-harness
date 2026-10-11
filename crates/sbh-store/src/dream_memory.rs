//! Explicit operator-side SQLite research memory. No default/production path.
//! Controllers receive only [`ValidatedMemoryReader`], never this writer.

use crate::{
    audit::fingerprint,
    memory_authority::{MemoryActor, MemoryRevisionPermit, OperatorGate, OperatorVerifier},
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sbh_core::dream::{
    ExperimentRun, GeneratedClaim, MemoryId, MemoryRevisionRequest, MemoryState, ProvenanceRecord,
    RunId, RunStatus, TaskId, TestOutcome,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

type Db = Arc<Mutex<Connection>>;
const SCHEMA: &str = r#"
CREATE TABLE evidence_runs (
 id TEXT PRIMARY KEY NOT NULL, task_id TEXT NOT NULL,
 input_fingerprint TEXT NOT NULL, record_json TEXT NOT NULL
);
CREATE TABLE memory_items (
 id TEXT PRIMARY KEY NOT NULL, source_json TEXT NOT NULL,
 input_fingerprint TEXT NOT NULL, current_revision INTEGER NOT NULL CHECK(current_revision >= 0)
);
CREATE TABLE approvals (
 reference TEXT PRIMARY KEY NOT NULL, operator_id TEXT NOT NULL,
 request_json TEXT NOT NULL
);
CREATE TABLE memory_revisions (
 memory_id TEXT NOT NULL REFERENCES memory_items(id),
 revision INTEGER NOT NULL CHECK(revision >= 0),
 state TEXT NOT NULL CHECK(state IN ('candidate', 'validated', 'retired')),
 content_json TEXT NOT NULL, request_json TEXT,
 approval_reference TEXT REFERENCES approvals(reference),
 evidence_run_id TEXT REFERENCES evidence_runs(id),
 PRIMARY KEY(memory_id, revision)
);
CREATE TRIGGER immutable_source BEFORE UPDATE OF id,source_json,input_fingerprint ON memory_items
 BEGIN SELECT RAISE(ABORT, 'immutable memory source'); END;
CREATE TRIGGER retain_memory BEFORE DELETE ON memory_items
 BEGIN SELECT RAISE(ABORT, 'retain memory history'); END;
CREATE TRIGGER immutable_revisions_update BEFORE UPDATE ON memory_revisions
 BEGIN SELECT RAISE(ABORT, 'append-only revisions'); END;
CREATE TRIGGER immutable_revisions_delete BEFORE DELETE ON memory_revisions
 BEGIN SELECT RAISE(ABORT, 'append-only revisions'); END;
CREATE TRIGGER immutable_runs_update BEFORE UPDATE ON evidence_runs
 BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER immutable_runs_delete BEFORE DELETE ON evidence_runs
 BEGIN SELECT RAISE(ABORT, 'append-only evidence'); END;
CREATE TRIGGER immutable_approvals_update BEFORE UPDATE ON approvals
 BEGIN SELECT RAISE(ABORT, 'append-only approvals'); END;
CREATE TRIGGER immutable_approvals_delete BEFORE DELETE ON approvals
 BEGIN SELECT RAISE(ABORT, 'append-only approvals'); END;
PRAGMA user_version = 1;
"#;

/// Authentication and run admission belong to the trusted host, outside models.
pub struct MemoryStore {
    db: Db,
    gate: OperatorGate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRevision {
    pub revision: u64,
    pub state: MemoryState,
    pub content: GeneratedClaim,
    pub request: Option<MemoryRevisionRequest>,
    pub approval_reference: Option<String>,
    pub operator_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryHistory {
    pub id: MemoryId,
    pub source: ProvenanceRecord,
    pub source_input_fingerprint: String,
    pub revisions: Vec<MemoryRevision>,
}

/// A database-selected snapshot. No public constructor or deserialization.
#[derive(Debug, Clone)]
pub struct ValidatedMemory {
    id: MemoryId,
    revision: u64,
    source: ProvenanceRecord,
    content: GeneratedClaim,
}
impl ValidatedMemory {
    pub fn id(&self) -> &MemoryId {
        &self.id
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn source(&self) -> &ProvenanceRecord {
        &self.source
    }
    pub fn content(&self) -> &GeneratedClaim {
        &self.content
    }
}

#[derive(Debug)]
pub struct MemorySelection {
    pub eligible: Vec<ValidatedMemory>,
    pub excluded: Vec<MemoryId>,
}

/// Read-only API capability; no connection, writer, verifier or permit access.
///
/// ```compile_fail
/// use sbh_store::dream_memory::ValidatedMemoryReader;
/// fn cannot_promote(reader: ValidatedMemoryReader) { reader.revise(()); }
/// ```
#[derive(Clone)]
pub struct ValidatedMemoryReader {
    db: Db,
}

fn lock(db: &Db) -> Result<MutexGuard<'_, Connection>, String> {
    db.lock().map_err(|_| "memory store lock poisoned".into())
}
// Do not return SQLite diagnostics containing caller text, paths or payloads.
fn sql(error: rusqlite::Error) -> String {
    match error {
        rusqlite::Error::QueryReturnedNoRows => "memory record not found".into(),
        _ => "memory database operation rejected".into(),
    }
}
fn encode<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| "invalid memory record".into())
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|_| "invalid stored memory record".into())
}
fn input_fingerprint(input: &str) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("source input must not be blank".into());
    }
    Ok(fingerprint(input.trim().as_bytes()))
}
fn state(value: &str) -> Result<MemoryState, String> {
    match value {
        "candidate" => Ok(MemoryState::Candidate),
        "validated" => Ok(MemoryState::Validated),
        "retired" => Ok(MemoryState::Retired),
        _ => Err("invalid stored memory state".into()),
    }
}

impl MemoryStore {
    pub fn in_memory(verifier: Box<dyn OperatorVerifier>) -> Result<Self, String> {
        Self::initialize(Connection::open_in_memory().map_err(sql)?, verifier)
    }
    /// Explicit caller-owned path only. No CLI or automatic production opening.
    pub fn open(path: &Path, verifier: Box<dyn OperatorVerifier>) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            // Secure new files at creation and existing files before SQLite
            // opens them or creates a rollback journal containing research data.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path)
                .map_err(|_| "cannot open private memory database")?;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| "cannot secure memory database permissions")?;
        }
        Self::initialize(Connection::open(path).map_err(sql)?, verifier)
    }
    fn initialize(
        mut connection: Connection,
        verifier: Box<dyn OperatorVerifier>,
    ) -> Result<Self, String> {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(sql)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(sql)?;
        // Read schema version AFTER acquiring the database write reservation.
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let version: u32 = tx
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(sql)?;
        match version {
            0 => tx.execute_batch(SCHEMA).map_err(sql)?,
            1 => {}
            _ => return Err("unsupported memory schema version".into()),
        }
        tx.commit().map_err(sql)?;
        Ok(Self {
            db: Arc::new(Mutex::new(connection)),
            gate: OperatorGate::new(verifier),
        })
    }
    pub fn reader(&self) -> ValidatedMemoryReader {
        ValidatedMemoryReader {
            db: self.db.clone(),
        }
    }

    /// Candidate admission is untrusted content, not validation authority.
    pub fn create_candidate(
        &self,
        id: &MemoryId,
        source: &ProvenanceRecord,
        original_input: &str,
        content: &GeneratedClaim,
    ) -> Result<(), String> {
        source.validate()?;
        content.validate()?;
        let input = input_fingerprint(original_input)?;
        let mut db = lock(&self.db)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        tx.execute(
            "INSERT INTO memory_items VALUES (?1, ?2, ?3, 0)",
            params![id.as_str(), encode(source)?, input],
        )
        .map_err(sql)?;
        tx.execute(
            "INSERT INTO memory_revisions VALUES (?1, 0, 'candidate', ?2, NULL, NULL, NULL)",
            params![id.as_str(), encode(content)?],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// Trusted host admission of evaluator-owned records, never model JSON.
    /// Admission is append-only; promotion checks completion and scoped evidence.
    pub fn record_run(&self, run: &ExperimentRun, original_input: &str) -> Result<(), String> {
        run.validate()?;
        let input = input_fingerprint(original_input)?;
        let mut db = lock(&self.db)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        tx.execute(
            "INSERT INTO evidence_runs VALUES (?1, ?2, ?3, ?4)",
            params![run.id.as_str(), run.task_id.as_str(), input, encode(run)?],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    pub fn authorize(
        &self,
        actor: &MemoryActor,
        credential: &str,
        request: MemoryRevisionRequest,
    ) -> Result<MemoryRevisionPermit, String> {
        self.gate.authorize(actor, credential, request)
    }

    /// Every transition, evidence check, approval nonce and revision is atomic.
    /// A failed operation drops the transaction and consumes the attempted permit.
    pub fn revise(&self, permit: MemoryRevisionPermit) -> Result<u64, String> {
        let approved = permit.consume(self.gate.identity())?;
        approved.request.validate()?;
        let (id, expected) = match &approved.request {
            MemoryRevisionRequest::ReviseCandidate {
                memory_id,
                expected_revision,
                ..
            }
            | MemoryRevisionRequest::Promote {
                memory_id,
                expected_revision,
                ..
            }
            | MemoryRevisionRequest::Retire {
                memory_id,
                expected_revision,
                ..
            }
            | MemoryRevisionRequest::RollbackToCandidate {
                memory_id,
                expected_revision,
                ..
            } => (memory_id, *expected_revision),
        };
        let next = expected
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or("memory revision overflow")?;
        let mut db = lock(&self.db)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql)?;
        let (head, old_state, source_json, input, content_json): (u64, String, String, String, String) = tx.query_row(
            "SELECT m.current_revision,r.state,m.source_json,m.input_fingerprint,r.content_json
             FROM memory_items m JOIN memory_revisions r ON r.memory_id=m.id AND r.revision=m.current_revision WHERE m.id=?1",
            [id.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(sql)?;
        if head != expected {
            return Err("stale memory revision".into());
        }
        let mut content: GeneratedClaim = decode(&content_json)?;
        content.validate()?;
        let old_state = state(&old_state)?;
        let mut evidence_id: Option<&str> = None;
        let new_state = match &approved.request {
            MemoryRevisionRequest::ReviseCandidate {
                content: replacement,
                ..
            } => {
                if old_state != MemoryState::Candidate {
                    return Err("only Candidate memory can be edited".into());
                }
                content = replacement.clone();
                "candidate"
            }
            MemoryRevisionRequest::Promote {
                evidence_run_id, ..
            } => {
                if old_state != MemoryState::Candidate {
                    return Err("only Candidate memory can be promoted".into());
                }
                let source: ProvenanceRecord = decode(&source_json)?;
                source.validate()?;
                if source.source_run_id == *evidence_run_id {
                    return Err("promotion requires a distinct evidence run".into());
                }
                let (task, run_input, record): (String,String,String) = tx.query_row(
                    "SELECT task_id,input_fingerprint,record_json FROM evidence_runs WHERE id=?1",
                    [evidence_run_id.as_str()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(sql)?;
                let run: ExperimentRun = decode(&record)?;
                run.validate()?;
                if task != source.source_task_id.as_str()
                    || run_input != input
                    || run.id != *evidence_run_id
                    || run.task_id != source.source_task_id
                    || run.status != RunStatus::Completed
                    || !run
                        .arms
                        .iter()
                        .flat_map(|a| &a.results)
                        .any(|r| r.outcome == TestOutcome::Passed)
                {
                    return Err(
                        "promotion requires completed matching observed test evidence".into(),
                    );
                }
                evidence_id = Some(evidence_run_id.as_str());
                "validated"
            }
            MemoryRevisionRequest::Retire { .. } => {
                if old_state != MemoryState::Validated {
                    return Err("only Validated memory can be retired".into());
                }
                "retired"
            }
            MemoryRevisionRequest::RollbackToCandidate {
                restore_revision, ..
            } => {
                let restored: String = tx.query_row(
                    "SELECT content_json FROM memory_revisions WHERE memory_id=?1 AND revision=?2",
                    params![id.as_str(), restore_revision], |r| r.get(0)).map_err(sql)?;
                content = decode(&restored)?;
                content.validate()?;
                "candidate"
            }
        };
        let request = encode(&approved.request)?;
        tx.execute(
            "INSERT INTO approvals VALUES (?1, ?2, ?3)",
            params![approved.approval_reference, approved.operator_id, request],
        )
        .map_err(sql)?;
        tx.execute(
            "UPDATE memory_items SET current_revision=?1 WHERE id=?2 AND current_revision=?3",
            params![next, id.as_str(), expected],
        )
        .map_err(sql)?;
        tx.execute(
            "INSERT INTO memory_revisions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id.as_str(),
                next,
                new_state,
                encode(&content)?,
                request,
                approved.approval_reference,
                evidence_id
            ],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(next)
    }

    pub fn history(&self, id: &MemoryId) -> Result<MemoryHistory, String> {
        let db = lock(&self.db)?;
        let (source, input): (String, String) = db
            .query_row(
                "SELECT source_json,input_fingerprint FROM memory_items WHERE id=?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(sql)?;
        let mut statement = db.prepare("SELECT r.revision,r.state,r.content_json,r.request_json,r.approval_reference,a.operator_id
            FROM memory_revisions r LEFT JOIN approvals a ON a.reference=r.approval_reference WHERE r.memory_id=?1 ORDER BY r.revision").map_err(sql)?;
        let rows = statement
            .query_map([id.as_str()], |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(sql)?;
        let mut revisions = vec![];
        for row in rows {
            let (revision, s, c, request, approval_reference, operator_id) = row.map_err(sql)?;
            revisions.push(MemoryRevision {
                revision,
                state: state(&s)?,
                content: decode(&c)?,
                request: request.as_deref().map(decode).transpose()?,
                approval_reference,
                operator_id,
            });
        }
        Ok(MemoryHistory {
            id: id.clone(),
            source: decode(&source)?,
            source_input_fingerprint: input,
            revisions,
        })
    }
}

impl ValidatedMemoryReader {
    /// Snapshot of current heads under a read transaction, with automatic scope
    /// exclusions. Returns no content for absent, Candidate or Retired records.
    pub fn select(
        &self,
        ids: &[MemoryId],
        current_task: &TaskId,
        current_run: &RunId,
        original_input: &str,
    ) -> Result<MemorySelection, String> {
        if ids.is_empty() || ids.len() > 32 {
            return Err("memory arm requires 1..32 memory IDs".into());
        }
        let mut unique = HashSet::new();
        if ids.iter().any(|id| !unique.insert(id)) {
            return Err("duplicate memory ID".into());
        }
        let input = input_fingerprint(original_input)?;
        let mut db = lock(&self.db)?;
        let tx = db.transaction().map_err(sql)?;
        let mut selection = MemorySelection {
            eligible: vec![],
            excluded: vec![],
        };
        for id in ids {
            let row: Option<(u64,String,String,String)> = tx.query_row(
                "SELECT r.revision,m.source_json,m.input_fingerprint,r.content_json FROM memory_items m
                JOIN memory_revisions r ON r.memory_id=m.id AND r.revision=m.current_revision
                WHERE m.id=?1 AND r.state='validated'", [id.as_str()],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(sql)?;
            if let Some((revision, source, source_input, content)) = row {
                let source: ProvenanceRecord = decode(&source)?;
                source.validate()?;
                if source.source_task_id != *current_task
                    && source.source_run_id != *current_run
                    && source_input != input
                {
                    let content: GeneratedClaim = decode(&content)?;
                    content.validate()?;
                    selection.eligible.push(ValidatedMemory {
                        id: id.clone(),
                        revision,
                        source,
                        content,
                    });
                    continue;
                }
            }
            selection.excluded.push(id.clone());
        }
        tx.commit().map_err(sql)?;
        Ok(selection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_authority::OperatorAttestation;
    use sbh_core::dream::{
        ArmRecord, ContextId, DreamMode, EvaluationBudget, ExperimentArm, SpeculativeTrust,
        TestResult,
    };

    struct Verifier;
    impl OperatorVerifier for Verifier {
        fn verify(
            &self,
            operator: &str,
            credential: &str,
            _: &MemoryRevisionRequest,
        ) -> Result<OperatorAttestation, String> {
            // Test fixture only: credential stands in for a host's scoped proof.
            if operator != "operator" || !credential.starts_with("verified-") {
                return Err("rejected".into());
            }
            Ok(OperatorAttestation {
                operator_id: operator.into(),
                approval_reference: credential[9..].into(),
            })
        }
    }
    fn store() -> MemoryStore {
        MemoryStore::in_memory(Box::new(Verifier)).unwrap()
    }
    fn id(value: &str) -> MemoryId {
        value.to_string().try_into().unwrap()
    }
    fn source(task: &str, run: &str) -> ProvenanceRecord {
        ProvenanceRecord {
            source_run_id: run.to_string().try_into().unwrap(),
            source_task_id: task.to_string().try_into().unwrap(),
            provider: "offline".into(),
            provider_version: None,
            model: "fixture".into(),
            model_version: None,
            temperature: 0.1,
            seed: Some(9),
            max_output_tokens: 64,
            system_prompt_fingerprint: "system-fp".into(),
        }
    }
    fn claim(text: &str) -> GeneratedClaim {
        GeneratedClaim {
            text: text.into(),
            trust: SpeculativeTrust::Hypothetical,
        }
    }
    fn run(name: &str, task: &str) -> ExperimentRun {
        ExperimentRun {
            id: name.to_string().try_into().unwrap(),
            task_id: task.to_string().try_into().unwrap(),
            mode: DreamMode::Lucid,
            status: RunStatus::Completed,
            inference_calls: 2,
            witness_calls: 0,
            max_total_calls: 2,
            trace: vec![],
            arms: vec![ArmRecord {
                arm: ExperimentArm::CleanBaseline,
                context_id: ContextId::try_from(format!("context-{name}")).unwrap(),
                budget: EvaluationBudget {
                    inference_calls_per_case: 1,
                    witness_calls_per_case: 0,
                    repeats: 2,
                    max_output_tokens: 64,
                },
                provenance: source(task, name),
                results: vec![TestResult {
                    test_id: "test".to_string().try_into().unwrap(),
                    evidence_run_id: name.to_string().try_into().unwrap(),
                    outcome: TestOutcome::Passed,
                    final_answer: "observed answer".into(),
                    best_of_n_outcome: None,
                    evidence: "observed test result".into(),
                }],
            }],
        }
    }
    fn seed(store: &MemoryStore, name: &str) {
        store
            .create_candidate(
                &id(name),
                &source("source-task", "source-run"),
                "source input",
                &claim("original"),
            )
            .unwrap();
    }
    fn promotion(
        name: &str,
        revision: u64,
        approval: &str,
        evidence_run: &str,
    ) -> MemoryRevisionRequest {
        MemoryRevisionRequest::Promote {
            memory_id: id(name),
            expected_revision: revision,
            operator_approval_reference: approval.into(),
            evidence: "operator reviewed observed test evidence".into(),
            evidence_run_id: evidence_run.to_string().try_into().unwrap(),
        }
    }
    fn permit(
        store: &MemoryStore,
        approval: &str,
        request: MemoryRevisionRequest,
    ) -> MemoryRevisionPermit {
        store
            .authorize(
                &MemoryActor::Operator("operator".into()),
                &format!("verified-{approval}"),
                request,
            )
            .unwrap()
    }
    fn apply(
        store: &MemoryStore,
        approval: &str,
        request: MemoryRevisionRequest,
    ) -> Result<u64, String> {
        store.revise(permit(store, approval, request))
    }
    fn select(
        store: &MemoryStore,
        name: &str,
        task: &str,
        current_run: &str,
        input: &str,
    ) -> MemorySelection {
        store
            .reader()
            .select(
                &[id(name)],
                &task.to_string().try_into().unwrap(),
                &current_run.to_string().try_into().unwrap(),
                input,
            )
            .unwrap()
    }
    fn evidence(store: &MemoryStore) {
        store
            .record_run(&run("evidence-run", "source-task"), "source input")
            .unwrap();
    }

    #[test]
    fn lifecycle_history_and_rollback_preserve_source_and_require_fresh_promotion() {
        let store = store();
        seed(&store, "m");
        evidence(&store);
        assert!(
            select(&store, "m", "other-task", "other-run", "other input")
                .eligible
                .is_empty()
        );
        assert_eq!(
            apply(
                &store,
                "edit",
                MemoryRevisionRequest::ReviseCandidate {
                    memory_id: id("m"),
                    expected_revision: 0,
                    content: claim("edited"),
                    reason: "reviewed edit".into()
                }
            )
            .unwrap(),
            1
        );
        assert_eq!(
            apply(
                &store,
                "promote",
                promotion("m", 1, "promote", "evidence-run")
            )
            .unwrap(),
            2
        );
        let selected = select(&store, "m", "other-task", "other-run", "other input");
        assert_eq!(selected.eligible[0].content().text, "edited");
        assert_eq!(selected.eligible[0].revision(), 2);
        assert!(apply(
            &store,
            "bad-edit",
            MemoryRevisionRequest::ReviseCandidate {
                memory_id: id("m"),
                expected_revision: 2,
                content: claim("bad"),
                reason: "edit".into()
            }
        )
        .is_err());
        apply(
            &store,
            "retire",
            MemoryRevisionRequest::Retire {
                memory_id: id("m"),
                expected_revision: 2,
                reason: "obsolete".into(),
            },
        )
        .unwrap();
        assert!(
            select(&store, "m", "other-task", "other-run", "other input")
                .eligible
                .is_empty()
        );
        apply(
            &store,
            "rollback",
            MemoryRevisionRequest::RollbackToCandidate {
                memory_id: id("m"),
                expected_revision: 3,
                restore_revision: 0,
                reason: "restore original".into(),
            },
        )
        .unwrap();
        let history = store.history(&id("m")).unwrap();
        assert_eq!(history.source, source("source-task", "source-run"));
        assert_eq!(
            history.source_input_fingerprint,
            input_fingerprint("source input").unwrap()
        );
        assert_eq!(history.revisions.len(), 5);
        assert_eq!(history.revisions[4].state, MemoryState::Candidate);
        assert_eq!(history.revisions[4].content.text, "original");
        assert_eq!(
            history.revisions[2].operator_id.as_deref(),
            Some("operator")
        );
        assert!(
            select(&store, "m", "other-task", "other-run", "other input")
                .eligible
                .is_empty()
        );
        assert!(apply(
            &store,
            "promote",
            promotion("m", 4, "promote", "evidence-run")
        )
        .is_err());
        apply(&store, "fresh", promotion("m", 4, "fresh", "evidence-run")).unwrap();
        assert_eq!(
            select(&store, "m", "other-task", "other-run", "other input").eligible[0]
                .content()
                .text,
            "original"
        );
    }

    #[test]
    fn excludes_task_run_and_same_trimmed_input_under_renamed_task() {
        let store = store();
        seed(&store, "m");
        evidence(&store);
        apply(&store, "a", promotion("m", 0, "a", "evidence-run")).unwrap();
        for (task, run, input) in [
            ("source-task", "new-run", "new input"),
            ("new-task", "source-run", "new input"),
            ("renamed-task", "new-run", " source input \n"),
        ] {
            let result = select(&store, "m", task, run, input);
            assert!(result.eligible.is_empty());
            assert_eq!(result.excluded, vec![id("m")]);
        }
        assert_eq!(
            select(&store, "m", "new-task", "new-run", "new input")
                .eligible
                .len(),
            1
        );
        let reader = store.reader();
        assert!(reader
            .select(
                &[],
                &"task".to_string().try_into().unwrap(),
                &"run".to_string().try_into().unwrap(),
                "input"
            )
            .is_err());
        assert!(reader
            .select(
                &[id("m"), id("m")],
                &"task".to_string().try_into().unwrap(),
                &"run".to_string().try_into().unwrap(),
                "input"
            )
            .is_err());
        assert!(select(&store, "missing", "task", "run", "input")
            .eligible
            .is_empty());
    }

    #[test]
    fn promotion_requires_recorded_completed_matching_distinct_observed_evidence() {
        let store = store();
        seed(&store, "m");
        assert!(apply(&store, "missing", promotion("m", 0, "missing", "absent")).is_err());
        for (name, task, input, status, passed) in [
            (
                "source-run",
                "source-task",
                "source input",
                RunStatus::Completed,
                true,
            ),
            (
                "wrong-task",
                "other-task",
                "source input",
                RunStatus::Completed,
                true,
            ),
            (
                "wrong-input",
                "source-task",
                "other input",
                RunStatus::Completed,
                true,
            ),
            (
                "failed",
                "source-task",
                "source input",
                RunStatus::Failed,
                true,
            ),
            (
                "cancelled",
                "source-task",
                "source input",
                RunStatus::Cancelled,
                true,
            ),
            (
                "planned",
                "source-task",
                "source input",
                RunStatus::Planned,
                true,
            ),
            (
                "no-result",
                "source-task",
                "source input",
                RunStatus::Completed,
                false,
            ),
        ] {
            let mut record = run(name, task);
            record.status = status;
            if !passed {
                record.arms[0].results.clear();
            }
            store.record_run(&record, input).unwrap();
            assert!(
                apply(&store, name, promotion("m", 0, name, name)).is_err(),
                "{name}"
            );
            assert_eq!(store.history(&id("m")).unwrap().revisions.len(), 1);
        }
        evidence(&store);
        apply(&store, "valid", promotion("m", 0, "valid", "evidence-run")).unwrap();
    }

    #[test]
    fn stale_scoped_permits_and_globally_reused_references_are_rejected() {
        let store = store();
        seed(&store, "a");
        seed(&store, "b");
        evidence(&store);
        let first = permit(
            &store,
            "shared",
            promotion("a", 0, "shared", "evidence-run"),
        );
        let stale = permit(&store, "other", promotion("a", 0, "other", "evidence-run"));
        store.revise(first).unwrap();
        assert_eq!(store.revise(stale).unwrap_err(), "stale memory revision");
        assert!(apply(
            &store,
            "shared",
            promotion("b", 0, "shared", "evidence-run")
        )
        .is_err());
        assert_eq!(store.history(&id("b")).unwrap().revisions.len(), 1);
        apply(
            &store,
            "distinct",
            promotion("b", 0, "distinct", "evidence-run"),
        )
        .unwrap();
    }

    #[test]
    fn write_failure_rolls_back_head_history_and_approval_reference() {
        let store = store();
        seed(&store, "m");
        evidence(&store);
        lock(&store.db).unwrap().execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON memory_revisions WHEN NEW.revision > 0 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
        assert!(apply(&store, "a", promotion("m", 0, "a", "evidence-run")).is_err());
        assert_eq!(store.history(&id("m")).unwrap().revisions.len(), 1);
        assert!(select(&store, "m", "other", "run", "input")
            .eligible
            .is_empty());
        let db = lock(&store.db).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT current_revision FROM memory_items WHERE id=?1",
                ["m"],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM approvals", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            0
        );
        db.execute_batch("DROP TRIGGER injected_failure").unwrap();
        drop(db);
        apply(&store, "a", promotion("m", 0, "a", "evidence-run")).unwrap();
    }

    #[test]
    fn source_history_runs_and_approvals_are_immutable_in_database() {
        let store = store();
        seed(&store, "m");
        evidence(&store);
        apply(&store, "a", promotion("m", 0, "a", "evidence-run")).unwrap();
        let db = lock(&store.db).unwrap();
        for query in [
            "UPDATE memory_items SET source_json='{}'",
            "UPDATE memory_items SET input_fingerprint='forged'",
            "DELETE FROM memory_items",
            "UPDATE memory_revisions SET state='validated'",
            "DELETE FROM memory_revisions",
            "UPDATE evidence_runs SET record_json='{}'",
            "DELETE FROM evidence_runs",
            "UPDATE approvals SET operator_id='forged'",
            "DELETE FROM approvals",
        ] {
            assert!(db.execute(query, []).is_err(), "{query}");
        }
        drop(db);
        assert!(store
            .record_run(&run("evidence-run", "source-task"), "source input")
            .is_err());
        assert!(store
            .create_candidate(
                &id("m"),
                &source("other", "other"),
                "other",
                &claim("overwrite")
            )
            .is_err());
        assert_eq!(store.history(&id("m")).unwrap().revisions.len(), 2);
    }

    #[test]
    fn temp_database_reopens_and_competing_connections_recheck_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.sqlite");
        let first = MemoryStore::open(&path, Box::new(Verifier)).unwrap();
        seed(&first, "m");
        evidence(&first);
        let second = MemoryStore::open(&path, Box::new(Verifier)).unwrap();
        let stale = permit(&second, "b", promotion("m", 0, "b", "evidence-run"));
        apply(&first, "a", promotion("m", 0, "a", "evidence-run")).unwrap();
        assert!(second.revise(stale).is_err());
        drop(first);
        drop(second);
        let reopened = MemoryStore::open(&path, Box::new(Verifier)).unwrap();
        assert_eq!(reopened.history(&id("m")).unwrap().revisions.len(), 2);
        assert_eq!(
            select(&reopened, "m", "other", "run", "other input")
                .eligible
                .len(),
            1
        );
    }

    #[test]
    fn unknown_schema_is_rejected_without_resetting_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("future.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 9).unwrap();
        drop(connection);
        assert!(MemoryStore::open(&path, Box::new(Verifier)).is_err());
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            9
        );
    }

    #[cfg(unix)]
    #[test]
    fn database_permissions_are_private_for_new_and_existing_files() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private.sqlite");
        let first = MemoryStore::open(&path, Box::new(Verifier)).unwrap();
        seed(&first, "m");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(first);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let reopened = MemoryStore::open(&path, Box::new(Verifier)).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(reopened.history(&id("m")).unwrap().revisions.len(), 1);
    }

    #[test]
    fn wrong_actor_credentials_and_cross_store_permit_cannot_change_state() {
        let first = store();
        let second = store();
        seed(&first, "m");
        seed(&second, "m");
        evidence(&first);
        evidence(&second);
        for actor in [
            MemoryActor::Model,
            MemoryActor::Dream,
            MemoryActor::Lucid,
            MemoryActor::Witness,
        ] {
            assert!(first
                .authorize(&actor, "verified-a", promotion("m", 0, "a", "evidence-run"))
                .is_err());
        }
        assert!(first
            .authorize(
                &MemoryActor::Operator("operator".into()),
                "forged",
                promotion("m", 0, "a", "evidence-run")
            )
            .is_err());
        let foreign = permit(&first, "a", promotion("m", 0, "a", "evidence-run"));
        assert!(second.revise(foreign).is_err());
        assert_eq!(first.history(&id("m")).unwrap().revisions.len(), 1);
        assert_eq!(second.history(&id("m")).unwrap().revisions.len(), 1);
    }
}
