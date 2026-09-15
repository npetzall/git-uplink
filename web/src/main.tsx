import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { HomePage } from "./pages/home";
import { CollaborationPage } from "./pages/collaboration";
import { LabPage } from "./pages/lab";
import { PlaybookPage } from "./pages/playbook";
import { QueuePage } from "./pages/queue";
import { WorkingPage } from "./pages/working";
import "./index.css";

function App() {
  return (
    <Routes>
      <Route path="/" element={<HomePage />} />
      <Route path="/collaboration" element={<CollaborationPage />} />
      <Route path="/lab" element={<LabPage />} />
      <Route path="/playbook" element={<PlaybookPage />} />
      <Route path="/queue" element={<QueuePage />} />
      <Route path="/working" element={<WorkingPage />} />
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
