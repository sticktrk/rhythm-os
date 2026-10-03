import assert from 'node:assert/strict';
import test from 'node:test';

import { probeTargets, watchVisibleProbeSweep } from '../src/pages/dashboard/probeSweep.ts';

test('hidden dashboard waits until visible, stops on hide, and unsubscribes on exit', () => {
  const surface = new EventTarget() as EventTarget & { visibilityState: DocumentVisibilityState };
  surface.visibilityState = 'hidden';
  let starts = 0;
  let stops = 0;
  const dispose = watchVisibleProbeSweep(surface, () => starts++, () => stops++);
  assert.equal(starts, 0);
  surface.visibilityState = 'visible';
  surface.dispatchEvent(new Event('visibilitychange'));
  assert.equal(starts, 1);
  surface.visibilityState = 'hidden';
  surface.dispatchEvent(new Event('visibilitychange'));
  assert.equal(stops, 2);
  dispose();
  surface.visibilityState = 'visible';
  surface.dispatchEvent(new Event('visibilitychange'));
  assert.equal(starts, 1);
});

test('hiding a busy dashboard stops queued requests and keeps bounded concurrency', async () => {
  let visible = true;
  const requested: number[] = [];
  const releases: (() => void)[] = [];
  const sweep = probeTargets([1, 2, 3, 4, 5], async (target) => {
    requested.push(target);
    await new Promise<void>((resolve) => releases.push(resolve));
  }, () => visible, 3);
  assert.deepEqual(requested, [1, 2, 3]);
  visible = false;
  releases.forEach((release) => release());
  await sweep;
  assert.deepEqual(requested, [1, 2, 3]);

  visible = true;
  await probeTargets([4, 5], async (target) => { requested.push(target); }, () => visible, 3);
  assert.deepEqual(requested, [1, 2, 3, 4, 5]);
});

test('directory replacement has one sweep owner and invalidates the previous owner', async () => {
  const surface = new EventTarget() as EventTarget & { visibilityState: DocumentVisibilityState };
  surface.visibilityState = 'visible';
  let generation = 0;
  let starts = 0;
  const requests: number[] = [];
  const releases: (() => void)[] = [];
  const pending: Promise<void>[] = [];
  const watchDirectory = (targets: number[]) => watchVisibleProbeSweep(surface, () => {
    starts++;
    const owner = ++generation;
    pending.push(probeTargets(targets, async (target) => {
      requests.push(target);
      await new Promise<void>((resolve) => releases.push(resolve));
    }, () => generation === owner, 1));
  }, () => { generation++; });

  let dispose = watchDirectory([1, 2]);
  assert.equal(starts, 1);
  // The directory dependency owns the sweep after refresh resolves. It first
  // disposes the old owner; a refresh handler does not launch its own sweep.
  dispose();
  dispose = watchDirectory([3, 4]);
  assert.equal(starts, 2);
  releases.splice(0).forEach((release) => release());
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(requests, [1, 3, 4]);
  dispose();
  releases.splice(0).forEach((release) => release());
  await Promise.all(pending);
});
