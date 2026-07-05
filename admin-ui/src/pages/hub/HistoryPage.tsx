import { useCallback, useEffect, useRef, useState } from 'react';
import { RefreshCw, Search } from 'lucide-react';

import { NumberField, TextField } from '../../components/controls/fields';
import { ToggleSwitch } from '../../components/controls/ToggleSwitch';
import { EmptyState, ErrorNotice, InlineSpinner } from '../../components/ui/bits';
import { RawPayloadToggle, SectionCard } from '../../components/ui/SectionCard';
import { getHistory } from '../../device/settings';
import {
  asArray,
  asNumber,
  asRecord,
  asRecordArray,
  asString
} from '../../device/values';
import { useDeviceClient } from '../../hooks/useDeviceClient';
import { errorMessage, formatDateTime, formatEpochMs } from '../../lib/format';
import { prettyJson } from '../../lib/json';
import { useHub } from '../../state/HubContext';

import '../../styles/pages-phase6.css';

type HistoryFilters = {
  limit: number;
  area: string;
  source: string;
  action: string;
};

const DEFAULT_FILTERS: HistoryFilters = {
  limit: 100,
  area: '',
  source: '',
  action: ''
};

export default function HistoryPage() {
  const client = useDeviceClient();
  const { hub } = useHub();

  // Draft vs applied filters so typing doesn't spam the device.
  const [draft, setDraft] = useState<HistoryFilters>(DEFAULT_FILTERS);
  const [applied, setApplied] = useState<HistoryFilters>(DEFAULT_FILTERS);
  const [autoRefresh, setAutoRefresh] = useState(false);

  const [data, setData] = useState<unknown>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

  const loadSeq = useRef(0);
  const load = useCallback(async () => {
    // Sequence guard: a slow response for an older filter must not
    // overwrite the result of a newer one.
    const seq = ++loadSeq.current;
    setBusy(true);
    try {
      const result = await getHistory(client, {
        limit: applied.limit,
        ...(applied.area.trim() ? { area: applied.area.trim() } : {}),
        ...(applied.source.trim() ? { source: applied.source.trim() } : {}),
        ...(applied.action.trim() ? { action: applied.action.trim() } : {})
      });
      if (seq !== loadSeq.current) return;
      setData(result);
      setError(null);
      setExpanded(new Set());
    } catch (err) {
      if (seq !== loadSeq.current) return;
      setError(errorMessage(err));
    } finally {
      if (seq === loadSeq.current) setBusy(false);
    }
  }, [client, applied]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!autoRefresh) return;
    const timer = window.setInterval(() => {
      if (document.hidden) return;
      void load();
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [autoRefresh, load]);

  const payload = asRecord(data);
  const entries = Array.isArray(data)
    ? asRecordArray(data)
    : asRecordArray(
        asArray(payload.entries).length > 0
          ? payload.entries
          : asArray(payload.history).length > 0
            ? payload.history
            : payload.items
      );

  function toggleRow(index: number) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  }

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Device console</div>
          <h2>Activity &amp; History</h2>
          <p className="pageIntro">
            Activity log on {hub.name} (api/history), newest first.
          </p>
        </div>
        <div className="pageHeaderActions">
          <ToggleSwitch
            checked={autoRefresh}
            onChange={setAutoRefresh}
            label="Auto-refresh 30s"
          />
          <button
            className="consoleButton"
            type="button"
            disabled={busy}
            onClick={() => void load()}
          >
            <RefreshCw size={15} />
            <span>Refresh</span>
          </button>
        </div>
      </header>

      {error && entries.length === 0 ? (
        <ErrorNotice message={`History unavailable: ${error}`} />
      ) : null}

      <SectionCard title="Filters">
        <div className="filtersRow">
          <label className="filterField">
            <span>Limit</span>
            <NumberField
              value={draft.limit}
              min={1}
              max={2000}
              onChange={(next) =>
                setDraft((current) => ({ ...current, limit: next ?? 100 }))
              }
            />
          </label>
          <label className="filterField">
            <span>Area</span>
            <TextField
              value={draft.area}
              onChange={(next) => setDraft((current) => ({ ...current, area: next }))}
            />
          </label>
          <label className="filterField">
            <span>Source</span>
            <TextField
              value={draft.source}
              onChange={(next) =>
                setDraft((current) => ({ ...current, source: next }))
              }
            />
          </label>
          <label className="filterField">
            <span>Action</span>
            <TextField
              value={draft.action}
              onChange={(next) =>
                setDraft((current) => ({ ...current, action: next }))
              }
            />
          </label>
          <button
            className="consoleButton primary"
            type="button"
            disabled={busy}
            onClick={() => setApplied({ ...draft })}
          >
            <Search size={15} />
            <span>Apply</span>
          </button>
          {busy ? <InlineSpinner label="Loading" /> : null}
        </div>
      </SectionCard>

      <SectionCard
        title="Entries"
        subtitle={`${entries.length} entr${entries.length === 1 ? 'y' : 'ies'}`}
        error={error}
      >
        {entries.length === 0 ? (
          <EmptyState message="No history entries match these filters." />
        ) : (
          <div className="dataTableWrap">
            <table className="dataTable">
              <thead>
                <tr>
                  <th>Time</th>
                  <th>Area</th>
                  <th>Source</th>
                  <th>Action</th>
                  <th>Detail</th>
                </tr>
              </thead>
              <tbody>
                {entries.map((entry, index) => {
                  const time =
                    formatEpochMs(asNumber(entry.epoch_ms)) ??
                    formatEpochMs(asNumber(entry.timestamp_ms)) ??
                    maybeDateTime(entry.timestamp) ??
                    maybeDateTime(entry.time) ??
                    maybeDateTime(entry.at);
                  const detail =
                    asString(entry.detail) ??
                    asString(entry.message) ??
                    asString(entry.description);
                  const isOpen = expanded.has(index);
                  return (
                    <tr
                      key={index}
                      onClick={() => toggleRow(index)}
                      style={{ cursor: 'pointer' }}
                    >
                      <td className="time">{time ?? '—'}</td>
                      <td>{asString(entry.area) ?? '—'}</td>
                      <td>{asString(entry.source) ?? '—'}</td>
                      <td>{asString(entry.action) ?? '—'}</td>
                      <td className={isOpen ? 'mono' : undefined}>
                        {isOpen ? (
                          <pre style={{ margin: 0, whiteSpace: 'pre-wrap' }}>
                            {prettyJson(entry)}
                          </pre>
                        ) : (
                          (detail ?? '(click for raw entry)')
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
        <RawPayloadToggle payload={data} />
      </SectionCard>

    </div>
  );
}

function maybeDateTime(value: unknown): string | undefined {
  const text = asString(value);
  if (!text) return undefined;
  return formatDateTime(text);
}
