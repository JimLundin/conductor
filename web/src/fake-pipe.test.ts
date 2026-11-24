import { expect, test } from "vitest";
import { type FakeHost, fakeHost } from "./fake-pipe.ts";
import type { Connection } from "./pipe.ts";

const bytes = (text: string): Uint8Array => new TextEncoder().encode(text);

const dial = (host: FakeHost): Promise<Connection> =>
	host.pipe.connect(
		new Uint8Array(32),
		"https://relay.example/",
		bytes("conductor/0"),
	);

test("frames the app sends reach the host in order, then the end", async () => {
	const host = await fakeHost();
	const stream = await (await dial(host)).openBi();
	await stream.send(bytes("one"));
	await stream.send(bytes(""));
	await stream.finish();
	const accepted = await host.accept();
	expect(await accepted.recv()).toEqual(bytes("one"));
	expect(await accepted.recv()).toEqual(bytes(""));
	expect(await accepted.recv()).toBeUndefined();
});

test("frames the host sends reach the app, then the end", async () => {
	const host = await fakeHost();
	const stream = await (await dial(host)).openBi();
	const accepted = await host.accept();
	await accepted.send(bytes("welcome"));
	accepted.finish();
	expect(await stream.recv()).toEqual(bytes("welcome"));
	expect(await stream.recv()).toBeUndefined();
});

test("each stream the app opens is its own", async () => {
	const host = await fakeHost();
	const connection = await dial(host);
	const first = await connection.openBi();
	const second = await connection.openBi();
	await second.send(bytes("second"));
	await first.send(bytes("first"));
	expect(await (await host.accept()).recv()).toEqual(bytes("first"));
	expect(await (await host.accept()).recv()).toEqual(bytes("second"));
});

test("the host closing resolves closed with its reason", async () => {
	const host = await fakeHost();
	const connection = await dial(host);
	host.close("bye");
	expect(await connection.closed()).toBe("bye");
});

test("the app closing resolves closed", async () => {
	const connection = await dial(await fakeHost());
	await connection.close();
	expect(await connection.closed()).toBe("closed");
});
