// The org catalog of Bricks on DataSea: how a published Brick version is signed and checked. An
// admin publishes an activated version from a machine ('crane brick publish', cli/brick/share.rs);
// the entry carries the exact definition, its digest, and a signature by that machine's signing
// key over the reference and the digest. DataSea keeps one entry per account (the GitHub owner of
// the repository's verified remote), Brick id, and version, and refuses a different digest under
// a version it holds, so a published version never changes. Another repository imports an entry
// ('crane brick import org:ID@N') as a draft, which grants nothing until its own admin activates
// it: the repository stays the authority for what applies there. The service and the machine both
// check an entry with verify().

use serde_json::{json, Value};

use super::model::Brick;
use crate::trust::crypto::verify_signature;

/** The signature domain of catalog entries */
pub(crate) const DOMAIN: &str = "brick/catalog/v1";

/** The format of a catalog entry */
pub(crate) const FORMAT: &str = "crane-brick-catalog/v1";

/** The format of a library file the dashboard downloads ('crane brick import FILE' reads it) */
pub(crate) const LIBRARY_FORMAT: &str = "crane-brick-library/v1";

/** The most entries one account's catalog holds */
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) const MAX_ENTRIES: u64 = 500;

/** The longest reason a publisher gives (characters) */
pub(crate) const MAX_REASON: usize = 500;

/** The bytes a publisher signs: the exact reference and the digest of its definition
 * Input
    - reference: &str - "ID@N"
    - digest: &str - Brick digest
 * Output
    - Vec<u8>
*/
pub(crate) fn message(reference: &str, digest: &str) -> Vec<u8> {
    format!("{reference}\n{digest}").into_bytes()
}

/** Build a catalog entry for an activated version
 * Input
    - brick: &Brick - definition
    - publisher_key: &str - the signing public key (hex)
    - signature: String - signature over message() under DOMAIN
    - published_by: &str - who published it
    - reason: &str - why
 * Output
    - Value
*/
pub(crate) fn entry(
    brick: &Brick,
    publisher_key: &str,
    signature: String,
    published_by: &str,
    reason: &str,
) -> Value {
    json!({
        "format": FORMAT,
        "ref": brick.reference().to_string(),
        "brick": brick,
        "digest": brick.digest(),
        "publisher_key": publisher_key,
        "signature": signature,
        "published_by": published_by,
        "reason": reason,
    })
}

/** Check a catalog entry: its format, that the definition is a current kind whose reference and
 * digest are the ones named, that the publisher's signature verifies, and that nothing in it looks
 * like a secret
 * Input
    - entry: &Value - catalog entry
 * Output
    - Result<Brick, String> the definition
    - Error naming what does not hold
*/
pub(crate) fn verify(entry: &Value) -> Result<Brick, String> {
    if entry["format"] != FORMAT {
        return Err(format!("not a catalog entry (format {FORMAT})"));
    }
    let brick = serde_json::from_value::<Brick>(entry["brick"].clone())
        .map_err(|error| format!("the entry's definition does not read: {error}"))?;
    if brick.brick_type.is_legacy() {
        return Err(format!(
            "{} is a former kind; migrate it before publishing",
            brick.brick_type.name()
        ));
    }
    let reference = brick.reference().to_string();
    if entry["ref"].as_str() != Some(reference.as_str()) {
        return Err(format!(
            "the entry names {} but holds {reference}",
            entry["ref"].as_str().unwrap_or("nothing")
        ));
    }
    let digest = brick.digest();
    if entry["digest"].as_str() != Some(digest.as_str()) {
        return Err(format!(
            "the definition of {reference} does not match its digest; it was changed after it was published"
        ));
    }
    verify_signature(
        entry["publisher_key"].as_str().unwrap_or_default(),
        DOMAIN,
        &message(&reference, &digest),
        entry["signature"].as_str().unwrap_or_default(),
    )
    .map_err(|error| format!("the publisher's signature on {reference}: {error}"))?;
    let text = serde_json::to_string(&json!([
        brick.config,
        brick.common_context.text,
        brick.description
    ]))
    .unwrap_or_default();
    if crate::telemetry::redact::redact(&text).1 > 0 {
        return Err(format!(
            "{reference} holds text that looks like a secret; it is not shared"
        ));
    }
    if entry["reason"]
        .as_str()
        .is_some_and(|reason| reason.chars().count() > MAX_REASON)
    {
        return Err(format!("the reason is longer than {MAX_REASON} characters"));
    }
    Ok(brick)
}

/** Parse a catalog reference
 * Input
    - text: &str - "org:ID@N" (or "ID@N")
 * Output
    - Result<(String, u32), String> the Brick id and version
    - Error when it is not an exact, valid reference
*/
pub(crate) fn reference(text: &str) -> Result<(String, u32), String> {
    let bare = text.trim().strip_prefix("org:").unwrap_or(text.trim());
    let (id, version) = bare
        .split_once('@')
        .ok_or_else(|| format!("'{text}' needs an exact version: org:ID@N"))?;
    crate::platform::validate_name("Brick", id)?;
    let version = version
        .parse::<u32>()
        .ok()
        .filter(|version| *version > 0)
        .ok_or_else(|| format!("'{text}' has no valid version"))?;
    Ok((id.to_string(), version))
}
