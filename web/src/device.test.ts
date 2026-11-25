import { describe, expect, it } from "@effect/vitest";
import { deviceName } from "@conductor/core/device";

const iphone =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1";
const mac =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";
const android =
  "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Mobile Safari/537.36";

describe(deviceName, () => {
  it.each([
    [iphone, 5, "iPhone"],
    [mac, 0, "Mac"],
    [android, 5, "Android"],
    ["curl/8.0", 0, "Browser"],
    [mac, 5, "iPad"],
  ] as const)("names the device %s with %i touch points as %s", (agent, touches, expected) => {
    expect.hasAssertions();
    expect(deviceName(agent, touches)).toBe(expected);
  });
});
