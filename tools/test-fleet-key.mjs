import assert from "node:assert/strict";
import { createPublicKey, verify } from "node:crypto";
import { fileURLToPath } from "node:url";
import { chromium, webkit } from "../crates/app/tests-e2e/node_modules/playwright/index.mjs";

const origin = "http://localhost:21036";
const modulePath = fileURLToPath(new URL("../crates/net/src/fleet.js", import.meta.url));

function challenge(peer) {
	return Buffer.concat([
		Buffer.from("conductor fleet device session v1"),
		Buffer.alloc(32, 1),
		Buffer.alloc(32, 2),
		Buffer.alloc(32, peer),
	]);
}

const identity = (page, data) =>
	page.evaluate(async (message) => {
		const key = await window.fleetKey.fleetPublicKey();
		const signature = await window.fleetKey.signFleet(Uint8Array.from(message));
		return {
			key: Array.from(new Uint8Array(key)),
			signature: Array.from(new Uint8Array(signature)),
			online: navigator.onLine,
		};
	}, Array.from(data));

function authenticate(publicBytes, proof, data) {
	assert.equal(publicBytes.length, 32);
	assert.equal(proof.length, 64);
	const key = createPublicKey({
		// RFC 8410 SubjectPublicKeyInfo prefix followed by the raw Ed25519 key.
		key: Buffer.concat([
			Buffer.from("302a300506032b6570032100", "hex"),
			Buffer.from(publicBytes),
		]),
		format: "der",
		type: "spki",
	});
	assert.equal(verify(null, data, key, Buffer.from(proof)), true);
	assert.equal(verify(null, challenge(0), key, Buffer.from(proof)), false);
}

async function check(engine, type) {
	const browser = await type.launch();
	try {
		const context = await browser.newContext({
			offline: true,
			serviceWorkers: "block",
		});
		// Both pages reach genuine key generation only after reading an empty store.
		const barrier = Promise.withResolvers();
		let generations = 0;
		await context.exposeBinding("meetFirstOpen", () => {
			generations += 1;
			if (generations === 2) barrier.resolve();
			return barrier.promise;
		});
		await context.addInitScript(() => {
			const generate = SubtleCrypto.prototype.generateKey;
			SubtleCrypto.prototype.generateKey = async function (...args) {
				await window.meetFirstOpen();
				return Reflect.apply(generate, this, args);
			};
		});
		await context.route("**/*", async (route) => {
			const requested = route.request().url();
			if (requested === `${origin}/fleet.js`) {
				await route.fulfill({ contentType: "text/javascript", path: modulePath });
			} else if (requested === `${origin}/`) {
				await route.fulfill({
					contentType: "text/html",
					body: '<script type="module">import * as key from "/fleet.js";window.fleetKey=key;</script>',
				});
			} else {
				await route.abort();
			}
		});

		const first = await context.newPage();
		const second = await context.newPage();
		await Promise.all([first.goto(origin), second.goto(origin)]);
		const [left, right] = await Promise.all([
			identity(first, challenge(3)),
			identity(second, challenge(4)),
		]);
		assert.equal(generations, 2);
		assert.deepEqual(left.key, right.key);
		assert.equal(left.online, false);
		assert.equal(right.online, false);
		authenticate(left.key, left.signature, challenge(3));
		authenticate(left.key, right.signature, challenge(4));

		await first.reload();
		const later = await identity(first, challenge(5));
		assert.deepEqual(left.key, later.key);
		assert.equal(later.online, false);
		assert.equal(generations, 2);
		authenticate(left.key, later.signature, challenge(5));
		const extractable = await first.evaluate(() => new Promise((resolve, reject) => {
			const open = indexedDB.open("conductor-device", 1);
			open.onerror = () => reject(open.error);
			open.onsuccess = () => {
				const db = open.result;
				const lookup = db.transaction("identity").objectStore("identity").get("owner");
				lookup.onsuccess = () => {
					resolve(lookup.result.privateKey.extractable);
					db.close();
				};
				lookup.onerror = () => reject(lookup.error);
			};
		}));
		assert.equal(extractable, false);
		console.log(`${engine} ${browser.version()}: both first opens authenticated with one IndexedDB key; reload authenticated a new challenge offline; private key non-extractable`);
	} finally {
		await browser.close();
	}
}

for (const [engine, type] of [["Chromium", chromium], ["WebKit", webkit]]) {
	await check(engine, type);
}
