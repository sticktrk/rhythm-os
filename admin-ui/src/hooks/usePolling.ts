import { useCallback, useEffect, useRef, useState } from 'react';

import { errorMessage } from '../lib/format';

export type PollingState<T> = {
  data: T | null;
  error: string | null;
  /** True until the first fetch settles. */
  loading: boolean;
  /** True while any fetch is in flight. */
  refreshing: boolean;
  refresh: () => Promise<void>;
};

/** Poll a fetcher. Single-in-flight (ticks are skipped while a request is
    pending) and paused while the tab is hidden — every poll is a full
    admin-api → device round trip. `intervalMs: 0` disables polling
    (fetch on mount only). */
export function usePolling<T>(
  fetcher: () => Promise<T>,
  {
    intervalMs = 0,
    enabled = true
  }: { intervalMs?: number; enabled?: boolean } = {}
): PollingState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const inFlight = useRef(false);
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;

  const run = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setRefreshing(true);
    try {
      const result = await fetcherRef.current();
      setData(result);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      inFlight.current = false;
      setRefreshing(false);
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    void run();
    if (intervalMs <= 0) return;

    const timer = window.setInterval(() => {
      if (document.hidden) return;
      void run();
    }, intervalMs);
    return () => window.clearInterval(timer);
  }, [enabled, intervalMs, run]);

  return { data, error, loading, refreshing, refresh: run };
}
