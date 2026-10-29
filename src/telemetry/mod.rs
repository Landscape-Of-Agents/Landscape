// Security telemetry: the taxonomy, the unified event store, canonical identity, the autonomy
// ledger, redaction, metrics, compliance evidence, and alerts. Runtime evidence lives in the
// trust directory's runtime folder, never in the application repository.

pub(crate) mod alerts;
pub(crate) mod compliance;
pub(crate) mod evals;
pub(crate) mod events;
pub(crate) mod identity;
pub(crate) mod ingest;
pub(crate) mod ledger;
pub(crate) mod model;
pub(crate) mod query;
pub(crate) mod redact;
pub(crate) mod rules;
pub(crate) mod spec;
pub(crate) mod spec_bricks;
pub(crate) mod spec_commands;
pub(crate) mod spec_flows;
pub(crate) mod spec_resident;
pub(crate) mod sync;
pub(crate) mod usage;

use std::path::{Path, PathBuf};

use events::EventStore;
use identity::IdentityStore;
use ledger::Ledger;

/** The runtime stores of one repository
 * Fields
    - directory: PathBuf - runtime directory
    - events: EventStore - event log
    - identity: IdentityStore - sessions and tasks
    - ledger: Ledger - autonomy credits
*/
#[derive(Debug, Clone)]
pub(crate) struct Stores {
    pub(crate) directory: PathBuf,
    pub(crate) events: EventStore,
    pub(crate) identity: IdentityStore,
    pub(crate) ledger: Ledger,
}

impl Stores {
    /** Open the stores in a runtime directory
     * Input
        - directory: &Path - runtime directory
     * Output
        - Stores
    */
    pub(crate) fn open(directory: &Path) -> Self {
        Self {
            directory: directory.to_path_buf(),
            events: EventStore::open(directory),
            identity: IdentityStore::open(directory),
            ledger: Ledger::open(directory),
        }
    }
}
