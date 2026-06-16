-- Subscriptions: source-of-truth for a user's plan tier.
-- Pricing/billing integration is out of scope; rows are inserted manually
-- (admin / service role) for now. Clients have read-only access.

DO $$ BEGIN
  CREATE TYPE subscription_tier AS ENUM ('basic', 'pro');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

DO $$ BEGIN
  CREATE TYPE subscription_status AS ENUM ('active', 'expired', 'canceled');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

CREATE TABLE IF NOT EXISTS subscriptions (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
  tier subscription_tier NOT NULL,
  status subscription_status NOT NULL DEFAULT 'active',
  started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  ended_at TIMESTAMPTZ,
  source TEXT, -- 'manual', 'stripe', 'revenuecat', etc.
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS subscriptions_user_active_idx
  ON subscriptions (user_id) WHERE status = 'active';

CREATE INDEX IF NOT EXISTS subscriptions_user_started_at_idx
  ON subscriptions (user_id, started_at DESC);

ALTER TABLE subscriptions ENABLE ROW LEVEL SECURITY;

-- Read-only for owner. Writes are reserved for the service role; no INSERT/
-- UPDATE/DELETE policies exist for authenticated users on purpose.
CREATE POLICY "Users read own subscriptions"
  ON subscriptions FOR SELECT
  USING (auth.uid() = user_id);

DROP TRIGGER IF EXISTS subscriptions_updated_at ON subscriptions;
CREATE TRIGGER subscriptions_updated_at
  BEFORE UPDATE ON subscriptions
  FOR EACH ROW
  EXECUTE FUNCTION update_updated_at_column();

ALTER PUBLICATION supabase_realtime ADD TABLE subscriptions;

COMMENT ON TABLE subscriptions IS
  'Per-user subscription rows. Effective tier is the most recent active row.';
COMMENT ON COLUMN subscriptions.tier IS
  'basic | pro. basic is displayed as Free; Pro grants the "standby" entitlement.';
COMMENT ON COLUMN subscriptions.status IS
  'active | expired | canceled. Only active rows count toward effective tier.';
