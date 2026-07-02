import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => {
  const env = {
    ...readEnvFile(resolve(__dirname, '../app/flutter/rhythm_app/.env')),
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

function readEnvFile(path: string): Record<string, string> {
  if (!existsSync(path)) return {};
  return Object.fromEntries(
    readFileSync(path, 'utf8')
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line.length > 0 && !line.startsWith('#'))
      .map((line) =>
        line.startsWith('export ') ? line.slice('export '.length).trim() : line
      )
      .map((line) => {
        const equals = line.indexOf('=');
        if (equals <= 0) return null;
        const key = line.slice(0, equals).trim();
        const value = unquote(line.slice(equals + 1).trim());
        return key.length > 0 ? [key, value] : null;
      })
      .filter((entry): entry is [string, string] => entry !== null)
  );
}

function unquote(value: string): string {
  if (
    (value.startsWith('"') && value.endsWith('"')) ||
    (value.startsWith("'") && value.endsWith("'"))
  ) {
    return value.slice(1, -1);
  }
  return value;
}

function pickBrowserEnv(values: Record<string, string | undefined>) {
  return Object.fromEntries(
    Object.entries(values).filter(
      (entry): entry is [string, string] =>
        typeof entry[1] === 'string' && entry[1].trim().length > 0
    )
  );
}
