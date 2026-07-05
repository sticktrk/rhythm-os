import {
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent
} from 'react';

import {
  CURVE_CHART_WIDTH,
  CurveChart,
  plotGeometry,
  type CurveSeries
} from './CurveChart';

export type CurveHandle = {
  id: string;
  hour: number;
  value: number;
  axis: 'brightness' | 'kelvin';
  /** Lock dragging on an axis (e.g. width handles move horizontally only). */
  xLocked?: boolean;
  yLocked?: boolean;
  label?: string;
  color?: string;
};

/** CurveChart plus draggable parameter handles. The chart series must come
    from the device's sampler (`POST api/curve`); handles express config
    parameters in data coordinates, and drags report back data-space deltas. */
export function CurveEditor({
  series,
  handles,
  height = 280,
  nowHour,
  solar,
  yLeft = { min: 0, max: 100 },
  yRight = { min: 500, max: 6500 },
  sampling,
  disabled,
  onHandleDrag,
  onCommit
}: {
  series: CurveSeries[];
  handles: CurveHandle[];
  height?: number;
  nowHour?: number;
  solar?: { sunriseHour?: number; sunsetHour?: number };
  yLeft?: { min: number; max: number };
  yRight?: { min: number; max: number };
  /** True while a re-sample is in flight (chart shows the previous samples). */
  sampling?: boolean;
  disabled?: boolean;
  onHandleDrag: (id: string, next: { hour?: number; value?: number }) => void;
  onCommit: () => void;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [activeHandle, setActiveHandle] = useState<string | null>(null);
  const geometry = plotGeometry(height, yLeft, yRight);

  function svgPointFromEvent(event: ReactPointerEvent): {
    x: number;
    y: number;
  } {
    const container = containerRef.current;
    if (!container) return { x: 0, y: 0 };
    const svg = container.querySelector('svg');
    if (!svg) return { x: 0, y: 0 };
    const rect = svg.getBoundingClientRect();
    return {
      x: ((event.clientX - rect.left) / rect.width) * CURVE_CHART_WIDTH,
      y: ((event.clientY - rect.top) / rect.height) * height
    };
  }

  function handlePointerDown(
    event: ReactPointerEvent<SVGCircleElement>,
    handle: CurveHandle
  ) {
    if (disabled) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    setActiveHandle(handle.id);
  }

  function handlePointerMove(
    event: ReactPointerEvent<SVGCircleElement>,
    handle: CurveHandle
  ) {
    if (disabled || activeHandle !== handle.id) return;
    if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
    const point = svgPointFromEvent(event);
    const next: { hour?: number; value?: number } = {};
    if (!handle.xLocked) next.hour = geometry.hourForX(point.x);
    if (!handle.yLocked) next.value = geometry.valueForY(point.y, handle.axis);
    onHandleDrag(handle.id, next);
  }

  function handlePointerUp(
    event: ReactPointerEvent<SVGCircleElement>,
    handle: CurveHandle
  ) {
    if (disabled || activeHandle !== handle.id) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    setActiveHandle(null);
    onCommit();
  }

  function handleKeyDown(
    event: KeyboardEvent<SVGCircleElement>,
    handle: CurveHandle
  ) {
    if (disabled) return;
    const next: { hour?: number; value?: number } = {};
    const hourStep = event.shiftKey ? 1 : 0.25;
    const valueStep =
      handle.axis === 'kelvin' ? (event.shiftKey ? 250 : 50) : event.shiftKey ? 10 : 2;
    if (event.key === 'ArrowLeft' && !handle.xLocked) {
      next.hour = Math.max(0, handle.hour - hourStep);
    } else if (event.key === 'ArrowRight' && !handle.xLocked) {
      next.hour = Math.min(24, handle.hour + hourStep);
    } else if (event.key === 'ArrowUp' && !handle.yLocked) {
      next.value = handle.value + valueStep;
    } else if (event.key === 'ArrowDown' && !handle.yLocked) {
      next.value = handle.value - valueStep;
    } else if (event.key === 'Enter') {
      onCommit();
      return;
    } else {
      return;
    }
    event.preventDefault();
    onHandleDrag(handle.id, next);
  }

  return (
    <div
      className={`curveEditor${sampling ? ' sampling' : ''}${disabled ? ' disabled' : ''}`}
      ref={containerRef}
    >
      <CurveChart
        series={series}
        height={height}
        nowHour={nowHour}
        solar={solar}
        yLeft={yLeft}
        yRight={yRight}
        overlay={
          <g className="curveHandles">
            {handles.map((handle) => {
              const x = geometry.xForHour(handle.hour);
              const y = geometry.yForValue(handle.value, handle.axis);
              return (
                <g key={handle.id}>
                  {handle.label ? (
                    <text className="curveHandleLabel" x={x} y={y - 12} textAnchor="middle">
                      {handle.label}
                    </text>
                  ) : null}
                  <circle
                    className={`curveHandle${activeHandle === handle.id ? ' active' : ''}`}
                    cx={x}
                    cy={y}
                    r={7}
                    fill={handle.color ?? 'var(--green)'}
                    tabIndex={disabled ? -1 : 0}
                    role="slider"
                    aria-label={handle.label ?? handle.id}
                    aria-valuenow={handle.yLocked ? handle.hour : handle.value}
                    onPointerDown={(event) => handlePointerDown(event, handle)}
                    onPointerMove={(event) => handlePointerMove(event, handle)}
                    onPointerUp={(event) => handlePointerUp(event, handle)}
                    onKeyDown={(event) => handleKeyDown(event, handle)}
                    onBlur={() => {
                      if (activeHandle === null) onCommit();
                    }}
                  />
                </g>
              );
            })}
          </g>
        }
      />
    </div>
  );
}
