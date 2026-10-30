//! Recovery engine: dependency scheduling, the recovery state machine,
//! budgets and mutexes (docs/guide/workspace.md: `custos-engine`).

pub mod budget;
pub mod clock;
pub mod engine;
pub mod events;
pub mod executor;

pub use budget::{BudgetExceeded, BudgetState, RecoveryBudget};
pub use clock::{Clock, FakeClock, SystemClock};
pub use engine::{Engine, RecoverError};
pub use events::{EngineEvent, MemoryStore, RunStore, Severity};
pub use executor::{ActionExecutor, BuiltinExecExecutor, ScriptedExecutor};
