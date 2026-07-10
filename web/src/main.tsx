// file: web/src/main.tsx
// version: 1.0.0
// guid: 8dd3049a-361e-4fdf-80a0-d2b4b98c4002
// last-edited: 2026-07-10

import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import "./index.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("root element #root not found in index.html");
}

ReactDOM.createRoot(container).render(
  <React.StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </React.StrictMode>,
);
