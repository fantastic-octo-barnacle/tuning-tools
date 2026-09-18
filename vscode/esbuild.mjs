// Bundles the extension, plus two scripts that run without VS Code: the smoke test (studio-server
// alone) and the harness (the extension against a stand-in `vscode` module).
import * as esbuild from "esbuild";

const watch = process.argv.includes("--watch");
const common = {
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
  sourcemap: true,
  logLevel: "info",
};
const builds = [
  { ...common, entryPoints: ["src/extension.ts"], outfile: "dist/extension.js", external: ["vscode"] },
  { ...common, entryPoints: ["scripts/smoke.ts"], outfile: "dist/smoke.js" },
  // The extension against a stand-in `vscode` module
  { ...common, entryPoints: ["scripts/harness.ts"], outfile: "dist/harness.js", alias: { vscode: "./scripts/fakeVscode.ts" } },
];

if (watch) {
  for (const options of builds) await (await esbuild.context(options)).watch();
} else {
  await Promise.all(builds.map((options) => esbuild.build(options)));
}
