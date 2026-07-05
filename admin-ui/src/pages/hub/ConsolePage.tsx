import { useState } from 'react';
import { Activity, Loader2 } from 'lucide-react';

import { runDeviceAdminProxy } from '../../api';
import {
  DEVICE_ADMIN_OPERATIONS,
  operationToBodyText,
  operationToQueryText,
  type DeviceAdminOperation
} from '../../deviceAdminOperations';
import { errorMessage, formatDateTime } from '../../lib/format';
import { parseOptionalJson, parseQueryText, prettyJson } from '../../lib/json';
import { useHub } from '../../state/HubContext';
import { useSession } from '../../state/SessionContext';
import type {
  DeviceAdminMethod,
  DeviceAdminProxyResponse
} from '../../types';

type ConsoleState = {
  loading: boolean;
  selectedOperationId: string;
  method: DeviceAdminMethod;
  path: string;
  queryText: string;
  bodyText: string;
  timeoutSeconds?: number;
  error?: string;
  result?: DeviceAdminProxyResponse;
};

function stateFromOperation(operation: DeviceAdminOperation): ConsoleState {
  return {
    loading: false,
    selectedOperationId: operation.id,
    method: operation.method,
    path: operation.path,
    queryText: operationToQueryText(operation),
    bodyText: operationToBodyText(operation),
    timeoutSeconds: operation.timeoutSeconds
  };
}

export default function ConsolePage() {
  const { accessToken } = useSession();
  const { hubId, hub } = useHub();
  const [state, setState] = useState<ConsoleState>(() =>
    stateFromOperation(DEVICE_ADMIN_OPERATIONS[0])
  );

  const selectedOperation = DEVICE_ADMIN_OPERATIONS.find(
    (operation) => operation.id === state.selectedOperationId
  );

  function handleOperationChange(operationId: string) {
    const operation =
      DEVICE_ADMIN_OPERATIONS.find((item) => item.id === operationId) ??
      DEVICE_ADMIN_OPERATIONS[0];
    setState(stateFromOperation(operation));
  }

  function patch(next: Partial<ConsoleState>) {
    setState((current) => ({ ...current, ...next, error: undefined }));
  }

  async function handleRun() {
    if (selectedOperation?.danger) {
      const confirmed = window.confirm(
        `Run ${selectedOperation.label} on ${hub.name}?`
      );
      if (!confirmed) return;
    }

    let query: Record<string, string>;
    let body: unknown;
    try {
      query = parseQueryText(state.queryText);
      body = parseOptionalJson(state.bodyText, 'Body');
    } catch (error) {
      setState((current) => ({
        ...current,
        loading: false,
        error: errorMessage(error)
      }));
      return;
    }

    setState((current) => ({ ...current, loading: true, error: undefined }));

    try {
      const result = await runDeviceAdminProxy(accessToken, hubId, {
        method: state.method,
        path: state.path,
        query,
        ...(body === undefined ? {} : { body }),
        timeoutSeconds: state.timeoutSeconds
      });
      setState((current) => ({ ...current, loading: false, result }));
    } catch (error) {
      setState((current) => ({
        ...current,
        loading: false,
        error: errorMessage(error)
      }));
    }
  }

  return (
    <div className="consolePage">
      <header className="pageHeader">
        <div>
          <div className="eyebrow">Raw device access</div>
          <h2>Console</h2>
          <p className="pageIntro">
            Templated JSON requests straight to the device. Prefer the
            purpose-built pages; this is the escape hatch.
          </p>
        </div>
      </header>

      <div className="deviceAdminPanel">
        <div className="deviceAdminHeader">
          <div>
            <div className="eyebrow">Device Admin</div>
            <h4>{selectedOperation?.label ?? 'Custom JSON request'}</h4>
          </div>
          <button
            className={selectedOperation?.danger ? 'probeButton danger' : 'probeButton'}
            type="button"
            onClick={() => void handleRun()}
            disabled={state.loading}
          >
            {state.loading ? <Loader2 className="spin" size={16} /> : <Activity size={16} />}
            <span>Run</span>
          </button>
        </div>

        {selectedOperation?.description ? (
          <div className="deviceAdminDescription">
            {selectedOperation.description}
          </div>
        ) : null}

        <div className="deviceAdminGrid">
          <label>
            <span>Operation</span>
            <select
              value={state.selectedOperationId}
              onChange={(event) => handleOperationChange(event.target.value)}
            >
              {DEVICE_ADMIN_OPERATIONS.map((operation) => (
                <option key={operation.id} value={operation.id}>
                  {operation.category} / {operation.label}
                </option>
              ))}
            </select>
          </label>

          <label>
            <span>Method</span>
            <select
              value={state.method}
              onChange={(event) =>
                patch({ method: event.target.value as DeviceAdminMethod })
              }
            >
              <option value="GET">GET</option>
              <option value="POST">POST</option>
              <option value="PUT">PUT</option>
              <option value="PATCH">PATCH</option>
              <option value="DELETE">DELETE</option>
            </select>
          </label>

          <label className="wide">
            <span>Path</span>
            <input
              value={state.path}
              onChange={(event) => patch({ path: event.target.value })}
              spellCheck={false}
            />
          </label>

          <label>
            <span>Timeout seconds</span>
            <input
              type="number"
              min={1}
              max={120}
              value={state.timeoutSeconds ?? ''}
              onChange={(event) =>
                patch({
                  timeoutSeconds:
                    event.target.value.trim() === ''
                      ? undefined
                      : Number(event.target.value)
                })
              }
            />
          </label>

          <label className="wide">
            <span>Query JSON</span>
            <textarea
              value={state.queryText}
              onChange={(event) => patch({ queryText: event.target.value })}
              spellCheck={false}
              rows={4}
            />
          </label>

          <label className="wide">
            <span>Body JSON</span>
            <textarea
              value={state.bodyText}
              onChange={(event) => patch({ bodyText: event.target.value })}
              spellCheck={false}
              rows={8}
            />
          </label>
        </div>

        {state.error ? <div className="hubMessage">{state.error}</div> : null}

        {state.result ? (
          <div className="deviceAdminResult">
            <div className="logMeta">
              <span>
                {state.result.method} /{state.result.path}
              </span>
              <span>
                {state.result.route} {state.result.baseUrl}
              </span>
              <span>HTTP {state.result.statusCode}</span>
              <span>{formatDateTime(state.result.completedAt)}</span>
            </div>
            <pre>{prettyJson(state.result.body)}</pre>
          </div>
        ) : null}
      </div>
    </div>
  );
}
