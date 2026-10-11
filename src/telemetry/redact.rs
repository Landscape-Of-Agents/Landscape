// Redaction applied before anything is persisted or sent: credentials, tokens, private keys, and
// password assignments are replaced by "[REDACTED:kind]", and the number of findings is recorded
// as a security measurement. Matching is deliberately conservative and deterministic; it reduces
// exposure but cannot prove that no secret remains, which is why raw tool payloads and prompts
// are never stored at all.

/** Remove the user information (user:password@) from every URL in a text, keeping the scheme
 * and host so the message stays useful; words without a URL are kept as they are
 * Input
    - text: &str - text that may contain URLs with credentials
 * Output
    - String
*/
pub(crate) fn strip_url_credentials(text: &str) -> String {
    text.split(' ')
        .map(|word| match (word.find("://"), word.rfind('@')) {
            (Some(scheme), Some(at)) if at > scheme => {
                format!("{}://[REDACTED]@{}", &word[..scheme], &word[at + 1..])
            }
            _ => word.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/** Known token prefixes and their kinds, with the minimum length of the token that follows */
const PREFIXES: &[(&str, &str, usize)] = &[
    ("AKIA", "aws_access_key", 16),
    ("ASIA", "aws_access_key", 16),
    ("ghp_", "github_token", 30),
    ("gho_", "github_token", 30),
    ("ghu_", "github_token", 30),
    ("ghs_", "github_token", 30),
    ("ghr_", "github_token", 30),
    ("github_pat_", "github_token", 30),
    ("glpat-", "gitlab_token", 20),
    ("xoxb-", "slack_token", 20),
    ("xoxp-", "slack_token", 20),
    ("xoxa-", "slack_token", 20),
    ("xoxs-", "slack_token", 20),
    ("sk-ant-", "anthropic_key", 20),
    ("sk-proj-", "openai_key", 20),
    ("sk_live_", "stripe_key", 16),
    ("rk_live_", "stripe_key", 16),
    ("AIza", "google_api_key", 30),
    ("eyJ", "jwt", 30),
    ("hooks.slack.com/services/", "slack_webhook", 20),
];

/** Words that, followed by '=' or ':', introduce a secret value */
const ASSIGNMENTS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "access_key",
    "private_key",
    "client_secret",
    "authorization",
    "bearer",
];

/** Check whether a character can be part of a token
 * Input
    - character: char - character
 * Output
    - bool
*/
fn token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || "-_./+=".contains(character)
}

/** Redact secrets in text
 * Input
    - text: &str - text that may contain secrets
 * Output
    - (String, usize) redacted text and the number of secrets found
*/
pub(crate) fn redact(text: &str) -> (String, usize) {
    let mut output = text.to_string();
    let mut found = 0;
    if let (Some(start), Some(end)) = (output.find("-----BEGIN"), output.find("PRIVATE KEY-----")) {
        if end > start {
            let close = output[end..]
                .find("-----END")
                .and_then(|offset| {
                    output[end + offset..]
                        .find("KEY-----")
                        .map(|tail| end + offset + tail + 8)
                })
                .unwrap_or(output.len());
            output.replace_range(start..close, "[REDACTED:private_key]");
            found += 1;
        }
    }
    for (prefix, kind, minimum) in PREFIXES {
        let mut from = 0;
        while let Some(offset) = output[from..].find(prefix) {
            let start = from + offset;
            let length = output[start..]
                .chars()
                .take_while(|character| token_char(*character))
                .map(char::len_utf8)
                .sum::<usize>();
            if length >= prefix.len() + minimum {
                let replacement = format!("[REDACTED:{kind}]");
                output.replace_range(start..start + length, &replacement);
                from = start + replacement.len();
                found += 1;
            } else {
                from = start + prefix.len();
            }
        }
    }
    let lower = output.to_ascii_lowercase();
    let mut replacements = Vec::new();
    for word in ASSIGNMENTS {
        let mut from = 0;
        while let Some(offset) = lower[from..].find(word) {
            let start = from + offset;
            let mut index = start + word.len();
            let rest = &lower[index..];
            let skipped = rest.len() - rest.trim_start_matches(['"', '\'', ' ']).len();
            index += skipped;
            let separator = lower[index..].chars().next();
            if matches!(separator, Some('=' | ':')) {
                index += 1;
                let rest = &lower[index..];
                index += rest.len() - rest.trim_start_matches(['"', '\'', ' ']).len();
                if word == &"authorization" && lower[index..].starts_with("bearer ") {
                    index += 7;
                }
                let length = output[index..]
                    .chars()
                    .take_while(|character| token_char(*character))
                    .map(char::len_utf8)
                    .sum::<usize>();
                if length >= 4 && !output[index..index + length].starts_with("[REDACTED") {
                    replacements.push((index, index + length));
                }
            }
            from = start + word.len();
        }
    }
    replacements.sort();
    replacements.dedup();
    for (start, end) in replacements.into_iter().rev() {
        output.replace_range(start..end, "[REDACTED:secret]");
        found += 1;
    }
    (output, found)
}

#[cfg(test)]
mod tests {
    /** Check that URL credentials are removed and other words are kept
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn strips_url_credentials() {
        use super::strip_url_credentials;
        assert_eq!(
            strip_url_credentials("clone https://token@github.com/a/b now"),
            "clone https://[REDACTED]@github.com/a/b now"
        );
        assert_eq!(
            strip_url_credentials("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
        assert_eq!(strip_url_credentials("app/payments.py"), "app/payments.py");
    }

    use super::redact;

    /** Check that common secrets are redacted and ordinary text is kept
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn redacts_common_secrets() {
        let (text, count) = redact("curl -H 'Authorization: Bearer abcdefghijklmnop' https://x");
        assert!(!text.contains("abcdefghijklmnop"), "{text}");
        assert_eq!(count, 1);
        let (text, count) = redact("export GITHUB_TOKEN=ghp_0123456789abcdefghijklmnopqrstuvwxyz");
        assert!(!text.contains("ghp_0123"));
        assert!(count >= 1);
        let (text, _) = redact("password = \"hunter22\" and AKIAABCDEFGHIJKLMNOP");
        assert!(
            !text.contains("hunter22") && !text.contains("AKIAABCD"),
            "{text}"
        );
        let (text, count) = redact("cargo test --release");
        assert_eq!((text.as_str(), count), ("cargo test --release", 0));
        let key = "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----";
        assert_eq!(redact(key).0, "[REDACTED:private_key]");
    }
}
