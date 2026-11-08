import React from "react";
import ReactDOM from "react-dom/client";
import {
  BrowserRouter,
  Navigate,
  Route,
  Routes,
  useLocation,
} from "react-router-dom";
import { CliPage } from "./pages/cli";
import { DeveloperPage } from "./pages/developer";
import { ExamplesPage } from "./pages/examples";
import { ForgePage } from "./pages/forge";
import { HomePage } from "./pages/home";
import { HowPage } from "./pages/how";
import { InstallPage } from "./pages/install";
import { InternalsPage } from "./pages/internals";
import { LabPage } from "./pages/lab";
import { OperatorPage } from "./pages/operator";
import { SecurityPage } from "./pages/security";
import { SetupPage } from "./pages/setup";
import { WhyPage } from "./pages/why";
import "./index.css";

const basename = (() => {
  const base = import.meta.env.BASE_URL;
  if (!base || base === "/") return undefined;
  return base.replace(/\/$/, "");
})();

/** Redirects an old page to its new path, keeping `#section` links working. */
function Moved({ to }: { to: string }) {
  const { hash } = useLocation();
  return <Navigate to={`${to}${hash}`} replace />;
}

function App() {
  return (
    <Routes>
      <Route path="/" element={<HomePage />} />
      <Route path="/cli" element={<CliPage />} />
      <Route path="/developer" element={<DeveloperPage />} />
      <Route path="/examples" element={<ExamplesPage />} />
      <Route path="/forge" element={<ForgePage />} />
      <Route path="/install" element={<InstallPage />} />
      <Route path="/internals" element={<InternalsPage />} />
      <Route path="/lab" element={<LabPage />} />
      <Route path="/operator" element={<OperatorPage />} />
      <Route path="/how" element={<HowPage />} />
      <Route path="/security" element={<SecurityPage />} />
      <Route path="/setup" element={<SetupPage />} />
      <Route path="/why" element={<WhyPage />} />
      <Route path="/playbook" element={<Navigate to="/why" replace />} />
      <Route path="/day-to-day" element={<Moved to="/developer" />} />
      <Route path="/collaboration" element={<Moved to="/developer" />} />
      <Route path="/working" element={<Moved to="/developer" />} />
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
    <BrowserRouter basename={basename}>
      <App />
    </BrowserRouter>
  </React.StrictMode>,
);
