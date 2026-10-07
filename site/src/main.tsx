import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { HomePage } from "./pages/home";
import { CliPage } from "./pages/cli";
import { DayToDayPage } from "./pages/day-to-day";
import { ExamplesPage } from "./pages/examples";
import { InstallPage } from "./pages/install";
import { InternalsPage } from "./pages/internals";
import { LabPage } from "./pages/lab";
import { HowPage } from "./pages/how";
import { SecurityPage } from "./pages/security";
import { SetupPage } from "./pages/setup";
import { WhyPage } from "./pages/why";
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
      <Route path="/cli" element={<CliPage />} />
      <Route path="/day-to-day" element={<DayToDayPage />} />
      <Route path="/examples" element={<ExamplesPage />} />
      <Route path="/install" element={<InstallPage />} />
      <Route path="/internals" element={<InternalsPage />} />
      <Route path="/lab" element={<LabPage />} />
      <Route path="/how" element={<HowPage />} />
      <Route path="/security" element={<SecurityPage />} />
      <Route path="/setup" element={<SetupPage />} />
      <Route path="/why" element={<WhyPage />} />
      <Route path="/playbook" element={<Navigate to="/why" replace />} />
      <Route path="/collaboration" element={<Navigate to="/day-to-day" replace />} />
      <Route path="/working" element={<Navigate to="/day-to-day" replace />} />
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
