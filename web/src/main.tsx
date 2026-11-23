import { createRoot } from "react-dom/client";
import { Root } from "./root.tsx";

const element = document.querySelector("#root");
if (element) {
	createRoot(element).render(<Root />);
}
