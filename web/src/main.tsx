import { Application } from "#ui/application.tsx";
import { RegistryProvider } from "@effect/atom-react";
import { Suspense } from "react";
import { createRoot } from "react-dom/client";

const element = document.querySelector("#root");
if (element) {
  createRoot(element).render(
    <RegistryProvider>
      <Suspense>
        <Application />
      </Suspense>
    </RegistryProvider>,
  );
}
