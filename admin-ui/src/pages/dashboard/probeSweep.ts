/** A directory load or a return to the tab refreshes status once. No idle timer. */
export function watchVisibleProbeSweep(
  visibility: Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'>,
  start: () => void,
  stop: () => void
): () => void {
  const update = () => {
    if (visibility.visibilityState === 'visible') start();
    else stop();
  };
  visibility.addEventListener('visibilitychange', update);
  update();
  return () => {
    visibility.removeEventListener('visibilitychange', update);
    stop();
  };
}

export async function probeTargets<T>(
  targets: T[],
  probe: (target: T) => Promise<void>,
  isCurrent: () => boolean,
  concurrency: number
): Promise<void> {
  let next = 0;
  const worker = async () => {
    while (isCurrent() && next < targets.length) {
      await probe(targets[next++]);
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, targets.length) }, worker));
}
