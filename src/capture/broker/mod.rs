// The Evidence Query Broker (cross-cutting): versioned Evidence Recipes with explicit branch
// predicates, and the bounded broker that resolves scope and authorization, validates every query,
// executes permitted connector operations, normalizes and correlates the results, and returns an
// Evidence Pack with a complete audit trail. It is not a pass-through proxy. Recipes learned from
// work arrive as proposals and enter the RecipeBook only when a person accepts them.

pub(crate) mod proposals;
pub(crate) mod recipe;
pub(crate) mod run;
