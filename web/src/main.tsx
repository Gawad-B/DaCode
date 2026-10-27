import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

// The logo, imported rather than copied, for the same reason as the screenshots.
import markOnDark from "../../logo/dacode/mark-on-dark-128.png";
import markOnLight from "../../logo/dacode/mark-on-light-128.png";

/*
 * The favicon sits in the browser's chrome, not on the page, so it follows the
 * chrome's scheme rather than the page's theme toggle.
 */
function setIcon() {
  const dark = window.matchMedia("(prefers-color-scheme: dark)");
  let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) {
    link = document.createElement("link");
    link.rel = "icon";
    link.type = "image/png";
    document.head.appendChild(link);
  }
  const apply = () => {
    link!.href = dark.matches ? markOnDark : markOnLight;
  };
  apply();
  dark.addEventListener("change", apply);
}

setIcon();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
