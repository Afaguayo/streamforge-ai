import { useCallback, useEffect, useState } from "react";
import { errorText } from "./api";

/** Calls `fn` now and every `ms`; returns the latest data, error and a manual refresh. */
export function usePoll<T>(fn: () => Promise<T>, ms = 2000) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setData(await fn());
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, ms);
    return () => clearInterval(t);
  }, [refresh, ms]);

  return { data, error, refresh };
}
