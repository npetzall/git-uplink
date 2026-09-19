import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { QueuePage } from "./pages/queue";
import { PatchPage } from "./pages/patch";
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

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </React.StrictMode>,
);
