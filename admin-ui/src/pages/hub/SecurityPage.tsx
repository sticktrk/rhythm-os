import { useCallback, useState } from 'react';
import { Copy, KeyRound, RefreshCw, ShieldCheck } from 'lucide-react';

import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { FormRow, TextField } from '../../components/controls/fields';
import { ErrorNotice, KeyValueGrid } from '../../components/ui/bits';
import { SectionCard } from '../../components/ui/SectionCard';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import {
  claimOwner,
  createSupportToken,
  getAuthStatus,
  setAuthSettings
} from '../../device/auth';
import { asBoolean, asNumber, asRecord, asString, pick } from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';

function extractToken(payload: unknown): string | undefined {
  return (
    asString(asRecord(payload).token) ??
    asString(pick(payload, 'token', 'value')) ??
    asString(asRecord(payload).support_token) ??
    asString(asRecord(payload).access_token)
  );
}

export default function SecurityPage() {
  const client = useDeviceClient();
  const confirm = useConfirm();

  const statusQuery = usePolling(
    useCallback(() => getAuthStatus(client), [client])
  );
  const status = asRecord(statusQuery.data);
  const requiresAuth = asBoolean(status.requires_auth);

  const [claimLabel, setClaimLabel] = useState('support-claim');
  const [supportLabel, setSupportLabel] = useState('support-session');
  const [issuedToken, setIssuedToken] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const statusRefresh = statusQuery.refresh;

  const claimCall = useDeviceCall(
    useCallback(async () => {
      const result = await claimOwner(client, claimLabel.trim() || 'support-claim');
      setIssuedToken(extractToken(result) ?? null);
      setCopied(false);
      await statusRefresh();
      return result;
    }, [client, claimLabel, statusRefresh])
  );

  const supportCall = useDeviceCall(
    useCallback(async () => {
      const result = await createSupportToken(
        client,
        supportLabel.trim() || 'support-session'
      );
      setIssuedToken(extractToken(result) ?? null);
      setCopied(false);
      await statusRefresh();
      return result;
    }, [client, supportLabel, statusRefresh])
  );

  const authToggleCall = useDeviceCall(
    useCallback(
      async (next: boolean) => {
        await setAuthSettings(client, { requireApiAuth: next });
        await statusRefresh();
      },
      [client, statusRefresh]
    )
  );

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Security</h2>
          <p className="pageIntro">
            Device API authentication, owner claim and support tokens.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => void statusQuery.refresh()}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      {statusQuery.error && !statusQuery.data ? (
        <ErrorNotice message={statusQuery.error} />
      ) : null}

      <div className="cardGrid two">
        <SectionCard
          title="Auth status"
          busy={statusQuery.refreshing}
          rawPayload={statusQuery.data ?? undefined}
        >
          <KeyValueGrid
            rows={[
              ['Requires auth', requiresAuth],
              ['Owner configured', asBoolean(status.owner_configured)],
              ['Token count', asNumber(status.token_count)],
              ['Claim available', asBoolean(status.claim_available)]
            ]}
          />
          <div className="breakerRow">
            <ToggleSwitch
              checked={requiresAuth ?? false}
              disabled={requiresAuth === undefined}
              busy={authToggleCall.busy}
              label="Require API authentication"
              onChange={(next) => {
                void (async () => {
                  const confirmed = await confirm({
                    title: next ? 'Enable API auth' : 'Disable API auth',
                    message: next
                      ? 'Require a bearer token for every device API call? Clients without tokens lose access immediately.'
                      : 'Disable API authentication? Anyone on the LAN can then control this device.',
                    confirmLabel: next ? 'Require auth' : 'Disable auth',
                    danger: !next
                  });
                  if (confirmed) await authToggleCall.run(next);
                })();
              }}
            />
          </div>
          {authToggleCall.error ? (
            <ErrorNotice message={authToggleCall.error} />
          ) : null}
        </SectionCard>

        <SectionCard
          title="Issue tokens"
          subtitle="Tokens are shown once — copy immediately"
          busy={claimCall.busy || supportCall.busy}
          error={claimCall.error ?? supportCall.error}
        >
          <FormRow label="Owner claim" hint="Only works while claim is available">
            <TextField value={claimLabel} onChange={setClaimLabel} />
            <button
              className="consoleButton small"
              type="button"
              disabled={claimCall.busy}
              onClick={() => {
                void (async () => {
                  const confirmed = await confirm({
                    title: 'Claim owner token',
                    message:
                      'Claim the owner token for this device? This is normally done by the customer app — only claim during recovery.',
                    confirmLabel: 'Claim',
                    danger: true
                  });
                  if (confirmed) await claimCall.run();
                })();
              }}
            >
              <ShieldCheck size={13} />
              <span>Claim</span>
            </button>
          </FormRow>
          <FormRow label="Support token" hint="Scoped support-role token">
            <TextField value={supportLabel} onChange={setSupportLabel} />
            <button
              className="consoleButton small"
              type="button"
              disabled={supportCall.busy}
              onClick={() => void supportCall.run()}
            >
              <KeyRound size={13} />
              <span>Issue</span>
            </button>
          </FormRow>

          {issuedToken ? (
            <div className="tokenReveal">
              <code>{issuedToken}</code>
              <button
                className="consoleButton small"
                type="button"
                onClick={() => {
                  void navigator.clipboard
                    .writeText(issuedToken)
                    .then(() => setCopied(true));
                }}
              >
                <Copy size={13} />
                <span>{copied ? 'Copied' : 'Copy'}</span>
              </button>
            </div>
          ) : null}
        </SectionCard>
      </div>
    </div>
  );
}
