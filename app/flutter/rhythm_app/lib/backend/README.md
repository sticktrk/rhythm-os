# Backend Abstraction Layer

This directory contains the backend abstraction layer for Rhythm Lighting, enabling easy swapping between different backend providers.

## Architecture

```
backend/
├── backend.dart              # Barrel file (import this)
├── backend_config.dart       # Configuration types
├── backend_provider.dart     # Singleton factory
├── auth/
│   ├── auth_backend.dart     # Abstract interface
│   ├── auth_user.dart        # Universal user model
│   └── supabase_auth_backend.dart
├── database/
│   ├── remote_data_backend.dart   # Abstract interface
│   └── supabase_data_backend.dart
├── realtime/
│   ├── realtime_backend.dart      # Abstract interface
│   └── supabase_realtime_backend.dart
└── analytics/
    ├── analytics_backend.dart     # Abstract interface
    └── console_analytics_backend.dart
```

## Usage

### Initialization

Initialize the backend provider at app startup (before using any services):

```dart
import 'package:rhythm_app/backend/backend.dart';

void main() async {
  await BackendProvider.initialize(BackendConfig.supabase(
    url: 'https://your-project.supabase.co',
    anonKey: 'your-anon-key',
    enableLogging: true,
  ));

  runApp(MyApp());
}
```

### Accessing Backends

After initialization, access backends through the singleton:

```dart
// Auth
final user = BackendProvider.instance.auth.currentUser;
await BackendProvider.instance.auth.signInAnonymously();

// Data
final homes = await BackendProvider.instance.data.getHomesForUser(userId);
await BackendProvider.instance.data.createHome(home);

// Realtime
BackendProvider.instance.realtime.watchHomesForUser(userId).listen((homes) {
  // Handle updates
});

// Analytics
await BackendProvider.instance.analytics.logEvent('button_click');
```

## Backends

### Auth Backend
- `signInAnonymously()` - Guest sign-in
- `signInWithGoogle()` - Google OAuth (links to anonymous account if exists)
- `signInWithEmailPassword()` - Email/password sign-in
- `createAccountWithEmailPassword()` - Create account (links to anonymous if exists)
- `linkWithEmailPassword()` - Upgrade anonymous account
- `signOut()` - Sign out

### Data Backend
- `getHomesForUser()`, `createHome()`, `updateHome()`, `deleteHome()`
- `getHubsForHome()`, `createHub()`, `updateHub()`, `deleteHub()`
- `batchSync()` - Batch operations for offline-first sync

### Realtime Backend
- `watchHomesForUser()` - Stream of homes list
- `watchHome()` - Stream of single home
- `watchHubsForHome()` - Stream of hubs list
- `watchHub()` - Stream of single hub

### Analytics Backend
- `logEvent()` - Custom event
- `setUserProperty()` - User property
- `logScreenView()` - Screen view

## Supabase Setup

1. Create a Supabase project at https://supabase.com

2. Run the schema migration:
   ```bash
   # In Supabase SQL Editor, run:
   ls ../../../tools/app/supabase/migrations
   ```

3. Configure Google OAuth (optional):
   - In Supabase Dashboard → Authentication → Providers
   - Enable Google and add your OAuth credentials

4. Set environment variables:
   ```bash
   flutter run --dart-define=SUPABASE_URL=https://xxx.supabase.co \
               --dart-define=SUPABASE_ANON_KEY=your-key
   ```

## Adding a New Backend

To add support for a new backend:

1. Add new enum value to `BackendType` in `backend_config.dart`

2. Create implementations in each subdirectory:
   - `auth/new_auth_backend.dart`
   - `database/new_data_backend.dart`
   - `realtime/new_realtime_backend.dart`
   - `analytics/new_analytics_backend.dart`

3. Add initialization logic in `BackendProvider.initialize()`

4. Export from `backend.dart`
