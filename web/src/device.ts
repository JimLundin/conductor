// The name the app gives in `Hello`: the kind of device it runs on, so a resolved card can say
// which device approved.
import type { ClientName } from "./proto/index.ts";

interface Device {
	readonly marker: string;
	readonly name: string;
	readonly touchName: string;
}

// Checked in this order: an iPhone's or iPad's user agent also says "Mac OS X", and Android's
// and ChromeOS's also say "Linux". iPadOS Safari sends a Mac's user agent, so a "Mac" with a
// touchscreen is an iPad.
const devices: readonly Device[] = [
	{ marker: "iPhone", name: "iPhone", touchName: "iPhone" },
	{ marker: "iPad", name: "iPad", touchName: "iPad" },
	{ marker: "Android", name: "Android", touchName: "Android" },
	{ marker: "CrOS", name: "ChromeOS", touchName: "ChromeOS" },
	{ marker: "Windows", name: "Windows", touchName: "Windows" },
	{ marker: "Macintosh", name: "Mac", touchName: "iPad" },
	{ marker: "Linux", name: "Linux", touchName: "Linux" },
];

// More than one touch point is a touchscreen; a user agent naming no known device is a
// "Browser".
export const deviceName = (
	userAgent: string,
	touchPoints: number,
): ClientName => {
	const device = devices.find(({ marker }) => userAgent.includes(marker));
	if (device === undefined) {
		return "Browser";
	}
	return touchPoints > 1 ? device.touchName : device.name;
};
