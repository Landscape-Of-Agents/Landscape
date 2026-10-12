// The stable local interface to Crane: the extension runs the Crane CLI without a shell and reads
// its JSON output. It never reads Crane's trust directory, keys, or secrets.

import { execFile } from "child_process";

/** The result of a Crane invocation. */
export interface CraneResult {
  code: number;
  stdout: string;
  stderr: string;
}

/** Where a selection resolves now, as reported by `crane validate --json`. */
export interface Span {
  path: string;
  start_line: number;
  end_line: number;
  start_byte: number;
  end_byte: number;
}

/** A structured finding reported by Crane. */
export interface Finding {
  code: string;
  severity: "LOW" | "MEDIUM" | "HIGH" | "CRITICAL";
  message: string;
  selection: string | null;
  path: string | null;
}

/** The JSON printed by `crane validate --json`. */
export interface ValidateReport {
  valid: boolean;
  generation: number;
  findings: Finding[];
  selections: Record<string, Span | null>;
}

/** One evaluated command in `crane test . --json`. */
export interface CommandResult {
  policy: string;
  file: string;
  line: number;
  operation: "preserve" | "target";
  id: string;
  outcome: "PASS" | "FAIL" | "UNRESOLVED" | "PENDING";
  message: string;
  location: Span | null;
}

/** The JSON printed by `crane test . --json`. */
export interface TestReport {
  passed: boolean;
  findings: Finding[];
  policies: { name: string; file: string; passed: boolean; commands: CommandResult[] }[];
}

/**
 * Run Crane with arguments in a directory, without a shell. Standard input always ends after the
 * given answers, so a question Crane asks can never leave it waiting.
 * @param cranePath executable
 * @param cwd working directory (the repository)
 * @param args arguments
 * @param input answers to Crane's questions, one per line (none by default)
 * @returns exit code and output; a missing executable is reported as code 127
 */
export function runCrane(cranePath: string, cwd: string, args: string[], input = ""): Promise<CraneResult> {
  return new Promise((resolve) => {
    const child = execFile(cranePath, args, { cwd, maxBuffer: 32 * 1024 * 1024, windowsHide: true }, (error, stdout, stderr) => {
      const code = error && typeof (error as NodeJS.ErrnoException).code === "string" ? 127 : error ? ((error as { code?: number }).code ?? 1) : 0;
      resolve({ code, stdout: String(stdout), stderr: String(stderr) || (error && code === 127 ? `could not run ${cranePath}: ${error.message}` : "") });
    });
    child.stdin?.on("error", () => undefined);
    child.stdin?.end(input);
  });
}

/**
 * Parse JSON printed by Crane.
 * @param result invocation result
 * @returns the parsed value, or undefined when stdout is not JSON
 */
export function parseJson<T>(result: CraneResult): T | undefined {
  try {
    return JSON.parse(result.stdout) as T;
  } catch {
    return undefined;
  }
}

/**
 * Build the arguments of `crane protect` or `crane target` for a line range.
 * @param operation preserve (protect) or target
 * @param file repository-relative path
 * @param startLine first line, 1-based
 * @param endLine last line, 1-based
 * @param options policy, change type, and name
 * @returns arguments
 */
export function selectionArgs(
  operation: "preserve" | "target",
  file: string,
  startLine: number,
  endLine: number,
  options: { policy?: string; changeType?: string; name?: string },
): string[] {
  const args = [operation === "preserve" ? "protect" : "target", file, "start-line", String(startLine), "end-line", String(endLine)];
  if (options.policy) args.push("policy", options.policy);
  if (operation === "target" && options.changeType) args.push("change_type", options.changeType);
  if (options.name) args.push("name", options.name);
  return args;
}

/**
 * Extract the selection identifier from the output of protect or target.
 * @param output command output
 * @returns the identifier, or undefined
 */
export function selectionIdFrom(output: string): string | undefined {
  return /as selection ([A-Z0-9]{8})/.exec(output)?.[1];
}
