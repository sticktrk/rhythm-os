import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent
} from 'react';

import { kelvinGradientCss } from './colorMath';

export type TrackStyle = 'plain' | 'kelvin';

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function snap(value: number, min: number, max: number, step: number): number {
  const stepped = Math.round((value - min) / step) * step + min;
  const decimals = step < 1 ? String(step).split('.')[1]?.length ?? 2 : 0;
  return clamp(Number(stepped.toFixed(decimals)), min, max);
}

function trackBackground(
  style: TrackStyle,
  min: number,
  max: number
): CSSProperties | undefined {
  if (style === 'kelvin') {
    return { background: kelvinGradientCss(min, max) };
  }
  return undefined;
}

/** Single-thumb slider. `onChange` fires during drag (update local UI only);
    `onCommit` fires on release / keyboard settle (send the network write). */
export function Slider({
  value,
  min,
  max,
  step = 1,
  label,
  format,
  disabled,
  trackStyle = 'plain',
  onChange,
  onCommit
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  label?: string;
  format?: (value: number) => string;
  disabled?: boolean;
  trackStyle?: TrackStyle;
  onChange?: (value: number) => void;
  onCommit: (value: number) => void;
}) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const [dragValue, setDragValue] = useState<number | null>(null);
  const keyboardTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    return () => {
      if (keyboardTimer.current !== undefined) {
        window.clearTimeout(keyboardTimer.current);
      }
    };
  }, []);

  const shown = dragValue ?? value;
  const fraction = max > min ? (shown - min) / (max - min) : 0;

  const valueFromPointer = useCallback(
    (clientX: number): number => {
      const track = trackRef.current;
      if (!track) return shown;
      const rect = track.getBoundingClientRect();
      const t = clamp((clientX - rect.left) / rect.width, 0, 1);
      return snap(min + t * (max - min), min, max, step);
    },
    [max, min, shown, step]
  );

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const next = valueFromPointer(event.clientX);
    setDragValue(next);
    onChange?.(next);
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled || dragValue === null) return;
    if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
    const next = valueFromPointer(event.clientX);
    setDragValue(next);
    onChange?.(next);
  }

  function handlePointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled || dragValue === null) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    const next = valueFromPointer(event.clientX);
    setDragValue(null);
    onCommit(next);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (disabled) return;
    let next: number | null = null;
    if (event.key === 'ArrowLeft' || event.key === 'ArrowDown') {
      next = snap(shown - step, min, max, step);
    } else if (event.key === 'ArrowRight' || event.key === 'ArrowUp') {
      next = snap(shown + step, min, max, step);
    } else if (event.key === 'Home') {
      next = min;
    } else if (event.key === 'End') {
      next = max;
    }
    if (next === null) return;
    event.preventDefault();
    setDragValue(next);
    onChange?.(next);
    // Debounce the commit so arrow-key runs produce one write.
    if (keyboardTimer.current !== undefined) {
      window.clearTimeout(keyboardTimer.current);
    }
    const settled = next;
    keyboardTimer.current = window.setTimeout(() => {
      setDragValue(null);
      onCommit(settled);
    }, 450);
  }

  return (
    <div className={`sliderRow${disabled ? ' disabled' : ''}`}>
      {label ? <span className="sliderLabel">{label}</span> : null}
      <div
        className={`sliderTrack ${trackStyle}`}
        ref={trackRef}
        role="slider"
        aria-label={label}
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={shown}
        aria-disabled={disabled}
        tabIndex={disabled ? -1 : 0}
        style={trackBackground(trackStyle, min, max)}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onKeyDown={handleKeyDown}
      >
        {trackStyle === 'plain' ? (
          <div
            className="sliderFill"
            style={{ width: `${fraction * 100}%` }}
          />
        ) : null}
        <div
          className="sliderThumb"
          style={{ left: `${fraction * 100}%` }}
        />
      </div>
      <span className="sliderReadout readout">
        {format ? format(shown) : String(shown)}
      </span>
    </div>
  );
}

/** Dual-thumb range slider for envelopes (min/max brightness, CCT range). */
export function RangeSlider({
  low,
  high,
  min,
  max,
  step = 1,
  minGap = 0,
  label,
  format,
  disabled,
  trackStyle = 'plain',
  onCommit
}: {
  low: number;
  high: number;
  min: number;
  max: number;
  step?: number;
  minGap?: number;
  label?: string;
  format?: (value: number) => string;
  disabled?: boolean;
  trackStyle?: TrackStyle;
  onCommit: (low: number, high: number) => void;
}) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const [drag, setDrag] = useState<{
    thumb: 'low' | 'high';
    low: number;
    high: number;
  } | null>(null);

  const shownLow = drag?.low ?? low;
  const shownHigh = drag?.high ?? high;
  const lowFraction = max > min ? (shownLow - min) / (max - min) : 0;
  const highFraction = max > min ? (shownHigh - min) / (max - min) : 1;

  const valueFromPointer = useCallback(
    (clientX: number): number => {
      const track = trackRef.current;
      if (!track) return min;
      const rect = track.getBoundingClientRect();
      const t = clamp((clientX - rect.left) / rect.width, 0, 1);
      return snap(min + t * (max - min), min, max, step);
    },
    [max, min, step]
  );

  function applyDrag(
    thumb: 'low' | 'high',
    pointerValue: number,
    current: { low: number; high: number }
  ): { low: number; high: number } {
    if (thumb === 'low') {
      return {
        low: Math.min(pointerValue, current.high - minGap),
        high: current.high
      };
    }
    return {
      low: current.low,
      high: Math.max(pointerValue, current.low + minGap)
    };
  }

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const pointerValue = valueFromPointer(event.clientX);
    const thumb =
      Math.abs(pointerValue - shownLow) <= Math.abs(pointerValue - shownHigh)
        ? 'low'
        : 'high';
    const next = applyDrag(thumb, pointerValue, {
      low: shownLow,
      high: shownHigh
    });
    setDrag({ thumb, ...next });
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled || !drag) return;
    if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
    const pointerValue = valueFromPointer(event.clientX);
    const next = applyDrag(drag.thumb, pointerValue, drag);
    setDrag({ thumb: drag.thumb, ...next });
  }

  function handlePointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled || !drag) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    setDrag(null);
    onCommit(drag.low, drag.high);
  }

  return (
    <div className={`sliderRow${disabled ? ' disabled' : ''}`}>
      {label ? <span className="sliderLabel">{label}</span> : null}
      <div
        className={`sliderTrack range ${trackStyle}`}
        ref={trackRef}
        style={trackBackground(trackStyle, min, max)}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
      >
        <div
          className="sliderFill range"
          style={{
            left: `${lowFraction * 100}%`,
            width: `${(highFraction - lowFraction) * 100}%`
          }}
        />
        <div className="sliderThumb" style={{ left: `${lowFraction * 100}%` }} />
        <div className="sliderThumb" style={{ left: `${highFraction * 100}%` }} />
      </div>
      <span className="sliderReadout readout">
        {format
          ? `${format(shownLow)} – ${format(shownHigh)}`
          : `${shownLow} – ${shownHigh}`}
      </span>
    </div>
  );
}

/** Kelvin slider with a blackbody gradient track. */
export function KelvinSlider(props: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  label?: string;
  disabled?: boolean;
  onChange?: (value: number) => void;
  onCommit: (value: number) => void;
}) {
  const { min = 500, max = 6500, step = 50, ...rest } = props;
  return (
    <Slider
      {...rest}
      min={min}
      max={max}
      step={step}
      trackStyle="kelvin"
      format={(value) => `${Math.round(value)}K`}
    />
  );
}
