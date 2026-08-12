//! sea-orm entities backing the run/protocol store. Schema-synced under the
//! path `not_yet_done_workflow::entity::*`, so pointing the store at a shared
//! database leaves other adapters' tables untouched.

pub mod run_step;
pub mod workflow_run;
