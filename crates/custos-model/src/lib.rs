//! Core domain model for Custos.
//!
//! A Resource is an object, a Dependency is a relationship, a Plugin is a
//! capability implementation, a RecoveryPolicy is a policy and a RecoveryRun
//! is one execution instance. These concepts must not be conflated
//! (docs/guide/overview.md §2.1).

pub mod audit;
pub mod dependency;
pub mod diagnosis;
pub mod evidence;
pub mod graph;
pub mod health;
pub mod ids;
pub mod incident;
pub mod notification;
pub mod recovery;
pub mod resource;
pub mod snapshot;

pub use audit::AuditEvent;
pub use dependency::{AlternativeGroup, Dependency, DependencyRequirement, HealthCondition};
pub use diagnosis::{Diagnosis, FaultCode};
pub use evidence::Evidence;
pub use graph::{DependencyGraph, GraphError};
pub use health::HealthStatus;
pub use ids::{ActionId, EventId, IncidentId, RecoveryRunId, ResourceId};
pub use incident::Incident;
pub use notification::{EventType, NotificationEvent, SanitizedDetails, Severity};
pub use recovery::{
    ActionAttempt, ActionOutcome, ActionSpec, FailureHandling, RecoveryPlan, RecoveryRun,
    RecoveryRunStatus, RiskLevel, TransitionError, VerificationResult,
};
pub use resource::{HealthPolicy, RecoveryPolicy, Resource};
pub use snapshot::ResourceHealthSnapshot;
