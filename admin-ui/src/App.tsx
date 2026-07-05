import { useEffect, useState, type FormEvent } from 'react';
import { BrowserRouter } from 'react-router-dom';
import type { Session } from '@supabase/supabase-js';
import { AlertTriangle, Loader2, ShieldCheck } from 'lucide-react';

import { AppRoutes } from './router';
import { isSupabaseConfigured, supabase } from './supabaseClient';
import { SessionProvider } from './state/SessionContext';
import { SnapshotProvider } from './state/SnapshotContext';

export default function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [authBusy, setAuthBusy] = useState(true);
  const [authError, setAuthError] = useState<string | null>(null);

  useEffect(() => {
    if (!supabase) {
      setAuthBusy(false);
      return;
    }

    let active = true;
    supabase.auth.getSession().then(({ data }) => {
      if (!active) return;
      setSession(data.session);
      setAuthBusy(false);
    });
    const {
      data: { subscription }
    } = supabase.auth.onAuthStateChange((_event, nextSession) => {
      setSession(nextSession);
      setAuthError(null);
    });

    return () => {
      active = false;
      subscription.unsubscribe();
    };
  }, []);

  async function handleSignIn(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!supabase) return;
    setAuthBusy(true);
    setAuthError(null);
    const { error } = await supabase.auth.signInWithPassword({
      email: email.trim(),
      password
    });
    if (error) setAuthError(error.message);
    setPassword('');
    setAuthBusy(false);
  }

  function handleSignOut() {
    if (!supabase) return;
    void supabase.auth.signOut();
  }

  if (!isSupabaseConfigured) {
    return <SetupScreen />;
  }

  if (authBusy && !session) {
    return <LoadingScreen label="Checking session" />;
  }

  if (!session) {
    return (
      <AuthScreen
        email={email}
        password={password}
        busy={authBusy}
        error={authError}
        onEmailChange={setEmail}
        onPasswordChange={setPassword}
        onSubmit={handleSignIn}
      />
    );
  }

  return (
    <SessionProvider session={session} signOut={handleSignOut}>
      {/* Keyed by user so switching accounts discards all cached dashboard
          and per-hub state instead of showing the previous user's data. */}
      <SnapshotProvider key={session.user.id}>
        <BrowserRouter>
          <AppRoutes />
        </BrowserRouter>
      </SnapshotProvider>
    </SessionProvider>
  );
}

function SetupScreen() {
  return (
    <main className="centeredScreen">
      <section className="authPanel">
        <div className="panelIcon warning">
          <AlertTriangle size={24} />
        </div>
        <h1>Admin UI is not configured</h1>
        <p>
          Set <code>VITE_SUPABASE_URL</code>, <code>VITE_SUPABASE_ANON_KEY</code>,
          and <code>VITE_ADMIN_API_URL</code> in <code>admin-ui/.env</code>.
        </p>
      </section>
    </main>
  );
}

function LoadingScreen({ label }: { label: string }) {
  return (
    <main className="centeredScreen">
      <div className="loadingBlock">
        <Loader2 className="spin" size={22} />
        <span>{label}</span>
      </div>
    </main>
  );
}

function AuthScreen({
  email,
  password,
  busy,
  error,
  onEmailChange,
  onPasswordChange,
  onSubmit
}: {
  email: string;
  password: string;
  busy: boolean;
  error: string | null;
  onEmailChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  return (
    <main className="centeredScreen">
      <form className="authPanel" onSubmit={onSubmit}>
        <div className="panelIcon">
          <ShieldCheck size={24} />
        </div>
        <h1>Rhythm Admin</h1>
        <label>
          <span>Email</span>
          <input
            type="email"
            value={email}
            onChange={(event) => onEmailChange(event.target.value)}
            autoComplete="email"
            required
          />
        </label>
        <label>
          <span>Password</span>
          <input
            type="password"
            value={password}
            onChange={(event) => onPasswordChange(event.target.value)}
            autoComplete="current-password"
            required
          />
        </label>
        {error ? <div className="notice error">{error}</div> : null}
        <button className="primaryButton" type="submit" disabled={busy}>
          {busy ? <Loader2 className="spin" size={18} /> : <ShieldCheck size={18} />}
          <span>Sign in</span>
        </button>
      </form>
    </main>
  );
}
