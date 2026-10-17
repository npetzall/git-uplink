import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { PatchPage } from "./pages/patch";
import { QueuePage } from "./pages/queue";
import "./index.css";

function App() {
  return (
    <Routes>
      <Route path="/" element={<QueuePage />} />
      <Route path="/patches/:id" element={<PatchPage />} />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}

const root = document.getElementById("root");
if (!root) {
  throw new Error("index.html has no #root element");
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </React.StrictMode>,
);
