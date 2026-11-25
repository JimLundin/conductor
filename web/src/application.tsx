import { useAtomSuspense } from "@effect/atom-react";
import type { ReactElement } from "react";
import { application } from "#ui/browser.ts";
import { Root } from "#ui/root.tsx";

const Application = (): ReactElement => <Root store={useAtomSuspense(application).value} />;

export { Application };
