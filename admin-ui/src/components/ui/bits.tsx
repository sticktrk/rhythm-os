import type { ReactNode } from 'react';
import { AlertTriangle, Inbox, Loader2 } from 'lucide-react';

import { stringValue } from '../../lib/format';

export function InlineSpinner({ label }: { label?: string }) {
  return (
    <span className="inlineSpinner">
      <Loader2 className="spin" size={15} />
      {label ? <span>{label}</span> : null}
    </span>
  );
}

export function ErrorNotice({ message }: { message: string }) {
  return (
    <div className="notice error">
      <AlertTriangle size={17} />
      <span>{message}</span>
    </div>
  );
}

export function EmptyState({
  message,
  icon
}: {
  message: string;
  icon?: ReactNode;
}) {
  return (
    <div className="consoleEmpty">
      {icon ?? <Inbox size={22} />}
      <span>{message}</span>
    </div>
  );
}

export function KeyValueGrid({
  rows
}: {
  rows: Array<[string, unknown]>;
}) {
  const present = rows.filter(
    ([, value]) => value !== undefined && value !== null && value !== ''
  );
  if (present.length === 0) {
    return <div className="kvEmpty">No data.</div>;
  }
  return (
    <dl className="kvGrid">
      {present.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd>{stringValue(value)}</dd>
        </div>
      ))}
    </dl>
  );
}
