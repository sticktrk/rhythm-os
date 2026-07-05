import { useState, type ReactNode } from 'react';
import { ChevronDown, ChevronRight, Loader2 } from 'lucide-react';

import { prettyJson } from '../../lib/json';

export function SectionCard({
  title,
  subtitle,
  actions,
  busy,
  error,
  rawPayload,
  children
}: {
  title: string;
  subtitle?: string;
  actions?: ReactNode;
  busy?: boolean;
  error?: string | null;
  /** When provided, renders a "Raw JSON" disclosure — the support-staff
      escape hatch when a payload doesn't match the structured UI. */
  rawPayload?: unknown;
  children?: ReactNode;
}) {
  return (
    <section className="sectionCard">
      <div className="sectionCardHeader">
        <div>
          <h3>{title}</h3>
          {subtitle ? <p>{subtitle}</p> : null}
        </div>
        <div className="sectionCardActions">
          {busy ? <Loader2 className="spin" size={16} /> : null}
          {actions}
        </div>
      </div>
      {error ? <div className="hubMessage">{error}</div> : null}
      {children}
      {rawPayload !== undefined ? <RawPayloadToggle payload={rawPayload} /> : null}
    </section>
  );
}

export function RawPayloadToggle({ payload }: { payload: unknown }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="rawPayload">
      <button
        className="rawPayloadToggle"
        type="button"
        onClick={() => setOpen((current) => !current)}
      >
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span>Raw JSON</span>
      </button>
      {open ? <pre className="rawPayloadBody">{prettyJson(payload)}</pre> : null}
    </div>
  );
}
