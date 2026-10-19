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

// Teams: values from `team::tests::vectors_stay_stable` in crates/core/src/team.rs.
const dec = (b) => new TextDecoder().decode(b);
const filled = (n, v) => new Uint8Array(n).fill(v);
const publicKey = c.exchangePublic(filled(32, 1));
assert.equal(hex(publicKey), "16ae5d36023369e4763ae8a0ae2b6bab2bec03b4ca2c72d9936079733aebe125");
assert.equal(hex(c.teamTag(filled(32, 9), 5, publicKey)), "380f0d7c20be9e5f0db88b309f6060561e8be16e00edb0568fdcbbc0bd15799f");
assert.equal(c.teamAccessPassword(filled(32, 9), 123456789), "c2fbab987d58ab1b2eec0a1c9c8d24b7e10c2772a9496694d4ec57c12ed7f541");
const teamId = filled(16, 7);
const ctx = c.teamContext(teamId, 3);
const boxed = c.fromHex("ab3db33ad7f1bca84cb50c22cc91ffaf2ca5121e9b134f211d1489d5b597591a9de4b6d8cbd87723503747a7cc29ab881a1afc178c39c15247820de2bc57f2471cbad5abec32e3cc9434794f4d7af516638087c9c8517c5dce346805");
assert.deepEqual(c.openWith(c.exchangeSecret(filled(32, 1)), ctx, boxed), filled(32, 9));
assert.throws(() => c.openWith(c.exchangeSecret(filled(32, 2)), ctx, boxed));
assert.throws(() => c.openWith(c.exchangeSecret(filled(32, 1)), c.teamContext(teamId, 4), boxed));
const teamBook = c.fromHex("6d52a2542760ef9708a4f728b8f5a47e9f71d3d566ae9ad652c02f73d1723896a961a7309b0ba4a6aa2ce141d2b1764e356b29a79db1047fefa40e6cb0f8f9f217280ebafa8c80994c4fc8730803d0b3a45417");
assert.equal(dec(c.open(filled(32, 9), c.teamAad(c.TEAM_AAD.book, teamId, 3), teamBook)), '{"name":"Werkstatt","book":{"entries":{},"removed":{}}}');
const accessList = c.fromHex("2b07cde8b3253f9d3f463811823a4790623d53a611c151f7f9d1f634376bd98c267f998fa97bfab2e75013167cd4620a2763cbf0ea253c8a7bb419e87a69c63488");
assert.equal(dec(c.open(filled(32, 9), c.teamAccessAad(teamId, 3, 5), accessList)), '{"hosts":{"123456789":["Empfang",1]}}');
assert.throws(() => c.open(filled(32, 9), c.teamAccessAad(teamId, 3, 6), accessList));
const invite = c.fromHex("b6b26c564ca21849b807b7ec218151220e485e2c96cef16fc5fb7318a0a4565a99dabd4ecfc16eca07e3e862c989778bdfde63c9b1d01eefdd6a2216");
assert.deepEqual(c.open(filled(32, 4), c.teamAad(c.TEAM_AAD.invite, teamId, 3), invite), filled(32, 9));
// Round trip with a fresh ephemeral key, and tags need the team key.
const again = c.sealTo(publicKey, ctx, filled(32, 9));
assert.deepEqual(c.openWith(c.exchangeSecret(filled(32, 1)), ctx, again), filled(32, 9));
assert.notDeepEqual(again, c.sealTo(publicKey, ctx, filled(32, 9)));
assert.equal(c.teamTagOk(filled(32, 9), 5, [publicKey, c.teamTag(filled(32, 9), 5, publicKey)]), true);
assert.equal(c.teamTagOk(filled(32, 8), 5, [publicKey, c.teamTag(filled(32, 9), 5, publicKey)]), false);
assert.equal(c.teamTagOk(filled(32, 9), 6, [publicKey, c.teamTag(filled(32, 9), 5, publicKey)]), false);
assert.equal(c.teamTagOk(filled(32, 9), 5, null), false);
assert.equal(c.normalizeCode("abcd-efgh-jkmn"), "ABCDEFGHJKMN");
assert.equal(c.normalizeCode(" O1IL 2345 6789 "), "011112345678".slice(0, 4) + "23456789");
assert.equal(c.normalizeCode("ABCD-EFGH-JKM"), null);
assert.equal(c.normalizeCode("ABCD-EFGH-JKMU"), null);
console.log("Krypto-Prüfwerte inkl. Team: ok (Browser = App)");
