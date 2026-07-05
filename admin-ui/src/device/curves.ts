import type { DeviceClient } from './client';

export type CurveSampleParams = {
  id?: string;
  date?: string;
  samplesPerHour?: number;
  startHour?: number;
  maxSteps?: number;
};

function curveQuery(params: CurveSampleParams): Record<string, string> {
  const query: Record<string, string> = {};
  if (params.id) query.id = params.id;
  if (params.date) query.date = params.date;
  if (params.samplesPerHour !== undefined) {
    query.samples_per_hour = String(params.samplesPerHour);
  }
  if (params.startHour !== undefined) {
    query.start_hour = String(params.startHour);
  }
  if (params.maxSteps !== undefined) {
    query.max_steps = String(params.maxSteps);
  }
  return query;
}

export function getProfiles(client: DeviceClient) {
  return client.get<Record<string, unknown>>('api/profiles');
}

export function getConfig(client: DeviceClient, id?: string) {
  return client.get<Record<string, unknown>>('api/config', {
    query: id ? { id } : undefined
  });
}

export function putConfig(
  client: DeviceClient,
  config: Record<string, unknown>,
  options: { id?: string; apply?: boolean } = {}
) {
  const query: Record<string, string> = {};
  if (options.id) query.id = options.id;
  if (options.apply) query.apply = 'true';
  return client.put('api/config', { query, body: config });
}

export function getCurve(client: DeviceClient, params: CurveSampleParams = {}) {
  return client.get<Record<string, unknown>>('api/curve', {
    query: curveQuery(params)
  });
}

/** Sample a candidate (possibly unsaved) config. Curve math is device-side
    Rust; the editor must always render these samples, never local math. */
export function sampleCurve(
  client: DeviceClient,
  overrideConfig: Record<string, unknown>,
  params: CurveSampleParams = {}
) {
  return client.post<Record<string, unknown>>('api/curve', {
    query: curveQuery(params),
    body: overrideConfig
  });
}

export function getCurveNow(
  client: DeviceClient,
  params: { id?: string; hour?: number } = {}
) {
  const query: Record<string, string> = {};
  if (params.id) query.id = params.id;
  if (params.hour !== undefined) query.hour = String(params.hour);
  return client.get<Record<string, unknown>>('api/curve/now', { query });
}

export function getSolar(client: DeviceClient, date?: string) {
  return client.get<Record<string, unknown>>('api/curve/solar', {
    query: date ? { date } : undefined
  });
}

export function absorbOffset(
  client: DeviceClient,
  offsetMinutes: number,
  id?: string
) {
  return client.post('api/config/absorb-offset', {
    query: id ? { id } : undefined,
    body: { offset_minutes: offsetMinutes }
  });
}

export function resetConfig(client: DeviceClient, id?: string) {
  return client.post('api/config/reset', {
    query: id ? { id } : undefined
  });
}
