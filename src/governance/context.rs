// .crane/context.json: policy context bindings. A text file bound to a policy is supplied to the agent with that policy when a session starts. Context is guidance, never authorization: it cannot relax a policy, and the hook decides independently of it. The digest of each bound file is recorded, so a context file changed after binding is reported as stale.

use serde::{Deserialize, Serialize};

/** Version of the context file layout */
pub(crate) const CONTEXT_FORMAT: u64 = 1;

/** Largest context file accepted, in bytes */
pub(crate) const MAX_CONTEXT_BYTES: usize = 256 * 1024;

/** One policy-to-file binding
 * Fields
    - policy: String - policy name
    - path: String - repository-relative path of the text file
    - digest: String - SHA-512 of the file content when bound
    - bound_at: u64 - Unix seconds
    - bound_by: String - actor
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Binding {
    pub(crate) policy: String,
    pub(crate) path: String,
    pub(crate) digest: String,
    pub(crate) bound_at: u64,
    pub(crate) bound_by: String,
}

/** The context file
 * Fields
    - format: u64 - layout version
    - bindings: Vec<Binding> - bindings in insertion order
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Bindings {
    pub(crate) format: u64,
    pub(crate) bindings: Vec<Binding>,
}

impl Default for Bindings {
    /** Build an empty binding list
     * Input
        - None
     * Output
        - Bindings
    */
    fn default() -> Self {
        Self {
            format: CONTEXT_FORMAT,
            bindings: Vec::new(),
        }
    }
}

impl Bindings {
    /** List the bindings of one policy
     * Input
        - policy: &str - policy name
     * Output
        - Vec<&Binding>
    */
    pub(crate) fn of(&self, policy: &str) -> Vec<&Binding> {
        self.bindings
            .iter()
            .filter(|binding| binding.policy == policy)
            .collect()
    }

    /** Bind a file to a policy, replacing an earlier binding of the same file to that policy
     * (which refreshes its digest), so the command is idempotent
     * Input
        - binding: Binding - new binding
     * Output
        - bool, true if an earlier binding was replaced
    */
    pub(crate) fn bind(&mut self, binding: Binding) -> bool {
        let before = self.bindings.len();
        self.bindings.retain(|existing| {
            !(existing.policy == binding.policy && existing.path == binding.path)
        });
        let replaced = self.bindings.len() != before;
        self.bindings.push(binding);
        replaced
    }
}
