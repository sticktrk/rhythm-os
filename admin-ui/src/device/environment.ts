import type { DeviceClient } from './client';

export function setLocation(
  client: DeviceClient,
  location: {
    lat: number;
    lon: number;
    utcOffset?: number;
    timezoneName?: string;
  }
) {
  return client.put('api/location', {
    body: {
      lat: location.lat,
      lon: location.lon,
      ...(location.utcOffset !== undefined
        ? { utc_offset: location.utcOffset }
        : {}),
      ...(location.timezoneName
        ? { timezone_name: location.timezoneName }
        : {})
    }
  });
}
