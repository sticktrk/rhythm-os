import {
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent
} from 'react';

import { formatHour } from '../../lib/format';

export type ClockMarker = {
  id: string;
  hour: number;
  label?: string;
  color?: string;
};

/** 24-hour dial with draggable markers (midnight at top, clockwise).
    Used for transition times and scheduled breakpoints. */
export function ClockDial({
  markers,
  size = 220,
  readOnly,
  onDrag,
  onCommit
}: {
  markers: ClockMarker[];
  size?: number;
  readOnly?: boolean;
  onDrag?: (id: string, hour: number) => void;
  onCommit?: (id: string, hour: number) => void;
}) {
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [active, setActive] = useState<string | null>(null);

  const center = size / 2;
  const radius = size / 2 - 22;

  function positionForHour(hour: number): { x: number; y: number } {
    const angle = ((hour / 24) * 360 - 90) * (Math.PI / 180);
    return {
      x: center + Math.cos(angle) * radius,
      y: center + Math.sin(angle) * radius
    };
  }

  function hourFromPointer(event: ReactPointerEvent): number {
    const svg = svgRef.current;
    if (!svg) return 0;
    const rect = svg.getBoundingClientRect();
    const dx = event.clientX - rect.left - rect.width / 2;
    const dy = event.clientY - rect.top - rect.height / 2;
    const degrees = (Math.atan2(dy, dx) * 180) / Math.PI + 90;
    const hour = (((degrees + 360) % 360) / 360) * 24;
    const snapped = Math.round(hour * 4) / 4; // quarter-hour snap
    return snapped >= 24 ? 0 : snapped; // top of dial rounds up to 24 → wrap to 0
  }

  return (
    <svg
      className="clockDial"
      ref={svgRef}
      viewBox={`0 0 ${size} ${size}`}
      width={size}
      height={size}
      role="img"
      aria-label="24 hour dial"
    >
      <circle className="clockFace" cx={center} cy={center} r={radius} />
      {Array.from({ length: 24 }, (_, hour) => {
        const outer = positionForHour(hour);
        const isMajor = hour % 6 === 0;
        const inner = {
          x: center + (outer.x - center) * (isMajor ? 0.88 : 0.94),
          y: center + (outer.y - center) * (isMajor ? 0.88 : 0.94)
        };
        return (
          <g key={hour}>
            <line
              className={`clockTick${isMajor ? ' major' : ''}`}
              x1={inner.x}
              y1={inner.y}
              x2={outer.x}
              y2={outer.y}
            />
            {isMajor ? (
              <text
                className="clockTickLabel"
                x={center + (outer.x - center) * 0.76}
                y={center + (outer.y - center) * 0.76 + 4}
                textAnchor="middle"
              >
                {formatHour(hour)}
              </text>
            ) : null}
          </g>
        );
      })}
      {markers.map((marker) => {
        const position = positionForHour(marker.hour);
        return (
          <g key={marker.id}>
            <line
              className="clockMarkerSpoke"
              x1={center}
              y1={center}
              x2={position.x}
              y2={position.y}
              stroke={marker.color ?? 'var(--green)'}
            />
            <circle
              className={`clockMarker${active === marker.id ? ' active' : ''}${readOnly ? ' readOnly' : ''}`}
              cx={position.x}
              cy={position.y}
              r={9}
              fill={marker.color ?? 'var(--green)'}
              onPointerDown={(event) => {
                if (readOnly) return;
                event.currentTarget.setPointerCapture(event.pointerId);
                setActive(marker.id);
              }}
              onPointerMove={(event) => {
                if (readOnly || active !== marker.id) return;
                if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
                onDrag?.(marker.id, hourFromPointer(event));
              }}
              onPointerUp={(event) => {
                if (readOnly || active !== marker.id) return;
                event.currentTarget.releasePointerCapture(event.pointerId);
                setActive(null);
                onCommit?.(marker.id, hourFromPointer(event));
              }}
            />
            {marker.label ? (
              <text
                className="clockMarkerLabel"
                x={position.x}
                y={position.y - 14}
                textAnchor="middle"
              >
                {marker.label} {formatHour(marker.hour)}
              </text>
            ) : null}
          </g>
        );
      })}
    </svg>
  );
}
