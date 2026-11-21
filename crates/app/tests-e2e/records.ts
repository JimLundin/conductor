import type { Page } from "playwright";
import { type Run, stamp, tools } from "./harness.ts";

const JOIN_MARKER = /#join=(?<ticket>[^\s"'<>]+)/gu;

const TAIL_LINES = 60;
const CONSOLE_STYLE = /%c|color:.*$/gsu;
const LINK_STATUS = /link status/u;
const LINK_ERROR = /error = Some/u;
// n0's public relays are best-effort: a correctly named relay can reset a
// WebSocket handshake, and the client retries. Those are reported, not failed.
// A relay named with the root dot is still a failure (Safari rejects it).
const RELAY_HICCUP =
	/^WebSocket connection to 'wss:\/\/[a-z0-9-]+\.relay\.n0\.iroh\.link\/relay' failed/u;
const DOTTED_RELAY = /relay\.n0\.iroh\.link\.\//u;
// WebKit, like iOS Safari, ignores the viewport's `interactive-widget`, which
// Chromium uses to keep the composer above the keyboard.
const IGNORED_VIEWPORT =
	/^Viewport argument key "interactive-widget" not recognized and ignored\.$/u;

const detail = (errors: readonly string[]): string => {
	if (errors.length === 0) {
		return "";
	}
	return `: ${errors.join(" | ")}`;
};

/** The errors left once the relay hiccups and the viewport notices are
 * logged as such. */
const excused = (run: Run, errors: readonly string[]): readonly string[] => {
	const hiccups = errors.filter(
		(e) => RELAY_HICCUP.test(e) && !DOTTED_RELAY.test(e),
	);
	const notices = errors.filter((e) => IGNORED_VIEWPORT.test(e));
	if (hiccups.length > 0) {
		run.log(
			`warning: ${hiccups.length} relay connection hiccup(s), retried by the client: ${hiccups[0]}`,
		);
	}
	if (notices.length > 0) {
		run.log(
			`note: ${notices.length} console notice(s) that the viewport's interactive-widget is ignored`,
		);
	}
	return errors.filter((e) => !(hiccups.includes(e) || notices.includes(e)));
};

/** What the pages logged: their errors, the link's errors, and the last
 * lines, kept to show after a failure. */
export interface Recorder {
	readonly watch: (page: Page) => void;
	readonly consoleError: (text: string) => void;
	readonly errors: () => readonly string[];
	readonly linkErrors: () => readonly string[];
	readonly tail: () => readonly string[];
}

export const recorder = (elapsed: () => number): Recorder => {
	let errors: readonly string[] = [];
	let linkErrors: readonly string[] = [];
	let tail: readonly string[] = [];
	const consoleError = (text: string): void => {
		errors = [...errors, text];
	};
	return {
		watch: (p: Page): void => {
			p.on("console", (msg) => {
				const text = msg.text();
				tail = [
					...tail,
					stamp(elapsed(), `${msg.type()}: ${text.replace(CONSOLE_STYLE, "")}`),
				].slice(-TAIL_LINES);
				if (msg.type() === "error") {
					consoleError(text);
				}
				if (LINK_STATUS.test(text) && LINK_ERROR.test(text)) {
					linkErrors = [...linkErrors, text.replace(CONSOLE_STYLE, "")];
				}
			});
			p.on("pageerror", (pageError) => {
				consoleError(String(pageError));
			});
		},
		consoleError,
		errors: (): readonly string[] => errors,
		linkErrors: (): readonly string[] => linkErrors,
		tail: (): readonly string[] => tail,
	};
};

export const checkConsole = (run: Run, record: Recorder): void => {
	const { check } = tools(run);
	const errors = record.errors();
	const linkErrors = record.linkErrors();
	check(
		linkErrors.length === 0,
		`the link never reported an error${detail(linkErrors)}`,
	);
	const realErrors = excused(run, errors);
	check(
		errors.every((e) => !DOTTED_RELAY.test(e)),
		"no relay dialled with the root dot",
	);
	check(realErrors.length === 0, `no console errors${detail(realErrors)}`);
};

export const pairingRedactor = (
	links: readonly (string | undefined)[],
): ((line: string) => string) => {
	const tickets = links.flatMap((link) =>
		Array.from(link?.matchAll(JOIN_MARKER) ?? [], (match) => match[1] ?? ""),
	);
	return (line: string): string => {
		let redacted = line.replace(JOIN_MARKER, "<ticket>");
		for (const ticket of tickets) {
			if (ticket) {
				redacted = redacted.replaceAll(ticket, "<ticket>");
			}
		}
		return redacted;
	};
};

export const traceOf = (error: unknown): string => {
	if (error instanceof Error && error.stack !== undefined) {
		return error.stack;
	}
	return String(error);
};

export const messageOf = (error: unknown): string => {
	if (error instanceof Error) {
		return error.message;
	}
	return String(error);
};
