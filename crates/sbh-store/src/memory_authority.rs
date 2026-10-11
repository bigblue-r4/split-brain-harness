//! Operator-only revision permits, not model-deserializable approval claims.
//!
//! The host supplies a trusted verifier backed by its authenticated operator
//! session/approval service. A name or reference string alone is not authority.
//! The verifier must authenticate the credential and authorize the entire
//! request, including memory ID, expected revision, evidence and evidence run.
//! No credentials are retained, serialized, or included in returned errors.

use sbh_core::dream::MemoryRevisionRequest;
use std::sync::atomic::{AtomicU64, Ordering};

static AUTHORITY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
/// Caller-declared role is defense in depth, not authentication. Only the
/// configured verifier authenticates the credential and approves the request.
pub enum MemoryActor {
    Operator(String),
    Model,
    Dream,
    Lucid,
    Witness,
}

/// Constructed only by a trusted host verifier, never parsed from model JSON.
pub struct OperatorAttestation {
    pub operator_id: String,
    /// Unique, externally verified approval/audit reference; not a credential.
    pub approval_reference: String,
}

pub trait OperatorVerifier: Send + Sync {
    fn verify(
        &self,
        operator_id: &str,
        credential: &str,
        request: &MemoryRevisionRequest,
    ) -> Result<OperatorAttestation, String>;
}

/// Opaque, non-cloneable and non-deserializable permit for one scoped request.
/// Its constructor and contents are private; only the store's configured
/// verifier can issue it. The store consumes it when attempting the revision.
///
/// ```compile_fail
/// use sbh_store::memory_authority::MemoryRevisionPermit;
/// let forged: MemoryRevisionPermit = serde_json::from_str("{}").unwrap();
/// ```
pub struct MemoryRevisionPermit {
    authority_id: u64,
    revision: AuthorizedRevision,
}
impl std::fmt::Debug for MemoryRevisionPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemoryRevisionPermit(<opaque scoped operator permit>)")
    }
}
impl MemoryRevisionPermit {
    pub(crate) fn consume(self, authority_id: u64) -> Result<AuthorizedRevision, String> {
        if self.authority_id != authority_id {
            return Err("revision permit belongs to another store authority".into());
        }
        Ok(self.revision)
    }
}

pub(crate) struct AuthorizedRevision {
    pub request: MemoryRevisionRequest,
    pub operator_id: String,
    pub approval_reference: String,
}

