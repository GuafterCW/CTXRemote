// Checks the browser cryptography (src/lib/crypto.ts) against values from
// the Rust code (`vectors_stay_stable` in crates/core/src/account.rs). If this
// fails, browser and app would derive different keys.
import { build } from "vite";
import { strict as assert } from "node:assert";
import { readdirSync } from "node:fs";
import { join, resolve } from "node:path";

// Bundle crypto.ts for Node with Vite, so the test runs the shipped code.
// Inside node_modules, so the libraries it imports resolve.
const out = resolve("node_modules/.cache/vectors");
await build({
  logLevel: "error",
  configFile: false,
  build: { lib: { entry: "src/lib/crypto.ts", formats: ["es"], fileName: "crypto" }, outDir: out, emptyOutDir: true, ssr: true },
});
const file = readdirSync(out).find((f) => f.startsWith("crypto"));
const c = await import(join(out, file));
const hex = (b) => c.toHex(b);

const kdf = { memoryKib: 19 * 1024, iterations: 2, parallelism: 1 };
const pw = await c.passwordKeys("correct horse battery", new Uint8Array(16).fill(1), kdf);
assert.equal(hex(pw.auth), "bfeaa4346470058b73858a7f1a22b7c14bc7333469cbc3669b3190ffe86cbe61");
assert.equal(hex(pw.wrap), "43c99233e76be19472996ce0fb75cf6c3f3482c3dd7031a2bd04cfc4dc83a933");

const rec = c.recoveryKeys("abcde fghjk-mnpqr stvwx yz012");
assert.equal(hex(rec.auth), "94ef0956acd1674365fdc35d6c457da4d2e3a93b672ad04c3b6f3200c4ddaa62");
assert.equal(hex(rec.wrap), "04f1a5a986fa44b667cf0b7a9b7f46e308b54f5298b52b2c03b26651dd359428");

const pairing = await c.pairingKeys("EFGHJKMN", new Uint8Array(16).fill(2));
assert.equal(hex(pairing.sealKey), "68c882c12cf480262a38328764a02873a37745a1787bf8b439eae5c7566943d4");
assert.equal(hex(pairing.proof), "38149ca52fcf37b53161554d801b005e73525581ceebef0495687178b77fa16e");

const key = new Uint8Array(32).fill(5);
const sealed = c.fromHex("0407bad1d051ff1bcfd6380aded890e7ea4e8e4e9e6e067c700dd820394995354249f07f449eb17a66a556387009ea44f1b1c014e3785b");
assert.equal(new TextDecoder().decode(c.open(key, c.AAD.book, sealed)), '{"entries":{},"removed":{}}');
assert.throws(() => c.open(key, c.AAD.label, sealed));
// Round trip with a fresh nonce.
assert.equal(new TextDecoder().decode(c.open(key, c.AAD.label, c.seal(key, c.AAD.label, new TextEncoder().encode("Büro")))), "Büro");

assert.equal(c.normalizeEmail(" Philipp@Example.ORG "), "philipp@example.org");
for (const bad of ["", "a@b", "@b.de", "a@.de", "a@b.", "a b@c.de", "a@b@c.de"]) assert.equal(c.normalizeEmail(bad), null, bad);
assert.match(c.newRecoveryCode(), /^([0-9A-HJKMNP-TV-Z]{5}-){4}[0-9A-HJKMNP-TV-Z]{5}$/);
assert.match(c.newPairingCode().display, /^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$/);
console.log("Krypto-Prüfwerte: ok (Browser = App)");
