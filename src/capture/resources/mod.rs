// Resources: the registry of evidence sources, the connector contract every source implements,
// the scope resolver that runs before any query, the deterministic fixture connector, the
// connectors over this repository's own sources (Git, DataSea, Landscape, Crane), and the live
// GitHub connector (pull requests, commit files, and check runs, through the GitHub CLI).

pub(crate) mod connector;
pub(crate) mod fixture;
pub(crate) mod github;
pub(crate) mod local;
pub(crate) mod registry;
pub(crate) mod scope;
