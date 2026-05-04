import { useEffect, useState } from "react";
import { SESSIONS_ENDPOINT, sessionEndpoint } from "./api";

export type DefaultActivityState =
  | { status: "loading" }
  | { status: "ready"; activityId: string }
  | { status: "error"; message: string };

export function useDefaultActivity(): DefaultActivityState {
  const [state, setState] = useState<DefaultActivityState>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await fetch(SESSIONS_ENDPOINT).then((r) => r.json());
        const sessionId = list.sessions?.[0]?.id;
        if (!sessionId) throw new Error("no default session");
        const session = await fetch(sessionEndpoint(sessionId)).then((r) => r.json());
        const activityId = session.panes?.[0]?.activities?.[0]?.id;
        if (!activityId) throw new Error("no default activity");
        if (!cancelled) setState({ status: "ready", activityId });
      } catch (e) {
        if (!cancelled) {
          const message = e instanceof Error ? e.message : String(e);
          setState({ status: "error", message });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  return state;
}
