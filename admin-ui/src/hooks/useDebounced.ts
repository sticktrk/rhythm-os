import { useEffect, useMemo, useRef } from 'react';

/** Returns a debounced wrapper of `fn`. The wrapper identity is stable;
    the latest `fn` is always invoked. Pending calls are dropped on unmount. */
export function useDebounced<A extends unknown[]>(
  fn: (...args: A) => void,
  delayMs: number
): (...args: A) => void {
  const fnRef = useRef(fn);
  fnRef.current = fn;
  const timerRef = useRef<number | undefined>(undefined);

  useEffect(() => {
    return () => {
      if (timerRef.current !== undefined) {
        window.clearTimeout(timerRef.current);
      }
    };
  }, []);

  return useMemo(
    () =>
      (...args: A) => {
        if (timerRef.current !== undefined) {
          window.clearTimeout(timerRef.current);
        }
        timerRef.current = window.setTimeout(() => {
          fnRef.current(...args);
        }, delayMs);
      },
    [delayMs]
  );
}
