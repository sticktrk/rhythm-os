import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode
} from 'react';

import {
  fetchHealth,
  fetchMe,
  fetchReadiness,
  fetchSupportSnapshot
} from '../api';
import { errorMessage } from '../lib/format';
import type {
  AdminApiHealth,
  AdminApiReadiness,
  MeResponse,
  SupportCustomer,
  SupportHome,
  SupportHub,
  SupportSnapshot
} from '../types';
import { useSession } from './SessionContext';

export type HubLookup = {
  hub: SupportHub;
  home: SupportHome;
  customer: SupportCustomer;
};

export type SnapshotContextValue = {
  snapshot: SupportSnapshot | null;
  me: MeResponse | null;
  health: AdminApiHealth | null;
  readiness: AdminApiReadiness | null;
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  findHub: (hubId: string) => HubLookup | null;
};

const SnapshotContext = createContext<SnapshotContextValue | null>(null);

export function SnapshotProvider({ children }: { children: ReactNode }) {
  const { accessToken } = useSession();
  const [snapshot, setSnapshot] = useState<SupportSnapshot | null>(null);
  const [me, setMe] = useState<MeResponse | null>(null);
  const [health, setHealth] = useState<AdminApiHealth | null>(null);
  const [readiness, setReadiness] = useState<AdminApiReadiness | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [nextHealth, nextReadiness, nextMe, nextSnapshot] =
        await Promise.all([
          fetchHealth().catch(() => null),
          fetchReadiness().catch(() => null),
          fetchMe(accessToken),
          fetchSupportSnapshot(accessToken)
        ]);
      setHealth(nextHealth);
      setReadiness(nextReadiness);
      setMe(nextMe);
      setSnapshot(nextSnapshot);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [accessToken]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const findHub = useCallback(
    (hubId: string): HubLookup | null => {
      if (!snapshot) return null;
      for (const customer of snapshot.customers) {
        for (const entry of customer.homes) {
          const hub = entry.hubs.find((candidate) => candidate.id === hubId);
          if (hub) return { hub, home: entry.home, customer };
        }
      }
      return null;
    },
    [snapshot]
  );

  const value = useMemo(
    () => ({
      snapshot,
      me,
      health,
      readiness,
      loading,
      error,
      refresh,
      findHub
    }),
    [snapshot, me, health, readiness, loading, error, refresh, findHub]
  );

  return (
    <SnapshotContext.Provider value={value}>
      {children}
    </SnapshotContext.Provider>
  );
}

export function useSnapshot(): SnapshotContextValue {
  const value = useContext(SnapshotContext);
  if (!value) {
    throw new Error('useSnapshot must be used within SnapshotProvider');
  }
  return value;
}
