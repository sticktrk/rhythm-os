import type { DeviceClient } from './client';

export function listScenes(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/scenes');
}

export function createScene(
  client: DeviceClient,
  scene: Record<string, unknown>
) {
  return client.post('api/scenes', { body: scene });
}

export function updateScene(
  client: DeviceClient,
  sceneId: string,
  scene: Record<string, unknown>
) {
  return client.put(`api/scenes/${encodeURIComponent(sceneId)}`, {
    body: scene
  });
}

export function deleteScene(client: DeviceClient, sceneId: string) {
  return client.delete(`api/scenes/${encodeURIComponent(sceneId)}`);
}

export function applyScene(
  client: DeviceClient,
  sceneId: string,
  options: { targetId: string; transitionMs?: number }
) {
  return client.post(`api/scenes/${encodeURIComponent(sceneId)}/apply`, {
    body: {
      target_id: options.targetId,
      ...(options.transitionMs !== undefined
        ? { transition_ms: options.transitionMs }
        : {})
    }
  });
}

export function previewScene(
  client: DeviceClient,
  sceneId: string,
  options: { targetId: string; transitionMs?: number; durationMs?: number }
) {
  return client.post(`api/scenes/${encodeURIComponent(sceneId)}/preview`, {
    body: {
      target_id: options.targetId,
      ...(options.transitionMs !== undefined
        ? { transition_ms: options.transitionMs }
        : {}),
      ...(options.durationMs !== undefined
        ? { duration_ms: options.durationMs }
        : {})
    }
  });
}

export function previewDraftScene(
  client: DeviceClient,
  options: {
    scene: Record<string, unknown>;
    targetId: string;
    transitionMs?: number;
    durationMs?: number;
  }
) {
  return client.post('api/scenes/preview', {
    body: {
      scene: options.scene,
      target_id: options.targetId,
      ...(options.transitionMs !== undefined
        ? { transition_ms: options.transitionMs }
        : {}),
      ...(options.durationMs !== undefined
        ? { duration_ms: options.durationMs }
        : {})
    }
  });
}

export function commitScenePreview(client: DeviceClient, previewId: string) {
  return client.post(
    `api/scene-previews/${encodeURIComponent(previewId)}/commit`
  );
}

export function cancelScenePreview(client: DeviceClient, previewId: string) {
  return client.post(
    `api/scene-previews/${encodeURIComponent(previewId)}/cancel`
  );
}
