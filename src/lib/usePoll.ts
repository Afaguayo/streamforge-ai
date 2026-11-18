import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "./api";

/** Calls `fn` now and every `ms`; returns the latest data, error and a manual refresh. Never overlaps calls; a refresh during a call re-runs after it. */
export function usePoll<T>(fn: () => Promise<T>, ms = 2000) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const again = useRef(false);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    // A refresh asked for mid-call (e.g. right after a user action) runs once that call ends,
    // so it never returns data fetched before the action.
    if (inFlight.current) { again.current = true; return; }
    inFlight.current = true;
    try {
      const r = await fn();
      if (alive.current) {
        setData(r);
        setError(null);
      }
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      inFlight.current = false;
    }
    if (again.current && alive.current) { again.current = false; void refresh(); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    alive.current = true;
    refresh();
    const t = setInterval(refresh, ms);
    return () => {
      alive.current = false;
      clearInterval(t);
    };
  }, [refresh, ms]);

  return { data, error, refresh };
}
