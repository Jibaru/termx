import { MainPane } from "./components/MainPane";
import { Sidebar } from "./components/Sidebar";

/** Top-level layout: the session sidebar on the left and the main region. */
export function App() {
  return (
    <div className="app-shell">
      <Sidebar />
      <MainPane />
    </div>
  );
}
