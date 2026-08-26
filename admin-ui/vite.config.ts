import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => {
  const env = {
    ...loadEnv(mode, __dirname, ''),
    ...process.env
  };
  const browserEnv = pickBrowserEnv({
    VITE_SUPABASE_URL: env.VITE_SUPABASE_URL ?? env.SUPABASE_URL,
    VITE_SUPABASE_ANON_KEY:
      env.VITE_SUPABASE_ANON_KEY ?? env.SUPABASE_ANON_KEY,
    VITE_ADMIN_API_URL: env.VITE_ADMIN_API_URL
  });

  return {
    plugins: [react()],
    define: Object.fromEntries(
      Object.entries(browserEnv).map(([key, value]) => [
        `import.meta.env.${key}`,
        JSON.stringify(value)
      ])
    ),
    server: {
      port: 5173,
      strictPort: false
    }
  };
});

function pickBrowserEnv(values: Record<string, string | undefined>) {
  return Object.fromEntries(
    Object.entries(values).filter(
      (entry): entry is [string, string] =>
        typeof entry[1] === 'string' && entry[1].trim().length > 0
    )
  );
}
