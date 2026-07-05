import { createContext, useContext, type ReactNode } from 'react';

import type {
  ProbeResult,
  SupportCustomer,
  SupportHome,
  SupportHub
} from '../types';

export type HubContextValue = {
  hubId: string;
  hub: SupportHub;
  home: SupportHome;
  customer: SupportCustomer;
  probe: ProbeResult | null;
  probing: boolean;
  reprobe: () => void;
};

const HubContext = createContext<HubContextValue | null>(null);

export function HubProvider({
  value,
  children
}: {
  value: HubContextValue;
  children: ReactNode;
}) {
  return <HubContext.Provider value={value}>{children}</HubContext.Provider>;
}

export function useHub(): HubContextValue {
  const value = useContext(HubContext);
  if (!value) {
    throw new Error('useHub must be used within HubProvider');
  }
  return value;
}
