import { useMemo } from 'react';

import { DeviceClient } from '../device/client';
import { useOptionalHub } from '../state/HubContext';
import { useOptionalSession } from '../state/SessionContext';
import { useLocalDevice } from '../state/LocalDeviceContext';

export function useDeviceClient(): DeviceClient {
  const local = useLocalDevice();
  const accessToken = useOptionalSession()?.accessToken;
  const hubId = useOptionalHub()?.hubId;
  return useMemo(
    () => {
      if (local) return local;
      if (!accessToken || !hubId) throw new Error('A device session is required.');
      return new DeviceClient(accessToken, hubId);
    },
    [local, accessToken, hubId]
  );
}
