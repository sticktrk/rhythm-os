import { Loader2 } from 'lucide-react';

export function ToggleSwitch({
  checked,
  onChange,
  disabled,
  busy,
  label
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  disabled?: boolean;
  busy?: boolean;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className={`toggleSwitch${checked ? ' on' : ''}`}
      disabled={disabled || busy}
      onClick={() => onChange(!checked)}
    >
      <span className="toggleTrack">
        <span className="toggleThumb">
          {busy ? <Loader2 className="spin" size={11} /> : null}
        </span>
      </span>
      {label ? <span className="toggleLabel">{label}</span> : null}
    </button>
  );
}
