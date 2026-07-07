import { useCallback, useState } from 'react';
import {
  AlertTriangle,
  Download,
  Power,
  RefreshCw,
  RotateCcw,
  Upload
} from 'lucide-react';

import {
  applyHubUpdate,
  checkHubUpdate,
  downloadDebugBundle,
  fetchHubLogSources,
  fetchHubLogTail
} from '../../api';
import { JsonEditor } from '../../components/controls/JsonEditor';
import { SegmentedControl } from '../../components/controls/SegmentedControl';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import {
  LogPanel,
  preferredLogSourceId,
  type LogState
} from '../../components/panels/LogPanel';
import { ErrorNotice, KeyValueGrid } from '../../components/ui/bits';
import { SectionCard } from '../../components/ui/SectionCard';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import {
  getBackup,
  getProfileBundle,
  getShareBundle,
  resetProfileBundle,
  resetShareBundle,
  restoreBackup
} from '../../device/backup';
import {
  factoryReset,
  getOtaStatus,
  restartDevice
} from '../../device/diag';
import { getSettings, setSettings } from '../../device/settings';
import { asBoolean, asRecord, asString } from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import {
  errorMessage,
  formatDateTime,
  triggerBrowserDownload
} from '../../lib/format';
import { isPlainObject, prettyJson } from '../../lib/json';
import { useHub } from '../../state/HubContext';
import { useSession } from '../../state/SessionContext';

