import { useMemo } from 'react';

import { DeviceClient } from '../device/client';
import { useHub } from '../state/HubContext';
import { useSession } from '../state/SessionContext';

export function useDeviceClient(): DeviceClient {
  const { accessToken } = useSession();
  const { hubId } = useHub();
  return useMemo(
    () => new DeviceClient(accessToken, hubId),
    [accessToken, hubId]
  );
}
