import { useCallback, useState } from 'react';

import { errorMessage } from '../lib/format';

export type DeviceCall<A extends unknown[], R> = {
  run: (...args: A) => Promise<R | undefined>;
  busy: boolean;
  error: string | null;
  reset: () => void;
};

/** Standard wrapper for write buttons: tracks busy/error, swallows the
    rejection (the error surfaces via `error`), returns undefined on failure. */
export function useDeviceCall<A extends unknown[], R>(
  fn: (...args: A) => Promise<R>
): DeviceCall<A, R> {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(
    async (...args: A): Promise<R | undefined> => {
      setBusy(true);
      setError(null);
      try {
        return await fn(...args);
      } catch (err) {
        setError(errorMessage(err));
        return undefined;
      } finally {
        setBusy(false);
      }
    },
    [fn]
  );

  const reset = useCallback(() => setError(null), []);

  return { run, busy, error, reset };
}