export default function SystemPage() {
  const client = useDeviceClient();
  const { accessToken } = useSession();
  const { hubId, hub } = useHub();
  const confirm = useConfirm();

  const settingsQuery = usePolling(
    useCallback(() => getSettings(client), [client])
  );
  const otaQuery = usePolling(
    useCallback(() => getOtaStatus(client), [client]),
    { intervalMs: 15_000 }
  );

  const settings = asRecord(settingsQuery.data);
  const ota = asRecord(otaQuery.data);

  const settingsRefresh = settingsQuery.refresh;
  const autoUpdateCall = useDeviceCall(
    useCallback(
      async (next: boolean) => {
        await setSettings(client, { auto_update: next });
        await settingsRefresh();
      },
      [client, settingsRefresh]
    )
  );
  const updateChannel = asString(settings.update_channel) ?? 'stable';
  const updateChannelCall = useDeviceCall(
    useCallback(
      async (next: string) => {
        await setSettings(client, { update_channel: next });
        await settingsRefresh();
      },
      [client, settingsRefresh]
    )
  );

  const otaRefresh = otaQuery.refresh;
  const [otaMessage, setOtaMessage] = useState<string | null>(null);
  const otaCheckCall = useDeviceCall(
    useCallback(async () => {
      const result = await checkHubUpdate(accessToken, hubId);
      setOtaMessage(
        asString(asRecord(result.result).message) ??
          (result.result.update_available === true
            ? `Update available: v${asString(result.result.latest_version) ?? '?'}`
            : 'Already up to date')
      );
      await otaRefresh();
    }, [accessToken, hubId, otaRefresh])
  );
  const otaApplyCall = useDeviceCall(
    useCallback(async () => {
      const result = await applyHubUpdate(accessToken, hubId);
      setOtaMessage(
        asString(asRecord(result.result).message) ?? 'Update requested'
      );
      await otaRefresh();
    }, [accessToken, hubId, otaRefresh])
  );

  // Logs (reuses the dashboard LogPanel with page-local state)
  const [logState, setLogState] = useState<LogState>({
    loading: false,
    opened: true
  });
  const loadLogs = useCallback(
    async (sourceId?: string) => {
      setLogState((current) => ({
        ...current,
        loading: true,
        opened: true,
        error: undefined
      }));
      try {
        const sources =
          logState.sources ??
          (await fetchHubLogSources(accessToken, hubId)).sources;
        const selectedSourceId =
          sourceId ?? logState.selectedSourceId ?? preferredLogSourceId(sources);
        const tail = selectedSourceId
          ? await fetchHubLogTail(accessToken, hubId, selectedSourceId)
          : undefined;
        setLogState({
          loading: false,
          opened: true,
          sources,
          selectedSourceId,
          tail
        });
      } catch (error) {
        setLogState((current) => ({
          ...current,
          loading: false,
          error: errorMessage(error)
        }));
      }
    },
    [accessToken, hubId, logState.sources, logState.selectedSourceId]
  );

  const [bundleBusy, setBundleBusy] = useState(false);
  const [bundleMessage, setBundleMessage] = useState<string | null>(null);

  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [restoreText, setRestoreText] = useState('');
  const [restoreParsed, setRestoreParsed] = useState<unknown>(undefined);

  const exportCall = useDeviceCall(
    useCallback(
      async (
        kind: 'backup' | 'profile-bundle' | 'share-bundle'
      ) => {
        const payload =
          kind === 'backup'
            ? await getBackup(client, includeSecrets)
            : kind === 'profile-bundle'
              ? await getProfileBundle(client)
              : await getShareBundle(client);
        const blob = new Blob([prettyJson(payload)], {
          type: 'application/json'
        });
        triggerBrowserDownload(
          blob,
          `rhythm-${kind}-${hub.name.replace(/[^A-Za-z0-9_-]+/g, '-')}-${new Date().toISOString().slice(0, 10)}.json`
        );
      },
      [client, includeSecrets, hub.name]
    )
  );

  const restoreCall = useDeviceCall(
    useCallback(async () => {
      if (!isPlainObject(restoreParsed)) {
        throw new Error('Backup payload must be a JSON object.');
      }
      await restoreBackup(client, restoreParsed);
    }, [client, restoreParsed])
  );

  const dangerCall = useDeviceCall(
    useCallback(async (action: () => Promise<unknown>) => {
      await action();
    }, [])
  );

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>System</h2>
          <p className="pageIntro">
            Settings, updates, logs, backups and destructive recovery actions
            for {hub.name}.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => {
              void settingsQuery.refresh();
              void otaQuery.refresh();
            }}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      <div className="cardGrid two">
        <SectionCard
          title="Settings"
          busy={
            settingsQuery.refreshing ||
            autoUpdateCall.busy ||
            updateChannelCall.busy
          }
          error={
            settingsQuery.error ??
            autoUpdateCall.error ??
            updateChannelCall.error
          }
          rawPayload={settingsQuery.data ?? undefined}
        >
          <div className="formRow">
            <div className="formRowLabel">
              <span>Update channel</span>
              <small>
                Beta receives every tagged release. Stable is the curated
                feed.
              </small>
            </div>
            <div className="formRowControl">
              <SegmentedControl
                value={updateChannel}
                disabled={settingsQuery.data === null || updateChannelCall.busy}
                onChange={(next) => {
                  if (next === updateChannel) return;
                  void (async () => {
                    if (next === 'beta') {
                      const confirmed = await confirm({
                        title: 'Switch to beta channel',
                        message: `Put ${hub.name} on the beta channel? It receives every tagged release ahead of the curated stable feed.`,
                        confirmLabel: 'Switch to beta'
                      });
                      if (!confirmed) return;
                    }
                    await updateChannelCall.run(next);
                  })();
                }}
                options={[
                  { value: 'stable', label: 'Stable' },
                  { value: 'beta', label: 'Beta' }
                ]}
              />
            </div>
          </div>
          <div className="formRow">
            <div className="formRowLabel">
              <span>Automatic updates</span>
              <small>
                Apply updates from the selected channel automatically during
                the daily window.
              </small>
            </div>
            <div className="formRowControl">
              <ToggleSwitch
                checked={asBoolean(settings.auto_update) ?? false}
                disabled={settingsQuery.data === null}
                busy={autoUpdateCall.busy}
                onChange={(next) => void autoUpdateCall.run(next)}
              />
            </div>
          </div>
        </SectionCard>

        <SectionCard
          title="Software update"
          subtitle="OTA runs through admin-api with a 2 minute window"
          busy={otaCheckCall.busy || otaApplyCall.busy || otaQuery.refreshing}
          error={otaCheckCall.error ?? otaApplyCall.error}
          rawPayload={otaQuery.data ?? undefined}
        >
          <KeyValueGrid
            rows={[
              ['State', asString(ota.state)],
              ['Channel', asString(ota.channel)],
              ['Current', asString(ota.current_version)],
              ['Latest', asString(ota.latest_version)],
              ['Update available', asBoolean(ota.update_available)],
              ['Last error', asString(ota.last_error)]
            ]}
          />
          {otaMessage ? <p className="cardNote">{otaMessage}</p> : null}
          <div className="actionRow">
            <button
              className="consoleButton small"
              type="button"
              disabled={otaCheckCall.busy || otaApplyCall.busy}
              onClick={() => void otaCheckCall.run()}
            >
              <RefreshCw size={13} />
              <span>Check</span>
            </button>
            <button
              className="consoleButton small primary"
              type="button"
              disabled={otaCheckCall.busy || otaApplyCall.busy}
              onClick={() => {
                void (async () => {
                  const confirmed = await confirm({
                    title: 'Apply OTA update',
                    message: `Update ${hub.name} now? Lights keep their last state while the server restarts.`,
                    confirmLabel: 'Update'
                  });
                  if (confirmed) await otaApplyCall.run();
                })();
              }}
            >
              <Download size={13} />
              <span>Update</span>
            </button>
          </div>
        </SectionCard>
      </div>

      <SectionCard
        title="Logs"
        subtitle="Tail device log sources"
        actions={
          <button
            className="consoleButton small"
            type="button"
            disabled={logState.loading}
            onClick={() => void loadLogs()}
          >
            <RefreshCw size={13} />
            <span>{logState.tail ? 'Refresh' : 'Load'}</span>
          </button>
        }
      >
        {logState.sources || logState.loading || logState.error ? (
          <LogPanel
            state={logState}
            onSourceChange={(sourceId) => void loadLogs(sourceId)}
            onRefresh={() => void loadLogs(logState.selectedSourceId)}
          />
        ) : (
          <p className="cardNote">Load log sources to tail them here.</p>
        )}
        <div className="actionRow">
          <button
            className="consoleButton small"
            type="button"
            disabled={bundleBusy}
            onClick={() => {
              setBundleBusy(true);
              setBundleMessage(null);
              downloadDebugBundle(accessToken, hubId)
                .then((bundle) => {
                  triggerBrowserDownload(bundle.blob, bundle.fileName);
                  setBundleMessage(
                    `Downloaded ${bundle.fileName} at ${formatDateTime(new Date().toISOString())}`
                  );
                })
                .catch((error) => setBundleMessage(errorMessage(error)))
                .finally(() => setBundleBusy(false));
            }}
          >
            <Download size={13} />
            <span>Debug bundle</span>
          </button>
          {bundleMessage ? <span className="cardNote">{bundleMessage}</span> : null}
        </div>
      </SectionCard>

      <SectionCard
        title="Backup & bundles"
        subtitle="Export device configuration as JSON, or restore a backup"
        busy={exportCall.busy || restoreCall.busy}
        error={exportCall.error ?? restoreCall.error}
      >
        <div className="actionRow">
          <button
            className="consoleButton small"
            type="button"
            onClick={() => void exportCall.run('backup')}
          >
            <Download size={13} />
            <span>Export backup</span>
          </button>
          <ToggleSwitch
            checked={includeSecrets}
            label="Include secrets"
            onChange={setIncludeSecrets}
          />
        </div>
        <div className="actionRow">
          <button
            className="consoleButton small"
            type="button"
            onClick={() => void exportCall.run('profile-bundle')}
          >
            <Download size={13} />
            <span>Profile bundle</span>
          </button>
          <button
            className="consoleButton small"
            type="button"
            onClick={() => void exportCall.run('share-bundle')}
          >
            <Download size={13} />
            <span>Share bundle</span>
          </button>
          <button
            className="consoleButton small danger"
            type="button"
            onClick={() => {
              void (async () => {
                const confirmed = await confirm({
                  title: 'Reset profile bundle',
                  message:
                    'Reset the profile bundle to factory defaults? All curve tuning is lost.',
                  confirmLabel: 'Reset',
                  danger: true,
                  requireTypedText: 'reset-profiles'
                });
                if (confirmed) await dangerCall.run(() => resetProfileBundle(client));
              })();
            }}
          >
            <RotateCcw size={13} />
            <span>Reset profiles</span>
          </button>
          <button
            className="consoleButton small danger"
            type="button"
            onClick={() => {
              void (async () => {
                const confirmed = await confirm({
                  title: 'Reset share bundle',
                  message: 'Reset the share bundle to factory defaults?',
                  confirmLabel: 'Reset',
                  danger: true
                });
                if (confirmed) await dangerCall.run(() => resetShareBundle(client));
              })();
            }}
          >
            <RotateCcw size={13} />
            <span>Reset share</span>
          </button>
        </div>

        <details className="restorePanel">
          <summary>Restore backup (paste JSON)</summary>
          <JsonEditor
            value={restoreText}
            rows={8}
            onChange={(text, parsed) => {
              setRestoreText(text);
              setRestoreParsed(parsed);
            }}
          />
          <button
            className="consoleButton small danger"
            type="button"
            disabled={!isPlainObject(restoreParsed) || restoreCall.busy}
            onClick={() => {
              void (async () => {
                const confirmed = await confirm({
                  title: 'Restore backup',
                  message: `Overwrite the entire configuration of ${hub.name} with the pasted backup? This replaces profiles, scenes, topology and settings.`,
                  confirmLabel: 'Restore',
                  danger: true,
                  requireTypedText: 'restore-backup'
                });
                if (confirmed) await restoreCall.run();
              })();
            }}
          >
            <Upload size={13} />
            <span>Restore</span>
          </button>
        </details>
      </SectionCard>

      <SectionCard
        title="Danger zone"
        subtitle="Recovery actions with real blast radius"
        busy={dangerCall.busy}
        error={dangerCall.error}
      >
        <div className="dangerZone">
          <div className="actionRow">
            <button
              className="consoleButton small danger"
              type="button"
              onClick={() => {
                void (async () => {
                  const confirmed = await confirm({
                    title: 'Restart device',
                    message: `Restart ${hub.name}? It drops offline for a minute; lights hold their last state.`,
                    confirmLabel: 'Restart',
                    danger: true
                  });
                  if (confirmed) await dangerCall.run(() => restartDevice(client));
                })();
              }}
            >
              <Power size={13} />
              <span>Restart</span>
            </button>
            <button
              className="consoleButton small danger"
              type="button"
              onClick={() => {
                void (async () => {
                  const confirmed = await confirm({
                    title: 'Factory reset',
                    message: `Factory reset ${hub.name}? ALL configuration, pairing and tokens are wiped. The customer must re-onboard the device.`,
                    confirmLabel: 'Factory reset',
                    danger: true,
                    requireTypedText: 'factory-reset'
                  });
                  if (confirmed) await dangerCall.run(() => factoryReset(client));
                })();
              }}
            >
              <AlertTriangle size={13} />
              <span>Factory reset</span>
            </button>
          </div>
        </div>
      </SectionCard>
    </div>
  );
}
