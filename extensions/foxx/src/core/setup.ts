// Setting up Crane in a repository from the editor: how to connect the clone's remote, and how to
// read the confirmation 'crane init' asks for when joining a repository that is already set up
// (trust the owner's key) or after a merge (accept another admin's signed rules). Editor-independent,
// so it is unit-tested.

/** What 'crane init' needs a person to confirm before it can finish. */
export type Confirmation = "trust_owner" | "accept_rules";

/**
 * Build the 'crane repo' arguments that connect a clone to its remote.
 * @param remote the clone's origin URL
 * @returns arguments, or undefined when the remote is not an HTTPS or SSH URL
 */
export function repoArgs(remote: string): string[] | undefined {
  const url = remote.trim();
  if (/^https:\/\//.test(url)) return ["repo", "--https", url];
  if (/^(ssh:\/\/|git@)/.test(url)) return ["repo", "--ssh", url];
  return undefined;
}

/**
 * Read the owner key fingerprint 'crane init' prints (groups of four hex characters).
 * @param output crane output
 * @returns the fingerprint as printed, or undefined
 */
export function ownerKey(output: string): string | undefined {
  const match = output.match(/owner key:\s+((?:[0-9a-f]{4} ?){16})/);
  return match ? match[1].trim() : undefined;
}

/**
 * Recognize the confirmation 'crane init' asked for (it was answered "no", so nothing changed).
 * @param output crane output (stdout and stderr)
 * @returns the confirmation, or undefined when none was asked
 */
export function confirmation(output: string): Confirmation | undefined {
  if (output.includes("Trust this owner key")) return "trust_owner";
  if (output.includes("Accept the current signed rules")) return "accept_rules";
  return undefined;
}

/**
 * Check a policy or checkpoint name the way Crane does.
 * @param value candidate
 * @returns an error message, or undefined when valid
 */
export function checkName(value: string): string | undefined {
  return /^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(value) ? undefined : "start with a letter; use letters, digits, '_' or '-' (at most 64)";
}
