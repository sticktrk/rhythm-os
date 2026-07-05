import { createContext, useContext, useMemo, type ReactNode } from 'react';
import type { Session } from '@supabase/supabase-js';

export type SessionContextValue = {
  session: Session;
  accessToken: string;
  signOut: () => void;
};

const SessionContext = createContext<SessionContextValue | null>(null);

export function SessionProvider({
  session,
  signOut,
  children
}: {
  session: Session;
  signOut: () => void;
  children: ReactNode;
}) {
  const value = useMemo(
    () => ({ session, accessToken: session.access_token, signOut }),
    [session, signOut]
  );
  return (
    <SessionContext.Provider value={value}>{children}</SessionContext.Provider>
  );
}

export function useSession(): SessionContextValue {
  const value = useContext(SessionContext);
  if (!value) {
    throw new Error('useSession must be used within SessionProvider');
  }
  return value;
}
