import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { HomePage } from "./pages/home";
import { CollaborationPage } from "./pages/collaboration";
import { ExamplesPage } from "./pages/examples";
import { InstallPage } from "./pages/install";
import { InternalsPage } from "./pages/internals";
import { LabPage } from "./pages/lab";
import { PlaybookPage } from "./pages/playbook";
import { SetupPage } from "./pages/setup";
import { WorkingPage } from "./pages/working";
import "./index.css";

const basename = (() => {
  const base = import.meta.env.BASE_URL;
  if (!base || base === "/") return undefined;
  return base.replace(/\/$/, "");
})();

function App() {
  return (
    <Routes>
      <Route path="/" element={<HomePage />} />
      <Route path="/collaboration" element={<CollaborationPage />} />
      <Route path="/examples" element={<ExamplesPage />} />
      <Route path="/install" element={<InstallPage />} />
      <Route path="/internals" element={<InternalsPage />} />
      <Route path="/lab" element={<LabPage />} />
      <Route path="/playbook" element={<PlaybookPage />} />
      <Route path="/setup" element={<SetupPage />} />
      <Route path="/working" element={<WorkingPage />} />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <BrowserRouter basename={basename}>
      <App />
    </BrowserRouter>
  </React.StrictMode>,
);
