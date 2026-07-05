import { Loader2, RefreshCw } from 'lucide-react';

import { formatDateTime } from '../../lib/format';
import type { DeviceLogSource, DeviceLogTail } from '../../types';

export type LogState = {
  loading: boolean;
  opened: boolean;
  error?: string;
  sources?: DeviceLogSource[];
  selectedSourceId?: string;
  tail?: DeviceLogTail;
};

export function preferredLogSourceId(
  sources: DeviceLogSource[]
): string | undefined {
  return (
    sources.find((source) => source.id === 'rhythm-server.log') ??
    sources.find((source) => source.fileName === 'rhythm-server.log') ??
    sources[0]
  )?.id;
}

/** Strip ANSI escape sequences, including orphaned CSI fragments like "[2K"
    that survive when the ESC byte was lost in transport. */
function stripAnsi(text: string): string {
  return text
    // eslint-disable-next-line no-control-regex
    .replace(/\[[0-9;]*[A-Za-z]/g, '')
    .replace(/\[[0-9;]*[mK]/g, '');
}

type ParsedLogLine = {
  time?: string;
  level?: 'INFO' | 'WARN' | 'ERROR' | 'DEBUG' | 'TRACE';
  module?: string;
  message: string;
};

// Matches the rhythm-server tracing format:
//   2026-07-05T07:04:13.813509-04:00  INFO  rhythm-main-rt  message…
const LOG_LINE_RE =
  /^(\d{4}-\d{2}-\d{2}T(\d{2}:\d{2}:\d{2})\.\d+(?:[+-]\d{2}:\d{2}|Z)?)\s+(INFO|WARN|ERROR|DEBUG|TRACE)\s+(\S+)\s+(.*)$/;

function parseLogLine(raw: string): ParsedLogLine {
  const text = stripAnsi(raw);
  const match = LOG_LINE_RE.exec(text);
  if (!match) return { message: text };
  return {
    time: match[2],
    level: match[3] as ParsedLogLine['level'],
    module: match[4],
    message: match[5]
  };
}

export function LogPanel({
  state,
  onSourceChange,
  onRefresh
}: {
  state: LogState;
  onSourceChange: (sourceId: string) => void;
  onRefresh: () => void;
}) {
  const sources = state.sources ?? [];
  return (
    <div className="logPanel">
      <div className="logToolbar">
        <div className="logMeta">
          {state.tail ? (
            <>
              <span>{state.tail.route} {state.tail.baseUrl}</span>
              <span>{state.tail.returnedLines} lines</span>
              <span>{formatDateTime(state.tail.fetchedAt)}</span>
            </>
          ) : (
            <span>Logs</span>
          )}
        </div>
        <div className="logControls">
          <select
            className="logSelect"
            value={state.selectedSourceId ?? ''}
            disabled={state.loading || sources.length === 0}
            onChange={(event) => onSourceChange(event.target.value)}
          >
            {sources.length === 0 ? (
              <option value="">No sources</option>
            ) : (
              sources.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.fileName}
                </option>
              ))
            )}
          </select>
          <button
            className="probeButton"
            type="button"
            onClick={onRefresh}
            disabled={state.loading || !state.selectedSourceId}
          >
            {state.loading ? <Loader2 className="spin" size={16} /> : <RefreshCw size={16} />}
            <span>Refresh</span>
          </button>
        </div>
      </div>

      {state.error ? <div className="hubMessage">{state.error}</div> : null}

      {state.tail ? (
        <div className="logOutput" aria-label={`${state.tail.source.fileName} tail`}>
          {state.tail.lines.length === 0 ? (
            <div className="logEmpty">No lines returned.</div>
          ) : (
            state.tail.lines.map((line) => {
              const parsed = parseLogLine(line.text);
              return (
                <div
                  className={`logLine parsed${parsed.level ? ` lvl${parsed.level}` : ''}`}
                  key={`${line.source}-${line.lineNumber}`}
                >
                  <span className="logLineNumber">{line.lineNumber}</span>
                  <span className="logTime">{parsed.time ?? ''}</span>
                  <span className={`logLevel ${parsed.level ?? 'RAW'}`}>
                    {parsed.level ?? ''}
                  </span>
                  <span className="logModule">{parsed.module ?? ''}</span>
                  <span className="logLineText">{parsed.message}</span>
                </div>
              );
            })
          )}
        </div>
      ) : null}
    </div>
  );
}
