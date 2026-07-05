import { useCallback, useState } from 'react';
import { RefreshCw } from 'lucide-react';

import { NumberField, TextField, FormRow } from '../../components/controls/fields';
import { KeyValueGrid } from '../../components/ui/bits';
import { useConfirm } from '../../components/ui/ConfirmDialog';
import { SectionCard } from '../../components/ui/SectionCard';
import { setLocation } from '../../device/environment';
import { getSolar } from '../../device/curves';
import { asRecord } from '../../device/values';
import { useDeviceCall } from '../../hooks/useDeviceCall';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { usePolling } from '../../hooks/usePolling';
import { useHub } from '../../state/HubContext';

import '../../styles/pages-phase6.css';

export default function EnvironmentPage() {
  const client = useDeviceClient();
  const { hub } = useHub();
  const confirm = useConfirm();

  const solarQuery = usePolling(
    useCallback(() => getSolar(client), [client]),
    { intervalMs: 0 }
  );
  const solar = asRecord(solarQuery.data);

  // Location form
  const [lat, setLat] = useState<number | undefined>(undefined);
  const [lon, setLon] = useState<number | undefined>(undefined);
  const [timezone, setTimezone] = useState('');
  const [locationResult, setLocationResult] = useState<string | null>(null);
  const locationCall = useDeviceCall(
    useCallback(
      async (body: { lat: number; lon: number; timezoneName?: string }) => {
        await setLocation(client, body);
        setLocationResult('Location updated.');
      },
      [client]
    )
  );

  async function handleSetLocation() {
    setLocationResult(null);
    if (lat === undefined || lon === undefined) return;
    const ok = await confirm({
      title: 'Update device location',
      message: `Set location to ${lat}, ${lon}${timezone ? ` (${timezone})` : ''}? Solar times and the entire rhythm curve follow the device location.`,
      confirmLabel: 'Update location'
    });
    if (!ok) return;
    await locationCall.run({
      lat,
      lon,
      ...(timezone.trim() ? { timezoneName: timezone.trim() } : {})
    });
  }

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Environment</h2>
          <p className="pageIntro">
            Location and solar context for {hub.name}.
          </p>
        </div>
        <div className="pageHeaderActions">
          <button
            className="consoleButton"
            type="button"
            onClick={() => void solarQuery.refresh()}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      <SectionCard
        title="Location"
        subtitle="Drives solar times and the shape of the day curve"
        busy={locationCall.busy}
        error={locationCall.error}
      >
        <FormRow label="Latitude">
          <NumberField value={lat} step={0.0001} onChange={setLat} />
        </FormRow>
        <FormRow label="Longitude">
          <NumberField value={lon} step={0.0001} onChange={setLon} />
        </FormRow>
        <FormRow label="Timezone" hint="IANA name, e.g. America/Chicago">
          <TextField value={timezone} onChange={setTimezone} mono />
        </FormRow>
        <div className="buttonRow">
          <button
            className="consoleButton primary"
            type="button"
            disabled={
              locationCall.busy || lat === undefined || lon === undefined
            }
            onClick={() => void handleSetLocation()}
          >
            <span>Update location</span>
          </button>
          {locationResult ? (
            <span className="resultNote">{locationResult}</span>
          ) : null}
        </div>
      </SectionCard>

      <SectionCard
        title="Solar times (today)"
        busy={solarQuery.refreshing}
        error={solarQuery.error}
        rawPayload={solarQuery.data ?? undefined}
      >
        <KeyValueGrid
          rows={Object.entries(solar)
            .filter(([, value]) => typeof value !== 'object' || value === null)
            .map(([key, value]) => [prettifyKey(key), value])}
        />
      </SectionCard>
    </div>
  );
}

function prettifyKey(key: string): string {
  return key.replace(/[_-]+/g, ' ').replace(/^\w/, (c) => c.toUpperCase());
}
