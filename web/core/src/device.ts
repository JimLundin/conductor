import type { ClientName } from "#core/schemas/index";

interface Device {
  readonly marker: string;
  readonly name: string;
  readonly multiTouchName: string;
}

const devicesBySpecificity = [
  { marker: "iPhone", multiTouchName: "iPhone", name: "iPhone" },
  { marker: "iPad", multiTouchName: "iPad", name: "iPad" },
  { marker: "Android", multiTouchName: "Android", name: "Android" },
  { marker: "CrOS", multiTouchName: "ChromeOS", name: "ChromeOS" },
  { marker: "Windows", multiTouchName: "Windows", name: "Windows" },
  { marker: "Macintosh", multiTouchName: "iPad", name: "Mac" },
  { marker: "Linux", multiTouchName: "Linux", name: "Linux" },
] as const satisfies ReadonlyArray<Device>;

const singleTouch = 1;

const deviceName = (userAgent: string, touchPoints: number): ClientName => {
  const device = devicesBySpecificity.find(({ marker }) => userAgent.includes(marker));
  if (device === undefined) {
    return "Browser";
  }
  return touchPoints > singleTouch ? device.multiTouchName : device.name;
};

export { deviceName };
