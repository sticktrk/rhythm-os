-- Run inside a disposable PostgreSQL cluster. The platform normally provides
-- Auth and Storage. Load Rhythm's real schema for all tables under review.
CREATE ROLE authenticated;
CREATE ROLE anon;
CREATE ROLE service_role BYPASSRLS;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO service_role;
CREATE SCHEMA auth;
CREATE TABLE auth.users (id uuid PRIMARY KEY, raw_user_meta_data jsonb NOT NULL DEFAULT '{}');
CREATE FUNCTION auth.uid() RETURNS uuid LANGUAGE sql AS $$ SELECT nullif(current_setting('request.jwt.claim.sub',true),'')::uuid $$;
CREATE FUNCTION public.is_rhythm_staff() RETURNS boolean LANGUAGE sql AS $$ SELECT false $$;
CREATE PUBLICATION supabase_realtime;
CREATE SCHEMA storage;
CREATE TABLE storage.buckets (id text PRIMARY KEY, name text, public boolean, file_size_limit bigint, allowed_mime_types text[]);
CREATE TABLE storage.objects (id uuid, name text, bucket_id text);
CREATE FUNCTION storage.foldername(text) RETURNS text[] LANGUAGE sql AS $$ SELECT string_to_array($1, '/') $$;
\ir ../app/supabase/migrations/20250129000000_initial_schema.sql
\ir ../app/supabase/migrations/20260314000000_add_server_hub_type.sql
\ir ../app/supabase/migrations/20260423000000_add_support_debug_bundle_submissions.sql
\ir ../app/supabase/migrations/20260428000000_add_debug_bundle_github_issue_metadata.sql
\ir ../app/supabase/migrations/20260429000000_allow_text_only_bug_reports.sql
\ir ../app/supabase/migrations/20260603000000_add_remote_access.sql
\ir ../app/supabase/migrations/20260604000000_add_remote_access_server_instance_id.sql
\ir ../app/supabase/migrations/20260625000000_add_hub_server_instance_id.sql
\ir ../app/supabase/migrations/20260629000000_add_hub_support_access_grants.sql
\ir ../app/supabase/migrations/20260709000000_add_remote_access_desired_state.sql
\ir ../app/supabase/migrations/20260808000000_add_support_report_kind.sql
\ir ../app/supabase/migrations/20260809010000_add_async_support_bundle_collection.sql
\ir ../app/supabase/migrations/20260809040000_allow_pending_support_bundle_size.sql
\ir ../app/supabase/migrations/20261119000000_protect_assistant_relay_routes.sql
\ir ../app/supabase/migrations/20261119010000_customer_assistant_reviews.sql
\ir ../app/supabase/tests/customer_assistant_reviews.sql
\ir ../app/supabase/migrations/20261122000000_lighting_chat_usage.sql
\ir ../app/supabase/tests/lighting_chat_usage.sql
\ir ../app/supabase/migrations/20261126000000_add_push_devices.sql
\ir ../app/supabase/tests/push_devices.sql
