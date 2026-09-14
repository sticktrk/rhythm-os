import { createContext, useContext } from 'react';
import type { DeviceClient } from '../device/client';

export const LocalDeviceContext = createContext<DeviceClient | null>(null);
export function useLocalDevice() { return useContext(LocalDeviceContext); }
