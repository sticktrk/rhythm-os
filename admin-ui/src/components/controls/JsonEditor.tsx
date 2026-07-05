import { useState } from 'react';

/** Validated JSON textarea — the structured-editor fallback for free-form
    device payloads (power schedules, hardware settings, cloud configs). */
export function JsonEditor({
  value,
  onChange,
  rows = 10,
  disabled,
  externalError
}: {
  value: string;
  onChange: (value: string, parsed: unknown | undefined) => void;
  rows?: number;
  disabled?: boolean;
  externalError?: string | null;
}) {
  const [parseError, setParseError] = useState<string | null>(null);

  function handleChange(next: string) {
    if (next.trim() === '') {
      setParseError(null);
      onChange(next, undefined);
      return;
    }
    try {
      const parsed = JSON.parse(next) as unknown;
      setParseError(null);
      onChange(next, parsed);
    } catch (error) {
      setParseError(error instanceof Error ? error.message : String(error));
      onChange(next, undefined);
    }
  }

  const error = externalError ?? parseError;

  return (
    <div className="jsonEditor">
      <textarea
        className={`jsonEditorArea${error ? ' invalid' : ''}`}
        value={value}
        rows={rows}
        spellCheck={false}
        disabled={disabled}
        onChange={(event) => handleChange(event.target.value)}
      />
      {error ? <div className="jsonEditorError">{error}</div> : null}
    </div>
  );
}
