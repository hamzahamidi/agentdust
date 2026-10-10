#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const targets = {
  "darwin-arm64": "darwin-arm64",
  "linux-x64": "linux-x64",
};
const target = targets[`${process.platform}-${process.arch}`];

if (!target) {
  console.error(`agentdust: no native binary for ${process.platform}-${process.arch}`);
  process.exit(1);
}

const binary = path.resolve(__dirname, "..", "vendor", target, "agentdust");
const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`agentdust: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