pub(crate) struct OperatorGate {
    identity: u64,
    verifier: Box<dyn OperatorVerifier>,
}
impl OperatorGate {
    pub(crate) fn new(verifier: Box<dyn OperatorVerifier>) -> Self {
        Self {
            identity: AUTHORITY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            verifier,
        }
    }
    pub(crate) fn identity(&self) -> u64 {
        self.identity
    }
    pub(crate) fn authorize(
        &self,
        actor: &MemoryActor,
        credential: &str,
        request: MemoryRevisionRequest,
    ) -> Result<MemoryRevisionPermit, String> {
        let MemoryActor::Operator(operator_id) = actor else {
            return Err("only an authenticated authorized operator may revise memory".into());
        };
        if operator_id.trim().is_empty() || credential.trim().is_empty() {
            return Err("operator identity and authenticated approval are required".into());
        }
        request.validate()?;
        // Raw verifier diagnostics may contain credentials; never propagate them.
        let attestation = self
            .verifier
            .verify(operator_id, credential, &request)
            .map_err(|_| "operator authentication or scoped approval rejected")?;
        if attestation.operator_id != *operator_id
            || attestation.approval_reference.trim().is_empty()
        {
            return Err(
                "operator attestation does not match the requested identity or approval".into(),
            );
        }
        if let MemoryRevisionRequest::Promote {
            operator_approval_reference,
            ..
        } = &request
        {
            if attestation.approval_reference != *operator_approval_reference {
                return Err("operator approval reference is not authenticated".into());
            }
        }
        Ok(MemoryRevisionPermit {
            authority_id: self.identity,
            revision: AuthorizedRevision {
                request,
                operator_id: attestation.operator_id,
                approval_reference: attestation.approval_reference,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbh_core::dream::{MemoryId, RunId};

    struct TestVerifier;
    impl OperatorVerifier for TestVerifier {
        fn verify(
            &self,
            operator: &str,
            credential: &str,
            request: &MemoryRevisionRequest,
        ) -> Result<OperatorAttestation, String> {
            if operator != "authorized-operator" || credential != "authenticated-scope-proof" {
                return Err("private verifier credential diagnostic".into());
            }
            let MemoryRevisionRequest::Promote {
                operator_approval_reference,
                ..
            } = request
            else {
                return Err("unapproved operation".into());
            };
            if operator_approval_reference != "approval-1" {
                return Err("unapproved reference".into());
            }
            Ok(OperatorAttestation {
                operator_id: operator.into(),
                approval_reference: operator_approval_reference.clone(),
            })
        }
    }
    fn request() -> MemoryRevisionRequest {
        MemoryRevisionRequest::Promote {
            memory_id: MemoryId::try_from("memory-1".to_string()).unwrap(),
            expected_revision: 0,
            operator_approval_reference: "approval-1".into(),
            evidence: "Operator reviewed repeated observed test results".into(),
            evidence_run_id: RunId::try_from("evidence-run".to_string()).unwrap(),
        }
    }
    fn operator() -> MemoryActor {
        MemoryActor::Operator("authorized-operator".into())
    }

    #[test]
    fn nonoperators_and_missing_identity_cannot_receive_permits() {
        let gate = OperatorGate::new(Box::new(TestVerifier));
        for actor in [
            MemoryActor::Model,
            MemoryActor::Dream,
            MemoryActor::Lucid,
            MemoryActor::Witness,
            MemoryActor::Operator(" ".into()),
        ] {
            assert!(gate
                .authorize(&actor, "authenticated-scope-proof", request())
                .is_err());
        }
    }
    #[test]
    fn operator_name_and_reference_alone_are_not_authority() {
        let gate = OperatorGate::new(Box::new(TestVerifier));
        for credential in ["", " ", "model-says-approved"] {
            let error = gate
                .authorize(&operator(), credential, request())
                .unwrap_err();
            assert!(!error.contains("private verifier"));
        }
        assert!(gate
            .authorize(
                &MemoryActor::Operator("intruder".into()),
                "authenticated-scope-proof",
                request()
            )
            .is_err());
        let mut wrong = request();
        if let MemoryRevisionRequest::Promote {
            operator_approval_reference,
            ..
        } = &mut wrong
        {
            *operator_approval_reference = "model-invented-approval".into();
        }
        assert!(gate
            .authorize(&operator(), "authenticated-scope-proof", wrong)
            .is_err());
    }
    #[test]
    fn missing_evidence_or_run_id_is_rejected() {
        let gate = OperatorGate::new(Box::new(TestVerifier));
        let mut invalid = request();
        if let MemoryRevisionRequest::Promote { evidence, .. } = &mut invalid {
            *evidence = " ".into();
        }
        assert!(gate
            .authorize(&operator(), "authenticated-scope-proof", invalid)
            .is_err());
        let mut json = serde_json::to_value(request()).unwrap();
        json.as_object_mut().unwrap().remove("evidence_run_id");
        assert!(serde_json::from_value::<MemoryRevisionRequest>(json).is_err());
    }
    #[test]
    fn scoped_permit_cannot_be_used_by_another_store_authority() {
        let first = OperatorGate::new(Box::new(TestVerifier));
        let second = OperatorGate::new(Box::new(TestVerifier));
        let permit = first
            .authorize(&operator(), "authenticated-scope-proof", request())
            .unwrap();
        assert!(permit.consume(second.identity()).is_err());
        let permit = first
            .authorize(&operator(), "authenticated-scope-proof", request())
            .unwrap();
        let approved = permit.consume(first.identity()).unwrap();
        assert_eq!(approved.operator_id, "authorized-operator");
        assert_eq!(approved.approval_reference, "approval-1");
        assert_eq!(approved.request, request());
    }
    #[test]
    fn wrong_actor_is_rejected_before_calling_host_verifier() {
        struct MustNotRun;
        impl OperatorVerifier for MustNotRun {
            fn verify(
                &self,
                _: &str,
                _: &str,
                _: &MemoryRevisionRequest,
            ) -> Result<OperatorAttestation, String> {
                panic!("model actors must never reach operator authentication");
            }
        }
        let gate = OperatorGate::new(Box::new(MustNotRun));
        assert!(gate
            .authorize(&MemoryActor::Model, "authenticated-scope-proof", request())
            .is_err());
    }

    #[test]
    fn misbound_host_attestations_are_rejected() {
        struct ConfusedVerifier(bool);
        impl OperatorVerifier for ConfusedVerifier {
            fn verify(
                &self,
                _: &str,
                _: &str,
                _: &MemoryRevisionRequest,
            ) -> Result<OperatorAttestation, String> {
                Ok(OperatorAttestation {
                    operator_id: if self.0 {
                        "different-operator"
                    } else {
                        "authorized-operator"
                    }
                    .into(),
                    approval_reference: if self.0 {
                        "approval-1"
                    } else {
                        "different-approval"
                    }
                    .into(),
                })
            }
        }
        for mismatch in [true, false] {
            let gate = OperatorGate::new(Box::new(ConfusedVerifier(mismatch)));
            assert!(gate
                .authorize(&operator(), "authenticated-scope-proof", request())
                .is_err());
        }
    }
}
