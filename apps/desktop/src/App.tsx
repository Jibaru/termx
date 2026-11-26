import { useEffect, useSyncExternalStore } from "react";

import { MainPane } from "./components/MainPane";
import { Sidebar } from "./components/Sidebar";
import { type SessionStore, sessionStore } from "./terminal/sessionStore";

interface AppProps {
  /** The terminals of the window. Tests pass their own. */
  store?: SessionStore;
}

/** Top-level layout: the session sidebar on the left and the main region. */
export function App({ store = sessionStore }: AppProps) {
  const { activeKey } = useSyncExternalStore(store.subscribe, store.getSnapshot);

  // A window always has a terminal: the first one opens with it.
  useEffect(() => {
    store.ensureOne();
  }, [store]);

  return (
    <div className="app-shell">
      <Sidebar />
      <MainPane controller={activeKey === null ? undefined : store.get(activeKey)} />
    </div>
  );
}
