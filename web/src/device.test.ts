import { expect, test } from "vitest";
import { deviceName } from "./device.ts";

const iphone =
	"Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1";
const mac =
	"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";
const android =
	"Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Mobile Safari/537.36";

test("a user agent names its device", () => {
	expect(deviceName(iphone, 5)).toBe("iPhone");
	expect(deviceName(mac, 0)).toBe("Mac");
	expect(deviceName(android, 5)).toBe("Android");
	expect(deviceName("curl/8.0", 0)).toBe("Browser");
});

test("a Mac with a touchscreen is an iPad", () => {
	expect(deviceName(mac, 5)).toBe("iPad");
});
