// Resources: the registry of evidence sources, the connector contract every source implements,
// the scope resolver that runs before any query, the deterministic fixture connector, and the
// connectors over this repository's own sources (Git, DataSea, Landscape, Crane).

pub(crate) mod connector;
pub(crate) mod fixture;
pub(crate) mod local;
pub(crate) mod registry;
pub(crate) mod scope;
