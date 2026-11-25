import type { ReactElement } from "react";
import { Root } from "#ui/root.tsx";
import { application } from "#ui/browser.ts";
import { useAtomSuspense } from "@effect/atom-react";

const Application: () => ReactElement = () => <Root store={useAtomSuspense(application).value} />;

export { Application };
