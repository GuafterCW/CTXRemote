// Builds ctxremote-service and copies it where Tauri expects the sidecar (Windows only).
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(process.env.CARGO_TARGET_DIR)
  : join(root, "target");

const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)?.[1];
if (!host) throw new Error("Target-Triple nicht ermittelbar (rustc -vV)");

execFileSync("cargo", ["build", "--release", "--locked", "-p", "ctxremote-service"], {
  cwd: root,
  stdio: "inherit",
});

const outDir = join(root, "app", "src-tauri", "binaries");
mkdirSync(outDir, { recursive: true });
const dest = join(outDir, `ctxremote-service-${host}.exe`);
copyFileSync(join(targetDir, "release", "ctxremote-service.exe"), dest);
console.log(`Dienst bereitgestellt: ${dest}`);
