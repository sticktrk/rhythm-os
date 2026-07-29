import type { DeviceClient } from './client';

export type RgbColor = { r: number; g: number; b: number };

export function sendNodeAction(
  client: DeviceClient,
  nodeId: string,
  action: string
) {
  return client.put('api/nodes/action', {
    body: { node_id: nodeId, action }
  });
}

export function sendBatchNodeAction(
  client: DeviceClient,
  nodeIds: string[],
  action: string,
  dispatchSpacingMs?: number
) {
  return client.put('api/nodes/action', {
    body: {
      nodes: nodeIds.map((node_id) => ({ node_id, action })),
      ...(dispatchSpacingMs !== undefined
        ? { dispatch_spacing_ms: dispatchSpacingMs }
        : {})
    }
  });
}

export function setNodeBrightness(
  client: DeviceClient,
  nodeId: string,
  brightness: number
) {
  return client.put('api/nodes/brightness', {
    body: { node_id: nodeId, brightness: Math.round(brightness) }
  });
}

export function setNodeColor(
  client: DeviceClient,
  nodeId: string,
  options: {
    rgb: RgbColor;
    brightness?: number;
    transitionMs?: number;
    scope?: 'preview' | 'mood' | 'auto';
  }
) {
  return client.put('api/nodes/color', {
    body: {
      node_id: nodeId,
      rgb: options.rgb,
      ...(options.brightness !== undefined
        ? { brightness: Math.round(options.brightness) }
        : {}),
      ...(options.transitionMs !== undefined
        ? { transition_ms: options.transitionMs }
        : {}),
      ...(options.scope ? { scope: options.scope } : {})
    }
  });
}

export function setNodeCurveBrightness(
  client: DeviceClient,
  nodeId: string,
  brightness: number
) {
  return client.put('api/nodes/curve', {
    body: { node_id: nodeId, brightness: Math.round(brightness) }
  });
}

export function setNodeCurveColorTemperature(
  client: DeviceClient,
  nodeId: string,
  kelvin: number,
  preserveBrightness = true
) {
  return client.put('api/nodes/curve', {
    body: {
      node_id: nodeId,
      color_temperature: Math.round(kelvin),
      preserve_brightness: preserveBrightness
    }
  });
}

export function setNodesOffset(
  client: DeviceClient,
  timeOffset: number,
  nodeIds?: string[]
) {
  return client.put('api/nodes/offset', {
    body: {
      time_offset: timeOffset,
      ...(nodeIds && nodeIds.length > 0 ? { nodes: nodeIds } : {})
    }
  });
}

export function setNodePreferences(
  client: DeviceClient,
  body: Record<string, unknown>
) {
  return client.put('api/nodes/preferences', { body });
}

export function setNodeProfileOverrides(
  client: DeviceClient,
  nodeId: string,
  profileOverrides: Record<string, unknown> | null,
  options: {
    replace?: boolean;
    correlationId?: string;
    resourcePrecondition?: {
      path: string;
      query?: Record<string, string>;
      bodySha256: string;
    };
  } = {}
) {
  return client.putReceipt('api/nodes/profile-overrides', {
    body: {
      node_id: nodeId,
      profile_overrides: profileOverrides,
      ...(options.replace ? { replace: true } : {}),
      ...(options.correlationId
        ? { correlation_id: options.correlationId }
        : {})
    },
    ...(options.correlationId ? { requestId: options.correlationId } : {}),
    ...(options.resourcePrecondition
      ? { resourcePrecondition: options.resourcePrecondition }
      : {})
  });
}
