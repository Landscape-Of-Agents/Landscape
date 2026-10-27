// Every webview page of Landscape must carry a script that parses: the scripts are written inside
// template literals, where one wrong escape (\" or \n) silently breaks the whole view. The pages
// are rendered with a stub of the vscode module (node --test).

import assert from "node:assert/strict";
import Module from "node:module";
import * as path from "node:path";
import { test } from "node:test";

/** The parts of Node's module loader this test hooks. */
const loader = Module as unknown as { _resolveFilename: (request: string, ...rest: unknown[]) => string; _cache: Record<string, unknown> };

/**
 * Render a view module's page through its html() function, with vscode stubbed.
 * @param file compiled module next to this test's parent folder
 * @returns the page
 */
function render(file: string, args: unknown[] = ["vscode-resource:", "logo.png"]): string {
  const resolve = loader._resolveFilename;
  loader._resolveFilename = (request, ...rest) => (request === "vscode" ? "vscode-stub" : resolve.call(Module, request, ...rest));
  loader._cache["vscode-stub"] = { id: "vscode-stub", filename: "vscode-stub", loaded: true, exports: {} };
  try {
    const full = path.join(__dirname, "..", file);
    const source = require("fs").readFileSync(full, "utf8") + "\nmodule.exports.__html = html;";
    const compiled = new Module(full) as unknown as { filename: string; paths: string[]; _compile: (code: string, name: string) => void; exports: { __html: (...values: unknown[]) => string } };
    compiled.filename = full;
    compiled.paths = (Module as unknown as { _nodeModulePaths: (directory: string) => string[] })._nodeModulePaths(path.dirname(full));
    compiled._compile(source, full);
    return compiled.exports.__html(...args);
  } finally {
    loader._resolveFilename = resolve;
  }
}

/**
 * Load a compiled module with vscode stubbed.
 * @param file compiled module next to this test's parent folder
 * @returns its exports
 */
function load<T>(file: string): T {
  const resolve = loader._resolveFilename;
  loader._resolveFilename = (request, ...rest) => (request === "vscode" ? "vscode-stub" : resolve.call(Module, request, ...rest));
  loader._cache["vscode-stub"] = { id: "vscode-stub", filename: "vscode-stub", loaded: true, exports: { TreeItem: class {} } };
  try {
    return require(path.join(__dirname, "..", file)) as T;
  } finally {
    loader._resolveFilename = resolve;
  }
}

test("every webview script parses", () => {
  const { BRICKS_PAGE } = load<{ BRICKS_PAGE: unknown }>("bricksPanel.js");
  const { AGENTS_PAGE } = load<{ AGENTS_PAGE: unknown }>("agentsPanel.js");
  const pages: [string, unknown[] | undefined][] = [["panel.js", undefined], ["sidebar.js", undefined], ["graphView.js", undefined], ["workbench.js", ["vscode-resource:", BRICKS_PAGE]], ["workbench.js", ["vscode-resource:", AGENTS_PAGE]]];
  for (const [file, args] of pages) {
    const page = render(file, args);
    const script = page.split(/<script nonce="[^"]+">/)[1]?.split("</script>")[0];
    assert.ok(script, `${file} has no script`);
    assert.doesNotThrow(() => new Function(script), `${file}'s script does not parse`);
  }
});
