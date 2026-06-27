# Rhythm Admin

Employee-only customer support portal for Rhythm.

This is intentionally a standalone Flutter app under `admin/`, separate from
the consumer app in `app/flutter/rhythm_app`.

## Run

```sh
cd admin
flutter pub get
flutter run -d chrome --dart-define-from-file=../app/flutter/rhythm_app/.env
```

Sign in with a Supabase email/password account that has an enabled row in
`public.rhythm_staff`.

## Build

```sh
cd admin
flutter build web --dart-define-from-file=../app/flutter/rhythm_app/.env
```
