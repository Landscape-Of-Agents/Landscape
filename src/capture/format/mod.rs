// Evidence Format: raw artifacts (the original content, hashed, redacted, and stored by
// reference), the versioned normalized Evidence Record, and the typed payload parsers that turn
// one into the other while preserving provenance and reporting what they could not parse.

pub(crate) mod artifact;
pub(crate) mod parsers;
pub(crate) mod record;
