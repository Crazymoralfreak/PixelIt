import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

// The context menu of the web view offers "Reload" and "Inspect", which make
// no sense in an application. Text fields keep theirs for copy and paste.
window.addEventListener("contextmenu", (event) => {
  const target = event.target;
  const editable =
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable);
  if (!editable) {
    event.preventDefault();
  }
});

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
